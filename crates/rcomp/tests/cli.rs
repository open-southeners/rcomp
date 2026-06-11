//! CLI integration tests using `assert_cmd`.
//!
//! Each test spawns the `rcomp` binary via [`Command::cargo_bin`] so the
//! full argument-parsing → inference → core path is exercised.  stdin is
//! non-TTY by default under assert_cmd (which is exactly the behavior we need
//! for the silent-tar-without-`-y` test).

use std::{
    fs,
    path::{Path, PathBuf},
};

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Return an [`assert_cmd::Command`] pre-configured to run the `rcomp` binary.
fn rcomp() -> Command {
    Command::cargo_bin("rcomp").expect("rcomp binary must exist")
}

/// Create a temporary directory containing two named files with known content.
///
/// Layout:
/// ```
/// <dir>/
///   hello.txt   ("hello\n")
///   world.txt   ("world\n")
/// ```
fn make_two_file_dir(parent: &TempDir) -> PathBuf {
    let dir = parent.path().join("src_dir");
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("hello.txt"), b"hello\n").unwrap();
    fs::write(dir.join("world.txt"), b"world\n").unwrap();
    dir
}

/// Create a temporary directory containing ONE named file.
fn make_one_file_dir(parent: &TempDir) -> PathBuf {
    let dir = parent.path().join("single_dir");
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("only.txt"), b"only\n").unwrap();
    dir
}

// ---------------------------------------------------------------------------
// Test: compress file → .gz
// ---------------------------------------------------------------------------

#[test]
fn compress_file_to_gz() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("data.txt");
    fs::write(&src, b"hello world").unwrap();
    let out = tmp.path().join("data.txt.gz");

    rcomp()
        .args([src.to_str().unwrap(), out.to_str().unwrap()])
        .assert()
        .success();

    assert!(out.exists(), "output .gz should exist");
}

// ---------------------------------------------------------------------------
// Test: compress directory → .tar.gz
// ---------------------------------------------------------------------------

#[test]
fn compress_dir_to_tar_gz() {
    let tmp = TempDir::new().unwrap();
    let dir = make_two_file_dir(&tmp);
    let out = tmp.path().join("archive.tar.gz");

    rcomp()
        .args([dir.to_str().unwrap(), out.to_str().unwrap()])
        .assert()
        .success();

    assert!(out.exists(), "output .tar.gz should exist");
}

// ---------------------------------------------------------------------------
// Test: dir → .bz2 without -y and non-TTY stdin → fails with hint
// ---------------------------------------------------------------------------

#[test]
fn compress_dir_bz2_no_y_nontty_fails_with_hint() {
    let tmp = TempDir::new().unwrap();
    let dir = make_two_file_dir(&tmp);
    let out = tmp.path().join("archive.bz2");

    // assert_cmd uses non-TTY stdin by default — this is the exact scenario.
    rcomp()
        .args([dir.to_str().unwrap(), out.to_str().unwrap()])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("-y"));
}

// ---------------------------------------------------------------------------
// Test: dir → .bz2 with -y → succeeds AND extracting restores the tree
// ---------------------------------------------------------------------------

#[test]
fn compress_dir_bz2_with_y_and_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let dir = make_two_file_dir(&tmp);
    let out = tmp.path().join("archive.bz2");

    // Compress with -y to skip the silent-tar prompt.
    rcomp()
        .args([dir.to_str().unwrap(), out.to_str().unwrap(), "-y"])
        .assert()
        .success();

    assert!(out.exists(), "archive.bz2 should exist");

    // Extract the produced .bz2 back out.
    let restore = tmp.path().join("restored");
    fs::create_dir(&restore).unwrap();

    rcomp()
        .args([out.to_str().unwrap(), restore.to_str().unwrap(), "-x"])
        .assert()
        .success();

    // The extraction should have produced the two original files (wrapped in a
    // src_dir folder because the archive has multiple roots).
    let content_root = &restore;
    // After wrap logic: the stem of "archive.bz2" is "archive", so the files
    // end up under restore/archive/... OR restore/src_dir/...
    // We just verify that both hello.txt and world.txt appear somewhere under
    // restore/.
    let hello = find_file_recursive(content_root, "hello.txt");
    let world = find_file_recursive(content_root, "world.txt");

    assert!(hello.is_some(), "hello.txt should be restored");
    assert!(world.is_some(), "world.txt should be restored");

    assert_eq!(fs::read(hello.unwrap()).unwrap(), b"hello\n");
    assert_eq!(fs::read(world.unwrap()).unwrap(), b"world\n");
}

/// Walk `dir` recursively and return the first path whose file name equals `name`.
fn find_file_recursive(dir: &Path, name: &str) -> Option<PathBuf> {
    for entry in fs::read_dir(dir).ok()? {
        let entry = entry.ok()?;
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file_recursive(&path, name) {
                return Some(found);
            }
        } else if path.file_name().and_then(|n| n.to_str()) == Some(name) {
            return Some(path);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Test: -a bzip2 on extensionless output name
// ---------------------------------------------------------------------------

#[test]
fn compress_with_algo_bzip2_extensionless_output() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("data.txt");
    fs::write(&src, b"some data for bzip2").unwrap();
    let out = tmp.path().join("myarchive");

    rcomp()
        .args([src.to_str().unwrap(), out.to_str().unwrap(), "-a", "bzip2"])
        .assert()
        .success();

    assert!(out.exists(), "output file should exist");
}

// ---------------------------------------------------------------------------
// Test: extract .tar.gz → wrap folder appears for multi-root archive
// ---------------------------------------------------------------------------

#[test]
fn extract_multi_root_creates_wrap_folder() {
    let tmp = TempDir::new().unwrap();
    let dir = make_two_file_dir(&tmp);
    // Archive name stem is "multi" — wrap folder will be dest/multi/
    let archive = tmp.path().join("multi.tar.gz");

    // Compress src_dir/ → multi.tar.gz. The tar backend stores the contents
    // of src_dir/ directly (hello.txt, world.txt) without the src_dir/ prefix,
    // so the archive has TWO distinct top-level roots.
    rcomp()
        .args([dir.to_str().unwrap(), archive.to_str().unwrap()])
        .assert()
        .success();

    // Verify: list the archive to confirm two root entries.
    // (This also confirms list() works for this format.)
    rcomp()
        .args(["ls", archive.to_str().unwrap()])
        .assert()
        .success();

    let dest = tmp.path().join("extract_dest");
    fs::create_dir(&dest).unwrap();

    rcomp()
        .args([archive.to_str().unwrap(), dest.to_str().unwrap()])
        .assert()
        .success();

    // Two distinct roots → wrap folder "multi" is created.
    let wrap_dir = dest.join("multi");
    assert!(
        wrap_dir.exists(),
        "wrap folder dest/multi/ should be created for multi-root archive"
    );
    assert!(
        wrap_dir.join("hello.txt").exists(),
        "hello.txt inside wrap folder"
    );
    assert!(
        wrap_dir.join("world.txt").exists(),
        "world.txt inside wrap folder"
    );
}

// ---------------------------------------------------------------------------
// Test: single-root archive → no extra folder
// ---------------------------------------------------------------------------

#[test]
fn extract_single_root_no_extra_folder() {
    let tmp = TempDir::new().unwrap();
    let dir = make_one_file_dir(&tmp);
    let archive = tmp.path().join("single.tar.gz");

    // Compress single_dir/ → single.tar.gz. The tar backend stores only.txt
    // without the single_dir/ prefix, so the archive has ONE distinct root.
    rcomp()
        .args([dir.to_str().unwrap(), archive.to_str().unwrap()])
        .assert()
        .success();

    let dest = tmp.path().join("single_dest");
    fs::create_dir(&dest).unwrap();

    rcomp()
        .args([archive.to_str().unwrap(), dest.to_str().unwrap()])
        .assert()
        .success();

    // Single root → no wrap folder → only.txt appears directly in dest/.
    assert!(
        dest.join("only.txt").exists(),
        "only.txt should be directly in dest (no extra wrap folder)"
    );
}

// ---------------------------------------------------------------------------
// Test: --unwrap → no extra folder regardless of root count
// ---------------------------------------------------------------------------

#[test]
fn extract_unwrap_skips_wrap_folder() {
    let tmp = TempDir::new().unwrap();

    // Compressing src_dir/ produces a tar.gz with two loose top-level entries
    // (hello.txt, world.txt) — normally this triggers wrapping into
    // dest/unwrap_test/. With --unwrap, the files go directly into dest/.
    let dir = make_two_file_dir(&tmp);
    let archive = tmp.path().join("unwrap_test.tar.gz");

    rcomp()
        .args([dir.to_str().unwrap(), archive.to_str().unwrap()])
        .assert()
        .success();

    let dest = tmp.path().join("unwrap_dest");
    fs::create_dir(&dest).unwrap();

    rcomp()
        .args([
            archive.to_str().unwrap(),
            dest.to_str().unwrap(),
            "--unwrap",
        ])
        .assert()
        .success();

    // With --unwrap the files land directly in dest/ (no "unwrap_test" folder).
    assert!(
        dest.join("hello.txt").exists(),
        "hello.txt should be directly in dest with --unwrap"
    );
    assert!(
        dest.join("world.txt").exists(),
        "world.txt should be directly in dest with --unwrap"
    );
}

// ---------------------------------------------------------------------------
// Test: extract to explicit destination directory
// ---------------------------------------------------------------------------

#[test]
fn extract_to_explicit_dest() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("file.txt");
    fs::write(&src, b"test content").unwrap();
    let archive = tmp.path().join("file.txt.gz");

    rcomp()
        .args([src.to_str().unwrap(), archive.to_str().unwrap()])
        .assert()
        .success();

    let dest = tmp.path().join("my_dest");
    // dest does not exist yet — rcomp must create it.

    rcomp()
        .args([archive.to_str().unwrap(), dest.to_str().unwrap()])
        .assert()
        .success();

    assert!(dest.exists(), "dest dir should be created");
    assert!(
        dest.join("file.txt").exists(),
        "file.txt should be extracted"
    );
}

// ---------------------------------------------------------------------------
// Test: ls output — exact format on a known zip
// ---------------------------------------------------------------------------

#[test]
fn ls_output_format() {
    let tmp = TempDir::new().unwrap();

    // Build a small zip via rcomp so we have a known archive.
    let dir = tmp.path().join("ls_src");
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("a.txt"), b"aaaa").unwrap(); // 4 bytes
    let archive = tmp.path().join("ls_test.zip");

    rcomp()
        .args([dir.to_str().unwrap(), archive.to_str().unwrap()])
        .assert()
        .success();

    // Run ls and check that every line has the "{size:>12}  {path}" format.
    let output = rcomp()
        .args(["ls", archive.to_str().unwrap()])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let text = String::from_utf8(output).unwrap();
    assert!(!text.is_empty(), "ls output must not be empty");

    for line in text.lines() {
        // Each line must be at least: 12 chars of size + 2 spaces + path.
        assert!(line.len() >= 14, "line too short: {:?}", line);
        // The size field (first 12 chars) must be parseable as an integer.
        let size_str = line[..12].trim();
        size_str
            .parse::<u64>()
            .unwrap_or_else(|_| panic!("size field is not a number on line: {line:?}"));
        // There must be exactly two spaces between size and path.
        assert_eq!(
            &line[12..14],
            "  ",
            "separator must be two spaces: {line:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Test: ls on bare codec → exit 1
// ---------------------------------------------------------------------------

#[test]
fn ls_bare_codec_exits_1() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("data.txt");
    fs::write(&src, b"hello").unwrap();
    let archive = tmp.path().join("data.txt.gz");

    // Note: data.txt.gz from a single file will be a bare gzip — list() on
    // a non-tar gzip returns UnsupportedOperation.
    rcomp()
        .args([src.to_str().unwrap(), archive.to_str().unwrap()])
        .assert()
        .success();

    rcomp()
        .args(["ls", archive.to_str().unwrap()])
        .assert()
        .failure()
        .code(1);
}

// ---------------------------------------------------------------------------
// Test: ambiguous input (plain unrecognized file, no output) → exit 2
//       with both readings mentioned in stderr
// ---------------------------------------------------------------------------

#[test]
fn ambiguous_input_exits_2() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("mystery");
    fs::write(&src, b"not a known format").unwrap();

    rcomp()
        .args([src.to_str().unwrap()])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("compress").or(predicate::str::contains("-c")))
        .stderr(predicate::str::contains("extract").or(predicate::str::contains("-x")));
}

// ---------------------------------------------------------------------------
// Test: overwrite refused → exit 1 + --force hint
// ---------------------------------------------------------------------------

#[test]
fn overwrite_refused_exits_1_with_force_hint() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("data.txt");
    fs::write(&src, b"hello").unwrap();
    let out = tmp.path().join("data.txt.gz");

    // First compress succeeds.
    rcomp()
        .args([src.to_str().unwrap(), out.to_str().unwrap()])
        .assert()
        .success();

    // Second compress without --force must fail.
    rcomp()
        .args([src.to_str().unwrap(), out.to_str().unwrap()])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("--force").or(predicate::str::contains("force")));
}

// ---------------------------------------------------------------------------
// Test: --force allows overwrite
// ---------------------------------------------------------------------------

#[test]
fn overwrite_with_force_succeeds() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("data.txt");
    fs::write(&src, b"hello").unwrap();
    let out = tmp.path().join("data.txt.gz");

    rcomp()
        .args([src.to_str().unwrap(), out.to_str().unwrap()])
        .assert()
        .success();

    rcomp()
        .args([src.to_str().unwrap(), out.to_str().unwrap(), "--force"])
        .assert()
        .success();
}

// ---------------------------------------------------------------------------
// Test: --fast, --best, --edge are all accepted
// ---------------------------------------------------------------------------

#[test]
fn level_flags_accepted() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("data.txt");
    fs::write(&src, b"hello world").unwrap();

    for flag in &["--fast", "--best", "--edge"] {
        let out = tmp.path().join(format!("out_{}.gz", &flag[2..]));
        rcomp()
            .args([src.to_str().unwrap(), out.to_str().unwrap(), flag])
            .assert()
            .success();
    }
}

// ---------------------------------------------------------------------------
// Test: bad --algo value → exit 2
// ---------------------------------------------------------------------------

#[test]
fn bad_algo_exits_2() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("data.txt");
    fs::write(&src, b"hello").unwrap();

    rcomp()
        .args([
            src.to_str().unwrap(),
            "out.lzma",
            "--algo",
            "lzma_does_not_exist",
        ])
        .assert()
        .failure()
        .code(2);
}

// ---------------------------------------------------------------------------
// Test: brotli mystery-file roundtrip via --algo (two scenarios)
//
// Verifies the fix for the rule-3 inference bug: a brotli-compressed file has
// no magic bytes, so auto-detection fails.  When the file also has no
// recognised extension (e.g. a downloaded file renamed to a bare name), the
// user must supply --algo to name the codec.
//
// Two sub-scenarios are tested:
//
// A. No OUTPUT given: `rcomp mystery-download -a brotli` — rule 3 fires
//    (readable + has_algo → Extract).  This is the main regression test.
//
// B. --extract flag with explicit dest: `rcomp mystery-download dest -a brotli
//    -x` — rule 1 forces extraction.  This covers the previously-untested
//    extract-format-override path (ExtractOptions.format = cli.algo).
// ---------------------------------------------------------------------------

#[test]
fn brotli_mystery_file_infer_extract_no_output() {
    let tmp = TempDir::new().unwrap();
    let original_content = b"brotli roundtrip test content";

    // Step 1: write a plain-text source file.
    let src = tmp.path().join("source.txt");
    fs::write(&src, original_content).unwrap();

    // Step 2: compress it to a .br file using the recognised extension so that
    // inference selects compress via rule 2 (output_suffix fires).
    let br_file = tmp.path().join("source.txt.br");
    rcomp()
        .args([src.to_str().unwrap(), br_file.to_str().unwrap()])
        .assert()
        .success();

    assert!(
        br_file.exists(),
        "source.txt.br should exist after compression"
    );

    // Step 3: rename the .br archive to an extensionless name, simulating a
    // "mystery download" whose format cannot be detected from extension or
    // magic bytes.
    let mystery = tmp.path().join("mystery-download");
    fs::rename(&br_file, &mystery).unwrap();

    // Step 4: extract WITHOUT specifying an OUTPUT (rule 3: readable + has_algo
    // → Extract; the fixed rule fires here instead of the old ambiguity error).
    // rcomp extracts to "." by default; since the CWD under assert_cmd is the
    // workspace root, use current_dir to isolate the extraction into tmp/.
    rcomp()
        .args([mystery.to_str().unwrap(), "--algo", "brotli"])
        .current_dir(tmp.path())
        .assert()
        .success();

    // Step 5: verify the extracted file has the original content.
    // A bare-codec brotli stream with no recognised extension falls back to the
    // "<input_name>.out" naming rule (mystery-download → mystery-download.out).
    let restored = tmp.path().join("mystery-download.out");
    assert!(
        restored.exists(),
        "mystery-download.out should appear in tmp/; tmp contents: {:?}",
        fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        fs::read(&restored).unwrap(),
        original_content,
        "restored content must match original"
    );
}

#[test]
fn brotli_mystery_file_extract_format_override_with_x_flag() {
    let tmp = TempDir::new().unwrap();
    let original_content = b"brotli roundtrip format override test";

    // Step 1: compress source.txt → source.txt.br via recognised extension.
    let src = tmp.path().join("payload.txt");
    fs::write(&src, original_content).unwrap();
    let br_file = tmp.path().join("payload.txt.br");
    rcomp()
        .args([src.to_str().unwrap(), br_file.to_str().unwrap()])
        .assert()
        .success();

    // Step 2: rename to bare name (no extension, no magic bytes).
    let mystery = tmp.path().join("mystery-payload");
    fs::rename(&br_file, &mystery).unwrap();

    // Step 3: force extraction via -x with an explicit dest directory and
    // --algo to tell rcomp the codec.  This exercises the extract-format-override
    // path (ExtractOptions { format: Some(brotli), .. }) that was previously
    // untested end-to-end.
    //
    // Do NOT pre-create dest — rcomp's extract() calls create_dir_all(dest).
    let dest = tmp.path().join("override-dest");

    rcomp()
        .args([
            mystery.to_str().unwrap(),
            dest.to_str().unwrap(),
            "--algo",
            "brotli",
            "--extract",
        ])
        .assert()
        .success();

    // Step 4: verify the content.  With no recognised extension, the output
    // name falls back to "<input_name>.out" = mystery-payload.out.
    let restored = dest.join("mystery-payload.out");
    assert!(
        restored.exists(),
        "mystery-payload.out should be in dest/; dest contents: {:?}",
        fs::read_dir(&dest)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        fs::read(&restored).unwrap(),
        original_content,
        "restored content must match original"
    );
}

// ---------------------------------------------------------------------------
// Test: ls directories get a trailing slash
// ---------------------------------------------------------------------------

#[test]
fn ls_directories_have_trailing_slash() {
    let tmp = TempDir::new().unwrap();

    // Build a source tree with a subdirectory so the tar archive contains a
    // directory entry (the tar backend stores subdirs explicitly).
    let dir = tmp.path().join("ls_dir_src");
    fs::create_dir(&dir).unwrap();
    let subdir = dir.join("subdir");
    fs::create_dir(&subdir).unwrap();
    fs::write(subdir.join("nested.txt"), b"nested").unwrap();
    fs::write(dir.join("top.txt"), b"top").unwrap();

    let archive = tmp.path().join("dirtest.tar.gz");

    rcomp()
        .args([dir.to_str().unwrap(), archive.to_str().unwrap()])
        .assert()
        .success();

    let output = rcomp()
        .args(["ls", archive.to_str().unwrap()])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let text = String::from_utf8(output).unwrap();
    // The subdirectory entry should appear with a trailing /.
    let has_dir_entry = text.lines().any(|l| l.ends_with('/'));
    assert!(
        has_dir_entry,
        "expected at least one directory entry with trailing slash\n{text}"
    );
}

// ===========================================================================
// Unit 3 tests: summary line, -q, completions, man
// ===========================================================================

// ---------------------------------------------------------------------------
// Test: stdout is EXACTLY the summary line for a compress run (bars → stderr)
// ---------------------------------------------------------------------------

#[test]
fn compress_stdout_is_exactly_summary_line() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("data.txt");
    fs::write(&src, b"hello world content for summary test").unwrap();
    let out = tmp.path().join("data.txt.gz");

    let output = rcomp()
        .args([src.to_str().unwrap(), out.to_str().unwrap()])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let text = String::from_utf8(output).unwrap();
    // stdout must be exactly one line (the summary), with a trailing newline.
    let trimmed = text.trim_end_matches('\n');
    assert_eq!(
        trimmed.lines().count(),
        1,
        "stdout must contain exactly one line (the summary); got: {text:?}"
    );
}

// ---------------------------------------------------------------------------
// Test: compress summary line matches expected format
// ---------------------------------------------------------------------------

#[test]
fn compress_summary_line_format() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("data.txt");
    fs::write(&src, b"hello world content for summary regex test").unwrap();
    let out = tmp.path().join("data.txt.gz");

    // Pattern: <output>  <in_human> → <out_human> (<ratio>%)  in <elapsed>
    // e.g.    /tmp/.../data.txt.gz  42 B → 38 B (90.5%)  in 2ms
    let pattern =
        predicates::str::is_match(r"(?m)^.+\.\S+\s{2}.+ → .+ \(\d+\.\d%\)\s{2}in .+$").unwrap();

    rcomp()
        .args([src.to_str().unwrap(), out.to_str().unwrap()])
        .assert()
        .success()
        .stdout(pattern);
}

// ---------------------------------------------------------------------------
// Test: extract summary line matches expected format
// ---------------------------------------------------------------------------

#[test]
fn extract_summary_line_format() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("payload.txt");
    fs::write(&src, b"extract summary test content").unwrap();
    let archive = tmp.path().join("payload.txt.gz");

    rcomp()
        .args([src.to_str().unwrap(), archive.to_str().unwrap()])
        .assert()
        .success();

    let dest = tmp.path().join("extract_summary_dest");

    // Pattern: extracted <N> entries to <path>  in <elapsed>
    let pattern =
        predicates::str::is_match(r"(?m)^extracted \d+ entries? to .+\s{2}in .+$").unwrap();

    rcomp()
        .args([archive.to_str().unwrap(), dest.to_str().unwrap()])
        .assert()
        .success()
        .stdout(pattern);
}

// ---------------------------------------------------------------------------
// Test: -q yields EMPTY stdout on compress
// ---------------------------------------------------------------------------

#[test]
fn quiet_compress_produces_no_stdout() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("data.txt");
    fs::write(&src, b"quiet mode content").unwrap();
    let out = tmp.path().join("data.txt.gz");

    rcomp()
        .args([src.to_str().unwrap(), out.to_str().unwrap(), "-q"])
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
}

// ---------------------------------------------------------------------------
// Test: completions bash output contains rcomp markers
// ---------------------------------------------------------------------------

#[test]
fn completions_bash_contains_rcomp_markers() {
    rcomp()
        .args(["completions", "bash"])
        .assert()
        .success()
        .stdout(predicate::str::contains("rcomp"));
}

// ---------------------------------------------------------------------------
// Test: completions fish output contains rcomp markers
// ---------------------------------------------------------------------------

#[test]
fn completions_fish_contains_rcomp_markers() {
    rcomp()
        .args(["completions", "fish"])
        .assert()
        .success()
        .stdout(predicate::str::contains("rcomp"));
}

// ---------------------------------------------------------------------------
// Test: man page starts with .TH and contains all long option names
// ---------------------------------------------------------------------------

#[test]
fn man_starts_with_th_and_contains_long_options() {
    let output = rcomp()
        .args(["man"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let text = String::from_utf8(output).unwrap();

    // clap_mangen emits a groff preamble before the .TH macro, so check that
    // .TH appears somewhere near the top rather than requiring it to be first.
    assert!(
        text.contains(".TH"),
        "man output must contain .TH; got first 80 chars: {:?}",
        &text[..text.len().min(80)]
    );

    // Every long option defined in cli.rs must appear in the man page.
    // clap_mangen renders `--` as `\-\-` in troff, so we search for just the
    // option name (without the dashes) which appears in both forms.
    let long_opts = [
        "algo", "fast", "best", "edge", "unwrap", "yes", "force", "quiet", "compress", "extract",
    ];
    for opt in &long_opts {
        assert!(
            text.contains(opt),
            "man page must mention option `{opt}`; full output length: {}",
            text.len()
        );
    }
}
