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

#[derive(Clone)]
pub struct MovementService {
    config: MovementConfig,
    multitasking: MultitaskingConfig,
    state: Arc<Mutex<MovementSnapshot>>,
    local_input: Arc<Mutex<LocalMovementInput>>,
    suppress_terminal_log: Arc<Mutex<bool>>,
    navigation_mode: Arc<Mutex<NavigationMode>>,
    last_goal_submission: Arc<Mutex<Option<SystemTime>>>,
    last_goal_destination: Arc<Mutex<Option<PositionSnapshot>>>,
}

impl MovementService {
    pub fn new(config: MovementConfig, multitasking: MultitaskingConfig) -> Self {
        Self {
            config,
            multitasking,
            state: Arc::new(Mutex::new(MovementSnapshot::default())),
            local_input: Arc::new(Mutex::new(LocalMovementInput::default())),
            suppress_terminal_log: Arc::new(Mutex::new(false)),
            navigation_mode: Arc::new(Mutex::new(NavigationMode::MovementOnly)),
            last_goal_submission: Arc::new(Mutex::new(None)),
            last_goal_destination: Arc::new(Mutex::new(None)),
        }
    }
    pub async fn snapshot(&self) -> MovementSnapshot {
        self.state.lock().await.clone()
    }

    /// Absolute backstop for how long a caller awaiting movement completion
    /// should wait before giving up, regardless of progress (see
    /// `MovementConfig::maximum_navigation_seconds`). See
    /// [`Self::stuck_timeout_seconds`] for the shorter, progress-based
    /// timeout that governs the common case.
    pub(crate) fn maximum_navigation_seconds(&self) -> u64 {
        self.config.maximum_navigation_seconds
    }

    /// How long a caller awaiting movement completion tolerates no
    /// improvement in distance to the destination before giving up (see
    /// `MovementConfig::stuck_timeout_seconds`).
    pub(crate) fn stuck_timeout_seconds(&self) -> u64 {
        self.config.stuck_timeout_seconds
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
        self.goto_internal(minecraft, destination, mode, true, None)
            .await
    }

    pub(crate) async fn goto_for_block_navigation(
        &self,
        minecraft: &MinecraftClient,
        destination: PositionSnapshot,
        mode: NavigationMode,
    ) -> Result<(), AppError> {
        self.goto_internal(minecraft, destination, mode, false, None)
            .await
    }

    /// Like [`Self::goto_for_block_navigation`], but stops `stop_distance`
    /// blocks short of `destination` rather than walking all the way up to
    /// it -- used for approaching a player (`#goto <player>`, `#drop <item>
    /// <amount> <player>`) so the bot doesn't crowd or try to stand on top
    /// of them. `stop_distance` is used both as Azalea's own pathfinder stop
    /// distance and as the app-level "have I arrived" threshold (see
    /// `MovementSnapshot::arrival_distance_override`), so the two can never
    /// disagree about whether the goal is reached.
    pub(crate) async fn goto_player_approach(
        &self,
        minecraft: &MinecraftClient,
        destination: PositionSnapshot,
        mode: NavigationMode,
        stop_distance: f64,
    ) -> Result<(), AppError> {
        self.goto_internal(minecraft, destination, mode, false, Some(stop_distance))
            .await
    }

    async fn goto_internal(
        &self,
        minecraft: &MinecraftClient,
        destination: PositionSnapshot,
        mode: NavigationMode,
        log_start: bool,
        arrival_distance_override: Option<f64>,
    ) -> Result<(), AppError> {
        if !destination.x.is_finite() || !destination.y.is_finite() || !destination.z.is_finite() {
            return Err(AppError::InvalidCoordinates(
                "coordinates must be finite".into(),
            ));
        }
        let world = minecraft.world_state_snapshot().await;
        *self.suppress_terminal_log.lock().await = !log_start;
        *self.navigation_mode.lock().await = mode;
        minecraft
            .start_navigation_to(destination, arrival_distance_override, mode)
            .await?;
        *self.last_goal_submission.lock().await = Some(SystemTime::now());
        *self.last_goal_destination.lock().await = Some(destination);
        if log_start {
            logger::going_to(destination);
        }
        self.replace_and_publish(
            minecraft,
            moving_snapshot(destination, world.bot.position, arrival_distance_override),
        )
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
                NavigationMode::AllowMining,
            )
            .await?;
        *self.navigation_mode.lock().await = NavigationMode::AllowMining;
        *self.last_goal_submission.lock().await = Some(SystemTime::now());
        *self.last_goal_destination.lock().await = Some(destination);
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
        let arrival_distance = snapshot
            .arrival_distance_override
            .unwrap_or(self.config.arrival_distance);
        self.update_local_input(
            world.bot.position,
            snapshot.destination,
            world.bot.yaw,
            explicit_look,
        )
        .await;
        if arrived(world.bot.position, snapshot.destination, arrival_distance) {
            let _ = minecraft.stop_navigation().await;
            snapshot.status = MovementStatus::Completed;
            snapshot.last_movement_update = Some(SystemTime::now());
            self.replace_and_publish(minecraft, snapshot).await;
            return;
        }
        if let Some(destination) = snapshot.destination {
            let mode = *self.navigation_mode.lock().await;
            if let Err(error) = self
                .refresh_navigation_goal(
                    minecraft,
                    destination,
                    snapshot.arrival_distance_override,
                    mode,
                )
                .await
            {
                self.fail(minecraft, snapshot, error.to_string()).await;
                return;
            }
        }
        match minecraft.navigation_status().await {
            Ok(status)
                if status.reached
                    && arrived(world.bot.position, snapshot.destination, arrival_distance) =>
            {
                snapshot.status = MovementStatus::Completed;
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

    async fn refresh_navigation_goal(
        &self,
        minecraft: &MinecraftClient,
        destination: PositionSnapshot,
        follow_distance: Option<f64>,
        mode: NavigationMode,
    ) -> Result<(), AppError> {
        let status = minecraft.navigation_status().await;
        // Never resubmit while a build move's placement or mining action is
        // physically in flight (a server round-trip is pending), no matter
        // how much the destination has changed -- see
        // `NavigationStatus::mid_build_action`'s doc comment. This has to be
        // checked unconditionally, before the destination-drift check below:
        // `/follow` re-reads the target player's live position every tick, so
        // the destination is essentially always "changed" by some amount
        // while chasing a moving player, which would otherwise bypass every
        // other guard here on nearly every tick and tear down an in-progress
        // pillar/bridge step the instant the tracked player so much as
        // breathes -- reintroducing the same endless place/break oscillation
        // these guards exist to prevent, just reachable through `/follow`
        // instead of a static `/goto` (typically once the target is far
        // enough away that reaching them needs a build move at all, e.g.
        // crossing a gap or climbing to catch up).
        if status.as_ref().is_ok_and(|status| status.mid_build_action) {
            return Ok(());
        }
        // Beyond that immediate in-flight check, only a genuinely meaningful
        // move -- more than a stride between repath ticks -- counts as
        // "changed" enough to justify redirecting toward a still-executing
        // route; a static `/goto` destination never moves at all, so it
        // always compares as unchanged here regardless of the threshold.
        const GOAL_DRIFT_TOLERANCE: f64 = 1.5;
        let destination_changed = {
            let mut last = self.last_goal_destination.lock().await;
            let changed =
                last.is_none_or(|previous| distance(previous, destination) > GOAL_DRIFT_TOLERANCE);
            *last = Some(destination);
            changed
        };
        if !destination_changed {
            if !self.goal_resubmission_due().await {
                return Ok(());
            }
            // A new goto event replaces (and cancels) Azalea's in-flight
            // background path computation -- see `goto_listener`'s `ComputePath`
            // component replacement in the vendored pathfinder. Resubmitting on
            // this fixed `repath_interval_ms` timer (as short as 150ms by
            // default) while a longer search is still running -- a distant
            // target, or one that needs the more expensive build-move successors
            // (pillar/bridge/staircase) -- would keep restarting that search
            // forever without ever letting it finish, so the bot never gets a
            // new route and appears stuck. The same is true once a route *is*
            // executing: skip resubmission while a calculation is already in
            // progress or a path is already executing toward this same,
            // unchanged destination; Azalea's own `max_timeout` (5s by
            // default) bounds calculation, and
            // `maximum_navigation_seconds`/`stuck_timeout_seconds` bound a
            // stalled execution, so this can't itself introduce an unbounded
            // stall -- it only stops us from interrupting progress that is
            // already being made.
            if status.is_ok_and(|status| status.calculating || status.executing) {
                return Ok(());
            }
        }
        minecraft
            .start_navigation_to(destination, follow_distance, mode)
            .await?;
        *self.last_goal_submission.lock().await = Some(SystemTime::now());
        Ok(())
    }

    async fn goal_resubmission_due(&self) -> bool {
        self.last_goal_submission
            .lock()
            .await
            .is_none_or(|submitted| {
                submitted.elapsed().unwrap_or_default()
                    >= Duration::from_millis(self.config.repath_interval_ms)
            })
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
        if let Err(error) = self
            .refresh_navigation_goal(
                minecraft,
                destination,
                Some(self.config.follow_distance),
                NavigationMode::AllowMining,
            )
            .await
        {
            self.fail(minecraft, snapshot, error.to_string()).await;
            return;
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
