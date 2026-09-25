//! Small helpers shared by the engine modules.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex as StdMutex, MutexGuard, PoisonError};
use std::time::Duration;

use ferry_core::Error;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

/// Lock a std mutex, ignoring poisoning: every critical section in the
/// engine is a plain map/set update that cannot leave the data half-changed.
pub(crate) fn lock<T>(m: &StdMutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One async mutex per key (service id, datastore id), created on demand.
#[derive(Default)]
pub(crate) struct KeyedLocks {
    map: StdMutex<HashMap<String, Arc<AsyncMutex<()>>>>,
}

impl KeyedLocks {
    fn get(&self, key: &str) -> Arc<AsyncMutex<()>> {
        lock(&self.map).entry(key.to_string()).or_default().clone()
    }

    /// Wait for the lock of `key`.
    pub(crate) async fn lock(&self, key: &str) -> OwnedMutexGuard<()> {
        self.get(key).lock_owned().await
    }

    /// The lock of `key` if nobody holds it.
    pub(crate) fn try_lock(&self, key: &str) -> Option<OwnedMutexGuard<()>> {
        self.get(key).try_lock_owned().ok()
    }

    /// Drop the lock of a deleted key (holders keep their guard).
    pub(crate) fn forget(&self, key: &str) {
        lock(&self.map).remove(key);
    }
}

/// A concise, user-facing message for an error (no "build failed:" /
/// "docker error:" prefixes: the deploy status already says what failed).
pub(crate) fn error_message(e: &Error) -> String {
    match e {
        Error::Build(m) | Error::Invalid(m) | Error::Conflict(m) | Error::Internal(m) | Error::Docker(m) => m.clone(),
        other => other.to_string(),
    }
}

/// Run `fut` with a timeout; a timeout becomes `Error::Docker(<what> timed out)`.
pub(crate) async fn with_timeout<T>(
    what: &str,
    limit: Duration,
    fut: impl Future<Output = ferry_core::Result<T>>,
) -> ferry_core::Result<T> {
    match tokio::time::timeout(limit, fut).await {
        Ok(r) => r,
        Err(_) => Err(Error::Docker(format!("{what} timed out after {}s", limit.as_secs()))),
    }
}

/// Last 6 characters of a container name: the instance id shown in logs.
pub(crate) fn instance_id(container_name: &str) -> String {
    let chars: Vec<char> = container_name.chars().collect();
    let start = chars.len().saturating_sub(6);
    chars[start..].iter().collect()
}

/// A short description of a panic payload.
pub(crate) fn panic_message(err: &tokio::task::JoinError) -> String {
    if err.is_cancelled() {
        return "the task was aborted".to_string();
    }
    "internal error (the task panicked; see the server log)".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_ids() {
        assert_eq!(instance_id("ferry-web-cdef0123-a1b2c3"), "a1b2c3");
        assert_eq!(instance_id("abc"), "abc");
        assert_eq!(instance_id(""), "");
    }

    #[test]
    fn error_messages_are_concise() {
        assert_eq!(error_message(&Error::Build("npm failed".into())), "npm failed");
        assert_eq!(error_message(&Error::Docker("daemon down".into())), "daemon down");
        assert_eq!(error_message(&Error::not_found("image", "x")), "image 'x' not found");
    }

    #[tokio::test]
    async fn keyed_locks() {
        let locks = KeyedLocks::default();
        let g = locks.lock("a").await;
        assert!(locks.try_lock("a").is_none());
        assert!(locks.try_lock("b").is_some());
        drop(g);
        assert!(locks.try_lock("a").is_some());
        locks.forget("a");
        let r: ferry_core::Result<()> = with_timeout("sleeping", Duration::from_millis(10), async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            Ok(())
        })
        .await;
        assert!(matches!(r, Err(Error::Docker(m)) if m.contains("timed out")));
    }
}
