//! Pure inference logic: decide whether to compress or extract.
//!
//! [`infer_mode`] accepts a set of already-resolved *facts* (no filesystem
//! access is performed inside the function) and applies the four inference
//! rules from the plan in order.

use rcomp_core::Format;

/// The result of a successful inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The caller should compress `INPUT` into `OUTPUT`.
    Compress,
    /// The caller should extract `INPUT` into the destination.
    Extract,
}

/// Structured error returned when inference is ambiguous (rule 4).
#[derive(Debug)]
pub struct AmbiguityError {
    /// Human-readable message explaining both possible interpretations.
    pub message: String,
}

impl std::fmt::Display for AmbiguityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AmbiguityError {}

/// Facts passed to [`infer_mode`].
///
/// All filesystem-dependent values are pre-computed by the caller (typically
/// `run.rs`) so the inference function remains purely logical and fully
/// testable without a filesystem.
#[derive(Debug)]
pub struct InferFacts<'a> {
    /// `--compress` flag was given.
    pub has_compress_flag: bool,
    /// `--extract` flag was given.
    pub has_extract_flag: bool,
    /// The raw OUTPUT argument string, if provided.
    pub output: Option<&'a str>,
    /// The result of `split_format_suffix(OUTPUT)`, if OUTPUT was provided.
    ///
    /// `Some((stem, format))` means OUTPUT has a recognised archive/codec
    /// extension; `None` means it is extensionless or unrecognised.
    pub output_suffix: Option<(&'a str, Format)>,
    /// `--algo` was given (resolves an otherwise-ambiguous compress invocation
    /// where OUTPUT has no recognised suffix).
    pub has_algo: bool,
    /// Whether INPUT is a readable file (i.e. `fs::File::open(INPUT)` succeeds).
    pub input_is_readable_file: bool,
    /// Whether `detect(INPUT)` succeeded (format was recognised).
    pub input_detects_ok: bool,
}

/// Apply inference rules 1–4 and return the [`Mode`].
///
/// Rules are applied in order and the first match wins:
///
/// 1. `--compress` / `--extract` flag → obey.
///    - `--compress` with neither `--algo` nor a recognised OUTPUT suffix →
///      [`AmbiguityError`] (usage error; caller maps to exit 2).
/// 2. OUTPUT given with a recognised archive/codec suffix → [`Mode::Compress`].
///    Also fires when `--algo` is given together with an explicit OUTPUT (the
///    user named both the algorithm and the destination).  This rule is checked
///    *before* rule 3 so that `rcomp <detected-file> out -a brotli` is
///    interpreted as "compress into `out` using brotli", not "extract".
/// 3. INPUT is a readable file AND (its format detects successfully OR `--algo`
///    was given) → [`Mode::Extract`].
///    The `--algo` branch handles codecs without magic bytes (e.g. brotli):
///    when the user writes `rcomp mystery-download -a brotli` the file cannot
///    be auto-detected but the explicit codec unambiguously selects extraction.
///    This branch is only reachable when OUTPUT is absent or has no recognised
///    suffix (rule 2 would have already fired otherwise).
/// 4. None of the above → [`AmbiguityError`] listing both interpretations.
pub fn infer_mode(facts: &InferFacts<'_>) -> Result<Mode, AmbiguityError> {
    // Rule 1: explicit flags.
    if facts.has_compress_flag {
        // Validate: compress requires a format target.
        if !facts.has_algo && facts.output_suffix.is_none() {
            return Err(AmbiguityError {
                message: concat!(
                    "--compress requires either a recognized OUTPUT extension ",
                    "or --algo to specify the target format. ",
                    "Example: rcomp -c INPUT output.tar.gz  OR  rcomp -c -a bzip2 INPUT OUTPUT"
                )
                .to_owned(),
            });
        }
        return Ok(Mode::Compress);
    }
    if facts.has_extract_flag {
        return Ok(Mode::Extract);
    }

    // Rule 2: OUTPUT given with a recognised suffix → compress.
    // Also fires when --algo is given along with an explicit OUTPUT, since the
    // user is naming both the algorithm and the destination.  This must precede
    // rule 3 so that a detectable (or --algo-named) INPUT file does not
    // accidentally trigger extraction when the user clearly wants to compress
    // into a named output.
    if facts.output_suffix.is_some() {
        return Ok(Mode::Compress);
    }
    if facts.has_algo && facts.output.is_some() {
        return Ok(Mode::Compress);
    }

    // Rule 3: INPUT is a readable file that is either auto-detectable or has
    // an explicit --algo override → extract.
    //
    // The `has_algo` branch handles formats with no magic bytes (notably
    // brotli): `rcomp mystery-download -a brotli` cannot auto-detect but the
    // user's explicit codec makes the intent unambiguous.  Rule 2 has already
    // handled the case where --algo is paired with an explicit OUTPUT, so
    // reaching here means OUTPUT is absent or has an unrecognised suffix.
    if facts.input_is_readable_file && (facts.input_detects_ok || facts.has_algo) {
        return Ok(Mode::Extract);
    }

    // Rule 4: ambiguous.
    let input_clause = if facts.input_is_readable_file && !facts.input_detects_ok {
        "INPUT is a readable file but its format was not recognized (cannot auto-extract; \
         if it is a compressed file with no magic bytes such as brotli, \
         pass --algo <codec> to extract it)"
    } else if !facts.input_is_readable_file {
        "INPUT is not a readable file (treating as directory or non-existent)"
    } else {
        "INPUT status is ambiguous"
    };

    let output_clause = match facts.output {
        Some(out) => {
            format!("OUTPUT `{out}` has no recognized archive extension (cannot auto-compress)")
        }
        None => "no OUTPUT was given".to_owned(),
    };

    Err(AmbiguityError {
        message: format!(
            "cannot determine whether to compress or extract: {input_clause}; \
             {output_clause}. \
             Use -c/--compress to force compression or -x/--extract to force extraction."
        ),
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rcomp_core::{Codec, Container, Format};

    // --- Rule 1: explicit --compress flag ---

    #[test]
    fn compress_flag_with_recognized_output_suffix() {
        let facts = InferFacts {
            has_compress_flag: true,
            has_extract_flag: false,
            output: Some("archive.tar.gz"),
            output_suffix: Some(("archive", Format::layered(Container::Tar, Codec::Gzip))),
            has_algo: false,
            input_is_readable_file: false,
            input_detects_ok: false,
        };
        assert_eq!(infer_mode(&facts).unwrap(), Mode::Compress);
    }

    #[test]
    fn compress_flag_with_algo_no_output_suffix() {
        // --compress + --algo, no recognized output extension → ok
        let facts = InferFacts {
            has_compress_flag: true,
            has_extract_flag: false,
            output: Some("output"),
            output_suffix: None,
            has_algo: true,
            input_is_readable_file: false,
            input_detects_ok: false,
        };
        assert_eq!(infer_mode(&facts).unwrap(), Mode::Compress);
    }

    #[test]
    fn compress_flag_no_algo_no_output_suffix_is_usage_error() {
        // --compress but no --algo and no recognized suffix → usage error
        let facts = InferFacts {
            has_compress_flag: true,
            has_extract_flag: false,
            output: Some("noext"),
            output_suffix: None,
            has_algo: false,
            input_is_readable_file: true,
            input_detects_ok: false,
        };
        let err = infer_mode(&facts).unwrap_err();
        assert!(err.message.contains("--compress"), "error: {}", err.message);
    }

    #[test]
    fn compress_flag_no_output_at_all_is_usage_error() {
        let facts = InferFacts {
            has_compress_flag: true,
            has_extract_flag: false,
            output: None,
            output_suffix: None,
            has_algo: false,
            input_is_readable_file: true,
            input_detects_ok: false,
        };
        let err = infer_mode(&facts).unwrap_err();
        assert!(err.message.contains("--compress"), "error: {}", err.message);
    }

    // --- Rule 1: explicit --extract flag ---

    #[test]
    fn extract_flag_overrides() {
        let facts = InferFacts {
            has_compress_flag: false,
            has_extract_flag: true,
            output: Some("archive.tar.gz"),
            output_suffix: Some(("archive", Format::layered(Container::Tar, Codec::Gzip))),
            has_algo: false,
            input_is_readable_file: false,
            input_detects_ok: false,
        };
        // Even though OUTPUT looks like a compress target, --extract wins.
        assert_eq!(infer_mode(&facts).unwrap(), Mode::Extract);
    }

    #[test]
    fn extract_flag_no_output() {
        let facts = InferFacts {
            has_compress_flag: false,
            has_extract_flag: true,
            output: None,
            output_suffix: None,
            has_algo: false,
            input_is_readable_file: true,
            input_detects_ok: true,
        };
        assert_eq!(infer_mode(&facts).unwrap(), Mode::Extract);
    }

    // --- Rule 2: OUTPUT has a recognised suffix ---

    #[test]
    fn output_suffix_implies_compress() {
        let facts = InferFacts {
            has_compress_flag: false,
            has_extract_flag: false,
            output: Some("backup.tar.bz2"),
            output_suffix: Some(("backup", Format::layered(Container::Tar, Codec::Bzip2))),
            has_algo: false,
            input_is_readable_file: true, // input could be detectable too
            input_detects_ok: true,       // rule 2 wins before rule 3
        };
        assert_eq!(infer_mode(&facts).unwrap(), Mode::Compress);
    }

    #[test]
    fn output_suffix_zip_implies_compress() {
        let facts = InferFacts {
            has_compress_flag: false,
            has_extract_flag: false,
            output: Some("out.zip"),
            output_suffix: Some(("out", Format::container(Container::Zip))),
            has_algo: false,
            input_is_readable_file: false,
            input_detects_ok: false,
        };
        assert_eq!(infer_mode(&facts).unwrap(), Mode::Compress);
    }

    // --- Rule 3: INPUT detectable → extract ---

    #[test]
    fn detectable_input_implies_extract() {
        let facts = InferFacts {
            has_compress_flag: false,
            has_extract_flag: false,
            output: None,
            output_suffix: None,
            has_algo: false,
            input_is_readable_file: true,
            input_detects_ok: true,
        };
        assert_eq!(infer_mode(&facts).unwrap(), Mode::Extract);
    }

    #[test]
    fn detectable_input_with_dest_output_implies_extract() {
        // OUTPUT given but without a recognised extension (so rule 2 doesn't
        // fire); INPUT detects → extract with OUTPUT as dest dir.
        let facts = InferFacts {
            has_compress_flag: false,
            has_extract_flag: false,
            output: Some("./my_dest"),
            output_suffix: None,
            has_algo: false,
            input_is_readable_file: true,
            input_detects_ok: true,
        };
        assert_eq!(infer_mode(&facts).unwrap(), Mode::Extract);
    }

    // --- Rule 3 (extended): --algo without output makes undetectable INPUT extract ---

    #[test]
    fn readable_undetectable_with_algo_no_output_extracts() {
        // Scenario: `rcomp mystery-download -a brotli`
        // INPUT is readable but brotli has no magic bytes so detect() fails.
        // --algo is given but no OUTPUT → rule 2 does NOT fire (output.is_none());
        // rule 3 fires because has_algo is true.
        let facts = InferFacts {
            has_compress_flag: false,
            has_extract_flag: false,
            output: None,
            output_suffix: None,
            has_algo: true,
            input_is_readable_file: true,
            input_detects_ok: false,
        };
        assert_eq!(infer_mode(&facts).unwrap(), Mode::Extract);
    }

    #[test]
    fn readable_undetectable_no_algo_no_output_is_ambiguous() {
        // Same scenario but without --algo → genuinely ambiguous; rule 4 fires.
        let facts = InferFacts {
            has_compress_flag: false,
            has_extract_flag: false,
            output: None,
            output_suffix: None,
            has_algo: false,
            input_is_readable_file: true,
            input_detects_ok: false,
        };
        let err = infer_mode(&facts).unwrap_err();
        // Error must mention --algo as a way to extract an unrecognised file.
        assert!(
            err.message.contains("--algo"),
            "ambiguity error should mention --algo: {}",
            err.message
        );
        // Must still mention both compress and extract so the user knows all options.
        assert!(
            err.message.contains("compress") || err.message.contains("-c"),
            "error: {}",
            err.message
        );
        assert!(
            err.message.contains("extract") || err.message.contains("-x"),
            "error: {}",
            err.message
        );
    }

    #[test]
    fn rule2_wins_over_rule3_when_algo_and_output_given() {
        // `rcomp detected-file.gz out-path -a brotli` — OUTPUT given + --algo:
        // rule 2 fires (compress) before rule 3 (extract) even though INPUT
        // is detectable.  The user named an output destination so compression
        // is the clear intent.
        let facts = InferFacts {
            has_compress_flag: false,
            has_extract_flag: false,
            output: Some("out-path"),
            output_suffix: None, // no recognised extension on the output
            has_algo: true,
            input_is_readable_file: true,
            input_detects_ok: true,
        };
        assert_eq!(infer_mode(&facts).unwrap(), Mode::Compress);
    }

    // --- Rule 4: ambiguous ---

    #[test]
    fn plain_file_no_output_ambiguous() {
        // Readable file but format undetected AND no OUTPUT → ambiguous.
        let facts = InferFacts {
            has_compress_flag: false,
            has_extract_flag: false,
            output: None,
            output_suffix: None,
            has_algo: false,
            input_is_readable_file: true,
            input_detects_ok: false,
        };
        let err = infer_mode(&facts).unwrap_err();
        assert!(err.message.contains("compress"), "error: {}", err.message);
        assert!(err.message.contains("extract"), "error: {}", err.message);
    }

    #[test]
    fn non_readable_input_no_output_ambiguous() {
        let facts = InferFacts {
            has_compress_flag: false,
            has_extract_flag: false,
            output: None,
            output_suffix: None,
            has_algo: false,
            input_is_readable_file: false,
            input_detects_ok: false,
        };
        let err = infer_mode(&facts).unwrap_err();
        // Error must suggest the disambiguating flags.
        assert!(
            err.message.contains("-c") || err.message.contains("--compress"),
            "error: {}",
            err.message
        );
    }

    #[test]
    fn ambiguity_error_names_both_flags() {
        let facts = InferFacts {
            has_compress_flag: false,
            has_extract_flag: false,
            output: None,
            output_suffix: None,
            has_algo: false,
            input_is_readable_file: true,
            input_detects_ok: false,
        };
        let err = infer_mode(&facts).unwrap_err();
        let msg = &err.message;
        // Must mention both modes so the user knows how to resolve it.
        assert!(msg.contains("compress") || msg.contains("-c"), "msg: {msg}");
        assert!(msg.contains("extract") || msg.contains("-x"), "msg: {msg}");
    }

    // --- edge: compress flag + extract flag conflict handled by clap, not here ---

    #[test]
    fn both_flags_never_reach_infer_compress_wins_in_logic() {
        // In practice clap would reject this, but ensure our logic is sound.
        // The function processes has_compress_flag first, so compress would win.
        let facts = InferFacts {
            has_compress_flag: true,
            has_extract_flag: true, // clap would normally reject this
            output: Some("out.tar.gz"),
            output_suffix: Some(("out", Format::layered(Container::Tar, Codec::Gzip))),
            has_algo: false,
            input_is_readable_file: false,
            input_detects_ok: false,
        };
        assert_eq!(infer_mode(&facts).unwrap(), Mode::Compress);
    }
}
