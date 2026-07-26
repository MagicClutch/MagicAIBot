//! Serialization boundary for every selected-slot inventory mutation.

use std::{future::Future, sync::Arc};

use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::error::AppError;

#[derive(Clone, Default)]
pub(crate) struct InventoryActionService {
    lease: Arc<Mutex<()>>,
}

impl InventoryActionService {
    pub fn try_acquire(&self) -> Result<InventoryActionLease, AppError> {
        self.lease
            .clone()
            .try_lock_owned()
            .map(|guard| InventoryActionLease { _guard: guard })
            .map_err(|_| AppError::InventoryBusy)
    }

    pub async fn mutate<T, F, Fut>(&self, mutation: F) -> Result<T, AppError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, AppError>>,
    {
        let _lease = self.try_acquire()?;
        mutation().await
    }
}

pub(crate) struct InventoryActionLease {
    _guard: OwnedMutexGuard<()>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn concurrent_mutation_fails_and_release_recovers() {
        let service = InventoryActionService::default();
        let lease = service.try_acquire().unwrap();
        assert!(matches!(
            service.mutate(|| async { Ok(()) }).await,
            Err(AppError::InventoryBusy)
        ));
        drop(lease);
        assert_eq!(service.mutate(|| async { Ok(7) }).await.unwrap(), 7);
    }

    #[tokio::test]
    async fn mutation_failure_releases_lease() {
        let service = InventoryActionService::default();
        assert!(
            service
                .mutate(|| async {
                    Err::<(), _>(AppError::InventoryMutationRejected("test rejection".into()))
                })
                .await
                .is_err()
        );
        assert!(service.try_acquire().is_ok());
    }

    #[tokio::test]
    async fn cancelled_future_releases_lease() {
        let service = InventoryActionService::default();
        let cloned = service.clone();
        let task = tokio::spawn(async move {
            cloned
                .mutate(|| async { std::future::pending::<Result<(), AppError>>().await })
                .await
        });
        tokio::task::yield_now().await;
        task.abort();
        let _ = task.await;
        assert!(service.try_acquire().is_ok());
    }
}
