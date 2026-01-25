use assert_cmd::cargo::cargo_bin_cmd;
use filetime::{FileTime, set_file_mtime};
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

#[test]
fn update_without_init_fails() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("update")
        .assert()
        .failure()
        .stderr(predicate::str::contains("Not initialized"));
}

/// Tests that update with --fingerprint succeeds when fingerprint matches.
/// Uses --verify on both status and update to ensure consistent fingerprints.
#[test]
fn update_respects_fingerprint() {
    let temp = TempDir::new().unwrap();
    let file_path = temp.path().join("file.txt");
    fs::write(&file_path, "hello").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("init")
        .assert()
        .success();

    fs::write(&file_path, "updated").unwrap();

    let status_output = cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("status")
        .arg("--verify")
        .output()
        .unwrap();
    assert!(
        !status_output.status.success(),
        "status should fail so we can capture the fingerprint for pending changes"
    );
    let output_str = String::from_utf8(status_output.stdout).unwrap();
    let fingerprint = extract_fingerprint(&output_str);

    // Must use same --verify flag as status to get matching fingerprint
    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("update")
        .arg("--verify")
        .arg("--fingerprint")
        .arg(&fingerprint)
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
}

#[test]
fn update_dry_run_skips_writes() {
    let temp = TempDir::new().unwrap();
    let file_path = temp.path().join("file.txt");
    fs::write(&file_path, "hello").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("init")
        .assert()
        .success();

    let ward_path = temp.path().join(".treeward");
    let before = fs::metadata(&ward_path).unwrap().modified().unwrap();

    fs::write(&file_path, "changed").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("update")
        .arg("--dry-run")
        .assert()
        .success()
        .stdout(predicate::str::is_empty());

    let after = fs::metadata(&ward_path).unwrap().modified().unwrap();
    assert_eq!(before, after, "dry run should not rewrite ward files");
}

#[test]
fn update_allow_init_initializes_when_missing() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("update")
        .arg("--allow-init")
        .assert()
        .success()
        .stdout(predicate::str::is_empty());

    assert!(temp.path().join(".treeward").exists());
}

#[test]
fn update_with_c_flag_changes_directory() {
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

    fs::write(subdir.join("file.txt"), "updated").unwrap();

    cargo_bin_cmd!("treeward")
        .current_dir(temp.path())
        .arg("-C")
        .arg("subdir")
        .arg("update")
        .assert()
        .success();

    assert!(subdir.join(".treeward").exists());
}

fn extract_fingerprint(output: &str) -> String {
    output
        .lines()
        .find_map(|line| line.strip_prefix("Fingerprint: "))
        .expect("fingerprint not found in output")
        .to_string()
}

#[test]
fn update_verify_matches_status_verify_fingerprint() {
    let temp = TempDir::new().unwrap();
    let file_path = temp.path().join("file.txt");
    fs::write(&file_path, "hello").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("init")
        .assert()
        .success();

    fs::write(&file_path, "modified").unwrap();

    // Get fingerprint with --verify
    let status_output = cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("status")
        .arg("--verify")
        .output()
        .unwrap();
    let output_str = String::from_utf8(status_output.stdout).unwrap();
    let fingerprint = extract_fingerprint(&output_str);

    // Update with --verify and matching fingerprint should succeed
    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("update")
        .arg("--verify")
        .arg("--fingerprint")
        .arg(&fingerprint)
        .assert()
        .success();
}

#[test]
fn update_always_verify_matches_status_always_verify_fingerprint() {
    let temp = TempDir::new().unwrap();
    let file_path = temp.path().join("file.txt");
    fs::write(&file_path, "hello").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("init")
        .assert()
        .success();

    fs::write(&file_path, "modified").unwrap();

    // Get fingerprint with --always-verify
    let status_output = cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("status")
        .arg("--always-verify")
        .output()
        .unwrap();
    let output_str = String::from_utf8(status_output.stdout).unwrap();
    let fingerprint = extract_fingerprint(&output_str);

    // Update with --always-verify and matching fingerprint should succeed
    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("update")
        .arg("--always-verify")
        .arg("--fingerprint")
        .arg(&fingerprint)
        .assert()
        .success();
}

/// Tests that default (metadata-only) fingerprints match between status and update
/// when a file's metadata changes but content stays the same.
#[test]
fn update_default_matches_status_default_fingerprint_metadata_only() {
    let temp = TempDir::new().unwrap();
    let file_path = temp.path().join("file.txt");
    fs::write(&file_path, "hello").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("init")
        .assert()
        .success();

    // Touch file to change mtime without changing content
    set_file_mtime(&file_path, FileTime::from_unix_time(1000000000, 0)).unwrap();

    // Get fingerprint with default (no --verify) - shows M?
    let status_output = cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("status")
        .output()
        .unwrap();
    let output_str = String::from_utf8(status_output.stdout).unwrap();
    assert!(
        output_str.contains("M?"),
        "default status should show M? for metadata-only change"
    );
    let fingerprint = extract_fingerprint(&output_str);

    // Update with default (no --verify) and matching fingerprint should succeed
    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("update")
        .arg("--fingerprint")
        .arg(&fingerprint)
        .assert()
        .success();
}

/// When content actually changes and both status and update use default policy
/// (no --verify), fingerprints match because both report M? (PossiblyModified).
/// The policy controls what is *reported* for fingerprint purposes, not what is
/// computed internally. Update still checksums for ward building, but reports M?
/// for fingerprint consistency.
#[test]
fn update_default_succeeds_when_fingerprints_match() {
    let temp = TempDir::new().unwrap();
    let file_path = temp.path().join("file.txt");
    fs::write(&file_path, "hello").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("init")
        .assert()
        .success();

    // Actually change the content
    fs::write(&file_path, "modified").unwrap();

    // Get fingerprint with default (no --verify) - shows M?
    let status_output = cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("status")
        .output()
        .unwrap();
    let output_str = String::from_utf8(status_output.stdout).unwrap();
    assert!(output_str.contains("M?"));
    let fingerprint = extract_fingerprint(&output_str);

    // Update with default (no --verify) also reports M?, fingerprints match
    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("update")
        .arg("--fingerprint")
        .arg(&fingerprint)
        .assert()
        .success();
}

/// Tests that mismatched verification flags cause fingerprint mismatch when
/// metadata changes but content stays the same.
#[test]
fn update_fingerprint_mismatch_shows_hint() {
    let temp = TempDir::new().unwrap();
    let file_path = temp.path().join("file.txt");
    fs::write(&file_path, "hello").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("init")
        .assert()
        .success();

    // Touch file to change mtime without changing content
    set_file_mtime(&file_path, FileTime::from_unix_time(1000000000, 0)).unwrap();

    // Get fingerprint with --verify (finds file unchanged, no M entry)
    let status_output = cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("status")
        .arg("--verify")
        .output()
        .unwrap();

    // With --verify and unchanged content, status should show clean (no changes)
    assert!(
        status_output.status.success(),
        "status --verify should succeed when only metadata changed"
    );

    // Now get fingerprint with default (no --verify) - shows M?
    let status_default = cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("status")
        .output()
        .unwrap();
    let output_str = String::from_utf8(status_default.stdout).unwrap();
    assert!(output_str.contains("M?"));
    let fingerprint = extract_fingerprint(&output_str);

    // Update WITH --verify but using fingerprint from non-verify status should fail
    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("update")
        .arg("--verify")
        .arg("--fingerprint")
        .arg(&fingerprint)
        .assert()
        .failure()
        .stderr(predicate::str::contains("Fingerprint mismatch"))
        .stderr(predicate::str::contains("--verify"))
        .stderr(predicate::str::contains("--always-verify"));
}

/// Verifies that `update --allow-init` is idempotent: running it multiple times
/// on unchanged files produces the same result and doesn't modify ward files.
#[test]
fn update_allow_init_is_idempotent() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();
    fs::create_dir(temp.path().join("subdir")).unwrap();
    fs::write(temp.path().join("subdir/nested.txt"), "world").unwrap();

    // First run: initializes ward files
    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("update")
        .arg("--allow-init")
        .assert()
        .success();

    let ward_content_1 = fs::read_to_string(temp.path().join(".treeward")).unwrap();
    let subdir_ward_content_1 = fs::read_to_string(temp.path().join("subdir/.treeward")).unwrap();

    // Second run: should succeed and produce identical ward files
    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("update")
        .arg("--allow-init")
        .assert()
        .success();

    let ward_content_2 = fs::read_to_string(temp.path().join(".treeward")).unwrap();
    let subdir_ward_content_2 = fs::read_to_string(temp.path().join("subdir/.treeward")).unwrap();

    assert_eq!(
        ward_content_1, ward_content_2,
        "ward file content should be identical after idempotent update"
    );
    assert_eq!(
        subdir_ward_content_1, subdir_ward_content_2,
        "subdir ward file content should be identical after idempotent update"
    );

    // Third run: still idempotent
    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("update")
        .arg("--allow-init")
        .assert()
        .success();

    let ward_content_3 = fs::read_to_string(temp.path().join(".treeward")).unwrap();
    assert_eq!(
        ward_content_1, ward_content_3,
        "ward file content should remain identical after third run"
    );

    // Verify status shows no changes
    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("verify")
        .assert()
        .success();
}
