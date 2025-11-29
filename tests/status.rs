use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

#[test]
fn status_reports_no_changes_after_initial_ward() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("init")
        .assert()
        .success();

    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
}

#[test]
fn status_shows_added_files_and_fingerprint() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("init")
        .assert()
        .success();

    fs::write(temp.path().join("new.txt"), "new").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("status")
        .assert()
        .failure()
        .stdout(predicate::str::contains("A new.txt"))
        .stdout(predicate::str::contains("Fingerprint:"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn status_verify_reports_modified_files() {
    let temp = TempDir::new().unwrap();
    let file_path = temp.path().join("file.txt");
    fs::write(&file_path, "hello").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("init")
        .assert()
        .success();

    fs::write(&file_path, "changed").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("status")
        .arg("--verify")
        .assert()
        .failure()
        .stdout(predicate::str::contains("M file.txt"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn status_default_uses_metadata_only_policy() {
    let temp = TempDir::new().unwrap();
    let file_path = temp.path().join("file.txt");
    fs::write(&file_path, "hello").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("init")
        .assert()
        .success();

    fs::write(&file_path, "changed").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("status")
        .assert()
        .failure()
        .stdout(predicate::str::contains("M? file.txt"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn status_always_verify_reports_modified_files() {
    let temp = TempDir::new().unwrap();
    let file_path = temp.path().join("file.txt");
    fs::write(&file_path, "hello").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("init")
        .assert()
        .success();

    fs::write(&file_path, "changed").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("status")
        .arg("--always-verify")
        .assert()
        .failure()
        .stdout(predicate::str::contains("M file.txt"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn status_with_c_flag_changes_directory() {
    let temp = TempDir::new().unwrap();
    let subdir = temp.path().join("subdir");
    fs::create_dir(&subdir).unwrap();
    fs::write(subdir.join("file.txt"), "hello").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(&subdir)
        .arg("init")
        .assert()
        .success();

    fs::write(subdir.join("new.txt"), "new").unwrap();

    cargo_bin_cmd!("treeward")
        .current_dir(temp.path())
        .arg("-C")
        .arg("subdir")
        .arg("status")
        .assert()
        .failure()
        .stdout(predicate::str::contains("A new.txt"));
}

#[test]
fn c_flag_with_nonexistent_directory_fails() {
    let temp = TempDir::new().unwrap();
    let nonexistent = temp.path().join("does_not_exist");

    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(&nonexistent)
        .arg("status")
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("Failed to change directory"))
        .stderr(predicate::str::contains("does_not_exist"));
}

#[test]
fn status_all_shows_unchanged_files() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file1.txt"), "hello").unwrap();
    fs::write(temp.path().join("file2.txt"), "world").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("init")
        .assert()
        .success();

    fs::write(temp.path().join("new.txt"), "new").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("status")
        .arg("--all")
        .assert()
        .failure()
        .stdout(predicate::str::contains(". file1.txt"))
        .stdout(predicate::str::contains(". file2.txt"))
        .stdout(predicate::str::contains("A new.txt"))
        .stdout(predicate::str::contains("Fingerprint:"))
        .stderr(predicate::str::is_empty());
}

/// Verifies that `status --all` exits with success (code 0) when there are no
/// actual changes, even though it displays unchanged files on stdout. The presence
/// of unchanged files in the output should not cause a non-zero exit code.
#[test]
fn status_all_exits_success_when_no_changes() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file1.txt"), "hello").unwrap();
    fs::write(temp.path().join("file2.txt"), "world").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("init")
        .assert()
        .success();

    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("status")
        .arg("--all")
        .assert()
        .success()
        .stdout(predicate::str::contains(". file1.txt"))
        .stdout(predicate::str::contains(". file2.txt"))
        .stderr(predicate::str::is_empty());
}

/// Verifies that `status` exits with code 1 when the tree has changes (unclean)
/// but no other errors occurred. Exit code 1 specifically means "unclean tree".
#[test]
fn status_exits_code_1_when_unclean() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("init")
        .assert()
        .success();

    fs::write(temp.path().join("new.txt"), "added file").unwrap();

    let output = cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("status")
        .output()
        .unwrap();

    assert_eq!(
        output.status.code(),
        Some(1),
        "status should exit with code 1 for unclean tree"
    );
}

/// Verifies that `status` exits with code 255 on errors (e.g., permission denied),
/// rather than a code with specific meaning (like exit code 1 for unclean tree).
///
/// NOTE: Exit code 255 is NOT a contractual guarantee. Any error that currently
/// produces exit code 255 could be promoted to a dedicated exit code in the future.
/// The purpose of this test is to ensure that errors don't accidentally return
/// an exit code that has a specific meaning (e.g., 1 for unclean), not to ensure
/// specific errors will not be promoted to dedicated errors in the future.
#[test]
#[cfg(unix)]
fn status_exits_code_255_on_permission_error() {
    use std::os::unix::fs::PermissionsExt;

    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();
    fs::create_dir(temp.path().join("subdir")).unwrap();
    fs::write(temp.path().join("subdir/nested.txt"), "nested").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("init")
        .assert()
        .success();

    // Remove read permission from subdir
    let subdir = temp.path().join("subdir");
    let mut perms = fs::metadata(&subdir).unwrap().permissions();
    perms.set_mode(0o000);
    fs::set_permissions(&subdir, perms.clone()).unwrap();

    let output = cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("status")
        .output()
        .unwrap();

    // Restore permissions for cleanup
    perms.set_mode(0o755);
    fs::set_permissions(&subdir, perms).unwrap();

    // Exit code 255 indicates an error (not just unclean tree).
    // NOTE: 255 is not a contract - this error could get a dedicated code in the future.
    assert_eq!(
        output.status.code(),
        Some(255),
        "status should exit with code 255 on permission error"
    );
}
