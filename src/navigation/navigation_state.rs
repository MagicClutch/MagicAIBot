use std::time::{Duration, SystemTime};

use crate::minecraft::world_state::{BlockPosition, PositionSnapshot};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BlockNavigationState {
    #[default]
    Idle,
    Searching,
    SelectingTarget,
    Moving,
    Repathing,
    Reached,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Default)]
pub struct BlockNavigationSnapshot {
    pub state: BlockNavigationState,
    pub requested_block_id: Option<String>,
    pub search_radius: Option<u32>,
    pub selected_block_position: Option<BlockPosition>,
    pub selected_approach_position: Option<BlockPosition>,
    pub candidates_checked: usize,
    pub start_time: Option<SystemTime>,
    pub last_progress_time: Option<SystemTime>,
    pub last_position: Option<PositionSnapshot>,
    pub failure_reason: Option<String>,
    pub current_attempt: usize,
    pub maximum_attempts: usize,
    pub generation: u64,
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

pub fn is_stuck(last_progress: Option<SystemTime>, timeout_seconds: u64) -> bool {
    last_progress.is_some_and(|last| {
        last.elapsed().unwrap_or_default() >= Duration::from_secs(timeout_seconds)
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
    fn detects_stuck_and_timeout() {
        let old = Some(SystemTime::now() - Duration::from_secs(20));
        assert!(is_stuck(old, 12));
        assert!(timed_out(old, 12));
    }
}
