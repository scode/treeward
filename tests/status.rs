mod common;

use assert_cmd::cargo::cargo_bin_cmd;
use common::{status_output, treeward_cmd};
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

#[test]
fn status_reports_no_changes_after_initial_ward() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();

    treeward_cmd(temp.path()).arg("init").assert().success();

    treeward_cmd(temp.path())
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
}

#[test]
fn status_diff_reports_no_changes_after_initial_ward() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();

    treeward_cmd(temp.path()).arg("init").assert().success();

    treeward_cmd(temp.path())
        .arg("status")
        .arg("--diff")
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
}

/// Verifies that removed files are displayed with "R" status code.
#[test]
fn status_shows_removed_files() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();
    fs::write(temp.path().join("to_remove.txt"), "will be removed").unwrap();

    treeward_cmd(temp.path()).arg("init").assert().success();

    fs::remove_file(temp.path().join("to_remove.txt")).unwrap();

    let output = status_output(temp.path(), &[]);

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("R  to_remove.txt"));
    assert!(stdout.contains("Fingerprint:"));
    assert_eq!(
        output.status.code(),
        Some(1),
        "status should exit with code 1 for unclean tree"
    );
}

#[test]
fn status_shows_added_files_and_fingerprint() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();

    treeward_cmd(temp.path()).arg("init").assert().success();

    fs::write(temp.path().join("new.txt"), "new").unwrap();

    treeward_cmd(temp.path())
        .arg("status")
        .assert()
        .failure()
        .stdout(predicate::str::contains("A  new.txt"))
        .stdout(predicate::str::contains("Fingerprint:"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn status_verify_reports_modified_files() {
    let temp = TempDir::new().unwrap();
    let file_path = temp.path().join("file.txt");
    fs::write(&file_path, "hello").unwrap();

    treeward_cmd(temp.path()).arg("init").assert().success();

    fs::write(&file_path, "changed").unwrap();

    treeward_cmd(temp.path())
        .arg("status")
        .arg("--verify")
        .assert()
        .failure()
        .stdout(predicate::str::contains("M  file.txt"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn status_default_uses_metadata_only_policy() {
    let temp = TempDir::new().unwrap();
    let file_path = temp.path().join("file.txt");
    fs::write(&file_path, "hello").unwrap();

    treeward_cmd(temp.path()).arg("init").assert().success();

    fs::write(&file_path, "changed").unwrap();

    treeward_cmd(temp.path())
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

    treeward_cmd(temp.path()).arg("init").assert().success();

    fs::write(&file_path, "changed").unwrap();

    treeward_cmd(temp.path())
        .arg("status")
        .arg("--always-verify")
        .assert()
        .failure()
        .stdout(predicate::str::contains("M  file.txt"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn status_with_c_flag_changes_directory() {
    let temp = TempDir::new().unwrap();
    let subdir = temp.path().join("subdir");
    fs::create_dir(&subdir).unwrap();
    fs::write(subdir.join("file.txt"), "hello").unwrap();

    treeward_cmd(&subdir).arg("init").assert().success();

    fs::write(subdir.join("new.txt"), "new").unwrap();

    cargo_bin_cmd!("treeward")
        .current_dir(temp.path())
        .arg("-C")
        .arg("subdir")
        .arg("status")
        .assert()
        .failure()
        .stdout(predicate::str::contains("A  new.txt"));
}

#[test]
fn c_flag_with_nonexistent_directory_fails() {
    let temp = TempDir::new().unwrap();
    let nonexistent = temp.path().join("does_not_exist");

    treeward_cmd(&nonexistent)
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

    treeward_cmd(temp.path()).arg("init").assert().success();

    fs::write(temp.path().join("new.txt"), "new").unwrap();

    treeward_cmd(temp.path())
        .arg("status")
        .arg("--all")
        .assert()
        .failure()
        .stdout(predicate::str::contains(".  file1.txt"))
        .stdout(predicate::str::contains(".  file2.txt"))
        .stdout(predicate::str::contains("A  new.txt"))
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

    treeward_cmd(temp.path()).arg("init").assert().success();

    treeward_cmd(temp.path())
        .arg("status")
        .arg("--all")
        .assert()
        .success()
        .stdout(predicate::str::contains(".  file1.txt"))
        .stdout(predicate::str::contains(".  file2.txt"))
        .stderr(predicate::str::is_empty());
}

/// Verifies that `status --diff --all` exits successfully when there are no
/// changes, even though unchanged files are shown.
#[test]
fn status_diff_all_exits_success_when_no_changes() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file1.txt"), "hello").unwrap();
    fs::write(temp.path().join("file2.txt"), "world").unwrap();

    treeward_cmd(temp.path()).arg("init").assert().success();

    treeward_cmd(temp.path())
        .arg("status")
        .arg("--diff")
        .arg("--all")
        .assert()
        .success()
        .stdout(predicate::str::contains(".  file1.txt"))
        .stdout(predicate::str::contains(".  file2.txt"))
        .stdout(predicate::str::contains("Fingerprint:").not())
        .stderr(predicate::str::is_empty());
}

/// Verifies that `status` exits with code 1 when the tree has changes (unclean)
/// but no other errors occurred. Exit code 1 specifically means "unclean tree".
#[test]
fn status_exits_code_1_when_unclean() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();

    treeward_cmd(temp.path()).arg("init").assert().success();

    fs::write(temp.path().join("new.txt"), "added file").unwrap();

    let output = status_output(temp.path(), &[]);

    assert_eq!(
        output.status.code(),
        Some(1),
        "status should exit with code 1 for unclean tree"
    );
}

#[test]
fn status_diff_shows_modified_file_details() {
    let temp = TempDir::new().unwrap();
    let file_path = temp.path().join("file.txt");
    fs::write(&file_path, "hello").unwrap();

    treeward_cmd(temp.path()).arg("init").assert().success();

    fs::write(&file_path, "changed content").unwrap();

    treeward_cmd(temp.path())
        .arg("status")
        .arg("--diff")
        .assert()
        .failure()
        .stdout(predicate::str::contains("M  file.txt"))
        .stdout(predicate::str::contains("size:"))
        // Intentionally assert mtime diff output for now. On coarse-timestamp
        // filesystems this can occasionally be flaky if writes land in one tick,
        // but we keep this strict unless it becomes a real issue in CI.
        .stdout(predicate::str::contains("mtime:"))
        .stdout(predicate::str::contains("sha256:"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn status_diff_shows_removed_file_details() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();
    fs::write(temp.path().join("to_remove.txt"), "will be removed").unwrap();

    treeward_cmd(temp.path()).arg("init").assert().success();

    fs::remove_file(temp.path().join("to_remove.txt")).unwrap();

    treeward_cmd(temp.path())
        .arg("status")
        .arg("--diff")
        .assert()
        .failure()
        .stdout(predicate::str::contains("R  to_remove.txt"))
        .stdout(predicate::str::contains("was: file ("))
        .stdout(predicate::str::contains("sha256:"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn status_diff_no_details_for_added_files() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();

    treeward_cmd(temp.path()).arg("init").assert().success();

    fs::write(temp.path().join("new.txt"), "new").unwrap();

    let output = status_output(temp.path(), &["--diff"]);

    assert_eq!(
        output.status.code(),
        Some(1),
        "status --diff should exit with code 1 for unclean tree"
    );
    assert!(
        output.stderr.is_empty(),
        "status --diff should not write to stderr on unclean tree"
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("A  new.txt"));
    assert!(stdout.contains("Fingerprint:"));
    // Added files should not have any indented diff details
    let lines: Vec<&str> = stdout.lines().collect();
    let added_idx = lines.iter().position(|l| l.contains("A  new.txt")).unwrap();
    // The next line should either be empty, another status line, or Fingerprint
    if added_idx + 1 < lines.len() {
        let next_line = lines[added_idx + 1];
        assert!(
            !next_line.starts_with("   "),
            "Added file should not have diff details, but found: {}",
            next_line
        );
    }
}

#[test]
#[cfg(unix)]
fn status_diff_shows_symlink_target_change() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("target1.txt"), "target1").unwrap();
    fs::write(temp.path().join("target2.txt"), "target2").unwrap();
    symlink("target1.txt", temp.path().join("link")).unwrap();

    treeward_cmd(temp.path()).arg("init").assert().success();

    fs::remove_file(temp.path().join("link")).unwrap();
    symlink("target2.txt", temp.path().join("link")).unwrap();

    treeward_cmd(temp.path())
        .arg("status")
        .arg("--diff")
        .assert()
        .failure()
        .stdout(predicate::str::contains("M  link"))
        .stdout(predicate::str::contains(
            "target: target1.txt -> target2.txt",
        ))
        .stderr(predicate::str::is_empty());
}

#[test]
#[cfg(unix)]
fn status_diff_shows_type_change() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("entry"), "file content").unwrap();

    treeward_cmd(temp.path()).arg("init").assert().success();

    fs::remove_file(temp.path().join("entry")).unwrap();
    symlink("somewhere", temp.path().join("entry")).unwrap();

    treeward_cmd(temp.path())
        .arg("status")
        .arg("--diff")
        .assert()
        .failure()
        .stdout(predicate::str::contains("M  entry"))
        .stdout(predicate::str::contains("was: file ("))
        .stdout(predicate::str::contains("now: symlink -> somewhere"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn status_diff_implies_verify() {
    let temp = TempDir::new().unwrap();
    let file_path = temp.path().join("file.txt");
    fs::write(&file_path, "hello").unwrap();

    treeward_cmd(temp.path()).arg("init").assert().success();

    fs::write(&file_path, "changed").unwrap();

    // --diff alone should show "M" not "M?" because it implies --verify
    treeward_cmd(temp.path())
        .arg("status")
        .arg("--diff")
        .assert()
        .failure()
        .stdout(predicate::str::contains("M  file.txt"))
        .stdout(predicate::str::is_match(r"M\?").unwrap().not())
        .stderr(predicate::str::is_empty());
}

#[test]
fn status_diff_all_no_details_for_unchanged() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("unchanged.txt"), "hello").unwrap();
    fs::write(temp.path().join("to_modify.txt"), "original").unwrap();

    treeward_cmd(temp.path()).arg("init").assert().success();

    fs::write(temp.path().join("to_modify.txt"), "modified").unwrap();

    let output = status_output(temp.path(), &["--diff", "--all"]);

    assert_eq!(
        output.status.code(),
        Some(1),
        "status --diff --all should exit with code 1 for unclean tree"
    );
    assert!(
        output.stderr.is_empty(),
        "status --diff --all should not write to stderr on unclean tree"
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains(".  unchanged.txt"));
    assert!(stdout.contains("M  to_modify.txt"));
    assert!(stdout.contains("Fingerprint:"));

    // Unchanged files should not have diff details
    let lines: Vec<&str> = stdout.lines().collect();
    let unchanged_idx = lines
        .iter()
        .position(|l| l.contains(".  unchanged.txt"))
        .unwrap();
    if unchanged_idx + 1 < lines.len() {
        let next_line = lines[unchanged_idx + 1];
        // Next line should be another status entry or empty/Fingerprint, not indented diff
        assert!(
            !next_line.starts_with("   ")
                || next_line.contains("size:") && !lines[unchanged_idx].contains("unchanged"),
            "Unchanged file should not have diff details"
        );
    }
}

#[test]
fn status_diff_works_with_always_verify() {
    let temp = TempDir::new().unwrap();
    let file_path = temp.path().join("file.txt");
    fs::write(&file_path, "hello").unwrap();

    treeward_cmd(temp.path()).arg("init").assert().success();

    fs::write(&file_path, "changed content").unwrap();

    treeward_cmd(temp.path())
        .arg("status")
        .arg("--diff")
        .arg("--always-verify")
        .assert()
        .failure()
        .stdout(predicate::str::contains("M  file.txt"))
        .stdout(predicate::str::contains("size:"))
        .stdout(predicate::str::contains("sha256:"))
        .stderr(predicate::str::is_empty());
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

    treeward_cmd(temp.path()).arg("init").assert().success();

    // Remove read permission from subdir
    let subdir = temp.path().join("subdir");
    let mut perms = fs::metadata(&subdir).unwrap().permissions();
    perms.set_mode(0o000);
    fs::set_permissions(&subdir, perms.clone()).unwrap();

    let output = status_output(temp.path(), &[]);

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
