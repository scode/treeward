use assert_cmd::{Command, cargo::cargo_bin_cmd};
use std::fs::{self, FileTimes};
use std::path::Path;
use std::process::Output;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

/// Sets the mtime of `path` and leaves its atime alone, panicking on failure.
///
/// Tests use this to fake metadata-only changes (a touched file whose content is unchanged) and
/// to put back the original mtime after rewriting content, so that only a checksum can tell the
/// difference. It follows symlinks, like `touch` does; none of the callers point it at a symlink.
#[allow(dead_code)]
pub fn set_mtime(path: impl AsRef<Path>, mtime: SystemTime) {
    let path = path.as_ref();
    fs::set_times(path, FileTimes::new().set_modified(mtime))
        .unwrap_or_else(|e| panic!("failed to set mtime of {}: {e}", path.display()));
}

/// Returns the instant `secs` seconds after the UNIX epoch, or before it when `secs` is negative.
///
/// Negative values matter: treeward refuses to persist pre-epoch file mtimes, and tests need to
/// create such files and directories on purpose.
#[allow(dead_code)]
pub fn unix_time(secs: i64) -> SystemTime {
    let offset = Duration::from_secs(secs.unsigned_abs());
    if secs >= 0 {
        UNIX_EPOCH + offset
    } else {
        UNIX_EPOCH - offset
    }
}

// This convenience wrapper is only needed by update-focused integration tests.
// Keep it shared here so fingerprint parsing logic stays in one place.
#[allow(dead_code)]
pub fn status_fingerprint(cwd: &Path, args: &[&str]) -> (Output, String) {
    let output = status_output(cwd, args);
    let fingerprint = extract_fingerprint(&output.stdout);
    (output, fingerprint)
}
