//! Deterministic lifecycle for an explicitly requested block break.
//!
//! This module deliberately does not replace Azalea's pathfinder mining.  It
//! owns only mining dispatched for a [`BlockBreakingRequest`].  The adapter
//! executes returned actions and feeds authoritative world observations back
//! into the machine.

use std::time::Duration;

use crate::minecraft::world_state::{BlockPosition, InventorySnapshot};

#[derive(Clone, Debug, PartialEq)]
pub struct BlockBreakingRequest {
    pub target: BlockPosition,
    pub expected_block: String,
    pub interaction_range: f64,
    pub navigation_allowed: bool,
    pub timeout: Duration,
    pub confirmation_timeout: Duration,
    pub maximum_retries: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockBreakingPhase {
    Validating,
    Navigating,
    Looking,
    Mining,
    Confirming,
    RetryWait,
    Finished,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BlockBreakingOutcome {
    Broken,
    TargetAlreadyAbsent,
    TargetChanged,
    TargetNotFound,
    ChunkUnloaded,
    OutOfRange,
    Unreachable,
    LookRejected,
    ServerNotConfirmed,
    TimedOut,
    Cancelled,
    Died,
    Disconnected,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BlockBreakingSnapshot {
    pub operation_id: u64,
    pub request: BlockBreakingRequest,
    pub phase: BlockBreakingPhase,
    pub elapsed: Duration,
    pub progress_percent: u8,
    pub dispatch_count: u32,
    pub retry_count: u32,
    pub outcome: Option<BlockBreakingOutcome>,
    /// Task 1 ownership: the operation generation guards movement, precise
    /// look, and interaction-hand leases. Cleanup releases only these leases.
    pub leases: BlockBreakingLeases,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BlockBreakingLeases {
    pub movement: bool,
    pub precise_look: bool,
    pub interaction_hand: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Observation {
    Valid { in_range: bool },
    NavigationReached,
    NavigationFailed,
    LookAcquired,
    LookRejected,
    Block(String),
    Air,
    ChunkUnloaded,
    Tick(Duration),
    Cancel,
    Death,
    Disconnect,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Navigate,
    AcquirePreciseLook,
    DispatchMining,
    AbortMining,
    Cleanup,
}

/// Integration point for Task 3 inventory policy. Implementations may select
/// a hotbar slot; this task intentionally contains no scoring policy.
pub trait ToolSelector: Send + Sync {
    fn select(&self, inventory: &InventorySnapshot, block_id: &str) -> Option<u8>;
}

#[derive(Debug, Default)]
pub struct KeepCurrentTool;
impl ToolSelector for KeepCurrentTool {
    fn select(&self, _inventory: &InventorySnapshot, _block_id: &str) -> Option<u8> {
        None
    }
}

impl BlockBreakingSnapshot {
    pub fn new(operation_id: u64, request: BlockBreakingRequest) -> Self {
        Self {
            operation_id,
            request,
            phase: BlockBreakingPhase::Validating,
            elapsed: Duration::ZERO,
            progress_percent: 0,
            dispatch_count: 0,
            retry_count: 0,
            outcome: None,
            leases: BlockBreakingLeases {
                movement: true,
                precise_look: true,
                interaction_hand: true,
            },
        }
    }

    pub fn observe(&mut self, observation: Observation) -> Vec<Action> {
        if self.outcome.is_some() {
            return vec![];
        }
        match observation {
            Observation::Cancel => return self.finish(BlockBreakingOutcome::Cancelled, true),
            Observation::Death => return self.finish(BlockBreakingOutcome::Died, true),
            Observation::Disconnect => {
                return self.finish(BlockBreakingOutcome::Disconnected, true);
            }
            Observation::Tick(delta) => {
                self.elapsed += delta;
                self.progress_percent = ((self.elapsed.as_secs_f64()
                    / self.request.timeout.as_secs_f64().max(0.001))
                    * 95.0) as u8;
                self.progress_percent = self.progress_percent.min(95);
                if self.elapsed >= self.request.timeout {
                    return self.finish(BlockBreakingOutcome::TimedOut, true);
                }
                if self.phase == BlockBreakingPhase::Confirming
                    && self.elapsed >= self.request.confirmation_timeout
                {
                    if self.retry_count < self.request.maximum_retries {
                        self.retry_count += 1;
                        self.phase = BlockBreakingPhase::RetryWait;
                        return vec![Action::AbortMining, Action::AcquirePreciseLook];
                    }
                    return self.finish(BlockBreakingOutcome::ServerNotConfirmed, true);
                }
            }
            Observation::Valid { in_range: true } => {
                self.phase = BlockBreakingPhase::Looking;
                return vec![Action::AcquirePreciseLook];
            }
            Observation::Valid { in_range: false } if self.request.navigation_allowed => {
                self.phase = BlockBreakingPhase::Navigating;
                return vec![Action::Navigate];
            }
            Observation::Valid { in_range: false } => {
                return self.finish(BlockBreakingOutcome::OutOfRange, false);
            }
            Observation::NavigationReached => {
                self.phase = BlockBreakingPhase::Looking;
                return vec![Action::AcquirePreciseLook];
            }
            Observation::NavigationFailed => {
                return self.finish(BlockBreakingOutcome::Unreachable, false);
            }
            Observation::LookAcquired
                if matches!(
                    self.phase,
                    BlockBreakingPhase::Looking | BlockBreakingPhase::RetryWait
                ) =>
            {
                // Transition before returning the side effect: duplicate look completions cannot dispatch twice.
                self.phase = BlockBreakingPhase::Mining;
                self.dispatch_count += 1;
                return vec![Action::DispatchMining];
            }
            Observation::LookRejected => {
                return self.finish(BlockBreakingOutcome::LookRejected, false);
            }
            Observation::Air if self.dispatch_count == 0 => {
                return self.finish(BlockBreakingOutcome::TargetAlreadyAbsent, false);
            }
            Observation::Air => return self.finish(BlockBreakingOutcome::Broken, false),
            Observation::Block(id) if id != self.request.expected_block => {
                return self.finish(BlockBreakingOutcome::TargetChanged, true);
            }
            Observation::Block(_) if self.phase == BlockBreakingPhase::Mining => {
                self.phase = BlockBreakingPhase::Confirming
            }
            Observation::ChunkUnloaded => {
                return self.finish(BlockBreakingOutcome::ChunkUnloaded, true);
            }
            _ => {}
        }
        vec![]
    }

    fn finish(&mut self, outcome: BlockBreakingOutcome, abort: bool) -> Vec<Action> {
        self.phase = BlockBreakingPhase::Finished;
        self.progress_percent = if outcome == BlockBreakingOutcome::Broken {
            100
        } else {
            self.progress_percent
        };
        self.outcome = Some(outcome);
        self.leases = BlockBreakingLeases::default();
        if abort {
            vec![Action::AbortMining, Action::Cleanup]
        } else {
            vec![Action::Cleanup]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn machine() -> BlockBreakingSnapshot {
        BlockBreakingSnapshot::new(
            7,
            BlockBreakingRequest {
                target: BlockPosition { x: 1, y: 2, z: 3 },
                expected_block: "minecraft:stone".into(),
                interaction_range: 4.5,
                navigation_allowed: true,
                timeout: Duration::from_secs(10),
                confirmation_timeout: Duration::from_secs(2),
                maximum_retries: 1,
            },
        )
    }
    #[test]
    fn disappearance_and_change_are_distinct() {
        let mut m = machine();
        assert_eq!(m.observe(Observation::Air), vec![Action::Cleanup]);
        assert_eq!(m.outcome, Some(BlockBreakingOutcome::TargetAlreadyAbsent));
        let mut m = machine();
        m.observe(Observation::Valid { in_range: true });
        m.observe(Observation::LookAcquired);
        assert!(
            m.observe(Observation::Block("minecraft:dirt".into()))
                .contains(&Action::AbortMining)
        );
        assert_eq!(m.outcome, Some(BlockBreakingOutcome::TargetChanged));
    }
    #[test]
    fn unload_is_terminal_and_cleans_up() {
        let mut m = machine();
        assert_eq!(
            m.observe(Observation::ChunkUnloaded),
            vec![Action::AbortMining, Action::Cleanup]
        );
        assert_eq!(m.leases, BlockBreakingLeases::default());
    }
    #[test]
    fn range_and_path_failures_are_typed() {
        let mut m = machine();
        m.request.navigation_allowed = false;
        m.observe(Observation::Valid { in_range: false });
        assert_eq!(m.outcome, Some(BlockBreakingOutcome::OutOfRange));
        let mut m = machine();
        m.observe(Observation::Valid { in_range: false });
        m.observe(Observation::NavigationFailed);
        assert_eq!(m.outcome, Some(BlockBreakingOutcome::Unreachable));
    }
    #[test]
    fn non_confirmation_retries_once_then_rejects() {
        let mut m = machine();
        m.observe(Observation::Valid { in_range: true });
        m.observe(Observation::LookAcquired);
        m.observe(Observation::Block("minecraft:stone".into()));
        assert_eq!(
            m.observe(Observation::Tick(Duration::from_secs(2))),
            vec![Action::AbortMining, Action::AcquirePreciseLook]
        );
        m.observe(Observation::LookAcquired);
        m.observe(Observation::Block("minecraft:stone".into()));
        m.observe(Observation::Tick(Duration::from_secs(1)));
        assert_eq!(m.outcome, Some(BlockBreakingOutcome::ServerNotConfirmed));
    }
    #[test]
    fn timeout_cancellation_death_and_disconnect_cleanup() {
        for (o, want) in [
            (
                Observation::Tick(Duration::from_secs(10)),
                BlockBreakingOutcome::TimedOut,
            ),
            (Observation::Cancel, BlockBreakingOutcome::Cancelled),
            (Observation::Death, BlockBreakingOutcome::Died),
            (Observation::Disconnect, BlockBreakingOutcome::Disconnected),
        ] {
            let mut m = machine();
            assert!(m.observe(o).contains(&Action::Cleanup));
            assert_eq!(m.outcome, Some(want));
        }
    }
    #[test]
    fn dispatch_is_one_shot_per_attempt() {
        let mut m = machine();
        m.observe(Observation::Valid { in_range: true });
        assert_eq!(
            m.observe(Observation::LookAcquired),
            vec![Action::DispatchMining]
        );
        assert!(m.observe(Observation::LookAcquired).is_empty());
        assert_eq!(m.dispatch_count, 1);
    }
}
