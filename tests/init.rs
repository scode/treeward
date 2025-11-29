use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

#[test]
fn init_creates_ward_files() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("init")
        .arg(temp.path())
        .assert()
        .success()
        .stdout(predicate::str::is_empty());

    assert!(temp.path().join(".treeward").exists());
}

#[test]
fn init_dry_run_skips_writes() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("init")
        .arg(temp.path())
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
        .arg("init")
        .arg(temp.path())
        .assert()
        .success();

    cargo_bin_cmd!("treeward")
        .arg("init")
        .arg(temp.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("Already initialized"))
        .stderr(predicate::str::contains("treeward update"));
}
