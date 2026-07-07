mod common;

use assert_cmd::cargo::cargo_bin_cmd;
use common::{status_fingerprint, treeward_cmd};
use filetime::{FileTime, set_file_mtime};
use predicates::prelude::*;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use tempfile::TempDir;

#[test]
fn init_creates_ward_files() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();

    treeward_cmd(temp.path())
        .arg("init")
        .assert()
        .success()
        .stdout(predicate::str::is_empty());

    assert!(temp.path().join(".treeward").exists());
}

#[test]
fn init_respects_matching_fingerprint() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();

    let (status_output, fingerprint) = status_fingerprint(temp.path(), &[]);
    assert!(
        !status_output.status.success(),
        "status should fail so we can capture the fingerprint for initial warding"
    );

    treeward_cmd(temp.path())
        .arg("init")
        .arg("--fingerprint")
        .arg(&fingerprint)
        .assert()
        .success()
        .stdout(predicate::str::is_empty());

    assert_file_checksum(
        &temp.path().join(".treeward"),
        "file.txt",
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
    );
}

#[test]
fn init_rejects_mismatched_fingerprint() {
    let temp = TempDir::new().unwrap();
    let file_path = temp.path().join("file.txt");
    fs::write(&file_path, "hello").unwrap();

    let (_, fingerprint) = status_fingerprint(temp.path(), &[]);

    fs::write(&file_path, "updated").unwrap();

    treeward_cmd(temp.path())
        .arg("init")
        .arg("--fingerprint")
        .arg(&fingerprint)
        .assert()
        .code(255)
        .stderr(predicate::str::contains("Fingerprint mismatch"));

    assert!(!temp.path().join(".treeward").exists());
}

#[test]
fn init_verify_matches_verify_fingerprint_and_writes_checksum() {
    let temp = TempDir::new().unwrap();
    make_partially_warded_tree_with_hidden_subdir_change(&temp);

    let (_, default_fingerprint) = status_fingerprint(temp.path(), &[]);
    let (_, fingerprint) = status_fingerprint(temp.path(), &["--verify"]);
    let (_, always_fingerprint) = status_fingerprint(temp.path(), &["--always-verify"]);
    assert_ne!(
        fingerprint, default_fingerprint,
        "--verify fingerprint should differ from default policy"
    );
    assert_ne!(
        fingerprint, always_fingerprint,
        "fixture should distinguish --verify from --always-verify"
    );

    treeward_cmd(temp.path())
        .arg("init")
        .arg("--verify")
        .arg("--fingerprint")
        .arg(&fingerprint)
        .assert()
        .success()
        .stdout(predicate::str::is_empty());

    assert_file_checksum(
        &temp.path().join(".treeward"),
        "root.txt",
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
    );
}

#[test]
fn init_always_verify_matches_always_verify_fingerprint_and_writes_checksum() {
    let temp = TempDir::new().unwrap();
    make_partially_warded_tree_with_hidden_subdir_change(&temp);

    let (_, verify_fingerprint) = status_fingerprint(temp.path(), &["--verify"]);
    let (_, fingerprint) = status_fingerprint(temp.path(), &["--always-verify"]);
    assert_ne!(
        fingerprint, verify_fingerprint,
        "fixture should distinguish --always-verify from --verify"
    );

    treeward_cmd(temp.path())
        .arg("init")
        .arg("--always-verify")
        .arg("--fingerprint")
        .arg(&fingerprint)
        .assert()
        .success()
        .stdout(predicate::str::is_empty());

    assert_file_checksum(
        &temp.path().join(".treeward"),
        "root.txt",
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
    );
    assert_file_checksum(
        &temp.path().join("subdir/.treeward"),
        "file.txt",
        "0baf982fcab396fdb1c6d82f8f1eb0d2aea9cdd347fb244cf0b2c748df350069",
    );
}

/// Verifies that -v flag enables info-level output showing warded file count.
#[test]
fn init_verbose_shows_warded_count() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file1.txt"), "hello").unwrap();
    fs::write(temp.path().join("file2.txt"), "world").unwrap();

    treeward_cmd(temp.path())
        .arg("-v")
        .arg("init")
        .assert()
        .success()
        .stderr(predicate::str::contains("Warded 2 files"));
}

#[test]
fn init_dry_run_skips_writes() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();

    treeward_cmd(temp.path())
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

    treeward_cmd(temp.path()).arg("init").assert().success();

    treeward_cmd(temp.path())
        .arg("init")
        .assert()
        .failure()
        .stderr(predicate::str::contains("Already initialized"))
        .stderr(predicate::str::contains("treeward update"));
}

// Intentionally hand-rolled: this test exercises -C with a relative path
// resolved against the process working directory, which the shared helper
// (absolute-path -C) cannot express.
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

/// Ward files must get standard umask-derived permissions, not the 0600 of the
/// temp file they are staged through. Owner-only ward files break `verify` for
/// other users in group-shared trees.
///
/// Two umask values are checked so a hardcoded mode cannot pass: only actual
/// masking of 0666 produces both results. The umask is set via `sh` in the
/// child process only; the test process's own umask is never touched (it is
/// global state, raceful across threads).
#[test]
#[cfg(unix)]
fn init_creates_ward_files_with_umask_permissions() {
    for (umask, expected_mode) in [("022", 0o644), ("027", 0o640)] {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("file.txt"), "hello").unwrap();

        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!(r#"umask {} && exec "$0" -C "$1" init"#, umask))
            .arg(assert_cmd::cargo::cargo_bin!("treeward"))
            .arg(temp.path())
            .status()
            .unwrap();
        assert!(status.success());

        let mode = fs::metadata(temp.path().join(".treeward"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, expected_mode,
            "expected 0666 masked by umask {}",
            umask
        );
    }
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

    let output = treeward_cmd(temp.path()).arg("init").output().unwrap();

    // Restore permissions for cleanup
    perms.set_mode(0o755);
    fs::set_permissions(temp.path(), perms).unwrap();

    assert_eq!(
        output.status.code(),
        Some(255),
        "init should exit with code 255 on permission error"
    );
}

/// Build a tree whose three checksum policies yield three distinct
/// fingerprints, so a test can tell which policy `init` actually ran with.
///
/// `subdir` is pre-warded and its file is then rewritten with the original
/// mtime restored: the stale checksum is invisible to metadata comparison and
/// only found by actually checksumming. `root.txt` is unwarded, which keeps
/// the default policy's fingerprint distinct as well. Tests assert the
/// fingerprints differ before relying on them, so a fixture regression fails
/// loudly instead of weakening the wiring check.
fn make_partially_warded_tree_with_hidden_subdir_change(temp: &TempDir) {
    fs::write(temp.path().join("root.txt"), "hello").unwrap();

    let subdir = temp.path().join("subdir");
    fs::create_dir(&subdir).unwrap();
    let file_path = subdir.join("file.txt");
    fs::write(&file_path, "hello").unwrap();

    treeward_cmd(&subdir).arg("init").assert().success();

    let original_mtime =
        FileTime::from_system_time(fs::metadata(&file_path).unwrap().modified().unwrap());
    fs::write(&file_path, "olleh").unwrap();
    set_file_mtime(&file_path, original_mtime).unwrap();
}

fn assert_file_checksum(ward_path: &std::path::Path, entry_name: &str, expected_sha256: &str) {
    let ward_content = fs::read_to_string(ward_path).unwrap();
    let ward: toml::Value = toml::from_str(&ward_content).unwrap();
    let sha256 = ward
        .get("entries")
        .and_then(|entries| entries.get(entry_name))
        .and_then(|entry| entry.get("sha256"))
        .and_then(toml::Value::as_str)
        .expect("file entry should have a sha256");

    assert_eq!(sha256, expected_sha256);
}
