//! The navigation state machine and the snapshot everything else observes.
//! Pure data, mirroring the shape every other controller's state module in
//! this codebase uses (`combat::state`, `container::model`,
//! `navigation::navigation_state`).

use std::time::SystemTime;

use crate::{minecraft::world_state::BlockPosition, pathfinding::segment::SegmentAction};

/// Lifecycle of one long-distance navigation task.
///
/// ```text
///                  start()
///     IDLE ------------------> PLANNING
///       ^                          |  coarse route + first segment search
///       |                          v
///       |                  FOLLOWING_SEGMENT <---------+
///       |                     |        |               |
///       |     segment done ---+        | blocked /     | new segment
///       |     (last one)               | chunk change  | calculated
///       |          |                   v               |
///       |          |             RECALCULATING --------+
///       |          v
///       +------ ARRIVED
/// ```
///
/// `Failed` and `Cancelled` are terminal alongside `Arrived`; they are not
/// in the diagram only because every state can reach them.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum NavigationState {
    #[default]
    Idle,
    /// Computing the coarse route and the first segment.
    Planning,
    /// Walking the current segment's waypoints.
    FollowingSegment,
    /// The current segment is unusable (blocked, invalidated by a chunk
    /// change, or the bot was displaced); a fresh search is in flight.
    Recalculating,
    Arrived,
    Failed,
    Cancelled,
}

impl NavigationState {
    #[must_use]
    pub fn active(self) -> bool {
        matches!(
            self,
            Self::Planning | Self::FollowingSegment | Self::Recalculating
        )
    }

    /// Upper-case name, matching the spec's state vocabulary in logs.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "IDLE",
            Self::Planning => "PLANNING",
            Self::FollowingSegment => "FOLLOWING_SEGMENT",
            Self::Recalculating => "RECALCULATING",
            Self::Arrived => "ARRIVED",
            Self::Failed => "FAILED",
            Self::Cancelled => "CANCELLED",
        }
    }
}

/// Why a navigation task ended, when it didn't end at the destination.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NavigationFailure {
    /// No route exists through known terrain, and repeated replans from
    /// different positions didn't find one either.
    NoRoute,
    /// The bot stopped making progress toward the destination for long
    /// enough that continuing is pointless.
    Stuck,
    /// The movement layer reported a hard failure.
    Movement(String),
    /// Nothing around the bot is loaded (just connected, or a desync).
    NoWorldData,
}

impl std::fmt::Display for NavigationFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoRoute => write!(formatter, "no route to the destination"),
            Self::Stuck => write!(formatter, "no progress toward the destination"),
            Self::Movement(reason) => write!(formatter, "movement failed: {reason}"),
            Self::NoWorldData => write!(formatter, "no loaded terrain around the bot"),
        }
    }
}

/// Everything an observer (status command, debug logging, the blocking wait
/// in `App`) can see about a navigation task.
#[derive(Clone, Debug, Default)]
pub struct NavigationSnapshot {
    pub state: NavigationState,
    pub start: Option<BlockPosition>,
    pub destination: Option<BlockPosition>,
    /// One-based index of the segment being walked.
    pub current_segment: usize,
    pub total_segments: usize,
    /// Straight-line blocks from the bot to the final destination.
    pub distance_remaining: Option<f64>,
    /// Sum of the estimated costs of every segment not yet walked.
    pub cost_remaining: f64,
    /// Actions the *current* segment requires.
    pub current_actions: Vec<SegmentAction>,
    /// How many times this task has had to recalculate. A useful health
    /// signal: a route that replans every few seconds is fighting the
    /// terrain even if it eventually arrives.
    pub replans: u32,
    /// Nodes expanded by the most recent block-level search.
    pub last_search_nodes: usize,
    /// Milliseconds the most recent block-level search took.
    pub last_search_millis: u64,
    pub failure: Option<NavigationFailure>,
    pub started_at: Option<SystemTime>,
}

impl NavigationSnapshot {
    /// Progress through the segment chain, 0.0-1.0. Reports 1.0 once
    /// arrived, and 0.0 before any segment exists, so a caller can render a
    /// bar without special-casing either end.
    #[must_use]
    pub fn progress(&self) -> f64 {
        if self.state == NavigationState::Arrived {
            return 1.0;
        }
        if self.total_segments == 0 {
            return 0.0;
        }
        ((self.current_segment.saturating_sub(1)) as f64 / self.total_segments as f64)
            .clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_snapshot_is_idle_and_shows_no_progress() {
        let snapshot = NavigationSnapshot::default();
        assert_eq!(snapshot.state, NavigationState::Idle);
        assert_eq!(snapshot.progress(), 0.0);
        assert!(!snapshot.state.active());
    }

    #[test]
    fn only_the_in_progress_states_count_as_active() {
        for state in [
            NavigationState::Planning,
            NavigationState::FollowingSegment,
            NavigationState::Recalculating,
        ] {
            assert!(state.active(), "{state:?}");
        }
        for state in [
            NavigationState::Idle,
            NavigationState::Arrived,
            NavigationState::Failed,
            NavigationState::Cancelled,
        ] {
            assert!(!state.active(), "{state:?}");
        }
    }

    #[test]
    fn state_labels_match_the_spec_vocabulary() {
        assert_eq!(NavigationState::Idle.label(), "IDLE");
        assert_eq!(NavigationState::Planning.label(), "PLANNING");
        assert_eq!(
            NavigationState::FollowingSegment.label(),
            "FOLLOWING_SEGMENT"
        );
        assert_eq!(NavigationState::Recalculating.label(), "RECALCULATING");
        assert_eq!(NavigationState::Arrived.label(), "ARRIVED");
    }

    #[test]
    fn progress_tracks_the_segment_index() {
        let snapshot = NavigationSnapshot {
            state: NavigationState::FollowingSegment,
            current_segment: 5,
            total_segments: 24,
            ..NavigationSnapshot::default()
        };
        assert!((snapshot.progress() - 4.0 / 24.0).abs() < 1e-9);
    }

    #[test]
    fn progress_is_complete_once_arrived_regardless_of_segment_bookkeeping() {
        let snapshot = NavigationSnapshot {
            state: NavigationState::Arrived,
            current_segment: 24,
            total_segments: 24,
            ..NavigationSnapshot::default()
        };
        assert_eq!(snapshot.progress(), 1.0);
    }

    #[test]
    fn failures_describe_themselves() {
        assert_eq!(
            NavigationFailure::NoRoute.to_string(),
            "no route to the destination"
        );
        assert_eq!(
            NavigationFailure::Movement("azalea gave up".into()).to_string(),
            "movement failed: azalea gave up"
        );
    }
}
