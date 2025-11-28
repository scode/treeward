use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

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
        .arg("status")
        .arg(temp.path())
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
        .arg("status")
        .arg(temp.path())
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
