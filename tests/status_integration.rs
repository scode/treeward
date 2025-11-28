use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

#[test]
fn status_reports_no_changes_after_initial_ward() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("init")
        .arg(temp.path())
        .assert()
        .success();

    cargo_bin_cmd!("treeward")
        .arg("status")
        .arg(temp.path())
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
}

#[test]
fn status_shows_added_files_and_fingerprint() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("init")
        .arg(temp.path())
        .assert()
        .success();

    fs::write(temp.path().join("new.txt"), "new").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("status")
        .arg(temp.path())
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
        .arg("init")
        .arg(temp.path())
        .assert()
        .success();

    fs::write(&file_path, "changed").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("status")
        .arg(temp.path())
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
        .arg("init")
        .arg(temp.path())
        .assert()
        .success();

    fs::write(&file_path, "changed").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("status")
        .arg(temp.path())
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
        .arg("init")
        .arg(temp.path())
        .assert()
        .success();

    fs::write(&file_path, "changed").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("status")
        .arg(temp.path())
        .arg("--always-verify")
        .assert()
        .failure()
        .stdout(predicate::str::contains("M file.txt"))
        .stderr(predicate::str::is_empty());
}
