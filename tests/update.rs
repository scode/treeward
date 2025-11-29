use assert_cmd::cargo::cargo_bin_cmd;
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

    cargo_bin_cmd!("treeward")
        .arg("-C")
        .arg(temp.path())
        .arg("update")
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
