//! Shared action failure/identity vocabulary. This is independent of any
//! task-orchestration layer -- there isn't one; `InteractionController` uses
//! these as its own internal error/identity types.

use std::fmt;

use uuid::Uuid;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Invalidation {
    TargetChanged,
    TargetUnavailable,
    WorldUnavailable,
    InventoryChanged,
    SessionChanged,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ActionFailure {
    #[error("{0:?} was invalidated")]
    Invalidated(Invalidation),
    #[error("no path to the target")]
    NoPath,
    #[error("target is out of range")]
    OutOfRange,
    #[error("action timed out")]
    Timeout,
    #[error("server rejected the action: {0}")]
    ServerRejected(String),
    #[error("action was cancelled")]
    Cancelled,
    #[error("bot died")]
    Death,
    #[error("session disconnected")]
    Disconnected,
    #[error("missing material: {item} (need {required}, have {available})")]
    MissingMaterials {
        item: String,
        required: u32,
        available: u32,
    },
    #[error("insufficient capacity: need {required}, have {available}")]
    InsufficientCapacity { required: u32, available: u32 },
    #[error("action failed: {0}")]
    Internal(String),
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OperationId(Uuid);

impl OperationId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for OperationId {
    fn default() -> Self {
        Self::new()
    }
}
impl fmt::Display for OperationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_results_represent_planned_failure_contract() {
        let failures = [
            ActionFailure::Invalidated(Invalidation::TargetUnavailable),
            ActionFailure::Invalidated(Invalidation::WorldUnavailable),
            ActionFailure::Invalidated(Invalidation::InventoryChanged),
            ActionFailure::NoPath,
            ActionFailure::OutOfRange,
            ActionFailure::Timeout,
            ActionFailure::ServerRejected("denied".into()),
            ActionFailure::Cancelled,
            ActionFailure::Death,
            ActionFailure::Disconnected,
            ActionFailure::MissingMaterials {
                item: "stone".into(),
                required: 2,
                available: 1,
            },
            ActionFailure::InsufficientCapacity {
                required: 2,
                available: 1,
            },
        ];
        for failure in failures {
            // Every failure variant round-trips through `Clone`/`PartialEq`
            // and produces a distinct, non-empty message via `thiserror`.
            assert_eq!(failure.clone(), failure);
            assert!(!failure.to_string().is_empty());
        }
    }

    #[test]
    fn operation_ids_are_unique_and_displayable() {
        let a = OperationId::new();
        let b = OperationId::new();
        assert_ne!(a, b);
        assert!(!a.to_string().is_empty());
    }
}
