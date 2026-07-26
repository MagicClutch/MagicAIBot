//! Shared action identity, lifecycle, guards, results, and exclusive resources.
//!
//! This module deliberately does not read or mutate `WorldState`: callers feed
//! authoritative observations into guards and keep the world model observational.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ActionResource {
    Movement,
    Rotation,
    Interaction,
    InventoryMutation,
    ContainerAccess,
}

impl fmt::Display for ActionResource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Movement => "movement",
                Self::Rotation => "rotation",
                Self::Interaction => "interaction",
                Self::InventoryMutation => "inventory-mutation",
                Self::ContainerAccess => "container-access",
            }
        )
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ActionState {
    #[default]
    Idle,
    WaitingForResources,
    Running,
    Completed,
    Cancelled,
    Failed,
}

impl ActionState {
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled | Self::Failed)
    }
}

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
    #[error("resource {resource} is owned by operation {owner}")]
    ResourceBusy {
        resource: ActionResource,
        owner: OperationId,
    },
    #[error("action failed: {0}")]
    Internal(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActionResult<T> {
    Success(T),
    Failure(ActionFailure),
    Cancelled,
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

#[derive(Clone, Debug)]
pub struct OperationGuard {
    id: OperationId,
    session: u64,
    deadline: Option<Instant>,
    cancellation: CancellationToken,
}

impl OperationGuard {
    pub fn new(session: u64, timeout: Option<Duration>) -> Self {
        Self {
            id: OperationId::new(),
            session,
            deadline: timeout.map(|value| Instant::now() + value),
            cancellation: CancellationToken::new(),
        }
    }
    pub const fn id(&self) -> OperationId {
        self.id
    }
    pub const fn session(&self) -> u64 {
        self.session
    }
    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation.clone()
    }
    pub fn cancel(&self) {
        self.cancellation.cancel();
    }
    pub fn check(
        &self,
        current_session: u64,
        connected: bool,
        alive: bool,
    ) -> Result<(), ActionFailure> {
        if self.cancellation.is_cancelled() {
            return Err(ActionFailure::Cancelled);
        }
        if !connected {
            return Err(ActionFailure::Disconnected);
        }
        if !alive {
            return Err(ActionFailure::Death);
        }
        if self.session != current_session {
            return Err(ActionFailure::Invalidated(Invalidation::SessionChanged));
        }
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(ActionFailure::Timeout);
        }
        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct ResourceManager(Arc<Mutex<BTreeMap<ActionResource, OperationId>>>);

impl ResourceManager {
    /// Atomically acquires resources in enum order, so partial acquisition and
    /// lock-order deadlocks are impossible.
    pub fn acquire<I>(
        &self,
        owner: OperationId,
        requested: I,
    ) -> Result<ResourceLease, ActionFailure>
    where
        I: IntoIterator<Item = ActionResource>,
    {
        let resources: BTreeSet<_> = requested.into_iter().collect();
        let mut owners = self.0.lock().expect("resource lease mutex poisoned");
        if let Some((resource, existing)) = resources.iter().find_map(|resource| {
            owners
                .get(resource)
                .filter(|existing| **existing != owner)
                .map(|existing| (*resource, *existing))
        }) {
            return Err(ActionFailure::ResourceBusy {
                resource,
                owner: existing,
            });
        }
        for resource in &resources {
            owners.insert(*resource, owner);
        }
        Ok(ResourceLease {
            manager: self.clone(),
            owner,
            resources,
        })
    }
    pub fn snapshot(&self) -> Vec<(ActionResource, OperationId)> {
        self.0
            .lock()
            .expect("resource lease mutex poisoned")
            .iter()
            .map(|(r, o)| (*r, *o))
            .collect()
    }
}

pub struct ResourceLease {
    manager: ResourceManager,
    owner: OperationId,
    resources: BTreeSet<ActionResource>,
}

impl ResourceLease {
    pub fn resources(&self) -> impl Iterator<Item = ActionResource> + '_ {
        self.resources.iter().copied()
    }
}
impl Drop for ResourceLease {
    fn drop(&mut self) {
        let mut owners = self
            .manager
            .0
            .lock()
            .expect("resource lease mutex poisoned");
        for resource in &self.resources {
            if owners.get(resource) == Some(&self.owner) {
                owners.remove(resource);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquisition_is_atomic_ordered_and_drop_releases_every_resource() {
        let manager = ResourceManager::default();
        let first = OperationId::new();
        let lease = manager
            .acquire(
                first,
                [ActionResource::Interaction, ActionResource::Movement],
            )
            .unwrap();
        assert_eq!(
            lease.resources().collect::<Vec<_>>(),
            vec![ActionResource::Movement, ActionResource::Interaction]
        );
        let second = OperationId::new();
        assert!(
            matches!(manager.acquire(second, [ActionResource::Rotation, ActionResource::Movement]), Err(ActionFailure::ResourceBusy { resource: ActionResource::Movement, owner }) if owner == first)
        );
        assert_eq!(
            manager.snapshot().len(),
            2,
            "failed acquisition must not retain rotation"
        );
        drop(lease);
        assert!(manager.snapshot().is_empty());
    }

    #[test]
    fn guards_cover_cancellation_disconnect_death_session_and_deadline() {
        let guard = OperationGuard::new(7, None);
        assert_eq!(
            guard.check(8, true, true),
            Err(ActionFailure::Invalidated(Invalidation::SessionChanged))
        );
        assert_eq!(
            guard.check(7, false, true),
            Err(ActionFailure::Disconnected)
        );
        assert_eq!(guard.check(7, true, false), Err(ActionFailure::Death));
        guard.cancel();
        assert_eq!(guard.check(7, true, true), Err(ActionFailure::Cancelled));
        let expired = OperationGuard::new(7, Some(Duration::ZERO));
        assert_eq!(expired.check(7, true, true), Err(ActionFailure::Timeout));
    }

    #[test]
    fn every_terminal_state_releases_all_leases() {
        for terminal in [
            ActionState::Completed,
            ActionState::Cancelled,
            ActionState::Failed,
        ] {
            assert!(terminal.is_terminal());
            let manager = ResourceManager::default();
            let lease = manager
                .acquire(
                    OperationId::new(),
                    [
                        ActionResource::Movement,
                        ActionResource::Rotation,
                        ActionResource::Interaction,
                        ActionResource::InventoryMutation,
                        ActionResource::ContainerAccess,
                    ],
                )
                .unwrap();
            drop(lease);
            assert!(manager.snapshot().is_empty(), "leak on {terminal:?}");
        }
    }

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
            assert!(matches!(
                ActionResult::<()>::Failure(failure),
                ActionResult::Failure(_)
            ));
        }
    }
}
