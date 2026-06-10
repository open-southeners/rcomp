//! Clap command-line surface for `rcomp`.
//!
//! Defines [`Cli`] (the top-level parser) and [`SubCommand`] with the `ls`,
//! `completions`, and `man` subcommands.

use clap::{Parser, Subcommand};
use clap_complete::Shell;
use rcomp_core::{Format, Level};

/// Unified compression and archive tool.
///
/// rcomp infers whether to compress or extract from the argument types and
/// file extensions.  Use `--compress`/`--extract` to resolve ambiguity.
#[derive(Debug, Parser)]
#[command(
    name = "rcomp",
    version,
    about = "Unified compression and archive tool",
    after_help = "\
EXAMPLES:
  rcomp folder/ archive.tar.gz          # compress folder to tar.gz
  rcomp big.iso big.iso.zst --edge      # max zstd compression
  rcomp archive.7z                      # extract to cwd (magic-byte detection)
  rcomp archive.7z ~/restored           # extract into explicit destination
  rcomp ls archive.zip                  # list entries without extracting
  rcomp weird-file -a bzip2             # force bzip2 algorithm
  rcomp folder/ out.bz2 -y              # silent-tar: auto-accept confirmation",
    args_conflicts_with_subcommands = true
)]
pub struct Cli {
    /// Input file or directory to compress/extract.
    pub input: Option<String>,

    /// Output file (compress) or destination directory (extract).
    ///
    /// When compressing: the format is inferred from the file extension unless
    /// `--algo` is given.  When extracting: the destination directory to
    /// extract into (created if missing).  Defaults to the current directory.
    pub output: Option<String>,

    /// Force compression/archive format (e.g. bzip2, zstd, tar.xz, zip).
    ///
    /// Accepts codec names, aliases, and layered names.  An unrecognized value
    /// is a usage error (exit 2).
    #[arg(short = 'a', long, value_parser = parse_format)]
    pub algo: Option<Format>,

    /// Use the fastest compression (lowest ratio).
    #[arg(long, group = "level_group")]
    pub fast: bool,

    /// Use balanced compression (default).
    #[arg(long, group = "level_group")]
    pub best: bool,

    /// Use maximum compression (highest ratio, hardware expensive).
    #[arg(long, group = "level_group")]
    pub edge: bool,

    /// Force compress mode (overrides inference).
    #[arg(short = 'c', long, conflicts_with = "extract")]
    pub compress: bool,

    /// Force extract mode (overrides inference).
    #[arg(short = 'x', long, conflicts_with = "compress")]
    pub extract: bool,

    /// Extract entries directly into the destination (skip the auto-wrap folder).
    #[arg(long)]
    pub unwrap: bool,

    /// Auto-accept all confirmation prompts (non-interactive / script use).
    #[arg(short = 'y', long)]
    pub yes: bool,

    /// Overwrite existing output files.
    #[arg(short = 'f', long)]
    pub force: bool,

    /// Suppress all non-error output (progress bars and summary).
    #[arg(short = 'q', long)]
    pub quiet: bool,

    #[command(subcommand)]
    pub command: Option<SubCommand>,
}

impl Cli {
    /// Resolve the compression level from the flag group.
    ///
    /// `--fast` → [`Level::Fast`], `--edge` → [`Level::Edge`],
    /// anything else (including the default) → [`Level::Best`].
    pub fn level(&self) -> Level {
        if self.fast {
            Level::Fast
        } else if self.edge {
            Level::Edge
        } else {
            Level::Best
        }
    }
}

/// Subcommands coexisting with the default positional invocation.
#[derive(Debug, Subcommand)]
pub enum SubCommand {
    /// List entries inside an archive without extracting.
    ///
    /// Prints one line per entry in the format `{size:>12}  {path}`.
    /// Directories have a trailing `/` appended to their path.
    Ls {
        /// The archive to list.
        archive: String,
    },

    /// Print shell completion script to stdout.
    ///
    /// Pipe to a file or source directly in your shell init script.
    ///
    /// Example: `rcomp completions bash >> ~/.bash_completion`
    Completions {
        /// Target shell (bash, zsh, fish, elvish, powershell).
        shell: Shell,
    },

    /// Print a troff man page for rcomp to stdout.
    ///
    /// Example: `rcomp man | gzip > /usr/local/share/man/man1/rcomp.1.gz`
    Man,
}

/// Value parser for `--algo`: calls [`Format::from_str`] and maps a
/// core `Error` to a `String` so clap can produce a usage error (exit 2).
fn parse_format(s: &str) -> Result<Format, String> {
    s.parse::<Format>().map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn parse(args: &[&str]) -> Cli {
        Cli::parse_from(std::iter::once("rcomp").chain(args.iter().copied()))
    }

    fn try_parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("rcomp").chain(args.iter().copied()))
    }

    #[test]
    fn level_defaults_to_best() {
        let cli = parse(&["file.gz"]);
        assert_eq!(cli.level(), Level::Best);
    }

    #[test]
    fn level_fast() {
        let cli = parse(&["file.gz", "--fast"]);
        assert_eq!(cli.level(), Level::Fast);
    }

    #[test]
    fn level_edge() {
        let cli = parse(&["file.gz", "--edge"]);
        assert_eq!(cli.level(), Level::Edge);
    }

    #[test]
    fn compress_and_extract_conflict() {
        assert!(try_parse(&["file.gz", "-c", "-x"]).is_err());
    }

    #[test]
    fn algo_parses_valid_format() {
        let cli = parse(&["in", "out.bz2", "-a", "bzip2"]);
        assert_eq!(cli.algo, Some(Format::codec(rcomp_core::Codec::Bzip2)));
    }

    #[test]
    fn algo_bad_value_is_err() {
        assert!(try_parse(&["in", "-a", "notaformat"]).is_err());
    }

    #[test]
    fn ls_subcommand_parses() {
        let cli = parse(&["ls", "archive.zip"]);
        assert!(matches!(cli.command, Some(SubCommand::Ls { .. })));
    }

    #[test]
    fn two_level_flags_conflict() {
        assert!(try_parse(&["file.gz", "--fast", "--edge"]).is_err());
    }
}
