use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

#[test]
fn verify_success_when_clean() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("init")
        .arg(temp.path())
        .assert()
        .success();

    cargo_bin_cmd!("treeward")
        .arg("verify")
        .arg(temp.path())
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
}

#[test]
fn verify_fails_on_added_file() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("init")
        .arg(temp.path())
        .assert()
        .success();

    fs::write(temp.path().join("new.txt"), "new").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("verify")
        .arg(temp.path())
        .assert()
        .failure()
        .stdout(predicate::str::contains("A new.txt"))
        .stderr(predicate::str::contains("Verification failed"));
}

#[test]
fn verify_fails_on_modified_file() {
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
        .arg("verify")
        .arg(temp.path())
        .assert()
        .failure()
        .stdout(predicate::str::contains("M file.txt"))
        .stderr(predicate::str::contains("Verification failed"));
}
