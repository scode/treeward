mod common;

use assert_cmd::cargo::cargo_bin_cmd;
use common::treeward_cmd;
use predicates::prelude::*;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use tempfile::TempDir;

fn temp_dir_with_file() -> TempDir {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();
    temp
}

#[test]
fn init_without_flags_respects_rust_log_info() {
    let temp = temp_dir_with_file();

    treeward_cmd(temp.path())
        .env("RUST_LOG", "info")
        .arg("init")
        .assert()
        .success()
        .stderr(predicate::str::contains("Warded 1 files"));
}

#[test]
fn init_without_flags_respects_rust_log_warn() {
    let temp = temp_dir_with_file();

    treeward_cmd(temp.path())
        .env("RUST_LOG", "warn")
        .arg("init")
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
}

#[test]
fn verbose_overrides_rust_log_warn() {
    let temp = temp_dir_with_file();

    treeward_cmd(temp.path())
        .env("RUST_LOG", "warn")
        .arg("-v")
        .arg("init")
        .assert()
        .success()
        .stderr(predicate::str::contains("Warded 1 files"));
}

#[test]
fn verbose_debug_overrides_rust_log_warn() {
    let temp = temp_dir_with_file();

    treeward_cmd(temp.path())
        .env("RUST_LOG", "warn")
        .arg("-vv")
        .arg("init")
        .assert()
        .success()
        .stderr(predicate::str::contains("Checksum of"));
}

#[test]
fn log_level_overrides_rust_log_warn() {
    let temp = temp_dir_with_file();

    treeward_cmd(temp.path())
        .env("RUST_LOG", "warn")
        .arg("--log-level")
        .arg("info")
        .arg("init")
        .assert()
        .success()
        .stderr(predicate::str::contains("Warded 1 files"));
}

#[test]
fn trace_log_level_emits_debug_messages() {
    let temp = temp_dir_with_file();

    treeward_cmd(temp.path())
        .env("RUST_LOG", "warn")
        .arg("--log-level")
        .arg("trace")
        .arg("init")
        .assert()
        .success()
        .stderr(predicate::str::contains("Checksum of"));
}

#[test]
fn log_level_conflicts_with_verbose() {
    cargo_bin_cmd!("treeward")
        .arg("--log-level")
        .arg("info")
        .arg("-v")
        .arg("status")
        .assert()
        .failure()
        .stderr(predicate::str::contains("--log-level <LEVEL>"))
        .stderr(predicate::str::contains("--verbose"));
}

#[test]
fn help_mentions_rust_log_precedence_for_logging_flags() {
    cargo_bin_cmd!("treeward")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("-v, --verbose"))
        .stdout(predicate::str::contains("--log-level <LEVEL>"))
        .stdout(predicate::str::contains("Takes precedence over RUST_LOG."));
}

#[cfg(unix)]
#[test]
fn status_permission_error_logs_to_stderr_not_stdout() {
    let temp = TempDir::new().unwrap();
    let protected = temp.path().join("protected");
    fs::create_dir(&protected).unwrap();

    // Remove all permissions so entering the directory fails.
    fs::set_permissions(&protected, fs::Permissions::from_mode(0o000)).unwrap();

    let assert = treeward_cmd(temp.path()).arg("status").assert();

    assert
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("Permission denied"));
}

#[cfg(unix)]
#[test]
fn warn_error_emojis_suppressed_when_not_tty() {
    let temp = TempDir::new().unwrap();
    let protected = temp.path().join("protected");
    fs::create_dir(&protected).unwrap();
    fs::set_permissions(&protected, fs::Permissions::from_mode(0o000)).unwrap();

    // capture() makes stdout/stderr non-tty
    let output = treeward_cmd(temp.path())
        .arg("status")
        .assert()
        .failure()
        .get_output()
        .clone();

    fs::set_permissions(&protected, fs::Permissions::from_mode(0o755)).unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Should not include emoji prefixes when not a TTY
    for ch in stderr.chars() {
        assert!(
            ch.is_ascii(),
            "stderr unexpectedly contains non-ASCII character: {ch:?}"
        );
    }
    assert!(
        stderr.contains("ERROR:"),
        "stderr should include the error prefix"
    );
    assert!(
        stderr.contains("Permission denied"),
        "stderr should include the error message"
    );
}

/// Hostile filenames must not be able to inject terminal escapes through stderr.
///
/// This pins the user-observable guarantee end to end: no raw control bytes
/// reach the terminal. It deliberately does not pin the exact escaped
/// rendering of the name — controls embedded in displayed paths may already
/// be pre-escaped by the standard library (`Path::display()`) in a different
/// style than the formatter's own `\u{..}` escapes, and either is fine as
/// long as nothing raw gets through. The formatter unit test in src/main.rs
/// pins the exact escaping applied at our own boundary.
#[cfg(unix)]
#[test]
fn status_escapes_control_characters_from_unsupported_file_type_errors() {
    use nix::sys::stat;
    use nix::unistd;

    let temp = temp_dir_with_file();
    treeward_cmd(temp.path()).arg("init").assert().success();

    // C0 (ESC, BEL) plus a C1 control (U+009B, the single-byte CSI) so the
    // spec's "including C1 controls" claim has end-to-end coverage.
    let hostile_name = "fifo-\x1b]0;pwned\x07\u{9b}31m";
    unistd::mkfifo(&temp.path().join(hostile_name), stat::Mode::S_IRWXU).unwrap();

    let output = treeward_cmd(temp.path())
        .arg("status")
        .assert()
        .failure()
        .get_output()
        .clone();
    // Strict decoding matters: a raw lone C1 byte (e.g. 0x9B) is invalid
    // UTF-8, and a lossy decode would launder it into U+FFFD, which
    // `is_control()` does not catch.
    let stderr = std::str::from_utf8(&output.stderr).expect("stderr must be valid UTF-8");

    assert!(
        stderr.contains("fifo-"),
        "stderr did not name the offending file: {stderr:?}"
    );
    assert!(
        !stderr.chars().any(|c| c.is_control() && c != '\n'),
        "stderr contained raw control characters: {stderr:?}"
    );
}
