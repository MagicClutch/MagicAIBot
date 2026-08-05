use std::time::{Duration, SystemTime};

use crate::{
    minecraft::world_state::{BlockPosition, PositionSnapshot},
    movement::NavigationMode,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BlockNavigationState {
    #[default]
    Idle,
    Searching,
    SelectingTarget,
    Moving,
    #[allow(dead_code)] // retained for status compatibility; Azalea now owns replanning.
    Repathing,
    Reached,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Default)]
pub struct BlockNavigationSnapshot {
    pub state: BlockNavigationState,
    /// The full set of block ids this search matches against -- always at
    /// least one entry once a search has started. `requested_block_id`
    /// mirrors the first entry for callers that only ever cared about a
    /// single id (existing `/gotoblock`/status-display code); anything that
    /// needs the complete set (multi-block `#get`/`#mine`) reads this
    /// directly.
    pub requested_block_ids: Vec<String>,
    pub requested_block_id: Option<String>,
    pub search_radius: Option<u32>,
    pub selected_block_position: Option<BlockPosition>,
    pub selected_approach_position: Option<BlockPosition>,
    /// The specific block id of `selected_block_position`, as observed when
    /// it was selected -- distinct from `requested_block_id` once more than
    /// one id is being searched for, since the two candidates picked across
    /// separate attempts need not share an id (e.g. `diamond_ore` vs.
    /// `deepslate_diamond_ore`). `tick`'s ongoing re-validation must key off
    /// this, not `requested_block_id`, or it would reject a perfectly valid
    /// target for "not matching" an id it was never supposed to match.
    pub selected_block_id: Option<String>,
    pub candidates_checked: usize,
    pub start_time: Option<SystemTime>,
    pub last_progress_time: Option<SystemTime>,
    pub last_position: Option<PositionSnapshot>,
    pub failure_reason: Option<String>,
    pub current_attempt: usize,
    pub maximum_attempts: usize,
    pub generation: u64,
    pub mode: NavigationMode,
}

pub fn arrival_valid(
    current: Option<PositionSnapshot>,
    approach: BlockPosition,
    target: BlockPosition,
    arrival_distance: f64,
    interaction_distance: f64,
) -> bool {
    let Some(current) = current else { return false };
    distance(current, center(approach)) <= arrival_distance
        && distance(current, center(target)) <= interaction_distance
}

pub fn timed_out(start: Option<SystemTime>, maximum_seconds: u64) -> bool {
    start.is_some_and(|start| {
        start.elapsed().unwrap_or_default() >= Duration::from_secs(maximum_seconds)
    })
}

fn center(position: BlockPosition) -> PositionSnapshot {
    PositionSnapshot {
        x: f64::from(position.x) + 0.5,
        y: f64::from(position.y),
        z: f64::from(position.z) + 0.5,
    }
}

fn distance(a: PositionSnapshot, b: PositionSnapshot) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2) + (a.z - b.z).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_arrival_and_rejects_false_path_completion() {
        let target = BlockPosition { x: 0, y: 64, z: 0 };
        let approach = BlockPosition { x: -1, y: 64, z: 0 };
        assert!(arrival_valid(
            Some(PositionSnapshot {
                x: -0.5,
                y: 64.0,
                z: 0.5
            }),
            approach,
            target,
            1.5,
            4.5
        ));
        assert!(!arrival_valid(
            Some(PositionSnapshot {
                x: 100.0,
                y: 64.0,
                z: 100.0
            }),
            approach,
            target,
            1.5,
            4.5
        ));
    }

    #[test]
    fn detects_timeout() {
        let old = Some(SystemTime::now() - Duration::from_secs(20));
        assert!(timed_out(old, 12));
    }
}
