use super::*;
use crate::ward_file::WardEntry;
use std::fs;
#[cfg(unix)]
use std::os::unix;
use tempfile::TempDir;

fn create_ward_file(dir: &Path, entries: BTreeMap<String, WardEntry>) {
    let ward = WardFile::new(entries);
    ward.save(&dir.join(".treeward")).unwrap();
}

/// Sets the mtime of `path` (following symlinks) and leaves its atime alone.
///
/// Status classification and the fingerprint both read mtimes, so tests use this to pin a value
/// rather than depend on the clock, including pre-epoch values the ward format cannot persist.
fn set_mtime(path: impl AsRef<Path>, mtime: std::time::SystemTime) {
    fs::set_times(path, fs::FileTimes::new().set_modified(mtime)).unwrap();
}

mod basic;
mod list_to_read_race;
mod mode_and_fingerprint;
mod policy;
#[path = "unix.rs"]
mod unix_tests;
mod ward_update;
