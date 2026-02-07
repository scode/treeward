use assert_cmd::{Command, cargo::cargo_bin_cmd};
use std::path::Path;
use std::process::Output;

pub fn treeward_cmd(cwd: &Path) -> Command {
    let mut cmd = cargo_bin_cmd!("treeward");
    cmd.arg("-C").arg(cwd);
    cmd
}

pub fn status_output(cwd: &Path, args: &[&str]) -> Output {
    let mut cmd = treeward_cmd(cwd);
    cmd.arg("status").args(args);
    cmd.output().expect("failed to run `treeward status`")
}

// Each integration test file is compiled as its own crate. Some crates only use
// `treeward_cmd` and `status_output`, so this helper is intentionally unused there.
#[allow(dead_code)]
pub fn extract_fingerprint(stdout: &[u8]) -> String {
    let output = std::str::from_utf8(stdout).expect("status stdout should be UTF-8");
    output
        .lines()
        .find_map(|line| line.strip_prefix("Fingerprint: "))
        .expect("fingerprint not found in output")
        .to_string()
}

// This convenience wrapper is only needed by update-focused integration tests.
// Keep it shared here so fingerprint parsing logic stays in one place.
#[allow(dead_code)]
pub fn status_fingerprint(cwd: &Path, args: &[&str]) -> (Output, String) {
    let output = status_output(cwd, args);
    let fingerprint = extract_fingerprint(&output.stdout);
    (output, fingerprint)
}
