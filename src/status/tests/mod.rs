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

mod basic;
mod mode_and_fingerprint;
mod policy;
#[path = "unix.rs"]
mod unix_tests;
mod ward_update;
