use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

#[test]
fn update_without_init_fails() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("file.txt"), "hello").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("update")
        .arg(temp.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("Not initialized"));
}

#[test]
fn update_respects_fingerprint() {
    let temp = TempDir::new().unwrap();
    let file_path = temp.path().join("file.txt");
    fs::write(&file_path, "hello").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("init")
        .arg(temp.path())
        .assert()
        .success();

    fs::write(&file_path, "updated").unwrap();

    let status_output = cargo_bin_cmd!("treeward")
        .arg("status")
        .arg("--verify")
        .arg(temp.path())
        .output()
        .unwrap();
    assert!(
        !status_output.status.success(),
        "status should fail so we can capture the fingerprint for pending changes"
    );
    let output_str = String::from_utf8(status_output.stdout).unwrap();
    let fingerprint = extract_fingerprint(&output_str);

    cargo_bin_cmd!("treeward")
        .arg("update")
        .arg(temp.path())
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
        .arg("init")
        .arg(temp.path())
        .assert()
        .success();

    let ward_path = temp.path().join(".treeward");
    let before = fs::metadata(&ward_path).unwrap().modified().unwrap();

    fs::write(&file_path, "changed").unwrap();

    cargo_bin_cmd!("treeward")
        .arg("update")
        .arg(temp.path())
        .arg("--dry-run")
        .assert()
        .success()
        .stdout(predicate::str::is_empty());

    let after = fs::metadata(&ward_path).unwrap().modified().unwrap();
    assert_eq!(before, after, "dry run should not rewrite ward files");
}

fn extract_fingerprint(output: &str) -> String {
    output
        .lines()
        .find_map(|line| line.strip_prefix("Fingerprint: "))
        .expect("fingerprint not found in output")
        .to_string()
}
