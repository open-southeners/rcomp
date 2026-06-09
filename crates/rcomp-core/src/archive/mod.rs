//! Archive backend dispatch layer.
//!
//! This module wires together the archive backends ([`tar`], [`zip`],
//! [`sevenz`], and optionally [`rar`]), the shared entry-path sanitizer, and
//! the `OpCtx` helper that threads cancellation and progress reporting through
//! every low-level operation.
//!
//! # Design overview
//!
//! - **[`OpCtx`]** — carries a [`CancelToken`], a mutable reference to the
//!   caller's `on_progress` callback, and the live [`Progress`] snapshot.
//!   All backend functions receive `&mut OpCtx` so that progress and
//!   cancellation are uniformly handled in one place.
//! - **[`sanitize`]** — validates archive entry paths and symlink targets
//!   against path-traversal (zip-slip) attacks before anything is written to
//!   disk.
//! - **[`tar`] / [`zip`] / [`sevenz`]** — per-format backends; each exposes
//!   `create`, `extract`, and `list` with the signatures specified in the
//!   shared contract.
//! - **[`rar`]** — extract-only RAR backend, available when the `rar` cargo
//!   feature is enabled.  RAR creation is proprietary and always unsupported.

use crate::{
    Error, Result,
    progress::{CancelToken, Progress},
};

pub(crate) mod sanitize;
pub(crate) mod sevenz;
pub(crate) mod tar;
pub(crate) mod zip;

#[cfg(feature = "rar")]
pub(crate) mod rar;

// ---------------------------------------------------------------------------
// OpCtx
// ---------------------------------------------------------------------------

/// Shared context threaded through every archive operation.
///
/// Bundles the cancellation token, the caller-supplied progress callback, and
/// the mutable progress snapshot.  All three backends receive `&mut OpCtx` so
/// that progress accounting and cancellation checks are implemented once.
pub(crate) struct OpCtx<'a> {
    /// Cancellation signal.  Check via [`OpCtx::check_cancel`].
    pub cancel: CancelToken,
    /// Callback invoked after every [`add_bytes`](OpCtx::add_bytes) or
    /// [`set_entry`](OpCtx::set_entry) mutation.
    pub on_progress: &'a mut dyn FnMut(&Progress),
    /// Live progress snapshot delivered to [`on_progress`](OpCtx::on_progress).
    pub progress: Progress,
}

impl OpCtx<'_> {
    /// Returns `Err(`[`Error::Cancelled`]`)` if the cancel token has been
    /// signalled; otherwise returns `Ok(())`.
    pub(crate) fn check_cancel(&self) -> Result<()> {
        if self.cancel.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }

    /// Advance `bytes_done` by `n` and invoke `on_progress`.
    pub(crate) fn add_bytes(&mut self, n: u64) {
        self.progress.bytes_done = self.progress.bytes_done.saturating_add(n);
        (self.on_progress)(&self.progress);
    }

    /// Update the current-entry name and invoke `on_progress`.
    pub(crate) fn set_entry(&mut self, name: impl Into<String>) {
        self.progress.current_entry = Some(name.into());
        (self.on_progress)(&self.progress);
    }
}

// ---------------------------------------------------------------------------
// copy_with_progress is in progress.rs — keep archive module clean.
// ---------------------------------------------------------------------------
