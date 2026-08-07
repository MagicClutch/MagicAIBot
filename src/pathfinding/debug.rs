//! `debug_pathfinding` output. Pure formatting plus a thin logging wrapper,
//! so the interesting part (what the lines actually say) is testable without
//! capturing stdout.
//!
//! Everything here is gated on `PathfindingConfig::debug_pathfinding`. With
//! it off, the planner emits only the ordinary milestone lines every other
//! task in this codebase emits; with it on, it narrates each plan, each
//! segment, and each replan -- the difference between "the bot is walking
//! somewhere" and "the bot is on segment 5 of 24, having replanned twice
//! because a chunk changed".

use crate::{
    logging,
    minecraft::world_state::BlockPosition,
    pathfinding::{
        segment::{PathSegment, SegmentPlan},
        state::NavigationSnapshot,
    },
};

/// The multi-line "planning route" block, matching the shape the spec asks
/// for.
#[must_use]
pub fn format_plan(start: BlockPosition, destination: BlockPosition, segments: usize) -> String {
    format!(
        "[Pathfinder]\nPlanning route:\nStart: {} {} {}\nTarget: {} {} {}\nSegments: {segments}",
        start.x, start.y, start.z, destination.x, destination.y, destination.z
    )
}

/// One line describing the segment about to be walked.
#[must_use]
pub fn format_segment(plan: &SegmentPlan, segment: &PathSegment) -> String {
    let actions = if segment.actions.is_empty() {
        "walk".to_owned()
    } else {
        segment
            .actions
            .iter()
            .map(|action| action.label())
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "[Pathfinder] Current segment: {}/{} -> ({} {} {}) cost {:.1}, {} waypoints [{actions}]",
        segment.index + 1,
        plan.total_segments(),
        segment.end.x,
        segment.end.y,
        segment.end.z,
        segment.estimated_cost,
        segment.waypoints.len(),
    )
}

/// One line for a replan, saying what forced it.
#[must_use]
pub fn format_replan(reason: &str, invalidated: usize, total: usize) -> String {
    format!("[Pathfinder] Recalculating ({reason}): {invalidated}/{total} segments discarded")
}

/// A compact status line, used by `/pathstatus` whether or not debug is on.
#[must_use]
pub fn format_status(snapshot: &NavigationSnapshot) -> String {
    let destination = snapshot
        .destination
        .map(|position| format!("({}, {}, {})", position.x, position.y, position.z))
        .unwrap_or_else(|| "none".to_owned());
    let origin = snapshot
        .start
        .map(|position| format!("({}, {}, {})", position.x, position.y, position.z))
        .unwrap_or_else(|| "none".to_owned());
    let distance = snapshot
        .distance_remaining
        .map(|blocks| format!("{blocks:.0} blocks"))
        .unwrap_or_else(|| "unknown".to_owned());
    let failure = snapshot
        .failure
        .as_ref()
        .map(|failure| format!("; {failure}"))
        .unwrap_or_default();
    let elapsed = snapshot
        .started_at
        .and_then(|started| started.elapsed().ok())
        .map(|elapsed| format!("; elapsed={}s", elapsed.as_secs()))
        .unwrap_or_default();
    format!(
        "Navigation: {}; from={origin}; target={destination}; segment={}/{} ({:.0}%); \
         remaining={distance}; cost={:.1}; replans={}; last search={} nodes in \
         {}ms{elapsed}{failure}",
        snapshot.state.label(),
        snapshot.current_segment,
        snapshot.total_segments,
        snapshot.progress() * 100.0,
        snapshot.cost_remaining,
        snapshot.replans,
        snapshot.last_search_nodes,
        snapshot.last_search_millis,
    )
}

/// Emits `message` only when debug pathfinding is enabled. Uses
/// `logging::info` rather than `logging::progress` so the whole debug
/// stream lands at one level a user can filter (see `crate::logging`'s
/// `OutputMode`).
pub fn trace(enabled: bool, message: impl std::fmt::Display) {
    if enabled {
        logging::info(message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pathfinding::{
        segment::{SegmentAction, SegmentPlan},
        state::NavigationState,
    };

    fn position(x: i32, y: i32, z: i32) -> BlockPosition {
        BlockPosition { x, y, z }
    }

    #[test]
    fn the_plan_block_matches_the_documented_shape() {
        let text = format_plan(position(10, 64, -5), position(5000, 100, -3000), 24);
        assert_eq!(
            text,
            "[Pathfinder]\nPlanning route:\nStart: 10 64 -5\nTarget: 5000 100 -3000\nSegments: 24"
        );
    }

    #[test]
    fn the_segment_line_reports_position_progress_and_actions() {
        let mut plan = SegmentPlan::from_waypoints(
            position(0, 64, 0),
            &[position(48, 64, 0), position(96, 64, 0)],
            1.0,
        );
        let segment = plan.current_mut().unwrap();
        segment.waypoints = vec![position(20, 64, 0), position(48, 64, 0)];
        segment.actions = vec![SegmentAction::Walk, SegmentAction::Jump];
        segment.estimated_cost = 51.25;
        let line = format_segment(&plan, plan.current().unwrap());
        assert!(line.contains("Current segment: 1/2"));
        assert!(line.contains("(48 64 0)"));
        assert!(line.contains("cost 51.2"));
        assert!(line.contains("2 waypoints"));
        assert!(line.contains("[walk, jump]"));
    }

    #[test]
    fn a_segment_with_no_actions_still_reads_sensibly() {
        let plan = SegmentPlan::from_waypoints(position(0, 64, 0), &[position(48, 64, 0)], 1.0);
        let line = format_segment(&plan, plan.current().unwrap());
        assert!(line.contains("[walk]"));
    }

    #[test]
    fn the_replan_line_names_the_reason() {
        assert_eq!(
            format_replan("chunk changed", 3, 24),
            "[Pathfinder] Recalculating (chunk changed): 3/24 segments discarded"
        );
    }

    #[test]
    fn the_status_line_covers_the_whole_snapshot() {
        let snapshot = NavigationSnapshot {
            state: NavigationState::FollowingSegment,
            start: Some(position(10, 64, -5)),
            destination: Some(position(5000, 100, -3000)),
            current_segment: 5,
            total_segments: 24,
            distance_remaining: Some(4211.7),
            cost_remaining: 5123.45,
            replans: 2,
            last_search_nodes: 4821,
            last_search_millis: 37,
            ..NavigationSnapshot::default()
        };
        let line = format_status(&snapshot);
        assert!(line.contains("FOLLOWING_SEGMENT"));
        assert!(line.contains("from=(10, 64, -5)"));
        assert!(line.contains("target=(5000, 100, -3000)"));
        assert!(line.contains("segment=5/24 (17%)"));
        assert!(line.contains("remaining=4212 blocks"));
        assert!(line.contains("replans=2"));
        assert!(line.contains("4821 nodes in 37ms"));
    }

    #[test]
    fn a_failed_snapshot_reports_why_in_the_status_line() {
        let snapshot = NavigationSnapshot {
            state: NavigationState::Failed,
            failure: Some(crate::pathfinding::state::NavigationFailure::NoRoute),
            ..NavigationSnapshot::default()
        };
        let line = format_status(&snapshot);
        assert!(line.contains("FAILED"));
        assert!(line.contains("no route to the destination"));
    }

    #[test]
    fn an_idle_snapshot_reports_no_target_rather_than_a_placeholder_position() {
        let line = format_status(&NavigationSnapshot::default());
        assert!(line.contains("target=none"));
        assert!(line.contains("remaining=unknown"));
    }
}
