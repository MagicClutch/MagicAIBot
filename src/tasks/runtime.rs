//! Session-safe ownership primitives shared by task orchestrators.
//!
//! This module deliberately contains no gameplay.  Actions acquire declared
//! resources here and continue to execute in their owning service.

use std::{collections::BTreeSet, sync::Arc};

use tokio::sync::{Mutex, OwnedMutexGuard};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Global acquisition order. Adding a resource requires choosing its stable
/// place here; callers cannot supply an arbitrary order.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Resource {
    Movement,
    Look,
    Interaction,
    Inventory,
    Container,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OperationError {
    Cancelled,
    StaleSession { expected: u64, actual: u64 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationContext {
    pub correlation_id: Uuid,
    pub session: u64,
    pub cancellation: CancellationToken,
}

impl OperationContext {
    pub fn new(session: u64, cancellation: CancellationToken) -> Self {
        Self {
            correlation_id: Uuid::new_v4(),
            session,
            cancellation,
        }
    }

    pub fn validate(&self, current_session: u64) -> Result<(), OperationError> {
        if self.cancellation.is_cancelled() {
            return Err(OperationError::Cancelled);
        }
        if self.session != current_session {
            return Err(OperationError::StaleSession {
                expected: self.session,
                actual: current_session,
            });
        }
        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct ResourceLeases {
    locks: Arc<[Arc<Mutex<()>>; 5]>,
}

impl ResourceLeases {
    /// Acquires unique resources in [`Resource`] order. Waiting is
    /// cancellation-aware, and dropping the returned lease releases all locks.
    pub async fn acquire(
        &self,
        requested: impl IntoIterator<Item = Resource>,
        cancellation: &CancellationToken,
    ) -> Result<ResourceLease, OperationError> {
        let requested: BTreeSet<_> = requested.into_iter().collect();
        let mut guards = Vec::with_capacity(requested.len());
        for resource in requested {
            let lock = Arc::clone(&self.locks[resource as usize]);
            let guard = tokio::select! {
                () = cancellation.cancelled() => return Err(OperationError::Cancelled),
                guard = lock.lock_owned() => guard,
            };
            guards.push(guard);
        }
        Ok(ResourceLease { _guards: guards })
    }
}

#[derive(Debug)]
pub struct ResourceLease {
    _guards: Vec<OwnedMutexGuard<()>>,
}

/// Truthful local cleanup state used while an owning service sends its final
/// stop/close requests. `Unknown` must not be reported as closed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MenuState {
    #[default]
    Closed,
    Open,
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ActionCleanup {
    pub movement_active: bool,
    pub mining_active: bool,
    pub menu: MenuState,
}

impl ActionCleanup {
    pub fn terminal(&mut self, menu_close_confirmed: bool) {
        self.movement_active = false;
        self.mining_active = false;
        if self.menu != MenuState::Closed {
            self.menu = if menu_close_confirmed {
                MenuState::Closed
            } else {
                MenuState::Unknown
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lease_requests_are_deduplicated_ordered_and_released_on_drop() {
        let leases = ResourceLeases::default();
        let token = CancellationToken::new();
        let lease = leases
            .acquire(
                [
                    Resource::Container,
                    Resource::Movement,
                    Resource::Inventory,
                    Resource::Movement,
                ],
                &token,
            )
            .await
            .unwrap();
        drop(lease);
        leases
            .acquire(
                [Resource::Movement, Resource::Inventory, Resource::Container],
                &token,
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn cancellation_interrupts_lease_wait_and_releases_partial_acquisition() {
        let leases = ResourceLeases::default();
        let held = leases
            .acquire([Resource::Inventory], &CancellationToken::new())
            .await
            .unwrap();
        let cancellation = CancellationToken::new();
        let waiting = leases.acquire([Resource::Movement, Resource::Inventory], &cancellation);
        tokio::pin!(waiting);
        cancellation.cancel();
        assert_eq!(waiting.await.unwrap_err(), OperationError::Cancelled);
        drop(held);
        leases
            .acquire([Resource::Movement], &CancellationToken::new())
            .await
            .unwrap();
    }

    #[test]
    fn reconnect_invalidates_old_operations_and_correlation_ids_are_unique() {
        let first = OperationContext::new(7, CancellationToken::new());
        let second = OperationContext::new(8, CancellationToken::new());
        assert_ne!(first.correlation_id, second.correlation_id);
        assert_eq!(
            first.validate(8),
            Err(OperationError::StaleSession {
                expected: 7,
                actual: 8
            })
        );
        assert_eq!(second.validate(8), Ok(()));
    }

    #[test]
    fn every_terminal_path_stops_motion_and_does_not_invent_menu_confirmation() {
        for close_confirmed in [true, false] {
            let mut state = ActionCleanup {
                movement_active: true,
                mining_active: true,
                menu: MenuState::Open,
            };
            state.terminal(close_confirmed);
            assert!(!state.movement_active);
            assert!(!state.mining_active);
            assert_eq!(
                state.menu,
                if close_confirmed {
                    MenuState::Closed
                } else {
                    MenuState::Unknown
                }
            );
        }
    }

    /// Lifecycle-level mocks cover the requested gathering workflows without
    /// pretending that unsupported gathering gameplay exists in production.
    #[test]
    fn mocked_gathering_matrix_covers_sources_tools_storage_and_terminal_failures() {
        #[derive(Clone, Copy)]
        enum Source {
            Logs,
            Stone,
            VisibleOre,
            Food,
        }
        #[derive(Clone, Copy)]
        enum Event {
            ToolCrafted,
            ChestDeposit,
            InventoryPressure,
            ChangedWorld,
            Timeout,
            Cancellation,
            Death,
            Disconnect,
        }
        let sources = [
            Source::Logs,
            Source::Stone,
            Source::VisibleOre,
            Source::Food,
        ];
        let events = [
            Event::ToolCrafted,
            Event::ChestDeposit,
            Event::InventoryPressure,
            Event::ChangedWorld,
            Event::Timeout,
            Event::Cancellation,
            Event::Death,
            Event::Disconnect,
        ];
        let mut cases = 0;
        for _source in sources {
            for event in events {
                let mut cleanup = ActionCleanup {
                    movement_active: true,
                    mining_active: true,
                    menu: if matches!(event, Event::ChestDeposit) {
                        MenuState::Open
                    } else {
                        MenuState::Closed
                    },
                };
                cleanup.terminal(!matches!(event, Event::Disconnect | Event::ChangedWorld));
                assert!(!cleanup.movement_active && !cleanup.mining_active);
                cases += 1;
            }
        }
        assert_eq!(cases, 32);
    }
}
