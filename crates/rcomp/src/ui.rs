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

/// Truncate `s` to at most `ENTRY_MSG_MAX` characters, keeping the end and
/// adding a leading `…` when the string is shortened.
///
/// Counts `char`s rather than bytes so a cut never lands inside a multibyte
/// character (entry names are arbitrary UTF-8).
fn truncate_entry(s: &str) -> String {
    let len = s.chars().count();
    if len <= ENTRY_MSG_MAX {
        s.to_owned()
    } else {
        let tail: String = s.chars().skip(len - (ENTRY_MSG_MAX - 1)).collect();
        format!("…{tail}")
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

#[cfg(test)]
mod tests {
    use super::{ENTRY_MSG_MAX, truncate_entry};

    #[test]
    fn short_entry_is_unchanged() {
        assert_eq!(truncate_entry("src/main.rs"), "src/main.rs");
    }

    #[test]
    fn long_ascii_entry_keeps_the_tail() {
        let s = "a".repeat(30) + &"b".repeat(30);
        let out = truncate_entry(&s);
        assert_eq!(out.chars().count(), ENTRY_MSG_MAX);
        assert!(out.starts_with('…') && out.ends_with(&"b".repeat(30)));
    }

    #[test]
    fn long_multibyte_entry_does_not_panic() {
        // Every cut position by byte count would land inside a character.
        for prefix in 0..4 {
            let s = "x".repeat(prefix) + &"写真/".repeat(20) + "ファイル.txt";
            let out = truncate_entry(&s);
            assert_eq!(out.chars().count(), ENTRY_MSG_MAX);
            assert!(out.ends_with("ファイル.txt"));
        }
    }
}
