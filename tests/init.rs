use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use tempfile::TempDir;

#[test]
fn init_creates_ward_files() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("init")
        .assert()
        .success()
        .stdout(predicate::str::is_empty());

    assert!(temp.path().join(".treeward").exists());
}

/// Verifies that -v flag enables info-level output showing warded file count.
#[test]
fn init_verbose_shows_warded_count() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file1.txt"), "hello").unwrap();
    fs::write(temp.path().join("file2.txt"), "world").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("-v")
        .arg("-C")
        .arg(temp.path())
        .arg("init")
        .assert()
        .success()
        .stderr(predicate::str::contains("Warded 2 files"));
}

#[test]
fn init_dry_run_skips_writes() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("init")
        .arg("--dry-run")
        .assert()
        .success()
        .stdout(predicate::str::is_empty());

    assert!(!temp.path().join(".treeward").exists());
}

#[test]
fn init_fails_when_already_initialized() {
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
        .arg("init")
        .assert()
        .failure()
        .stderr(predicate::str::contains("Already initialized"))
        .stderr(predicate::str::contains("treeward update"));
}

#[test]
fn init_with_c_flag_changes_directory() {
    let temp = TempDir::new().unwrap();
    let subdir = temp.path().join("subdir");
    fs::create_dir(&subdir).unwrap();
    fs::write(subdir.join("file.txt"), "hello").unwrap();

    cargo_bin_cmd!("treeward")
        .current_dir(temp.path())
        .arg("-C")
        .arg("subdir")
        .arg("init")
        .assert()
        .success();

    assert!(subdir.join(".treeward").exists());
    assert!(!temp.path().join(".treeward").exists());
}

/// Verifies that `init` exits with code 255 on errors (e.g., permission denied),
/// rather than silently succeeding or returning a misleading exit code.
#[test]
#[cfg(unix)]
fn init_exits_code_255_on_permission_error() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();

    // Remove write permission from the directory so .treeward can't be created
    let mut perms = fs::metadata(temp.path()).unwrap().permissions();
    perms.set_mode(0o555);
    fs::set_permissions(temp.path(), perms.clone()).unwrap();

    let output = cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("init")
        .output()
        .unwrap();

    // Restore permissions for cleanup
    perms.set_mode(0o755);
    fs::set_permissions(temp.path(), perms).unwrap();

    assert_eq!(
        output.status.code(),
        Some(255),
        "init should exit with code 255 on permission error"
    );
}
