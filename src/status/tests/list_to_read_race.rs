//! Tests for the list-to-read concurrent-modification check.
//!
//! `checksum_file` guards its own read window; these tests pin the second
//! guard, which compares the checksum against the metadata captured when the
//! directory was listed. A real race is not reproducible deterministically,
//! so each test feeds `compare_entries` a listing that is deliberately stale
//! (the file was edited "after listing") and asserts the walk aborts with
//! `ConcurrentModification` instead of quietly recording the new state.

use super::*;
use std::time::Duration;

/// Runs `compare_entries` for one file in `root` with the given listing view.
fn compare_one(
    root: &Path,
    name: &str,
    listed: FsEntry,
    ward: Option<WardEntry>,
    policy: ChecksumPolicy,
    purpose: StatusPurpose,
) -> Result<Vec<FingerprintRecord>, StatusError> {
    let ctx = WalkContext {
        tree_root: root,
        policy,
        mode: StatusMode::Interesting,
        purpose,
        diff_mode: DiffMode::None,
    };
    let mut ward_entries = BTreeMap::new();
    if let Some(w) = ward {
        ward_entries.insert(name.to_string(), w);
    }
    let mut fs_entries = BTreeMap::new();
    fs_entries.insert(name.to_string(), listed);
    let mut statuses = Vec::new();
    let mut records = Vec::new();
    compare_entries(
        ctx,
        root,
        &ward_entries,
        &fs_entries,
        &mut statuses,
        &mut records,
    )?;
    Ok(records)
}

/// The listing as it actually is right now, for the happy-path controls.
fn fresh_listing(path: &Path) -> FsEntry {
    let md = fs::metadata(path).unwrap();
    FsEntry::File {
        mtime: md.modified().unwrap(),
        size: md.len(),
    }
}

/// A listing taken "before" an edit: same size, older mtime.
fn stale_listing(path: &Path) -> FsEntry {
    let md = fs::metadata(path).unwrap();
    FsEntry::File {
        mtime: md.modified().unwrap() - Duration::from_secs(60),
        size: md.len(),
    }
}

fn is_concurrent_modification(err: &StatusError) -> bool {
    matches!(
        err,
        StatusError::Checksum(ChecksumError::ConcurrentModification(_))
    )
}

/// The direct contract of the helper: a checksum whose mtime or size
/// disagrees with the listing is a concurrent modification, and an agreeing
/// one passes through unchanged.
#[test]
fn test_checksum_listed_file_rejects_drift_in_mtime_or_size() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("f.txt");
    fs::write(&path, "content").unwrap();
    let md = fs::metadata(&path).unwrap();
    let mtime = md.modified().unwrap();

    assert!(checksum_listed_file(&path, &mtime, md.len()).is_ok());
    match checksum_listed_file(&path, &(mtime - Duration::from_secs(1)), md.len()) {
        Err(e) => assert!(is_concurrent_modification(&e), "got {e}"),
        Ok(_) => panic!("stale mtime was accepted"),
    }
    match checksum_listed_file(&path, &mtime, md.len() + 1) {
        Err(e) => assert!(is_concurrent_modification(&e), "got {e}"),
        Ok(_) => panic!("stale size was accepted"),
    }
}

/// The hole this closes: under the default metadata-only policy, `update`
/// checksums a file whose metadata differs from the ward. An edit landing
/// between listing and that checksum used to be recorded in the ward while
/// the fingerprint kept the listing-time values, so `update --fingerprint`
/// accepted state the user never reviewed. Now the walk aborts.
#[test]
fn test_modified_file_edited_after_listing_aborts_ward_update() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let path = root.join("f.txt");
    fs::write(&path, "new content").unwrap();
    let ward = WardEntry::File {
        sha256: "0".repeat(64),
        mtime_nanos: 1,
        size: 3,
    };

    let err = compare_one(
        root,
        "f.txt",
        stale_listing(&path),
        Some(ward.clone()),
        ChecksumPolicy::Never,
        StatusPurpose::WardUpdate,
    )
    .unwrap_err();
    assert!(is_concurrent_modification(&err), "got {err}");

    // Control: the same call with an accurate listing succeeds.
    compare_one(
        root,
        "f.txt",
        fresh_listing(&path),
        Some(ward),
        ChecksumPolicy::Never,
        StatusPurpose::WardUpdate,
    )
    .unwrap();
}

/// Added files have the same split: the ward entry comes from a checksum
/// while the fingerprint uses the listing. A stale listing must abort here
/// too, for both the ward-building path and the checksum-driven fingerprint.
#[test]
fn test_added_file_edited_after_listing_aborts() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let path = root.join("new.txt");
    fs::write(&path, "content").unwrap();

    for (policy, purpose) in [
        (ChecksumPolicy::Never, StatusPurpose::WardUpdate),
        (ChecksumPolicy::Always, StatusPurpose::Display),
    ] {
        let err =
            compare_one(root, "new.txt", stale_listing(&path), None, policy, purpose).unwrap_err();
        assert!(
            is_concurrent_modification(&err),
            "{policy:?}/{purpose:?}: got {err}"
        );
        compare_one(root, "new.txt", fresh_listing(&path), None, policy, purpose).unwrap();
    }
}

/// A stale listing is harmless when nothing reads the file: plain `status`
/// under the metadata-only policy reports from the listing alone, so it must
/// not start failing on trees that are being written to.
#[test]
fn test_stale_listing_without_checksum_is_not_an_error() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let path = root.join("f.txt");
    fs::write(&path, "content").unwrap();

    let records = compare_one(
        root,
        "f.txt",
        stale_listing(&path),
        None,
        ChecksumPolicy::Never,
        StatusPurpose::Display,
    )
    .unwrap();
    assert_eq!(records.len(), 1);
    let FsEntry::File { mtime, size } = stale_listing(&path) else {
        unreachable!()
    };
    assert_eq!(
        records[0].payload,
        FingerprintPayload::File {
            mtime_nanos: mtime_to_nanos(&mtime, &path).unwrap(),
            size,
            sha256: None,
        },
        "the fingerprint must carry the listing's values, not a fresh read"
    );
}
