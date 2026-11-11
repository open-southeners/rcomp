//! `rcomp` CLI entry point.
//!
//! Parses arguments with `Cli`, dispatches to [`run::run`], and maps
//! the result to an exit code:
//!
//! - **0** — success
//! - **1** — operation error (I/O, already-exists, unsupported, cancelled, …)
//! - **2** — usage/ambiguity error (clap parse failure, inference ambiguity,
//!   bad `--algo` value)

mod cli;
mod infer;
mod run;
mod ui;

use std::panic::PanicHookInfo;
use std::sync::{Mutex, PoisonError};

use clap::Parser;
use rcomp_core::CancelToken;

fn main() {
    let cli = cli::Cli::parse();
    install_panic_hook();

    // Create a shared cancellation token.  The ctrlc handler trips a clone of
    // it when SIGINT is received; the same token is forwarded into every
    // compress/extract operation so core can unwind cleanly (deleting partial
    // output) and surface `Error::Cancelled`.
    let cancel = CancelToken::default();
    let cancel_for_handler = cancel.clone();

    // Install the Ctrl-C handler.  On platforms where ctrlc can't register a
    // handler (very unlikely in practice) we print a warning and carry on
    // without a handler rather than aborting the whole process at startup.
    if let Err(e) = ctrlc::set_handler(move || {
        cancel_for_handler.cancel();
    }) {
        eprintln!("warning: could not install Ctrl-C handler: {e}");
    }

    match run::run(&cli, cancel) {
        Ok(()) => {}
        Err(e) => {
            // Check for usage errors → exit 2.
            if e.downcast_ref::<infer::AmbiguityError>().is_some()
                || e.downcast_ref::<run::UsageError>().is_some()
            {
                eprintln!("error: {e}");
                std::process::exit(2);
            }
            // All other errors → exit 1.
            eprintln!("error: {e}");
            if let Some(panic) = quieted_panic() {
                eprintln!("note: the brotli encoder {panic}");
            }
            std::process::exit(1);
        }
    }
}

// ---------------------------------------------------------------------------
// Panic hook
// ---------------------------------------------------------------------------

/// Name `rcomp-core` gives its codec worker threads (`codec/parallel.rs`).
const CODEC_WORKER_THREAD: &str = "rcomp-codec-worker";

/// The latest panic [`install_panic_hook`] kept off stderr.
static QUIETED_PANIC: Mutex<Option<String>> = Mutex::new(None);

/// Keep known `brotli` encoder panics off stderr.
///
/// `rcomp-core` recovers from a panic in the `brotli` crate's encoder (a bug
/// on its custom-dictionary path) by compressing the chunk again without the
/// dictionary, so the operation succeeds and the default hook's "thread
/// panicked" message would only alarm.  Only panics raised on a codec worker
/// thread at a location inside the `brotli` crate's `src/enc/` are kept back,
/// and the latest one is printed if the operation fails after all; every
/// other panic goes to the previous hook as before.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info: &PanicHookInfo<'_>| {
        let on_worker = std::thread::current().name() == Some(CODEC_WORKER_THREAD);
        if on_worker
            && info
                .location()
                .is_some_and(|l| is_brotli_encoder_source(l.file()))
        {
            *QUIETED_PANIC.lock().unwrap_or_else(PoisonError::into_inner) = Some(info.to_string());
            return;
        }
        previous(info);
    }));
}

/// The latest panic kept off stderr, as "panicked at <location>:\n<message>".
fn quieted_panic() -> Option<String> {
    QUIETED_PANIC
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
}

/// Whether `file` (a panic location) is in the `brotli` crate's encoder
/// sources: `…/brotli-<version>/src/enc/…` from the registry, or
/// `…/brotli/src/enc/…` when vendored.
fn is_brotli_encoder_source(file: &str) -> bool {
    let file = file.replace('\\', "/");
    let parts: Vec<&str> = file.split('/').collect();
    parts.windows(3).any(|w| {
        let is_brotli = w[0] == "brotli"
            || w[0]
                .strip_prefix("brotli-")
                .is_some_and(|version| version.starts_with(|c: char| c.is_ascii_digit()));
        is_brotli && w[1] == "src" && w[2] == "enc"
    })
}

#[cfg(test)]
mod tests {
    use super::is_brotli_encoder_source;

    #[test]
    fn brotli_encoder_sources_are_recognised() {
        for file in [
            "/home/u/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/brotli-8.0.3/src/enc/brotli_bit_stream.rs",
            "/home/u/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/brotli-9.1.1/src/enc/command.rs",
            r"C:\Users\u\.cargo\registry\src\index.crates.io-x\brotli-8.0.3\src\enc\encode.rs",
            "/build/vendor/brotli/src/enc/backward_references/mod.rs",
        ] {
            assert!(is_brotli_encoder_source(file), "{file}");
        }
    }

    #[test]
    fn other_sources_are_not() {
        for file in [
            "/home/u/.cargo/registry/src/index.crates.io-x/brotli-decompressor-5.0.1/src/decode.rs",
            "/home/u/.cargo/registry/src/index.crates.io-x/brotli-8.0.3/src/concat/mod.rs",
            "/home/u/.cargo/registry/src/index.crates.io-x/brotli-8.0.3/src/dec/mod.rs",
            "crates/rcomp-core/src/codec/brotli.rs",
            "crates/rcomp-core/src/codec/parallel.rs",
        ] {
            assert!(!is_brotli_encoder_source(file), "{file}");
        }
    }
}
