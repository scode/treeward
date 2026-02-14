use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

fn temp_dir_with_file() -> TempDir {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();
    temp
}

#[test]
fn init_without_flags_respects_rust_log_info() {
    let temp = temp_dir_with_file();

    cargo_bin_cmd!("treeward")
        .env("RUST_LOG", "info")
        .arg("-C")
        .arg(temp.path())
        .arg("init")
        .assert()
        .success()
        .stderr(predicate::str::contains("Warded 1 files"));
}

#[test]
fn init_without_flags_respects_rust_log_warn() {
    let temp = temp_dir_with_file();

    cargo_bin_cmd!("treeward")
        .env("RUST_LOG", "warn")
        .arg("-C")
        .arg(temp.path())
        .arg("init")
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
}

#[test]
fn verbose_overrides_rust_log_warn() {
    let temp = temp_dir_with_file();

    cargo_bin_cmd!("treeward")
        .env("RUST_LOG", "warn")
        .arg("-v")
        .arg("-C")
        .arg(temp.path())
        .arg("init")
        .assert()
        .success()
        .stderr(predicate::str::contains("Warded 1 files"));
}

#[test]
fn verbose_debug_overrides_rust_log_warn() {
    let temp = temp_dir_with_file();

    cargo_bin_cmd!("treeward")
        .env("RUST_LOG", "warn")
        .arg("-vv")
        .arg("-C")
        .arg(temp.path())
        .arg("init")
        .assert()
        .success()
        .stderr(predicate::str::contains("Checksum of"));
}

#[test]
fn log_level_overrides_rust_log_warn() {
    let temp = temp_dir_with_file();

    cargo_bin_cmd!("treeward")
        .env("RUST_LOG", "warn")
        .arg("--log-level")
        .arg("info")
        .arg("-C")
        .arg(temp.path())
        .arg("init")
        .assert()
        .success()
        .stderr(predicate::str::contains("Warded 1 files"));
}

#[test]
fn trace_log_level_emits_debug_messages() {
    let temp = temp_dir_with_file();

    cargo_bin_cmd!("treeward")
        .env("RUST_LOG", "warn")
        .arg("--log-level")
        .arg("trace")
        .arg("-C")
        .arg(temp.path())
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
    use std::os::unix::fs::PermissionsExt;

    let temp = TempDir::new().unwrap();
    let protected = temp.path().join("protected");
    fs::create_dir(&protected).unwrap();

    // Remove all permissions so entering the directory fails.
    fs::set_permissions(&protected, fs::Permissions::from_mode(0o000)).unwrap();

    let assert = cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("status")
        .assert();

    assert
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("Permission denied"));
}

#[cfg(unix)]
#[test]
fn warn_error_emojis_suppressed_when_not_tty() {
    use std::os::unix::fs::PermissionsExt;

    let temp = TempDir::new().unwrap();
    let protected = temp.path().join("protected");
    fs::create_dir(&protected).unwrap();
    fs::set_permissions(&protected, fs::Permissions::from_mode(0o000)).unwrap();

    // capture() makes stdout/stderr non-tty
    let output = cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
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
