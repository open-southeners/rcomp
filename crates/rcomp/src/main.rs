//! `rcomp` CLI entry point.
//!
//! Parses arguments with [`Cli`], dispatches to [`run::run`], and maps
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

use clap::Parser;
use rcomp_core::CancelToken;

fn main() {
    let cli = cli::Cli::parse();

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
            // Check for ambiguity errors → exit 2.
            if e.downcast_ref::<infer::AmbiguityError>().is_some() {
                eprintln!("error: {e}");
                std::process::exit(2);
            }
            // All other errors → exit 1.
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
