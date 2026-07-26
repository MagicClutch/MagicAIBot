use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use tokio::sync::Mutex;

use crate::{
    config::{MovementConfig, MultitaskingConfig},
    error::AppError,
    minecraft::{
        client::MinecraftClient,
        world_state::{MovementSnapshot, MovementStatus, PositionSnapshot},
    },
    movement::multitasking::{LocalMovementInput, local_input_for_direction},
    movement::navigator::{arrived, distance, following_snapshot, moving_snapshot},
    movement::{NavigationMode, logger},
};

/// Azalea starts path calculation asynchronously. Immediately after
/// `start_goto_with_opts` its status can still describe the previous goal (or
/// report no active calculation at all), so do not reject a freshly submitted
/// destination until the pathfinder has had a chance to observe it.
const PATHFINDER_STARTUP_GRACE: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub struct MovementService {
    config: MovementConfig,
    multitasking: MultitaskingConfig,
    state: Arc<Mutex<MovementSnapshot>>,
    local_input: Arc<Mutex<LocalMovementInput>>,
    suppress_terminal_log: Arc<Mutex<bool>>,
}

impl MovementService {
    pub fn new(config: MovementConfig, multitasking: MultitaskingConfig) -> Self {
        Self {
            config,
            multitasking,
            state: Arc::new(Mutex::new(MovementSnapshot::default())),
            local_input: Arc::new(Mutex::new(LocalMovementInput::default())),
            suppress_terminal_log: Arc::new(Mutex::new(false)),
        }
    }
    pub async fn snapshot(&self) -> MovementSnapshot {
        self.state.lock().await.clone()
    }

    /// The latest camera-relative movement recommendation. Azalea's
    /// pathfinder remains the sole writer of live movement controls; exposing
    /// this bounded adapter lets navigation and tests share human-like
    /// direction policy without creating a competing controller.
    pub async fn local_input(&self) -> LocalMovementInput {
        *self.local_input.lock().await
    }

    pub async fn goto(
        &self,
        minecraft: &MinecraftClient,
        destination: PositionSnapshot,
        mode: NavigationMode,
    ) -> Result<(), AppError> {
        self.goto_internal(minecraft, destination, mode, true).await
    }

    pub(crate) async fn goto_for_block_navigation(
        &self,
        minecraft: &MinecraftClient,
        destination: PositionSnapshot,
        mode: NavigationMode,
    ) -> Result<(), AppError> {
        self.goto_internal(minecraft, destination, mode, false)
            .await
    }

    async fn goto_internal(
        &self,
        minecraft: &MinecraftClient,
        destination: PositionSnapshot,
        mode: NavigationMode,
        log_start: bool,
    ) -> Result<(), AppError> {
        if !destination.x.is_finite() || !destination.y.is_finite() || !destination.z.is_finite() {
            return Err(AppError::InvalidCoordinates(
                "coordinates must be finite".into(),
            ));
        }
        let world = minecraft.world_state_snapshot().await;
        *self.suppress_terminal_log.lock().await = !log_start;
        minecraft
            .start_navigation_to(destination, None, mode)
            .await?;
        if log_start {
            logger::going_to(destination);
        }
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
        *self.suppress_terminal_log.lock().await = false;
        let player = world
            .find_player_by_name(name)
            .ok_or_else(|| AppError::UnknownPlayer(name.to_owned()))?;
        let destination = player
            .position
            .ok_or_else(|| AppError::UnknownPlayer(format!("{name} has no known position")))?;
        minecraft
            .start_navigation_to(
                destination,
                Some(self.config.follow_distance),
                NavigationMode::MovementOnly,
            )
            .await?;
        logger::following(&player.username);
        self.replace_and_publish(
            minecraft,
            following_snapshot(player.username.clone(), destination, world.bot.position),
        )
        .await;
        Ok(())
    }

    pub async fn tick(&self, minecraft: &MinecraftClient, explicit_look: bool) {
        let snapshot = self.snapshot().await;
        match snapshot.status {
            MovementStatus::MovingToPosition => {
                self.tick_goto(minecraft, snapshot, explicit_look).await
            }
            MovementStatus::FollowingPlayer => {
                self.tick_follow(minecraft, snapshot, explicit_look).await
            }
            _ => {}
        }
    }

    async fn tick_goto(
        &self,
        minecraft: &MinecraftClient,
        mut snapshot: MovementSnapshot,
        explicit_look: bool,
    ) {
        let world = minecraft.world_state_snapshot().await;
        self.update_local_input(
            world.bot.position,
            snapshot.destination,
            world.bot.yaw,
            explicit_look,
        )
        .await;
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
            Ok(status)
                if status.reached
                    && arrived(
                        world.bot.position,
                        snapshot.destination,
                        self.config.arrival_distance,
                    ) =>
            {
                snapshot.status = MovementStatus::Completed;
                snapshot.last_movement_update = Some(SystemTime::now());
                self.replace_and_publish(minecraft, snapshot).await;
            }
            Ok(status) if status.reached && pathfinder_startup_grace_elapsed(&snapshot) => {
                // Azalea can briefly report the previous goal as complete
                // while a new goal event is still being consumed. Re-submit
                // the goal instead of abandoning movement at that point.
                if let Some(destination) = snapshot.destination {
                    let _ = minecraft
                        .start_navigation_to(destination, None, NavigationMode::MovementOnly)
                        .await;
                    snapshot.started_at = Some(SystemTime::now());
                }
                snapshot.last_movement_update = Some(SystemTime::now());
                self.replace_and_publish(minecraft, snapshot).await;
            }
            Ok(status)
                if !status.calculating
                    && !status.executing
                    && pathfinder_startup_grace_elapsed(&snapshot) =>
            {
                if let Some(destination) = snapshot.destination {
                    let _ = minecraft
                        .start_navigation_to(destination, None, NavigationMode::MovementOnly)
                        .await;
                    snapshot.started_at = Some(SystemTime::now());
                }
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

    async fn tick_follow(
        &self,
        minecraft: &MinecraftClient,
        mut snapshot: MovementSnapshot,
        explicit_look: bool,
    ) {
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
        self.update_local_input(
            world.bot.position,
            Some(destination),
            world.bot.yaw,
            explicit_look,
        )
        .await;
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
                .start_navigation_to(
                    destination,
                    Some(self.config.follow_distance),
                    NavigationMode::MovementOnly,
                )
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
        let suppress_terminal_log = *self.suppress_terminal_log.lock().await;
        if previous != snapshot.status && !suppress_terminal_log {
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
        if matches!(
            snapshot.status,
            MovementStatus::Completed
                | MovementStatus::Failed
                | MovementStatus::Cancelled
                | MovementStatus::Idle
        ) {
            *self.suppress_terminal_log.lock().await = false;
        }
        minecraft.set_movement_snapshot(snapshot).await;
    }

    async fn update_local_input(
        &self,
        position: Option<PositionSnapshot>,
        destination: Option<PositionSnapshot>,
        camera_yaw: Option<f32>,
        explicit_look: bool,
    ) {
        let Some((position, destination, camera_yaw)) = position
            .zip(destination)
            .zip(camera_yaw)
            .map(|((a, b), c)| (a, b, c))
        else {
            return;
        };
        let dx = destination.x - position.x;
        let dz = destination.z - position.z;
        if dx.hypot(dz) < 0.01 {
            return;
        }
        let travel_yaw = (-dx).atan2(dz).to_degrees() as f32;
        *self.local_input.lock().await =
            local_input_for_direction(travel_yaw, camera_yaw, explicit_look, &self.multitasking);
    }
}

fn pathfinder_startup_grace_elapsed(snapshot: &MovementSnapshot) -> bool {
    snapshot
        .started_at
        .is_none_or(|started| started.elapsed().unwrap_or_default() >= PATHFINDER_STARTUP_GRACE)
}

#[cfg(test)]
mod startup_tests {
    use super::*;

    #[test]
    fn waits_for_fresh_pathfinder_goal_to_start() {
        let snapshot = MovementSnapshot {
            started_at: Some(SystemTime::now()),
            ..MovementSnapshot::default()
        };
        assert!(!pathfinder_startup_grace_elapsed(&snapshot));
    }

    #[test]
    fn eventually_treats_inactive_pathfinder_as_failure() {
        let snapshot = MovementSnapshot {
            started_at: Some(SystemTime::now() - PATHFINDER_STARTUP_GRACE),
            ..MovementSnapshot::default()
        };
        assert!(pathfinder_startup_grace_elapsed(&snapshot));
    }
}
