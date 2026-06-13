//! Job registry for tracking in-flight operations and their cancel tokens.
//!
//! Each running compress/extract command registers itself under a caller-supplied
//! [`JobId`] and receives a [`rcomp_core::CancelToken`] to pass into the core
//! operation.  The frontend (or an app-shutdown hook) can cancel individual jobs
//! or all jobs at once.
//!
//! This module is intentionally free of `tauri::` imports so it can be compiled
//! and tested as plain Rust.

use std::{collections::HashMap, sync::Mutex};

use rcomp_core::CancelToken;

/// An opaque identifier for a single in-flight job.
pub type JobId = String;

/// Thread-safe registry mapping [`JobId`]s to their [`CancelToken`]s.
///
/// Tauri commands should call [`register`] when they start an operation,
/// pass the returned token into the core call, and call [`finish`] when the
/// operation completes (whether it succeeded, failed, or was cancelled).
///
/// [`register`]: JobRegistry::register
/// [`finish`]: JobRegistry::finish
#[derive(Default)]
pub struct JobRegistry {
    inner: Mutex<HashMap<JobId, CancelToken>>,
}

impl JobRegistry {
    /// Register a new job and return a [`CancelToken`] for the worker thread.
    ///
    /// Creates a fresh token, stores a clone under `id`, and returns another
    /// clone for the worker.  All three copies share the same underlying flag,
    /// so calling [`cancel`] on any of them cancels the operation.
    ///
    /// If `id` is already registered the old token is **overwritten** and the
    /// previous operation will not be reachable for cancellation via this
    /// registry.  Callers should ensure job IDs are unique for the lifetime of
    /// each operation.
    ///
    /// [`cancel`]: JobRegistry::cancel
    pub fn register(&self, id: JobId) -> CancelToken {
        let token = CancelToken::default();
        let worker_clone = token.clone();
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, token);
        worker_clone
    }

    /// Cancel the job identified by `id`.
    ///
    /// Returns `true` if the job was found and the token was tripped.
    /// Returns `false` if no job with the given `id` is registered.
    ///
    /// The entry is **not** removed; removal happens in [`finish`] once the
    /// worker acknowledges the cancellation and returns.
    ///
    /// [`finish`]: JobRegistry::finish
    pub fn cancel(&self, id: &str) -> bool {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(token) = guard.get(id) {
            token.cancel();
            true
        } else {
            false
        }
    }

    /// Remove `id` from the registry once the associated operation has ended.
    ///
    /// Should be called by the Tauri command after the core operation returns,
    /// regardless of whether it succeeded, failed, or was cancelled.
    pub fn finish(&self, id: &str) {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
    }

    /// Cancel every in-flight job.
    ///
    /// Intended for app/window close events.  This method trips every token in
    /// the registry but does **not** clear the map; entries are removed
    /// individually as workers call [`finish`].
    ///
    /// [`finish`]: JobRegistry::finish
    pub fn cancel_all(&self) {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        for token in guard.values() {
            token.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_then_cancel_trips_worker_token() {
        let registry = JobRegistry::default();
        let worker_token = registry.register("job-1".into());
        assert!(
            !worker_token.is_cancelled(),
            "token should start uncancelled"
        );

        let found = registry.cancel("job-1");
        assert!(found, "cancel should return true for a known job");
        assert!(
            worker_token.is_cancelled(),
            "worker token must be cancelled after cancel()"
        );
    }

    #[test]
    fn cancel_unknown_id_returns_false() {
        let registry = JobRegistry::default();
        assert!(
            !registry.cancel("does-not-exist"),
            "cancel on unknown id should return false"
        );
    }

    #[test]
    fn finish_removes_entry() {
        let registry = JobRegistry::default();
        registry.register("job-2".into());
        registry.finish("job-2");

        // After finish the job should be gone; a new cancel returns false.
        assert!(
            !registry.cancel("job-2"),
            "cancel after finish should return false"
        );
    }

    #[test]
    fn cancel_all_trips_all_registered_tokens() {
        let registry = JobRegistry::default();
        let t1 = registry.register("a".into());
        let t2 = registry.register("b".into());
        let t3 = registry.register("c".into());

        assert!(!t1.is_cancelled());
        assert!(!t2.is_cancelled());
        assert!(!t3.is_cancelled());

        registry.cancel_all();

        assert!(t1.is_cancelled(), "token a must be cancelled");
        assert!(t2.is_cancelled(), "token b must be cancelled");
        assert!(t3.is_cancelled(), "token c must be cancelled");
    }

    #[test]
    fn register_overwrites_existing_id() {
        let registry = JobRegistry::default();
        // Register job-x once, stash its token.
        let old_token = registry.register("job-x".into());

        // Register again under the same id.
        let new_token = registry.register("job-x".into());

        // Cancelling via the registry now trips the new token.
        registry.cancel("job-x");
        assert!(
            new_token.is_cancelled(),
            "new token must be cancelled via registry"
        );
        // The old token is unreachable via the registry; it should NOT be cancelled
        // by this call (the new entry replaced it).
        assert!(
            !old_token.is_cancelled(),
            "old token should not be cancelled by the registry after overwrite"
        );
    }
}
