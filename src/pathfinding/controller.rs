//! [`PathfindingController`]: the public face of the pathfinding layer and
//! the only part of it `App` talks to. Drives the state machine in
//! `crate::pathfinding::state`, exactly like every other task-shaped
//! controller in this codebase (`combat::KillController`,
//! `InteractionController`, ...) -- `start`/`cancel`/`snapshot`/`tick`.
//!
//! # What one tick does
//!
//! ```text
//! PLANNING          coarse route (cheap, chunk-level) -> segment plan,
//!                   then sample terrain and start the first segment search
//! FOLLOWING_SEGMENT walk waypoints; meanwhile pre-compute the *next*
//!                   segment so the bot never stands still at a boundary;
//!                   periodically re-verify the route still exists
//! RECALCULATING     a search is in flight for a segment that has none
//! ARRIVED           within `arrival_radius` of the destination
//! ```
//!
//! # Replanning triggers, all of which land in RECALCULATING
//!
//! - the current segment is blocked (no progress for `segment_stuck_seconds`)
//! - revalidation finds a waypoint that is no longer walkable (someone
//!   built, a chunk changed, terrain streamed in differently)
//! - the bot is displaced off its own route (a fall, knockback, a teleport)
//! - a segment ends short of where it was planned to (the search hit the
//!   edge of loaded world and delivered a partial path)
//! - [`PathfindingController::notify_world_change`] is called for a position
//!   the plan passes through
//!
//! Each of those invalidates *only* the affected segments -- completed work
//! and unaffected future segments survive, which is what keeps a 5000-block
//! trip from restarting every time a creeper rearranges a hillside.

use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use tokio::sync::Mutex;

use crate::{
    config::{PathfindingConfig, VerticalNavigationConfig},
    error::AppError,
    logging,
    minecraft::{
        client::MinecraftClient,
        world_state::{BlockPosition, PositionSnapshot},
    },
    movement::MovementService,
    pathfinding::{
        astar::Goal,
        debug,
        executor::{FollowOutcome, SegmentFollower, displaced_from},
        grid::block_distance,
        moves::BODY_HEIGHT,
        planner::{self, Planner},
        route::{self, RouteProfile},
        sampler,
        segment::{SegmentPlan, SegmentState},
        state::{NavigationFailure, NavigationSnapshot, NavigationState},
        world_cache::NavigationCache,
    },
};

/// How often the route ahead is re-verified against freshly sampled terrain
/// while following a segment. Deliberately a constant rather than a config
/// knob: it trades one ~10ms sample against how quickly the bot notices the
/// world changed under it, and there is no reason for a user to tune that
/// independently of everything else.
const REVALIDATE_INTERVAL: Duration = Duration::from_secs(3);

/// How far off its own waypoints the bot may drift before the route is
/// considered to no longer describe where it is. Wide enough to absorb the
/// movement layer's own wandering, narrow enough to catch a fall down a
/// ravine.
const DISPLACEMENT_TOLERANCE: f64 = 12.0;

/// How much closer to the destination counts as real progress for the
/// whole-trip stall detector. Matches the per-waypoint epsilon in
/// `crate::pathfinding::executor` -- below this is positional noise.
const PROGRESS_EPSILON: f64 = 0.35;

/// How many per-segment stuck timeouts of no net progress end the whole
/// trip. Generous on purpose: a bot working its way around a mountain range
/// makes very little net progress for a while and is not stuck.
const STUCK_TRIP_MULTIPLIER: u32 = 6;

/// Fraction of a segment that must be walked before the *next* segment's
/// search is started. Early enough that the search (a second at worst) has
/// finished by the time the bot arrives, late enough that a segment
/// invalidated behind the bot doesn't waste the work.
const PREFETCH_AT: f64 = 0.6;

#[derive(Clone)]
pub struct PathfindingController {
    inner: Arc<Mutex<Inner>>,
    config: PathfindingConfig,
    /// Only ever read to decide what the *executor* may build -- see
    /// `planner::move_policy`.
    vertical: VerticalNavigationConfig,
}

struct Inner {
    snapshot: NavigationSnapshot,
    plan: Option<SegmentPlan>,
    follower: SegmentFollower,
    planner: Planner,
    cache: NavigationCache,
    /// Consecutive searches that produced nothing walkable. Reset by any
    /// segment that completes.
    failed_searches: u32,
    last_revalidation: Instant,
    /// Closest the bot has ever been to the final destination on this trip,
    /// and when that happened -- the whole-journey stuck detector, one level
    /// above the per-segment one in `crate::pathfinding::executor`. A trip
    /// can replan forever without ever getting closer (a walled-off
    /// destination, an island); this is what ends it.
    best_distance: Option<f64>,
    last_progress: Instant,
    /// Set while a search was started for a segment other than the current
    /// one (the prefetch), so its result is applied to the right slot.
    destination: Option<BlockPosition>,
}

impl PathfindingController {
    #[must_use]
    pub fn new(config: PathfindingConfig, vertical: VerticalNavigationConfig) -> Self {
        let cache = NavigationCache::new(config.cache_capacity, config.cache_max_age);
        Self {
            inner: Arc::new(Mutex::new(Inner {
                snapshot: NavigationSnapshot::default(),
                plan: None,
                follower: SegmentFollower::new(),
                planner: Planner::new(),
                cache,
                failed_searches: 0,
                last_revalidation: Instant::now(),
                best_distance: None,
                last_progress: Instant::now(),
                destination: None,
            })),
            config,
            vertical,
        }
    }

    pub async fn snapshot(&self) -> NavigationSnapshot {
        self.inner.lock().await.snapshot.clone()
    }

    /// Begins navigating to `destination`, replacing whatever trip was
    /// previously running.
    ///
    /// Returns as soon as the task is accepted; the actual planning happens
    /// on the first [`Self::tick`], so this never blocks the caller behind a
    /// search.
    pub async fn start(
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
        let start = world
            .bot
            .position
            .ok_or(AppError::MovementUnavailable)?
            .block();
        let destination = destination.block();

        let mut inner = self.inner.lock().await;
        inner.planner.cancel();
        inner.plan = None;
        inner.follower = SegmentFollower::new();
        inner.failed_searches = 0;
        inner.last_revalidation = Instant::now();
        inner.best_distance = None;
        inner.last_progress = Instant::now();
        inner.destination = Some(destination);
        inner.snapshot = NavigationSnapshot {
            state: NavigationState::Planning,
            start: Some(start),
            destination: Some(destination),
            distance_remaining: Some(block_distance(start, destination)),
            started_at: Some(SystemTime::now()),
            ..NavigationSnapshot::default()
        };
        logging::milestone(format!(
            "Navigating to ({}, {}, {})",
            destination.x, destination.y, destination.z
        ));
        Ok(())
    }

    /// Cancels the trip, stopping any movement it started. A no-op on an
    /// idle or already-terminal controller, matching every other controller
    /// in this codebase.
    pub async fn cancel(&self, minecraft: &MinecraftClient, movement: &MovementService) {
        let was_active = {
            let inner = self.inner.lock().await;
            inner.snapshot.state.active()
        };
        if !was_active {
            return;
        }
        let _ = movement.stop(minecraft).await;
        let mut inner = self.inner.lock().await;
        inner.planner.cancel();
        inner.plan = None;
        inner.follower = SegmentFollower::new();
        inner.snapshot.state = NavigationState::Cancelled;
        inner.snapshot.failure = None;
        logging::info("Navigation cancelled");
    }

    /// Tells the pathfinder that the world changed at `position` -- a block
    /// broken or placed, a chunk reloaded.
    ///
    /// Forgets the cached chunk knowledge there, and invalidates only the
    /// segments whose route actually passes nearby; a change the plan
    /// doesn't touch costs nothing. Returns whether anything was
    /// invalidated.
    pub async fn notify_world_change(&self, position: BlockPosition) -> bool {
        let mut inner = self.inner.lock().await;
        inner.cache.invalidate_around(position);
        let Some(plan) = inner.plan.as_mut() else {
            return false;
        };
        let total = plan.total_segments();
        let invalidated = plan.invalidate_near(position, 24.0);
        if invalidated == 0 {
            return false;
        }
        let debug_enabled = self.config.debug_pathfinding;
        debug::trace(
            debug_enabled,
            debug::format_replan("world changed", invalidated, total),
        );
        if inner.snapshot.state == NavigationState::FollowingSegment
            && inner
                .plan
                .as_ref()
                .and_then(SegmentPlan::current)
                .is_some_and(|segment| !segment.is_calculated())
        {
            inner.enter_recalculating();
        }
        true
    }

    /// One step of the state machine. Cheap on most ticks: the expensive
    /// work (searching) happens on a blocking thread and is only polled
    /// here.
    pub async fn tick(&self, minecraft: &MinecraftClient, movement: &MovementService) {
        let state = { self.inner.lock().await.snapshot.state };
        if !state.active() {
            return;
        }
        let world = minecraft.world_state_snapshot().await;
        if !world.joined_world() || world.bot.alive == Some(false) {
            return;
        }
        let Some(bot_position) = world.bot.position else {
            return;
        };

        // Checked here rather than inside `tick_following` so it holds in
        // every active state: a destination a few blocks away is reached
        // just as easily mid-replan (or before the first segment is even
        // calculated) as it is while walking one.
        if self.check_arrival(minecraft, movement, bot_position).await {
            return;
        }

        match state {
            NavigationState::Planning => self.tick_planning(minecraft, bot_position).await,
            NavigationState::FollowingSegment => {
                self.tick_following(minecraft, movement, bot_position).await;
            }
            NavigationState::Recalculating => {
                self.tick_recalculating(minecraft, bot_position).await;
            }
            _ => {}
        }
    }

    /// Builds the coarse route and the segment plan, then starts the first
    /// segment's search.
    async fn tick_planning(&self, minecraft: &MinecraftClient, bot_position: PositionSnapshot) {
        let (destination, needs_plan) = {
            let inner = self.inner.lock().await;
            (inner.destination, inner.plan.is_none())
        };
        let Some(destination) = destination else {
            return;
        };
        if needs_plan {
            let start = bot_position.block();
            let mut inner = self.inner.lock().await;
            // Once per trip is the natural moment to drop stale chunk
            // knowledge: it is the only point where the whole route is about
            // to be decided from it.
            inner.cache.prune();
            debug::trace(
                self.config.debug_pathfinding,
                format!(
                    "[Pathfinder] Navigation cache: {} chunks known",
                    inner.cache.len()
                ),
            );
            let route = route::plan(
                start,
                destination,
                &inner.cache,
                &RouteProfile::default(),
                self.config.segment_length,
            );
            let plan = SegmentPlan::from_waypoints(start, &route.waypoints, self.config.costs.walk);
            inner.snapshot.total_segments = plan.total_segments();
            inner.snapshot.current_segment = plan.current_number();
            inner.snapshot.cost_remaining = plan.remaining_cost();
            inner.plan = Some(plan);
            debug::trace(
                self.config.debug_pathfinding,
                debug::format_plan(start, destination, route.waypoints.len()),
            );
        }
        // Planning and recalculating differ only in how they got here; the
        // work of sampling and starting a search is the same.
        self.drive_search(minecraft, bot_position, false).await;
    }

    /// Waits for the in-flight search, or -- when there is nothing left to
    /// search -- gets the state machine moving again.
    ///
    /// Without this last part the controller could sit in `RECALCULATING`
    /// forever: `drive_search` has nothing to start once the target segment
    /// is already calculated (a prefetch landed) or once the plan has run
    /// out of segments entirely, and neither case would otherwise ever be
    /// noticed.
    async fn tick_recalculating(
        &self,
        minecraft: &MinecraftClient,
        bot_position: PositionSnapshot,
    ) {
        self.drive_search(minecraft, bot_position, false).await;
        let resolution = {
            let inner = self.inner.lock().await;
            if inner.planner.is_searching()
                || inner.snapshot.state != NavigationState::Recalculating
            {
                None
            } else {
                match inner.plan.as_ref().and_then(SegmentPlan::current) {
                    Some(segment) if segment.is_calculated() => Some(true),
                    Some(_) => None,
                    None => Some(false),
                }
            }
        };
        match resolution {
            Some(true) => {
                let mut inner = self.inner.lock().await;
                inner.follower = SegmentFollower::new();
                inner.snapshot.state = NavigationState::FollowingSegment;
            }
            Some(false) => self.rebuild_plan_from_here(bot_position).await,
            None => {}
        }
    }

    /// Polls for a finished search, or starts one if none is in flight.
    ///
    /// `prefetch` starts the search for the segment *after* the current one,
    /// which is what keeps the bot from stopping at every segment boundary.
    async fn drive_search(
        &self,
        minecraft: &MinecraftClient,
        bot_position: PositionSnapshot,
        prefetch: bool,
    ) {
        // Apply a finished search first: it may be exactly the one this call
        // would otherwise start.
        if self.apply_finished_search(bot_position).await {
            return;
        }
        let (already_searching, target) = {
            let inner = self.inner.lock().await;
            let searching = inner.planner.is_searching();
            let target = inner.search_target(prefetch);
            (searching, target)
        };
        if already_searching {
            return;
        }
        let Some((segment_index, from, to)) = target else {
            return;
        };

        // Sampling reads Azalea's world under its lock; it is bounded work
        // (see `MinecraftClient::sample_terrain`) and happens once per
        // search, not once per tick.
        let sample = match sampler::sample_corridor(
            minecraft,
            from,
            to,
            self.config.sample_margin,
            self.config.vertical_window,
        )
        .await
        {
            Ok(sample) => sample,
            Err(error) => {
                if !prefetch {
                    self.fail(NavigationFailure::Movement(error.to_string()))
                        .await;
                }
                return;
            }
        };
        if !sample.has_terrain() {
            // Nothing loaded around the bot at all: not a routing failure,
            // just a moment to wait out (a fresh join, a dimension change).
            if !prefetch {
                let mut inner = self.inner.lock().await;
                inner.failed_searches += 1;
                if inner.failed_searches > self.config.max_consecutive_replans {
                    drop(inner);
                    self.fail(NavigationFailure::NoWorldData).await;
                }
            }
            return;
        }

        // Live hostiles become cost, not hard obstacles: the route prefers
        // to give a creeper a wide berth, but a bot boxed in by mobs still
        // finds *a* way out rather than concluding it is trapped.
        let entity_hazards: Vec<(BlockPosition, f64)> = minecraft
            .world_state_snapshot()
            .await
            .entities
            .iter()
            .filter(|entity| entity.alive != Some(false))
            .filter_map(|entity| {
                crate::pathfinding::moves::hazard_radius(&entity.entity_type)
                    .map(|radius| (entity.position.block(), radius))
            })
            .collect();
        let mut inner = self.inner.lock().await;
        sampler::record(&mut inner.cache, &sample);
        // The goal for a segment is its planned end, snapped onto real
        // ground now that terrain is available -- a coarse waypoint's Y is a
        // guess, and searching for an exact cell floating in the air would
        // burn the whole budget proving it unreachable.
        // Clamped into the sampled region first: a coarse waypoint can sit
        // just outside the corridor that was actually sampled, and searching
        // for a cell the grid knows nothing about can only ever fail.
        let clamped = sample.grid.bounds().clamp(to);
        let goal_position = sample
            .grid
            .nearest_standable(clamped, BODY_HEIGHT, self.config.vertical_window)
            .unwrap_or(clamped);
        let goal = Goal::within(goal_position, self.config.segment_arrival_radius);
        let mut policy = planner::move_policy(&self.config, &self.vertical);
        policy.entity_hazards = entity_hazards;
        // The current segment is searched from where the bot actually is
        // (it drifts, falls, gets knocked around); a *prefetched* future
        // segment is searched from its own planned start, since that is
        // where the bot will be standing when it starts walking it.
        let search_start = if prefetch { from } else { bot_position.block() };
        inner.planner.start(
            sample.grid,
            planner::cost_profile(&self.config.costs),
            policy,
            planner::search_budget(&self.config),
            search_start,
            goal,
            segment_index,
        );
        if !prefetch && inner.snapshot.state == NavigationState::FollowingSegment {
            inner.snapshot.state = NavigationState::Recalculating;
        }
    }

    /// Applies a completed search to its segment. Returns whether a result
    /// was consumed.
    async fn apply_finished_search(&self, bot_position: PositionSnapshot) -> bool {
        let mut inner = self.inner.lock().await;
        let Some(completed) = inner.planner.poll() else {
            return false;
        };
        inner.snapshot.last_search_nodes = completed.result.expanded;
        inner.snapshot.last_search_millis = completed.result.elapsed.as_millis() as u64;

        if !completed.result.outcome.usable() || completed.result.is_empty() {
            inner.failed_searches += 1;
            let exhausted = inner.failed_searches > self.config.max_consecutive_replans;
            drop(inner);
            if exhausted {
                self.fail(NavigationFailure::NoRoute).await;
            } else {
                // Re-plan the coarse route from where the bot actually is:
                // the previous corridor is demonstrably not working, and by
                // now more terrain is usually loaded.
                self.rebuild_plan_from_here(bot_position).await;
            }
            return true;
        }

        let max_safe_drop = self.config.costs.max_safe_drop;
        let Some(plan) = inner.plan.as_mut() else {
            return true;
        };
        let is_current_segment = plan.current_number() == completed.segment_index + 1;
        let Some(segment) = plan.segment_mut(completed.segment_index) else {
            return true;
        };
        segment.apply_path(&completed.result, max_safe_drop, 0.5);
        let actions = segment.actions.clone();
        let line = debug::format_segment(plan, &plan.segments()[completed.segment_index]);
        let cost_remaining = plan.remaining_cost();
        inner.failed_searches = 0;
        if is_current_segment {
            inner.follower = SegmentFollower::new();
            inner.snapshot.state = NavigationState::FollowingSegment;
            inner.snapshot.current_actions = actions;
            inner.snapshot.cost_remaining = cost_remaining;
            debug::trace(self.config.debug_pathfinding, line);
        }
        true
    }

    async fn tick_following(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        bot_position: PositionSnapshot,
    ) {
        let (segment, current_index, total) = {
            let mut inner = self.inner.lock().await;
            let Some(plan) = inner.plan.as_mut() else {
                return;
            };
            let total = plan.total_segments();
            let current_index = plan.current_number().saturating_sub(1);
            if let Some(current) = plan.current_mut()
                && current.state == SegmentState::Calculated
            {
                current.state = SegmentState::Active;
            }
            (plan.current().cloned(), current_index, total)
        };
        let Some(segment) = segment else {
            // Every segment walked, but not yet within arrival radius --
            // extend the plan from here.
            self.rebuild_plan_from_here(bot_position).await;
            return;
        };
        if !segment.is_calculated() {
            self.enter_recalculating_and_search(minecraft, bot_position)
                .await;
            return;
        }

        // Displacement: a fall, knockback, or a teleport means the route no
        // longer describes where the bot is.
        if displaced_from(&segment, bot_position.block(), DISPLACEMENT_TOLERANCE) {
            // Everything ahead is suspect, not just this segment: wherever
            // the bot fell (or was teleported) to, the rest of the plan was
            // computed from a position it is no longer at.
            let invalidated = {
                let mut inner = self.inner.lock().await;
                inner.snapshot.replans += 1;
                match inner.plan.as_mut() {
                    Some(plan) => {
                        plan.invalidate_from_current();
                        plan.total_segments().saturating_sub(current_index)
                    }
                    None => 0,
                }
            };
            debug::trace(
                self.config.debug_pathfinding,
                debug::format_replan("bot displaced from route", invalidated, total),
            );
            self.enter_recalculating_and_search(minecraft, bot_position)
                .await;
            return;
        }

        // Periodic revalidation against freshly sampled terrain -- how a
        // chunk change, a closed door, or someone else's building is noticed
        // without depending on a block-change event feed.
        if self.revalidation_due().await
            && !self
                .revalidate(minecraft, &segment, bot_position, total)
                .await
        {
            return;
        }

        let outcome = {
            let mut inner = self.inner.lock().await;
            let arrival = self.config.segment_arrival_radius;
            let stuck = Duration::from_secs(self.config.segment_stuck_seconds);
            let Inner { follower, .. } = &mut *inner;
            follower
                .tick(minecraft, movement, &segment, bot_position, arrival, stuck)
                .await
        };

        match outcome {
            FollowOutcome::Walking => {
                self.maybe_prefetch(minecraft, &segment, bot_position).await;
                if self.track_progress(bot_position).await {
                    self.fail(NavigationFailure::Stuck).await;
                    return;
                }
                let mut inner = self.inner.lock().await;
                inner.snapshot.current_segment = current_index + 1;
            }
            FollowOutcome::SegmentComplete => {
                self.complete_segment(minecraft, bot_position, &segment)
                    .await;
            }
            FollowOutcome::Blocked => {
                debug::trace(
                    self.config.debug_pathfinding,
                    debug::format_replan("segment blocked", 1, total),
                );
                self.invalidate_current_and_recalculate(minecraft, bot_position)
                    .await;
            }
            FollowOutcome::Failed(reason) => {
                self.fail(NavigationFailure::Movement(reason)).await;
            }
        }
    }

    /// Records how close the bot has got to the final destination, and
    /// reports whether the whole trip has stalled.
    ///
    /// "Stalled" here means genuinely no net progress for several times the
    /// per-segment stuck timeout -- long enough that the segment-level
    /// detector has already had multiple chances to replan around whatever
    /// is in the way. Distance is measured along the remaining segment chain
    /// rather than as the crow flies, so walking a long way around an
    /// obstacle still counts as progress.
    async fn track_progress(&self, bot_position: PositionSnapshot) -> bool {
        let mut inner = self.inner.lock().await;
        let Some(destination) = inner.destination else {
            return false;
        };
        let direct = block_distance(bot_position.block(), destination);
        let remaining = inner
            .plan
            .as_ref()
            .map(SegmentPlan::remaining_distance)
            .filter(|distance| *distance > 0.0)
            .unwrap_or(direct);
        inner.snapshot.distance_remaining = Some(remaining.max(direct));
        let improved = inner
            .best_distance
            .is_none_or(|best| direct < best - PROGRESS_EPSILON);
        if improved {
            inner.best_distance = Some(direct);
            inner.last_progress = Instant::now();
            return false;
        }
        inner.last_progress.elapsed()
            >= Duration::from_secs(self.config.segment_stuck_seconds) * STUCK_TRIP_MULTIPLIER
    }

    /// Whether the bot is close enough to the destination to stop. Also
    /// stops the movement layer, so nothing keeps walking after arrival.
    async fn check_arrival(
        &self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        bot_position: PositionSnapshot,
    ) -> bool {
        let destination = { self.inner.lock().await.destination };
        let Some(destination) = destination else {
            return false;
        };
        if block_distance(bot_position.block(), destination) > self.config.arrival_radius {
            return false;
        }
        let _ = movement.stop(minecraft).await;
        let mut inner = self.inner.lock().await;
        inner.planner.cancel();
        inner.snapshot.state = NavigationState::Arrived;
        inner.snapshot.distance_remaining = Some(0.0);
        inner.snapshot.cost_remaining = 0.0;
        if let Some(plan) = inner.plan.as_ref() {
            inner.snapshot.current_segment = plan.total_segments();
        }
        // Deliberately not logged here: `App`'s own command handler
        // reports the user-facing "Destination reached", the same division
        // every other controller in this codebase follows (see
        // `combat::executor::lose_target`'s doc comment). Logging it in both
        // places would print it twice for one `/goto`.
        debug::trace(
            self.config.debug_pathfinding,
            format!(
                "[Pathfinder] Arrived at ({}, {}, {})",
                destination.x, destination.y, destination.z
            ),
        );
        true
    }

    /// Moves to the next segment. When the segment ended short of where it
    /// was planned to (a partial path -- the search ran out of loaded world),
    /// the rest of the plan is rebuilt from the new position instead, since
    /// every later segment's start is now wrong.
    async fn complete_segment(
        &self,
        minecraft: &MinecraftClient,
        bot_position: PositionSnapshot,
        segment: &crate::pathfinding::segment::PathSegment,
    ) {
        // A segment whose route stopped short of its planned end (the
        // search hit the edge of the loaded world) leaves every later
        // segment starting from a position the bot will never stand at, so
        // the rest of the plan has to be rebuilt rather than walked.
        let ended_short = segment.partial;

        let mut inner = self.inner.lock().await;
        inner.failed_searches = 0;
        if let Some(plan) = inner.plan.as_mut() {
            plan.advance();
            let (current, cost) = (plan.current_number(), plan.remaining_cost());
            inner.snapshot.current_segment = current;
            inner.snapshot.cost_remaining = cost;
        }
        inner.follower = SegmentFollower::new();
        let finished = inner.plan.as_ref().is_some_and(SegmentPlan::finished);
        drop(inner);

        if ended_short || finished {
            self.rebuild_plan_from_here(bot_position).await;
            return;
        }
        // The next segment may already be calculated (prefetch); if not,
        // start its search now.
        let needs_search = {
            let inner = self.inner.lock().await;
            inner
                .plan
                .as_ref()
                .and_then(SegmentPlan::current)
                .is_some_and(|next| !next.is_calculated())
        };
        if needs_search {
            self.enter_recalculating_and_search(minecraft, bot_position)
                .await;
        } else {
            let mut inner = self.inner.lock().await;
            if let Some(next) = inner.plan.as_ref().and_then(SegmentPlan::current) {
                inner.snapshot.current_actions = next.actions.clone();
            }
        }
    }

    /// Starts the next segment's search while the current one is still being
    /// walked, so the bot doesn't stop at the boundary waiting for it.
    async fn maybe_prefetch(
        &self,
        minecraft: &MinecraftClient,
        segment: &crate::pathfinding::segment::PathSegment,
        bot_position: PositionSnapshot,
    ) {
        let walked = {
            let total = segment.length().max(1.0);
            let remaining = block_distance(bot_position.block(), segment.end);
            1.0 - (remaining / total).clamp(0.0, 1.0)
        };
        if walked < PREFETCH_AT {
            return;
        }
        let should_prefetch = {
            let inner = self.inner.lock().await;
            !inner.planner.is_searching() && inner.search_target(true).is_some()
        };
        if should_prefetch {
            self.drive_search(minecraft, bot_position, true).await;
        }
    }

    /// Re-samples the corridor of the current segment and checks its
    /// remaining waypoints are still standable. Returns whether the route
    /// survived.
    async fn revalidate(
        &self,
        minecraft: &MinecraftClient,
        segment: &crate::pathfinding::segment::PathSegment,
        bot_position: PositionSnapshot,
        total: usize,
    ) -> bool {
        {
            let mut inner = self.inner.lock().await;
            inner.last_revalidation = Instant::now();
        }
        let Ok(sample) = sampler::sample_corridor(
            minecraft,
            bot_position.block(),
            segment.end,
            self.config.sample_margin,
            self.config.vertical_window,
        )
        .await
        else {
            return true;
        };
        if !sample.has_terrain() {
            return true;
        }
        {
            let mut inner = self.inner.lock().await;
            sampler::record(&mut inner.cache, &sample);
        }
        // Only waypoints inside the freshly sampled, known region can be
        // judged: one outside it is unverifiable, not broken.
        let broken = segment.waypoints.iter().any(|waypoint| {
            sample.grid.bounds().contains(*waypoint)
                && sample.grid.get(*waypoint).known()
                && sample
                    .grid
                    .nearest_standable(*waypoint, BODY_HEIGHT, 2)
                    .is_none()
        });
        if !broken {
            return true;
        }
        debug::trace(
            self.config.debug_pathfinding,
            debug::format_replan("route no longer walkable", 1, total),
        );
        self.invalidate_current_and_recalculate(minecraft, bot_position)
            .await;
        false
    }

    async fn revalidation_due(&self) -> bool {
        let inner = self.inner.lock().await;
        inner.last_revalidation.elapsed() >= REVALIDATE_INTERVAL
    }

    async fn invalidate_current_and_recalculate(
        &self,
        minecraft: &MinecraftClient,
        bot_position: PositionSnapshot,
    ) {
        {
            let mut inner = self.inner.lock().await;
            if let Some(segment) = inner.plan.as_mut().and_then(SegmentPlan::current_mut) {
                segment.invalidate();
            }
            inner.snapshot.replans += 1;
        }
        self.enter_recalculating_and_search(minecraft, bot_position)
            .await;
    }

    async fn enter_recalculating_and_search(
        &self,
        minecraft: &MinecraftClient,
        bot_position: PositionSnapshot,
    ) {
        {
            let mut inner = self.inner.lock().await;
            inner.enter_recalculating();
        }
        self.drive_search(minecraft, bot_position, false).await;
    }

    /// Throws away the remaining plan and builds a fresh coarse route from
    /// the bot's current position, keeping the segments already walked.
    ///
    /// This is the "a better route appeared" / "the world data changed" path
    /// -- and also how a trip continues past the edge of what was loaded
    /// when it started, which is the normal case for any long journey.
    async fn rebuild_plan_from_here(&self, bot_position: PositionSnapshot) {
        let mut inner = self.inner.lock().await;
        let Some(destination) = inner.destination else {
            return;
        };
        // Always from the bot's live position, never from where the trip
        // started or where the old plan thought this segment began: this is
        // called precisely when those two have diverged (a partial path that
        // stopped short, a plan that ran out of segments before arriving).
        let current_start = bot_position.block();
        let route = route::plan(
            current_start,
            destination,
            &inner.cache,
            &RouteProfile::default(),
            self.config.segment_length,
        );
        let walked = inner
            .plan
            .as_ref()
            .map(|plan| plan.current_number().saturating_sub(1))
            .unwrap_or(0);
        let cost_per_block = self.config.costs.walk;
        match inner.plan.as_mut() {
            Some(plan) => {
                plan.replace_tail(walked, current_start, &route.waypoints, cost_per_block)
            }
            None => {
                inner.plan = Some(SegmentPlan::from_waypoints(
                    current_start,
                    &route.waypoints,
                    cost_per_block,
                ));
            }
        }
        inner.follower = SegmentFollower::new();
        if let Some(totals) = inner.plan.as_ref().map(|plan| {
            (
                plan.total_segments(),
                plan.current_number(),
                plan.remaining_cost(),
            )
        }) {
            let (total, current, cost) = totals;
            inner.snapshot.total_segments = total;
            inner.snapshot.current_segment = current;
            inner.snapshot.cost_remaining = cost;
        }
        inner.snapshot.replans += 1;
        inner.enter_recalculating();
        debug::trace(
            self.config.debug_pathfinding,
            debug::format_plan(current_start, destination, route.waypoints.len()),
        );
    }

    async fn fail(&self, failure: NavigationFailure) {
        let mut inner = self.inner.lock().await;
        inner.planner.cancel();
        inner.snapshot.state = NavigationState::Failed;
        inner.snapshot.failure = Some(failure);
    }
}

impl Inner {
    fn enter_recalculating(&mut self) {
        self.snapshot.state = NavigationState::Recalculating;
        self.follower = SegmentFollower::new();
    }

    /// The segment a search should run for, as `(index, from, to)`.
    ///
    /// `prefetch` asks for the one after the current segment instead;
    /// `None` when there is nothing to search (no plan, or the target
    /// segment already has a route).
    fn search_target(&self, prefetch: bool) -> Option<(usize, BlockPosition, BlockPosition)> {
        let plan = self.plan.as_ref()?;
        let offset = usize::from(prefetch);
        let index = plan.current_number().saturating_sub(1) + offset;
        let segment = plan.segments().get(index)?;
        if matches!(
            segment.state,
            SegmentState::Calculated | SegmentState::Active
        ) {
            return None;
        }
        Some((index, segment.start, segment.end))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        AccountMode, BridgingConfig, ConsoleConfig, MinecraftConfig, MovementConfig,
        MultitaskingConfig, ReconnectConfig, VerticalNavigationConfig, WorldStateConfig,
    };

    fn minecraft() -> MinecraftClient {
        MinecraftClient::new(
            MinecraftConfig {
                server: "localhost:25565".to_owned(),
                username: "MagicBot".to_owned(),
                account_mode: AccountMode::Offline,
            },
            ReconnectConfig {
                enabled: false,
                delay_seconds: 10,
                maximum_attempts: 5,
            },
            ConsoleConfig::default(),
            WorldStateConfig::default(),
            VerticalNavigationConfig::default(),
            BridgingConfig::default(),
        )
    }

    fn movement() -> MovementService {
        MovementService::new(MovementConfig::default(), MultitaskingConfig::default())
    }

    fn controller() -> PathfindingController {
        PathfindingController::new(
            PathfindingConfig::default(),
            VerticalNavigationConfig::default(),
        )
    }

    fn position(x: f64, y: f64, z: f64) -> PositionSnapshot {
        PositionSnapshot { x, y, z }
    }

    #[tokio::test]
    async fn starts_idle() {
        let controller = controller();
        assert_eq!(controller.snapshot().await.state, NavigationState::Idle);
        assert!(!controller.snapshot().await.state.active());
    }

    #[tokio::test]
    async fn starting_without_a_connection_fails_without_touching_state() {
        let controller = controller();
        let result = controller
            .start(&minecraft(), position(100.0, 64.0, 100.0))
            .await;
        assert!(matches!(result, Err(AppError::MovementUnavailable)));
        assert_eq!(controller.snapshot().await.state, NavigationState::Idle);
    }

    #[tokio::test]
    async fn non_finite_coordinates_are_rejected() {
        let controller = controller();
        let result = controller
            .start(&minecraft(), position(f64::NAN, 64.0, 0.0))
            .await;
        assert!(matches!(result, Err(AppError::InvalidCoordinates(_))));
    }

    #[tokio::test]
    async fn cancelling_an_idle_controller_is_a_no_op() {
        let controller = controller();
        controller.cancel(&minecraft(), &movement()).await;
        assert_eq!(controller.snapshot().await.state, NavigationState::Idle);
    }

    #[tokio::test]
    async fn ticking_an_untouched_controller_does_not_panic() {
        let controller = controller();
        controller.tick(&minecraft(), &movement()).await;
        assert_eq!(controller.snapshot().await.state, NavigationState::Idle);
    }

    #[tokio::test]
    async fn a_world_change_with_no_plan_invalidates_nothing() {
        let controller = controller();
        assert!(
            !controller
                .notify_world_change(BlockPosition { x: 0, y: 64, z: 0 })
                .await
        );
    }

    #[tokio::test]
    async fn search_targets_track_the_current_and_next_segment() {
        let controller = controller();
        {
            let mut inner = controller.inner.lock().await;
            inner.plan = Some(SegmentPlan::from_waypoints(
                BlockPosition { x: 0, y: 64, z: 0 },
                &[
                    BlockPosition { x: 48, y: 64, z: 0 },
                    BlockPosition { x: 96, y: 64, z: 0 },
                    BlockPosition {
                        x: 144,
                        y: 64,
                        z: 0,
                    },
                ],
                1.0,
            ));
        }
        let inner = controller.inner.lock().await;
        let (index, from, to) = inner.search_target(false).expect("a current segment");
        assert_eq!(index, 0);
        assert_eq!(from, BlockPosition { x: 0, y: 64, z: 0 });
        assert_eq!(to, BlockPosition { x: 48, y: 64, z: 0 });
        let (next_index, _, next_to) = inner.search_target(true).expect("a next segment");
        assert_eq!(next_index, 1);
        assert_eq!(next_to, BlockPosition { x: 96, y: 64, z: 0 });
    }

    #[tokio::test]
    async fn an_already_calculated_segment_is_not_searched_again() {
        let controller = controller();
        {
            let mut inner = controller.inner.lock().await;
            let mut plan = SegmentPlan::from_waypoints(
                BlockPosition { x: 0, y: 64, z: 0 },
                &[BlockPosition { x: 48, y: 64, z: 0 }],
                1.0,
            );
            plan.current_mut().unwrap().state = SegmentState::Calculated;
            inner.plan = Some(plan);
        }
        let inner = controller.inner.lock().await;
        assert!(inner.search_target(false).is_none());
    }
}
