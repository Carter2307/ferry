//! Request serialization and cancellation safety.
//!
//! * Handlers that read a row, change it and write it back hold the lock of
//!   the row's **owner** (a service or env group id) for the whole
//!   read-modify-write, so two concurrent requests can't silently overwrite
//!   each other's acknowledged changes.
//! * Custom-domain / default-host uniqueness is a check-then-write across all
//!   services, so it runs under one global **domain** lock.
//! * [`detached`] runs a handler body on its own task: a client disconnect
//!   drops the handler future, which must not stop a multi-step change
//!   halfway (row written, first deploy never queued...).
//!
//! Lock order — never acquire against it: blueprint apply lock → domain lock
//! → owner locks (several at once only in ascending id order). Nobody waits
//! for the domain lock while holding an owner lock.
//!
//! The locks are process-wide (one API server per process; ids are unique
//! across stores, so independent routers in tests don't interfere).

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, LazyLock, Mutex as StdMutex};

use ferry_core::Error;
use tokio::sync::{Mutex, MutexGuard, OwnedMutexGuard};

use crate::error::ApiResult;

static OWNER_LOCKS: LazyLock<StdMutex<HashMap<String, Arc<Mutex<()>>>>> = LazyLock::new(Default::default);

static DOMAIN_LOCK: Mutex<()> = Mutex::const_new(());

/// Lock the rows of one owner (service or env group id).
pub async fn owner(id: &str) -> OwnedMutexGuard<()> {
    let lock = {
        // A poisoned map is still a valid map (no invariant spans the panic).
        let mut map = OWNER_LOCKS.lock().unwrap_or_else(|e| e.into_inner());
        // Forget locks nobody holds or waits for (deleted services...).
        map.retain(|_, l| Arc::strong_count(l) > 1);
        map.entry(id.to_string()).or_default().clone()
    };
    lock.lock_owned().await
}

/// Lock several owners at once (sorted, deduplicated: see the lock order).
pub async fn owners(ids: impl IntoIterator<Item = String>) -> Vec<OwnedMutexGuard<()>> {
    let mut ids: Vec<String> = ids.into_iter().collect();
    ids.sort();
    ids.dedup();
    let mut guards = Vec::with_capacity(ids.len());
    for id in &ids {
        guards.push(owner(id).await);
    }
    guards
}

/// The global lock around custom-domain / default-host claims.
pub async fn domains() -> MutexGuard<'static, ()> {
    DOMAIN_LOCK.lock().await
}

/// Run `fut` to completion on its own task and return its result, so that a
/// client disconnect (which drops the handler future) can't cancel it midway.
pub async fn detached<T, F>(fut: F) -> ApiResult<T>
where
    T: Send + 'static,
    F: Future<Output = ApiResult<T>> + Send + 'static,
{
    match tokio::spawn(fut).await {
        Ok(res) => res,
        Err(e) => Err(Error::internal(format!("the request handler failed: {e}")).into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    #[tokio::test]
    async fn owner_locks_serialize_per_id() {
        let g = owner("srv-lock-test-a").await;
        // another id is independent
        let other = tokio::time::timeout(Duration::from_millis(100), owner("srv-lock-test-b")).await;
        assert!(other.is_ok());
        // the same id waits
        let same = tokio::time::timeout(Duration::from_millis(50), owner("srv-lock-test-a")).await;
        assert!(same.is_err());
        drop(g);
        assert!(tokio::time::timeout(Duration::from_millis(100), owner("srv-lock-test-a")).await.is_ok());
    }

    #[tokio::test]
    async fn detached_work_survives_a_dropped_caller() {
        let done = Arc::new(AtomicBool::new(false));
        let flag = done.clone();
        let call = detached(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            flag.store(true, Ordering::SeqCst);
            Ok(())
        });
        // The caller gives up (client disconnect) before the work finishes.
        let _ = tokio::time::timeout(Duration::from_millis(5), call).await;
        for _ in 0..100 {
            if done.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(done.load(Ordering::SeqCst));
    }
}
