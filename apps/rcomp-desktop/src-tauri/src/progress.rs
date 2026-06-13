//! Progress types and throttle logic for the Tauri bridge.
//!
//! [`ProgressEvent`] is the serialisable snapshot sent to the frontend.
//! [`ProgressThrottle`] decides which `on_progress` ticks are worth forwarding;
//! the core fires a callback every 64 KiB, which would flood the IPC channel.
//!
//! # Guarantee: final snapshot
//!
//! The **final** progress snapshot is always sent unconditionally by the Tauri
//! command after the operation returns, regardless of the throttle state.  That
//! logic lives in the command layer (`commands.rs`), not here.
//!
//! This module is intentionally free of `tauri::` imports so it can be compiled
//! and tested as plain Rust.

use std::time::{Duration, Instant};

/// A serialisable progress snapshot forwarded to the frontend via a Tauri event.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProgressEvent {
    /// Number of bytes processed so far.
    pub bytes_done: u64,
    /// Total expected bytes, when known in advance.
    pub bytes_total: Option<u64>,
    /// The archive entry currently being processed, if applicable.
    pub current_entry: Option<String>,
}

impl From<&rcomp_core::Progress> for ProgressEvent {
    fn from(p: &rcomp_core::Progress) -> Self {
        Self {
            bytes_done: p.bytes_done,
            bytes_total: p.bytes_total,
            current_entry: p.current_entry.clone(),
        }
    }
}

/// Decides whether a given progress tick should be forwarded to the webview.
///
/// The core fires [`rcomp_core::Progress`] callbacks every 64 KiB of data
/// processed, which is far too frequent for IPC.  `ProgressThrottle` coalesces
/// those ticks so the frontend receives at most one update per [`interval`]
/// (default 100 ms), **plus** an immediate update whenever the active archive
/// entry changes.
///
/// # Final snapshot
///
/// The throttle does **not** guarantee that the very last progress state is
/// forwarded.  Callers must send the final snapshot unconditionally after the
/// operation returns.
///
/// [`interval`]: ProgressThrottle::new
pub struct ProgressThrottle {
    last_sent: Option<Instant>,
    last_entry: Option<String>,
    interval: Duration,
}

impl Default for ProgressThrottle {
    fn default() -> Self {
        Self::new(Duration::from_millis(100))
    }
}

impl ProgressThrottle {
    /// Create a new throttle that forwards at most one tick per `interval`.
    pub fn new(interval: Duration) -> Self {
        Self {
            last_sent: None,
            last_entry: None,
            interval,
        }
    }

    /// Decide whether `p` should be forwarded to the webview.
    ///
    /// Returns `true` — and updates internal state — when **any** of:
    ///
    /// - This is the first tick ever (`last_sent` is `None`).
    /// - `now.duration_since(last_sent) >= interval`.
    /// - `p.current_entry` differs from the previously observed entry.
    ///
    /// # Testability
    ///
    /// `now` is accepted as a parameter rather than calling `Instant::now()`
    /// internally so tests can drive time deterministically.
    pub fn should_forward(&mut self, p: &rcomp_core::Progress, now: Instant) -> bool {
        let entry_changed = p.current_entry != self.last_entry;
        let interval_elapsed = self
            .last_sent
            .is_none_or(|t| now.duration_since(t) >= self.interval);

        if self.last_sent.is_none() || interval_elapsed || entry_changed {
            self.last_sent = Some(now);
            self.last_entry = p.current_entry.clone();
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcomp_core::Progress;

    fn make_progress(entry: Option<&str>) -> Progress {
        Progress {
            bytes_done: 0,
            bytes_total: None,
            current_entry: entry.map(str::to_owned),
        }
    }

    #[test]
    fn first_tick_always_forwarded() {
        let mut throttle = ProgressThrottle::new(Duration::from_millis(100));
        let base = Instant::now();
        let p = make_progress(None);
        assert!(
            throttle.should_forward(&p, base),
            "first tick must always be forwarded"
        );
    }

    #[test]
    fn second_tick_within_interval_same_entry_suppressed() {
        let mut throttle = ProgressThrottle::new(Duration::from_millis(100));
        let base = Instant::now();
        let p = make_progress(None);

        // First tick is always forwarded.
        throttle.should_forward(&p, base);

        // Second tick arrives 10 ms later — below the 100 ms interval.
        let result = throttle.should_forward(&p, base + Duration::from_millis(10));
        assert!(
            !result,
            "tick within interval with same entry should be suppressed"
        );
    }

    #[test]
    fn tick_after_interval_elapsed_forwarded() {
        let mut throttle = ProgressThrottle::new(Duration::from_millis(100));
        let base = Instant::now();
        let p = make_progress(None);

        // Consume first tick.
        throttle.should_forward(&p, base);

        // Tick arrives exactly at the interval boundary.
        let result = throttle.should_forward(&p, base + Duration::from_millis(100));
        assert!(result, "tick at interval boundary should be forwarded");
    }

    #[test]
    fn tick_within_interval_but_entry_changed_forwarded() {
        let mut throttle = ProgressThrottle::new(Duration::from_millis(100));
        let base = Instant::now();

        // First tick: entry "a".
        throttle.should_forward(&make_progress(Some("a")), base);

        // Second tick 10 ms later with a different entry — must be forwarded.
        let result =
            throttle.should_forward(&make_progress(Some("b")), base + Duration::from_millis(10));
        assert!(
            result,
            "tick with changed current_entry should be forwarded regardless of interval"
        );
    }
}
