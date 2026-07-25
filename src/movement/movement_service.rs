use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use tokio::sync::Mutex;

use crate::{
    config::MovementConfig,
    error::AppError,
    minecraft::{
        client::MinecraftClient,
        world_state::{MovementSnapshot, MovementStatus, PositionSnapshot},
    },
    movement::logger,
    movement::navigator::{arrived, distance, following_snapshot, moving_snapshot},
};

#[derive(Clone)]
pub struct MovementService {
    config: MovementConfig,
    state: Arc<Mutex<MovementSnapshot>>,
}

impl MovementService {
    pub fn new(config: MovementConfig) -> Self {
        Self {
            config,
            state: Arc::new(Mutex::new(MovementSnapshot::default())),
        }
    }
    pub async fn snapshot(&self) -> MovementSnapshot {
        self.state.lock().await.clone()
    }

    pub async fn goto(
        &self,
        minecraft: &MinecraftClient,
        destination: PositionSnapshot,
    ) -> Result<(), AppError> {
        if !destination.x.is_finite() || !destination.y.is_finite() || !destination.z.is_finite() {
            return Err(AppError::InvalidCoordinates(
                "coordinates must be finite".into(),
            ));
        }
        let world = minecraft.world_state_snapshot().await;
        minecraft.start_navigation_to(destination, None).await?;
        logger::going_to(destination);
        self.replace_and_publish(minecraft, moving_snapshot(destination, world.bot.position))
            .await;
        Ok(())
    }

    pub async fn stop(&self, minecraft: &MinecraftClient) -> Result<(), AppError> {
        let _ = minecraft.stop_navigation().await;
        let mut snapshot = self.snapshot().await;
        let was_following = snapshot.status == MovementStatus::FollowingPlayer;
        let was_moving = snapshot.status == MovementStatus::MovingToPosition;
        if !was_following && !was_moving {
            return Ok(());
        }
        snapshot.status = if was_following {
            MovementStatus::Idle
        } else {
            MovementStatus::Cancelled
        };
        if was_following {
            snapshot.target_player = None;
        }
        snapshot.failure_reason = None;
        snapshot.last_movement_update = Some(SystemTime::now());
        self.replace_and_publish(minecraft, snapshot).await;
        Ok(())
    }

    pub async fn follow(&self, minecraft: &MinecraftClient, name: &str) -> Result<(), AppError> {
        let world = minecraft.world_state_snapshot().await;
        let player = world
            .find_player_by_name(name)
            .ok_or_else(|| AppError::UnknownPlayer(name.to_owned()))?;
        let destination = player
            .position
            .ok_or_else(|| AppError::UnknownPlayer(format!("{name} has no known position")))?;
        minecraft
            .start_navigation_to(destination, Some(self.config.follow_distance))
            .await?;
        logger::following(&player.username);
        self.replace_and_publish(
            minecraft,
            following_snapshot(player.username.clone(), destination, world.bot.position),
        )
        .await;
        Ok(())
    }

    pub async fn tick(&self, minecraft: &MinecraftClient) {
        let snapshot = self.snapshot().await;
        match snapshot.status {
            MovementStatus::MovingToPosition => self.tick_goto(minecraft, snapshot).await,
            MovementStatus::FollowingPlayer => self.tick_follow(minecraft, snapshot).await,
            _ => {}
        }
    }

    async fn tick_goto(&self, minecraft: &MinecraftClient, mut snapshot: MovementSnapshot) {
        let world = minecraft.world_state_snapshot().await;
        if arrived(
            world.bot.position,
            snapshot.destination,
            self.config.arrival_distance,
        ) {
            let _ = minecraft.stop_navigation().await;
            snapshot.status = MovementStatus::Completed;
            snapshot.last_movement_update = Some(SystemTime::now());
            self.replace_and_publish(minecraft, snapshot).await;
            return;
        }
        match minecraft.navigation_status().await {
            Ok(status) if status.reached => {
                snapshot.status = MovementStatus::Completed;
                snapshot.last_movement_update = Some(SystemTime::now());
                self.replace_and_publish(minecraft, snapshot).await;
            }
            Ok(status) if !status.calculating && !status.executing => {
                snapshot.status = MovementStatus::Failed;
                snapshot.failure_reason = Some("no path was found".into());
                snapshot.last_movement_update = Some(SystemTime::now());
                self.replace_and_publish(minecraft, snapshot).await;
            }
            Ok(_) => {
                snapshot.estimated_distance = world
                    .bot
                    .position
                    .zip(snapshot.destination)
                    .map(|(a, b)| distance(a, b));
                snapshot.last_movement_update = Some(SystemTime::now());
                self.replace_and_publish(minecraft, snapshot).await;
            }
            Err(error) => self.fail(minecraft, snapshot, error.to_string()).await,
        }
    }

    async fn tick_follow(&self, minecraft: &MinecraftClient, mut snapshot: MovementSnapshot) {
        let world = minecraft.world_state_snapshot().await;
        let Some(name) = snapshot.target_player.clone() else {
            return self
                .fail(minecraft, snapshot, "follow target missing".into())
                .await;
        };
        let Some(player) = world.find_player_by_name(&name) else {
            logger::lost_player(&name);
            let _ = minecraft.stop_navigation().await;
            snapshot.status = MovementStatus::Idle;
            snapshot.target_player = None;
            snapshot.failure_reason = None;
            self.replace_and_publish(minecraft, snapshot).await;
            return;
        };
        let Some(destination) = player.position else {
            return;
        };
        snapshot.destination = Some(destination);
        snapshot.estimated_distance = world
            .bot
            .position
            .map(|position| distance(position, destination));
        if snapshot
            .estimated_distance
            .is_some_and(|current| current <= self.config.follow_distance)
        {
            let _ = minecraft.stop_navigation().await;
            snapshot.last_movement_update = Some(SystemTime::now());
            self.replace_and_publish(minecraft, snapshot).await;
            return;
        }
        let should_repath = snapshot.last_movement_update.is_none_or(|updated| {
            updated.elapsed().unwrap_or_default()
                >= Duration::from_millis(self.config.repath_interval_ms)
        });
        if should_repath
            && snapshot
                .estimated_distance
                .is_none_or(|d| d > self.config.follow_distance)
        {
            if let Err(error) = minecraft
                .start_navigation_to(destination, Some(self.config.follow_distance))
                .await
            {
                self.fail(minecraft, snapshot, error.to_string()).await;
                return;
            }
            snapshot.last_movement_update = Some(SystemTime::now());
        }
        self.replace_and_publish(minecraft, snapshot).await;
    }

    async fn fail(
        &self,
        minecraft: &MinecraftClient,
        mut snapshot: MovementSnapshot,
        reason: String,
    ) {
        snapshot.status = MovementStatus::Failed;
        snapshot.failure_reason = Some(reason);
        snapshot.last_movement_update = Some(SystemTime::now());
        self.replace_and_publish(minecraft, snapshot).await;
    }
    async fn replace_and_publish(&self, minecraft: &MinecraftClient, snapshot: MovementSnapshot) {
        let previous = {
            let mut state = self.state.lock().await;
            let previous = state.status;
            *state = snapshot.clone();
            previous
        };
        if previous != snapshot.status {
            match snapshot.status {
                MovementStatus::Completed => logger::reached(),
                MovementStatus::Failed => logger::cannot_reach(
                    snapshot
                        .failure_reason
                        .as_deref()
                        .unwrap_or("unknown reason"),
                ),
                MovementStatus::Cancelled if previous == MovementStatus::FollowingPlayer => {
                    logger::stopped_following()
                }
                MovementStatus::Cancelled => logger::cancelled(),
                MovementStatus::Idle if previous == MovementStatus::FollowingPlayer => {
                    logger::stopped_following()
                }
                _ => {}
            }
        }
        minecraft.set_movement_snapshot(snapshot).await;
    }
}
