//! Progress-bar construction from the core progress callback.
//!
//! [`build`] returns a [`Progress`] value that contains:
//!
//! - `callback` — a boxed `FnMut(&rcomp_core::Progress)` to pass to
//!   [`rcomp_core::compress`] / [`rcomp_core::extract`].
//! - `guard` — a [`ProgressGuard`] whose `Drop` impl calls
//!   `finish_and_clear()` on the bar, preventing any residue on the terminal.
//!
//! ## Bar style selection
//!
//! The bar style is fixed on the **first tick** once `bytes_total` is known:
//!
//! - `Some(total)` → bytes bar: `HumanBytes + percentage + ETA`.
//! - `None` → spinner with a `HumanBytes` processed counter.
//!
//! ## Output stream
//!
//! The bar draws to **stderr**.  `indicatif` automatically suppresses all
//! output when stderr is not a TTY (its default behaviour), so no extra check
//! is needed for that case.
//!
//! ## Quiet mode
//!
//! When `quiet = true` both the callback and the guard are no-ops (the bar is
//! initialised as hidden and never displayed).

use std::sync::{Arc, Mutex};

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use rcomp_core::Progress as CoreProgress;

/// Maximum display width for the current-entry message.
const ENTRY_MSG_MAX: usize = 40;

/// Truncate `s` so it fits in `ENTRY_MSG_MAX` columns, adding a leading `…`
/// when the string is shortened.
fn truncate_entry(s: &str) -> String {
    if s.len() <= ENTRY_MSG_MAX {
        s.to_owned()
    } else {
        format!("…{}", &s[s.len() - (ENTRY_MSG_MAX - 1)..])
    }
}

// ---------------------------------------------------------------------------
// ProgressGuard
// ---------------------------------------------------------------------------

/// RAII wrapper that clears the progress bar when dropped.
///
/// Dropping this at the end of a compress/extract scope guarantees that the
/// bar never pollutes the terminal output that follows.
pub struct ProgressGuard(pub ProgressBar);

impl Drop for ProgressGuard {
    fn drop(&mut self) {
        self.0.finish_and_clear();
    }
}

// ---------------------------------------------------------------------------
// Progress (the combined handle)
// ---------------------------------------------------------------------------

/// Combined progress callback + finish guard for one operation.
///
/// Construct with [`build`].  Pass `callback` to the core API and let `guard`
/// drop automatically at the end of the scope.
pub struct Progress {
    /// Closure to hand to `compress` / `extract`.
    pub callback: Box<dyn FnMut(&CoreProgress)>,
    /// Dropped on scope exit — clears / finishes the bar.
    pub guard: ProgressGuard,
}

// ---------------------------------------------------------------------------
// build
// ---------------------------------------------------------------------------

/// Build the progress infrastructure for one compress or extract operation.
///
/// When `quiet` is `true` the callback is a no-op and the guard holds a hidden
/// (permanent no-draw) bar.  Otherwise the bar is styled lazily on the first
/// progress tick so `bytes_total` can be inspected.
pub fn build(quiet: bool) -> Progress {
    // A hidden bar satisfies the guard's Drop while doing nothing visible.
    let hidden = || ProgressBar::with_draw_target(None, ProgressDrawTarget::hidden());

    if quiet {
        return Progress {
            callback: Box::new(|_| {}),
            guard: ProgressGuard(hidden()),
        };
    }

    // Start with a hidden spinner; the real style is applied on the first tick.
    let bar = hidden();

    // `ProgressBar` is internally `Arc`-backed, so cloning is cheap and
    // shares the same underlying state.  The guard clone lets Drop call
    // finish_and_clear() even after the callback closure has moved `bar`.
    let guard_bar = bar.clone();

    // Track whether the style has been set yet.
    let styled = Arc::new(Mutex::new(false));

    let callback = Box::new(move |p: &CoreProgress| {
        // First tick: lock, check, configure style, redirect to stderr.
        {
            let mut done = styled.lock().unwrap_or_else(|e| e.into_inner());
            if !*done {
                if let Some(total) = p.bytes_total {
                    bar.set_length(total);
                    bar.set_style(
                        ProgressStyle::with_template(
                            "{msg:40} [{wide_bar:.cyan/blue}] \
                             {bytes}/{total_bytes} ({percent}%) eta {eta}",
                        )
                        .unwrap()
                        .progress_chars("=>-"),
                    );
                } else {
                    bar.set_style(
                        ProgressStyle::with_template("{spinner:.green} {msg:40} {bytes}").unwrap(),
                    );
                }
                bar.set_draw_target(ProgressDrawTarget::stderr());
                *done = true;
            }
        }

        // Update entry message.
        if let Some(ref entry) = p.current_entry {
            bar.set_message(truncate_entry(entry));
        }

        // Advance the byte counter / position.
        bar.set_position(p.bytes_done);
    });

    Progress {
        callback,
        guard: ProgressGuard(guard_bar),
    }
}
