//! Top-level execution: compress, extract, list, completions, and man-page
//! operations wired to the CLI flags.
//!
//! This module owns the confirmation prompt (silent-tar), the wrap-folder
//! decision, the progress bars (via [`crate::ui`]), the summary-line output,
//! and the error-to-exit-code mapping.  It calls into `rcomp-core` for all
//! format work.

use std::{
    fs,
    io::{self, IsTerminal, Write as _},
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};
use indicatif::HumanBytes;
use rcomp_core::{
    CancelToken, CompressOptions, Error as CoreError, ExtractOptions, Format, Report, compress,
    detect, distinct_roots, extract, format_sidecar, list, split_format_suffix, wrap_dir_name,
};

use crate::cli::{Cli, SubCommand};
use crate::infer::{InferFacts, Mode, infer_mode};
use crate::ui;

// ---------------------------------------------------------------------------
// UsageError — maps to exit 2 in main.rs
// ---------------------------------------------------------------------------

/// A usage error that the CLI surface detected (not an inference ambiguity).
///
/// `main.rs` downcasts for this type alongside [`crate::infer::AmbiguityError`]
/// and exits with code 2.
#[derive(Debug)]
pub struct UsageError {
    /// Human-readable description of the problem.
    pub message: String,
}

impl std::fmt::Display for UsageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for UsageError {}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Run the operation described by `cli`.
///
/// `cancel` is a shared [`CancelToken`] that has already been wired to the
/// process's SIGINT handler in `main.rs`.  It is forwarded into every
/// compress/extract call so that Ctrl-C causes the operation to unwind
/// through [`CoreError::Cancelled`] (cleaning up partial output).
///
/// Returns `Ok(())` on success or an `anyhow::Error` whose root cause may be
/// [`CoreError`] or [`AmbiguityError`].  `main.rs` maps the error to the
/// appropriate exit code.
pub fn run(cli: &Cli, cancel: CancelToken) -> anyhow::Result<()> {
    // Subcommands bypass the inference path entirely.
    match &cli.command {
        Some(SubCommand::Ls { archive }) => return cmd_ls(Path::new(archive), cli.quiet),
        Some(SubCommand::Completions { shell }) => return cmd_completions(*shell),
        Some(SubCommand::Man) => return cmd_man(),
        None => {}
    }

    // INPUT is required for compress/extract.
    let input_str = cli
        .input
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("INPUT is required; run `rcomp --help` for usage"))?;
    let input = Path::new(input_str);

    // Pre-compute facts for the inference engine.
    let output_str = cli.output.as_deref();
    let output_suffix: Option<(&str, Format)> =
        output_str.and_then(|o| split_format_suffix(Path::new(o).file_name()?.to_str()?));

    let input_is_readable_file = std::fs::File::open(input).is_ok();
    let input_detects_ok = detect(input).is_ok();

    let facts = InferFacts {
        has_compress_flag: cli.compress,
        has_extract_flag: cli.extract,
        output: output_str,
        output_suffix,
        has_algo: cli.algo.is_some(),
        input_is_readable_file,
        input_detects_ok,
    };

    let mode = infer_mode(&facts)?;

    // Compress-only flags on an extract operation are a usage error (exit 2).
    if mode == Mode::Extract && (cli.checksum || cli.all || !cli.exclude.is_empty()) {
        return Err(anyhow::Error::new(UsageError {
            message: "--checksum, --all, and --exclude apply to compression only \
                      and cannot be used when extracting"
                .to_owned(),
        }));
    }

    match mode {
        Mode::Compress => cmd_compress(input, output_str, cli, cancel),
        Mode::Extract => {
            let dest = output_str.unwrap_or(".");
            cmd_extract(input, Path::new(dest), cli, cancel)
        }
    }
}

// ---------------------------------------------------------------------------
// Compress
// ---------------------------------------------------------------------------

fn cmd_compress(
    input: &Path,
    output_str: Option<&str>,
    cli: &Cli,
    cancel: CancelToken,
) -> anyhow::Result<()> {
    // OUTPUT is required for compress.
    let output_str = output_str.ok_or_else(|| {
        anyhow::anyhow!(
            "OUTPUT is required when compressing; \
             provide an output path with a recognized extension (e.g. archive.tar.gz)"
        )
    })?;
    let output = Path::new(output_str);

    // Determine the effective format:
    //   - --algo override
    //   - otherwise inferred from OUTPUT extension by the core
    //   - a None format means the core will infer from the output extension
    let format = cli.algo;

    // Silent-tar check: input is a directory AND the effective format is
    // codec-only (after the core applies the silent-tar rule).
    //
    // We detect this *before* calling core compress so we can prompt.
    let input_is_dir = input.is_dir();
    if input_is_dir {
        let effective_format: Option<Format> = format.or_else(|| {
            output
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(rcomp_core::detect_from_extension)
        });

        if let Some(fmt) = effective_format
            && fmt.container.is_none()
            && fmt.codec.is_some()
        {
            // Codec-only + directory input → silent-tar confirmation needed.
            confirm_silent_tar(output_str, cli.yes)?;
        }
    }

    // Sidecar pre-flight: if --checksum is requested and <output>.sha256
    // already exists without --force, refuse before doing any work.
    let sidecar_path = PathBuf::from(format!("{output_str}.sha256"));
    if cli.checksum && sidecar_path.exists() && !cli.force {
        return Err(anyhow::anyhow!(
            "sidecar `{}` already exists\nhint: use --force to overwrite `{}`",
            sidecar_path.display(),
            sidecar_path.display(),
        ));
    }

    let opts = CompressOptions {
        format,
        level: cli.level(),
        overwrite: cli.force,
        cancel,
        checksum: cli.checksum,
        follow_gitignore: !cli.all,
        exclude: cli.exclude.clone(),
    };

    let mut prog = ui::build(cli.quiet);
    let result = compress(input, output, &opts, |p| (prog.callback)(p));

    // On cancellation: abandon the bar so the terminal is not left corrupted,
    // then print a dedicated "cancelled" line to stderr and exit 1.
    if let Err(CoreError::Cancelled) = result {
        prog.guard.0.abandon();
        eprintln!("cancelled");
        std::process::exit(1);
    }

    // Map InvalidGlob → UsageError (exit 2).
    if let Err(CoreError::InvalidGlob {
        ref pattern,
        ref message,
    }) = result
    {
        return Err(anyhow::Error::new(UsageError {
            message: format!("invalid glob pattern `{pattern}`: {message}"),
        }));
    }

    let report = result.map_err(|e| map_core_error(e, "compress"))?;
    drop(prog.guard); // finish_and_clear the bar before printing summary

    // Write the sidecar when --checksum was requested.
    if let Some(ref artifact_hex) = report.sha256 {
        write_sidecar(
            &sidecar_path,
            output,
            artifact_hex,
            report.content_sha256.as_deref(),
        )
        .context("failed to write checksum sidecar")?;
    }

    if !cli.quiet {
        print_compress_summary(output_str, &report);

        // Show the artifact digest after the summary line.
        if let Some(ref hex) = report.sha256 {
            println!("sha256: {hex}");
        }
    }

    // Print excluded-paths note to stderr (advisory; shown unless -q).
    if !cli.quiet && report.entries_excluded > 0 {
        let n = report.entries_excluded;
        // Choose wording from which exclusion sources were CONFIGURED.
        let note = if !cli.all && !cli.exclude.is_empty() {
            // Both gitignore (follow_gitignore=true) and --exclude active.
            format!("excluded {n} paths via .gitignore and --exclude")
        } else if cli.all {
            // follow_gitignore=false but --exclude was given.
            format!("excluded {n} paths via --exclude")
        } else {
            // Only gitignore filtering (no --exclude).
            format!("excluded {n} paths via .gitignore (use --all to include)")
        };
        eprintln!("{note}");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Silent-tar confirmation
// ---------------------------------------------------------------------------

/// Prompt the user to confirm the silent-tar operation (directory → codec-only
/// output that will contain tar data).
///
/// - `yes` flag skips the prompt.
/// - Non-TTY stdin without `yes` → error with "pass -y" hint.
/// - User answers anything other than `y`/`Y` → "aborted" error.
fn confirm_silent_tar(output: &str, yes: bool) -> anyhow::Result<()> {
    if yes {
        return Ok(());
    }

    // Non-TTY without -y → hard error.
    if !io::stdin().is_terminal() {
        bail!(
            "the output `{output}` will contain a tar archive inside a codec stream \
             (other tools expect a `.tar.*` extension). \
             Stdin is not a TTY — pass -y to auto-accept."
        );
    }

    // Interactive prompt.
    eprintln!(
        "warning: `{output}` will contain a tar archive inside a codec stream.\n\
         Other tools (tar, bzip2 -d, …) expect a `.tar.*` extension for this content.\n\
         Continue anyway? [y/N]"
    );

    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    let trimmed = answer.trim();

    if trimmed.eq_ignore_ascii_case("y") {
        Ok(())
    } else {
        bail!("aborted");
    }
}

// ---------------------------------------------------------------------------
// Extract
// ---------------------------------------------------------------------------

fn cmd_extract(input: &Path, dest: &Path, cli: &Cli, cancel: CancelToken) -> anyhow::Result<()> {
    // Parse the sidecar BEFORE the wrap-decision list() call.
    //
    // A sidecar alongside the archive is a promise: if it exists but cannot be
    // parsed, that is an error.  When absent, verification is skipped entirely
    // (exactly today's behaviour).
    let sidecar_path = {
        let mut p = input.as_os_str().to_os_string();
        p.push(".sha256");
        PathBuf::from(p)
    };
    let (verify_sha256, verify_content_sha256) = if sidecar_path.exists() {
        parse_sidecar(input, &sidecar_path)?
    } else {
        (None, None)
    };
    let has_sidecar = verify_sha256.is_some();

    // Wrap decision: call list() to inspect the archive's top-level entries.
    //
    // - Ok(entries): count distinct first-path-components.
    //   >1 root AND no --unwrap → extract into dest/<stem>/
    //   Otherwise: extract directly into dest.
    // - Err(UnsupportedOperation): true bare-codec stream → no wrapping.
    let effective_dest = if cli.unwrap {
        dest.to_path_buf()
    } else {
        match list(input) {
            Ok(entries) => {
                let roots = distinct_roots(&entries);
                if roots > 1 {
                    // Wrap: dest/<archive-stem>/
                    let file_name = input.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    let stem = wrap_dir_name(file_name);
                    dest.join(stem)
                } else {
                    dest.to_path_buf()
                }
            }
            Err(CoreError::UnsupportedOperation { .. }) => {
                // Bare codec (e.g. plain .bz2 of a single file) — no wrap.
                dest.to_path_buf()
            }
            Err(CoreError::UnknownFormat { .. }) => {
                // Format could not be auto-detected (e.g. brotli — no magic
                // bytes) but the caller supplied --algo to name the codec.
                // list() requires detection to succeed, so it fails here.
                // Treat this the same as UnsupportedOperation: the input is a
                // bare codec stream, so extract directly into dest with no
                // wrapping folder.
                dest.to_path_buf()
            }
            Err(e) => {
                return Err(map_core_error(e, "list before extract"));
            }
        }
    };

    let opts = ExtractOptions {
        format: cli.algo,
        overwrite: cli.force,
        cancel,
        verify_sha256,
        verify_content_sha256,
    };

    let mut prog = ui::build(cli.quiet);
    let result = extract(input, &effective_dest, &opts, |p| (prog.callback)(p));

    // On cancellation: abandon the bar so the terminal is not left corrupted,
    // then print a dedicated "cancelled" line to stderr and exit 1.
    if let Err(CoreError::Cancelled) = result {
        prog.guard.0.abandon();
        eprintln!("cancelled");
        std::process::exit(1);
    }

    // Map ChecksumMismatch → clear message, exit 1.
    if let Err(CoreError::ChecksumMismatch {
        kind,
        ref expected,
        ref actual,
    }) = result
    {
        drop(prog.guard);
        eprintln!("error: checksum mismatch ({kind}): expected {expected}, got {actual}");
        std::process::exit(1);
    }

    let report = result.map_err(|e| map_core_error(e, "extract"))?;
    drop(prog.guard); // finish_and_clear the bar before printing summary

    // Print verification note to stderr when a sidecar was used (not under -q).
    if has_sidecar && !cli.quiet {
        eprintln!("verified sha256");
    }

    if !cli.quiet {
        print_extract_summary(&effective_dest, &report);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// List (ls subcommand)
// ---------------------------------------------------------------------------

fn cmd_ls(archive: &Path, quiet: bool) -> anyhow::Result<()> {
    let entries = list(archive).map_err(|e| {
        if let CoreError::UnsupportedOperation { .. } = &e {
            anyhow::anyhow!(
                "{e}\n\
                 hint: bare codec streams (e.g. .gz of a single file) do not have \
                 listable entries — extract it first or inspect the decompressed content"
            )
        } else {
            map_core_error(e, "list")
        }
    })?;

    if !quiet {
        let stdout = io::stdout();
        let mut out = io::BufWriter::new(stdout.lock());
        for entry in &entries {
            let path_str = if entry.is_dir {
                format!("{}/", entry.path.display())
            } else {
                entry.path.display().to_string()
            };
            writeln!(out, "{:>12}  {path_str}", entry.size).context("failed to write ls output")?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Write a `sha256sum`-compatible sidecar file at `sidecar_path`.
///
/// Format:
///
/// ```text
/// # content-sha256: <hex>          ← only when content_hex is Some
/// <artifact_hex>  <output_file_name>
/// ```
///
/// The two-space separator between the digest and filename is the standard
/// `sha256sum` output format so that `sha256sum -c <sidecar>` works.
fn write_sidecar(
    sidecar_path: &Path,
    output: &Path,
    artifact_hex: &str,
    content_hex: Option<&str>,
) -> anyhow::Result<()> {
    let file_name = output.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let text = format_sidecar(artifact_hex, content_hex, file_name);
    fs::write(sidecar_path, text.as_bytes())?;
    Ok(())
}

/// Read and parse a `sha256sum`-style sidecar file for `input`.
///
/// Reads the file at `sidecar_path` from disk, then delegates the pure parse
/// to [`rcomp_core::parse_sidecar`].
///
/// Returns `(verify_sha256, verify_content_sha256)`.
///
/// # Errors
///
/// Returns an error (exit 1) when the sidecar file cannot be read, when no
/// line matches the input's file name, or when a matched artifact line has a
/// malformed digest.
fn parse_sidecar(
    input: &Path,
    sidecar_path: &Path,
) -> anyhow::Result<(Option<String>, Option<String>)> {
    let raw = fs::read_to_string(sidecar_path)
        .with_context(|| format!("failed to read sidecar `{}`", sidecar_path.display()))?;

    let input_name = input.file_name().and_then(|n| n.to_str()).unwrap_or("");

    rcomp_core::parse_sidecar(&raw, input_name)
        .map_err(|e| anyhow::anyhow!("sidecar `{}`: {e}", sidecar_path.display()))
}

// ---------------------------------------------------------------------------
// Summary-line printers
// ---------------------------------------------------------------------------

/// Format a [`std::time::Duration`] as a short human string, e.g. `1.2s`,
/// `450ms`, `2m 3s`.
fn fmt_duration(d: std::time::Duration) -> String {
    let total_secs = d.as_secs();
    if total_secs >= 60 {
        let m = total_secs / 60;
        let s = total_secs % 60;
        format!("{m}m {s}s")
    } else if total_secs >= 1 {
        // One decimal place for sub-10-second durations.
        let tenths = d.subsec_millis() / 100;
        format!("{total_secs}.{tenths}s")
    } else {
        format!("{}ms", d.subsec_millis())
    }
}

/// Print the compress summary line to **stdout**.
///
/// Format: `<output>  <in> → <out> (<ratio>%)  in <elapsed>`
fn print_compress_summary(output: &str, r: &Report) {
    let ratio_pct = r.ratio() * 100.0;
    println!(
        "{output}  {} \u{2192} {} ({:.1}%)  in {}",
        HumanBytes(r.input_bytes),
        HumanBytes(r.output_bytes),
        ratio_pct,
        fmt_duration(r.duration),
    );
}

/// Print the extract summary line to **stdout**.
///
/// Format: `extracted <entries> entries to <dest>  in <elapsed>`
fn print_extract_summary(dest: &Path, r: &Report) {
    println!(
        "extracted {} entries to {}  in {}",
        r.entries,
        dest.display(),
        fmt_duration(r.duration),
    );
}

// ---------------------------------------------------------------------------
// Completions and man-page subcommands
// ---------------------------------------------------------------------------

/// Generate shell completion script to stdout.
fn cmd_completions(shell: clap_complete::Shell) -> anyhow::Result<()> {
    use clap::CommandFactory;
    use clap_complete::generate;
    let mut cmd = crate::cli::Cli::command();
    let mut stdout = io::stdout();
    generate(shell, &mut cmd, "rcomp", &mut stdout);
    Ok(())
}

/// Render a troff man page to stdout.
fn cmd_man() -> anyhow::Result<()> {
    use clap::CommandFactory;
    let cmd = crate::cli::Cli::command();
    let man = clap_mangen::Man::new(cmd);
    let mut stdout = io::stdout();
    man.render(&mut stdout)
        .context("failed to render man page")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------

/// Map a [`CoreError`] to an `anyhow::Error` with contextual hints.
fn map_core_error(e: CoreError, operation: &str) -> anyhow::Error {
    match e {
        CoreError::AlreadyExists { ref path } => {
            anyhow::anyhow!("{e}\nhint: use --force to overwrite `{}`", path.display())
        }
        other => anyhow::anyhow!("{other}").context(format!("{operation} failed")),
    }
}
