use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use std::fs;
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
