mod common;

use assert_cmd::cargo::cargo_bin_cmd;
use common::treeward_cmd;
use filetime::{FileTime, set_file_mtime};
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

#[test]
fn verify_success_when_clean() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();

    treeward_cmd(temp.path()).arg("init").assert().success();

    treeward_cmd(temp.path())
        .arg("verify")
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
}

#[test]
fn verify_fails_on_added_file() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();

    treeward_cmd(temp.path()).arg("init").assert().success();

    fs::write(temp.path().join("new.txt"), "new").unwrap();

    treeward_cmd(temp.path())
        .arg("verify")
        .assert()
        .failure()
        .stdout(predicate::str::contains("A  new.txt"))
        .stderr(predicate::str::contains("Verification failed"));
}

#[test]
fn verify_fails_on_modified_file() {
    let temp = TempDir::new().unwrap();
    let file_path = temp.path().join("file.txt");
    fs::write(&file_path, "hello").unwrap();

    treeward_cmd(temp.path()).arg("init").assert().success();

    fs::write(&file_path, "changed").unwrap();

    treeward_cmd(temp.path())
        .arg("verify")
        .assert()
        .failure()
        .stdout(predicate::str::contains("M  file.txt"))
        .stderr(predicate::str::contains("Verification failed"));
}

/// The distinguishing behavior of `verify` is catching content changes when
/// metadata is unchanged — anything weaker than ChecksumPolicy::Always in the
/// CLI wiring would pass every metadata-changing test while silently losing
/// this guarantee. The same-size write plus mtime restore makes the file look
/// untouched to metadata-only comparison.
#[test]
fn verify_detects_content_change_with_unchanged_metadata() {
    let temp = TempDir::new().unwrap();
    let file_path = temp.path().join("file.txt");
    fs::write(&file_path, "hello").unwrap();

    treeward_cmd(temp.path()).arg("init").assert().success();

    let original_mtime =
        FileTime::from_system_time(fs::metadata(&file_path).unwrap().modified().unwrap());
    fs::write(&file_path, "olleh").unwrap();
    set_file_mtime(&file_path, original_mtime).unwrap();

    // Sanity-check the premise: metadata-only status must see a clean tree,
    // otherwise this test is not exercising the checksum path at all.
    treeward_cmd(temp.path())
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::is_empty());

    treeward_cmd(temp.path())
        .arg("verify")
        .assert()
        .failure()
        .stdout(predicate::str::contains("M  file.txt"))
        .stderr(predicate::str::contains("Verification failed"));
}

// Intentionally hand-rolled: this test exercises -C with a relative path
// resolved against the process working directory, which the shared helper
// (absolute-path -C) cannot express.
#[test]
fn verify_with_c_flag_changes_directory() {
    let temp = TempDir::new().unwrap();
    let subdir = temp.path().join("subdir");
    fs::create_dir(&subdir).unwrap();
    fs::write(subdir.join("file.txt"), "hello").unwrap();

    treeward_cmd(&subdir).arg("init").assert().success();

    cargo_bin_cmd!("treeward")
        .current_dir(temp.path())
        .arg("-C")
        .arg("subdir")
        .arg("verify")
        .assert()
        .success();
}

/// Verifies that `verify` exits with code 1 when verification fails (tree is unclean)
/// but no other errors occurred. Exit code 1 specifically means "verification failed".
#[test]
fn verify_exits_code_1_when_unclean() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();

    treeward_cmd(temp.path()).arg("init").assert().success();

    fs::write(temp.path().join("new.txt"), "added file").unwrap();

    let output = treeward_cmd(temp.path()).arg("verify").output().unwrap();

    assert_eq!(
        output.status.code(),
        Some(1),
        "verify should exit with code 1 for unclean tree"
    );
}

/// Verifies that `verify` exits with code 255 on errors (e.g., permission denied),
/// rather than a code with specific meaning (like exit code 1 for verification failure).
///
/// NOTE: Exit code 255 is NOT a contractual guarantee. Any error that currently
/// produces exit code 255 could be promoted to a dedicated exit code in the future.
/// The purpose of this test is to ensure that errors don't accidentally return
/// an exit code that has a specific meaning (e.g., 1 for verification failure).
#[test]
#[cfg(unix)]
fn verify_exits_code_255_on_permission_error() {
    use std::os::unix::fs::PermissionsExt;

    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();
    fs::create_dir(temp.path().join("subdir")).unwrap();
    fs::write(temp.path().join("subdir/nested.txt"), "nested").unwrap();

    treeward_cmd(temp.path()).arg("init").assert().success();

    // Remove read permission from subdir
    let subdir = temp.path().join("subdir");
    let mut perms = fs::metadata(&subdir).unwrap().permissions();
    perms.set_mode(0o000);
    fs::set_permissions(&subdir, perms.clone()).unwrap();

    let output = treeward_cmd(temp.path()).arg("verify").output().unwrap();

    // Restore permissions for cleanup
    perms.set_mode(0o755);
    fs::set_permissions(&subdir, perms).unwrap();

    // Exit code 255 indicates an error (not just verification failure).
    // NOTE: 255 is not a contract - this error could get a dedicated code in the future.
    assert_eq!(
        output.status.code(),
        Some(255),
        "verify should exit with code 255 on permission error"
    );
}
