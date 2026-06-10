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

fn main() {
    let cli = cli::Cli::parse();

    match run::run(&cli) {
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
