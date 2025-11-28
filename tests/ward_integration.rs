use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

#[test]
fn ward_init_creates_ward_files() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("ward")
        .arg(temp.path())
        .arg("--init")
        .assert()
        .success()
        .stdout(predicate::str::contains("Warded 1 files"));

    assert!(temp.path().join(".treeward").exists());
}

#[test]
fn ward_without_init_fails() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("ward")
        .arg(temp.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("Not initialized"));
}

#[test]
fn ward_dry_run_skips_writes() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("ward")
        .arg(temp.path())
        .arg("--init")
        .arg("--dry-run")
        .assert()
        .success()
        .stdout(predicate::str::contains("DRY RUN"))
        .stdout(predicate::str::contains("Warded 1 files"));

    assert!(!temp.path().join(".treeward").exists());
}

#[test]
fn ward_respects_fingerprint() {
    let temp = TempDir::new().unwrap();
    let file_path = temp.path().join("file.txt");
    fs::write(&file_path, "hello").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("ward")
        .arg(temp.path())
        .arg("--init")
        .assert()
        .success();

    fs::write(&file_path, "updated").unwrap();

    let status_output = cargo_bin_cmd!("treeward")
        .arg("status")
        .arg(temp.path())
        .output()
        .unwrap();
    assert!(status_output.status.success());
    let output_str = String::from_utf8(status_output.stdout).unwrap();
    let fingerprint = extract_fingerprint(&output_str);

    cargo_bin_cmd!("treeward")
        .arg("ward")
        .arg(temp.path())
        .arg("--fingerprint")
        .arg(&fingerprint)
        .assert()
        .success()
        .stdout(predicate::str::contains("Warded 1 files"));
}

fn extract_fingerprint(output: &str) -> String {
    let command_line = output
        .lines()
        .find(|line| line.contains("--fingerprint"))
        .expect("fingerprint not found in output");

    let quoted_command = command_line
        .split('\'')
        .nth(1)
        .expect("expected single-quoted command");

    quoted_command
        .split_whitespace()
        .last()
        .expect("missing fingerprint argument")
        .to_string()
}
