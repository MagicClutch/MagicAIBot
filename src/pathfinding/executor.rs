//! Walks one calculated segment: feeds its waypoints to the movement layer
//! one at a time and reports what happened.
//!
//! # Why this delegates instead of driving the bot itself
//!
//! Everything above this module is planning -- which cells, in which order,
//! at what cost. Actually *travelling* between two nearby cells is a
//! different and already-solved problem: `MovementService` hands a goal to
//! Azalea's pathfinder, which does jump timing, mining, scaffolding and
//! stuck recovery with a lot of tuning behind it (see
//! `MinecraftClient::start_navigation_to`'s `PathfindingPolicy`).
//!
//! So this module hands down one waypoint at a time. Each hop is short and
//! -- because the planner only ever routes through terrain it sampled as
//! loaded and walkable -- inside terrain Azalea can path through trivially,
//! which is the regime it is reliable in. The planner keeps the properties
//! Azalea's pathfinder alone can't provide (segmentation, cost tuning,
//! hazard avoidance, chunk knowledge, replanning) without giving up the
//! execution quality it already has.

use std::time::{Duration, Instant};

use crate::{
    minecraft::{
        client::MinecraftClient,
        world_state::{BlockPosition, MovementStatus, PositionSnapshot},
    },
    movement::{MovementService, NavigationMode},
    pathfinding::{
        grid::{block_center, block_distance},
        segment::PathSegment,
    },
};

/// How much closer the bot must get for a tick to count as progress. Below
/// this is noise -- a bot standing still still jitters by a few hundredths
/// of a block.
const PROGRESS_EPSILON: f64 = 0.35;

/// What one tick of walking a segment concluded.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FollowOutcome {
    /// Still walking; nothing to decide.
    Walking,
    /// Reached the end of this segment -- advance the plan.
    SegmentComplete,
    /// The bot stopped getting closer for long enough that this segment is
    /// considered blocked. The caller recalculates.
    Blocked,
    /// The movement layer failed outright.
    Failed(String),
}

/// Live state for the segment currently being walked. Owned by the
/// controller; one of these exists per active segment.
#[derive(Clone, Debug)]
pub struct SegmentFollower {
    /// Index into the current segment's waypoint list.
    waypoint_index: usize,
    /// Best (smallest) distance to the current waypoint seen so far, and
    /// when it was seen -- together, the stuck detector.
    best_distance: Option<f64>,
    last_progress: Instant,
    /// Whether a goal has been submitted for the current waypoint yet.
    submitted: bool,
}

impl Default for SegmentFollower {
    fn default() -> Self {
        Self::new()
    }
}

impl SegmentFollower {
    #[must_use]
    pub fn new() -> Self {
        Self {
            waypoint_index: 0,
            best_distance: None,
            last_progress: Instant::now(),
            submitted: false,
        }
    }

    /// Resets progress tracking for a new waypoint (or a new segment).
    fn advance_waypoint(&mut self) {
        self.waypoint_index += 1;
        self.best_distance = None;
        self.last_progress = Instant::now();
        self.submitted = false;
    }

    /// Drives one tick of walking `segment`.
    ///
    /// `arrival_radius` is how close counts as reaching a waypoint, and
    /// `stuck_timeout` how long without progress counts as blocked.
    pub async fn tick(
        &mut self,
        minecraft: &MinecraftClient,
        movement: &MovementService,
        segment: &PathSegment,
        bot_position: PositionSnapshot,
        arrival_radius: f64,
        stuck_timeout: Duration,
    ) -> FollowOutcome {
        let Some(waypoint) = segment.waypoints.get(self.waypoint_index).copied() else {
            return FollowOutcome::SegmentComplete;
        };

        let distance = block_distance(bot_position.block(), waypoint);
        if distance <= arrival_radius {
            if self.waypoint_index + 1 >= segment.waypoints.len() {
                return FollowOutcome::SegmentComplete;
            }
            self.advance_waypoint();
            // Submit the next waypoint on the very next tick rather than
            // recursing: one goal submission per tick keeps the movement
            // layer's own repath cadence in charge of pacing.
            return FollowOutcome::Walking;
        }

        // A movement failure is the only hard error; everything else is
        // handled by the stuck detector below, because Azalea reports plenty
        // of transient "no path" conditions that resolve as chunks load.
        let movement_snapshot = movement.snapshot().await;
        if movement_snapshot.status == MovementStatus::Failed {
            return FollowOutcome::Failed(
                movement_snapshot
                    .failure_reason
                    .unwrap_or_else(|| "movement failed".into()),
            );
        }

        let improved = self
            .best_distance
            .is_none_or(|best| distance < best - PROGRESS_EPSILON);
        if improved {
            self.best_distance = Some(distance);
            self.last_progress = Instant::now();
        } else if self.last_progress.elapsed() >= stuck_timeout {
            return FollowOutcome::Blocked;
        }

        // Resubmit whenever the movement layer has gone idle under us --
        // it completes, fails, or is cancelled on its own schedule, and the
        // segment isn't finished until *this* module says so.
        let needs_goal = !self.submitted
            || !matches!(
                movement_snapshot.status,
                MovementStatus::MovingToPosition | MovementStatus::FollowingPlayer
            );
        if needs_goal {
            let destination = block_center(waypoint);
            if let Err(error) = submit(minecraft, movement, destination).await {
                return FollowOutcome::Failed(error);
            }
            self.submitted = true;
        }
        FollowOutcome::Walking
    }
}

/// Hands one waypoint to the movement layer.
///
/// Uses `goto_for_block_navigation` rather than `goto` so the movement layer
/// stays quiet: it logs "Going to (x, y, z)" for a user-initiated `/goto`,
/// which would print once per waypoint -- dozens of times per segment -- for
/// what is, to the user, a single trip. The pathfinder does its own
/// reporting at segment granularity instead (see
/// `crate::pathfinding::debug`).
async fn submit(
    minecraft: &MinecraftClient,
    movement: &MovementService,
    destination: PositionSnapshot,
) -> Result<(), String> {
    movement
        .goto_for_block_navigation(minecraft, destination, NavigationMode::AllowMining)
        .await
        .map_err(|error| error.to_string())
}

/// Whether the bot has strayed far enough from a calculated route that the
/// route no longer describes where it is -- a fall, a teleport, knockback
/// into a ravine. Checked against the *nearest* waypoint rather than the
/// current one, so ordinary drift along the path never trips it.
#[must_use]
pub fn displaced_from(segment: &PathSegment, position: BlockPosition, tolerance: f64) -> bool {
    if segment.waypoints.is_empty() {
        return false;
    }
    let nearest = segment
        .waypoints
        .iter()
        .map(|waypoint| block_distance(*waypoint, position))
        .fold(f64::MAX, f64::min);
    nearest > tolerance
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pathfinding::segment::PathSegment;

    fn position(x: i32, y: i32, z: i32) -> BlockPosition {
        BlockPosition { x, y, z }
    }

    fn segment_with(waypoints: Vec<BlockPosition>) -> PathSegment {
        let mut segment = PathSegment::planned(
            0,
            position(0, 64, 0),
            waypoints.last().copied().unwrap_or(position(0, 64, 0)),
            1.0,
        );
        segment.waypoints = waypoints;
        segment
    }

    #[test]
    fn a_new_follower_starts_at_the_first_waypoint() {
        let follower = SegmentFollower::new();
        assert_eq!(follower.waypoint_index, 0);
    }

    #[test]
    fn advancing_resets_progress_tracking_for_the_next_waypoint() {
        let mut follower = SegmentFollower::new();
        follower.best_distance = Some(4.0);
        follower.submitted = true;
        follower.advance_waypoint();
        assert_eq!(follower.waypoint_index, 1);
        assert_eq!(follower.best_distance, None);
        assert!(!follower.submitted);
    }

    #[test]
    fn displacement_is_measured_against_the_nearest_waypoint() {
        let segment = segment_with(vec![
            position(0, 64, 0),
            position(20, 64, 0),
            position(40, 64, 0),
        ]);
        assert!(
            !displaced_from(&segment, position(21, 64, 2), 6.0),
            "ordinary drift along the route is not displacement"
        );
        assert!(
            displaced_from(&segment, position(20, 20, 0), 6.0),
            "falling 44 blocks down a ravine is"
        );
        assert!(displaced_from(&segment, position(200, 64, 200), 6.0));
    }

    #[test]
    fn a_segment_with_no_waypoints_is_never_displaced() {
        let segment = segment_with(Vec::new());
        assert!(!displaced_from(&segment, position(500, 64, 500), 2.0));
    }

    #[test]
    fn follow_outcomes_compare_by_value_for_the_controller_state_machine() {
        assert_eq!(FollowOutcome::Walking, FollowOutcome::Walking);
        assert_ne!(FollowOutcome::Walking, FollowOutcome::Blocked);
        assert_eq!(
            FollowOutcome::Failed("x".into()),
            FollowOutcome::Failed("x".into())
        );
    }
}
