//! End-to-end tests for CLI/environment acceleration selection.

use std::fs;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn rcomp() -> Command {
    Command::cargo_bin("rcomp").expect("rcomp binary must exist")
}

fn paths(tmp: &TempDir, suffix: &str) -> (String, String) {
    let input = tmp.path().join("input.txt");
    let output = tmp.path().join(format!("output.{suffix}"));
    fs::write(&input, b"hardware acceleration selection test").unwrap();
    (
        input.to_string_lossy().into_owned(),
        output.to_string_lossy().into_owned(),
    )
}

#[test]
fn auto_from_environment_falls_back_with_warning() {
    let tmp = TempDir::new().unwrap();
    let (input, output) = paths(&tmp, "zst");

    rcomp()
        .env("RCOMP_ACCELERATOR", "auto")
        .args([&input, &output])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "warning: no compatible hardware accelerator",
        ));

    assert!(std::path::Path::new(&output).is_file());
}

#[test]
fn required_flag_fails_before_creating_output() {
    let tmp = TempDir::new().unwrap();
    let (input, output) = paths(&tmp, "gz");

    rcomp()
        .args([&input, &output, "--accelerator", "required"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "hardware acceleration unavailable",
        ));

    assert!(!std::path::Path::new(&output).exists());
}

#[test]
fn command_line_cpu_overrides_required_environment() {
    let tmp = TempDir::new().unwrap();
    let (input, output) = paths(&tmp, "lz4");

    rcomp()
        .env("RCOMP_ACCELERATOR", "required")
        .args([&input, &output, "--accelerator", "cpu"])
        .assert()
        .success()
        .stderr(predicate::str::contains("hardware acceleration").not());

    assert!(std::path::Path::new(&output).is_file());
}

#[test]
fn invalid_environment_value_is_a_usage_error() {
    let tmp = TempDir::new().unwrap();
    let (input, output) = paths(&tmp, "zst");

    rcomp()
        .env("RCOMP_ACCELERATOR", "maybe")
        .args([&input, &output])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("invalid accelerator mode"));
}

#[cfg(all(target_os = "macos", feature = "metal"))]
#[test]
#[ignore = "requires a Metal-capable macOS runner"]
fn required_metal_lz4_fast_round_trips_through_the_cli() {
    let tmp = TempDir::new().unwrap();
    let input = tmp.path().join("input.bin");
    let output = tmp.path().join("output.lz4");
    let destination = tmp.path().join("decoded");
    let contents = b"metal provider end-to-end ".repeat(8_192);
    fs::write(&input, &contents).unwrap();

    rcomp()
        .args([
            input.to_str().unwrap(),
            output.to_str().unwrap(),
            "--fast",
            "--accelerator",
            "required",
        ])
        .assert()
        .success();

    rcomp()
        .args([
            output.to_str().unwrap(),
            destination.to_str().unwrap(),
            "--extract",
            "--accelerator",
            "cpu",
        ])
        .assert()
        .success();

    assert_eq!(fs::read(destination.join("output")).unwrap(), contents);
}

#[cfg(all(target_os = "macos", feature = "metal"))]
#[test]
#[ignore = "requires a Metal-capable macOS runner"]
fn required_metal_tar_lz4_round_trips_a_directory() {
    let tmp = TempDir::new().unwrap();
    let input = tmp.path().join("input");
    let output = tmp.path().join("output.tar.lz4");
    let destination = tmp.path().join("decoded");
    fs::create_dir(&input).unwrap();
    fs::write(input.join("first.txt"), b"first Metal tar entry").unwrap();
    fs::write(input.join("second.txt"), b"second Metal tar entry").unwrap();

    rcomp()
        .args([
            input.to_str().unwrap(),
            output.to_str().unwrap(),
            "--fast",
            "--accelerator",
            "required",
        ])
        .assert()
        .success();

    rcomp()
        .args([
            output.to_str().unwrap(),
            destination.to_str().unwrap(),
            "--extract",
            "--unwrap",
            "--accelerator",
            "cpu",
        ])
        .assert()
        .success();

    assert_eq!(
        fs::read(destination.join("first.txt")).unwrap(),
        b"first Metal tar entry"
    );
    assert_eq!(
        fs::read(destination.join("second.txt")).unwrap(),
        b"second Metal tar entry"
    );
}
