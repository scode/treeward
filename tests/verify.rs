use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

#[test]
fn verify_success_when_clean() {
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
        .arg("verify")
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
}

#[test]
fn verify_fails_on_added_file() {
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
        .arg("verify")
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
        .arg("-C")
        .arg(temp.path())
        .arg("init")
        .assert()
        .success();

    fs::write(&file_path, "changed").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("verify")
        .assert()
        .failure()
        .stdout(predicate::str::contains("M file.txt"))
        .stderr(predicate::str::contains("Verification failed"));
}

#[test]
fn verify_with_c_flag_changes_directory() {
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

    cargo_bin_cmd!("treeward")
        .current_dir(temp.path())
        .arg("-C")
        .arg("subdir")
        .arg("verify")
        .assert()
        .success();
}
