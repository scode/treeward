use super::*;

#[test]
fn test_fingerprint_deterministic() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    create_ward_file(root, BTreeMap::new());

    fs::write(root.join("file1.txt"), "content").unwrap();

    let result1 = compute_status(
        root,
        ChecksumPolicy::Never,
        StatusMode::Interesting,
        StatusPurpose::Display,
        DiffMode::None,
    )
    .unwrap();
    let result2 = compute_status(
        root,
        ChecksumPolicy::Never,
        StatusMode::Interesting,
        StatusPurpose::Display,
        DiffMode::None,
    )
    .unwrap();

    assert_eq!(result1.fingerprint, result2.fingerprint);
}

#[test]
fn test_different_fingerprints() {
    let temp1 = TempDir::new().unwrap();
    let root1 = temp1.path();
    fs::write(root1.join("file3.txt"), "content").unwrap();
    create_ward_file(root1, BTreeMap::new());

    let temp2 = TempDir::new().unwrap();
    let root2 = temp2.path();
    fs::write(root2.join("file4.txt"), "content").unwrap();
    create_ward_file(root2, BTreeMap::new());

    let result1 = compute_status(
        root1,
        ChecksumPolicy::Never,
        StatusMode::Interesting,
        StatusPurpose::Display,
        DiffMode::None,
    )
    .unwrap();
    let result2 = compute_status(
        root2,
        ChecksumPolicy::Never,
        StatusMode::Interesting,
        StatusPurpose::Display,
        DiffMode::None,
    )
    .unwrap();

    assert_ne!(result1.fingerprint, result2.fingerprint);
}

#[test]
fn test_status_mode_all_shows_unchanged() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(root.join("file1.txt"), "content1").unwrap();
    fs::write(root.join("file2.txt"), "content2").unwrap();

    let checksum1 = checksum_file(&root.join("file1.txt")).unwrap();
    let checksum2 = checksum_file(&root.join("file2.txt")).unwrap();
    let metadata1 = std::fs::metadata(root.join("file1.txt")).unwrap();
    let metadata2 = std::fs::metadata(root.join("file2.txt")).unwrap();

    let mut entries = BTreeMap::new();
    entries.insert(
        "file1.txt".to_string(),
        WardEntry::File {
            sha256: checksum1.sha256.clone(),
            mtime_nanos: metadata1
                .modified()
                .unwrap()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u64,
            size: metadata1.len(),
        },
    );
    entries.insert(
        "file2.txt".to_string(),
        WardEntry::File {
            sha256: checksum2.sha256.clone(),
            mtime_nanos: metadata2
                .modified()
                .unwrap()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u64,
            size: metadata2.len(),
        },
    );
    create_ward_file(root, entries);

    let result = compute_status(
        root,
        ChecksumPolicy::Never,
        StatusMode::All,
        StatusPurpose::Display,
        DiffMode::None,
    )
    .unwrap();
    assert_eq!(result.statuses.len(), 2);

    for change in &result.statuses {
        assert_eq!(change.status_type(), StatusType::Unchanged);
    }

    let paths: Vec<&str> = result.statuses.iter().map(|c| c.path()).collect();
    assert!(paths.contains(&"file1.txt"));
    assert!(paths.contains(&"file2.txt"));
}

#[test]
fn test_status_mode_all_with_changes() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(root.join("unchanged.txt"), "unchanged").unwrap();
    fs::write(root.join("modified.txt"), "modified").unwrap();

    let checksum_unchanged = checksum_file(&root.join("unchanged.txt")).unwrap();
    let metadata_unchanged = std::fs::metadata(root.join("unchanged.txt")).unwrap();

    let mut entries = BTreeMap::new();
    entries.insert(
        "unchanged.txt".to_string(),
        WardEntry::File {
            sha256: checksum_unchanged.sha256.clone(),
            mtime_nanos: metadata_unchanged
                .modified()
                .unwrap()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u64,
            size: metadata_unchanged.len(),
        },
    );
    entries.insert(
        "modified.txt".to_string(),
        WardEntry::File {
            sha256: "baadbaadbaadbaadbaadbaadbaadbaadbaadbaadbaadbaadbaadbaadbaadbaad".to_string(),
            mtime_nanos: 1000,
            size: 8,
        },
    );
    entries.insert(
        "removed.txt".to_string(),
        WardEntry::File {
            sha256: "abcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabca".to_string(),
            mtime_nanos: 1000,
            size: 100,
        },
    );
    create_ward_file(root, entries);

    fs::write(root.join("added.txt"), "new file").unwrap();

    let result = compute_status(
        root,
        ChecksumPolicy::Never,
        StatusMode::All,
        StatusPurpose::Display,
        DiffMode::None,
    )
    .unwrap();
    assert_eq!(result.statuses.len(), 4);

    let change_types: BTreeMap<&str, StatusType> = result
        .statuses
        .iter()
        .map(|c| (c.path(), c.status_type()))
        .collect();

    assert_eq!(
        change_types.get("unchanged.txt"),
        Some(&StatusType::Unchanged)
    );
    assert_eq!(
        change_types.get("modified.txt"),
        Some(&StatusType::PossiblyModified)
    );
    assert_eq!(change_types.get("added.txt"), Some(&StatusType::Added));
    assert_eq!(change_types.get("removed.txt"), Some(&StatusType::Removed));
}

/// Unchanged entries must not contribute to the fingerprint.
///
/// The cross-mode assertion below is not enough by itself: fingerprint records
/// are collected independently of display mode, so a bug that hashed unchanged
/// entries would leak into both mode results equally. Comparing against an empty
/// tree pins the actual exclusion property because `compute_fingerprint` hashes
/// only its records and does not include the root path.
#[test]
fn test_unchanged_not_in_fingerprint() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let empty_temp = TempDir::new().unwrap();
    let empty_root = empty_temp.path();

    fs::write(root.join("file1.txt"), "content1").unwrap();

    let checksum = checksum_file(&root.join("file1.txt")).unwrap();
    let metadata = std::fs::metadata(root.join("file1.txt")).unwrap();

    let mut entries = BTreeMap::new();
    entries.insert(
        "file1.txt".to_string(),
        WardEntry::File {
            sha256: checksum.sha256,
            mtime_nanos: metadata
                .modified()
                .unwrap()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u64,
            size: metadata.len(),
        },
    );
    create_ward_file(root, entries);
    create_ward_file(empty_root, BTreeMap::new());

    let result_interesting = compute_status(
        root,
        ChecksumPolicy::Never,
        StatusMode::Interesting,
        StatusPurpose::Display,
        DiffMode::None,
    )
    .unwrap();
    let result_all = compute_status(
        root,
        ChecksumPolicy::Never,
        StatusMode::All,
        StatusPurpose::Display,
        DiffMode::None,
    )
    .unwrap();
    let result_empty = compute_status(
        empty_root,
        ChecksumPolicy::Never,
        StatusMode::Interesting,
        StatusPurpose::Display,
        DiffMode::None,
    )
    .unwrap();

    assert_eq!(result_interesting.statuses.len(), 0);
    assert_eq!(result_all.statuses.len(), 1);
    assert_eq!(result_empty.statuses.len(), 0);
    assert_eq!(result_all.statuses[0].status_type(), StatusType::Unchanged);

    assert_eq!(result_interesting.fingerprint, result_all.fingerprint);
    assert_eq!(result_interesting.fingerprint, result_empty.fingerprint);
}

/// A Removed entry must bind the fingerprint to the prior ward state, not just
/// path + "R".
///
/// `FingerprintPayload::Removed` exists so ward-state drift between status and
/// update cannot hide behind an unchanged-looking `R` entry: if the recorded
/// ward data for the removed path changes, the fingerprint must change too,
/// even though path and status class are identical.
#[test]
fn test_removed_fingerprint_bound_to_prior_ward_state() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    let fingerprint_for_recorded_sha = |sha: &str| {
        let mut entries = BTreeMap::new();
        entries.insert(
            "removed.txt".to_string(),
            WardEntry::File {
                sha256: sha.to_string(),
                mtime_nanos: 1000,
                size: 5,
            },
        );
        create_ward_file(root, entries);

        let result = compute_status(
            root,
            ChecksumPolicy::Never,
            StatusMode::Interesting,
            StatusPurpose::Display,
            DiffMode::None,
        )
        .unwrap();

        assert_eq!(result.statuses.len(), 1);
        assert_eq!(result.statuses[0].status_type(), StatusType::Removed);
        assert_eq!(result.statuses[0].path(), "removed.txt");
        result.fingerprint
    };

    let fingerprint1 = fingerprint_for_recorded_sha(&"a".repeat(64));
    let fingerprint2 = fingerprint_for_recorded_sha(&"b".repeat(64));

    assert_ne!(
        fingerprint1, fingerprint2,
        "fingerprint must reflect the removed entry's prior ward state"
    );
}

/// A removed symlink must bind the fingerprint to its recorded target.
///
/// This uses only ward state and an empty filesystem. No platform symlink
/// support is involved: the property being pinned is that two missing symlinks
/// with the same path but different recorded `symlink_target` values do not
/// share a fingerprint.
#[test]
fn test_removed_symlink_fingerprint_bound_to_prior_target() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    let fingerprint_for_recorded_target = |target: &str| {
        let mut entries = BTreeMap::new();
        entries.insert(
            "removed-link".to_string(),
            WardEntry::Symlink {
                symlink_target: PathBuf::from(target),
            },
        );
        create_ward_file(root, entries);

        let result = compute_status(
            root,
            ChecksumPolicy::Never,
            StatusMode::Interesting,
            StatusPurpose::Display,
            DiffMode::None,
        )
        .unwrap();

        assert_eq!(result.statuses.len(), 1);
        assert_eq!(result.statuses[0].status_type(), StatusType::Removed);
        assert_eq!(result.statuses[0].path(), "removed-link");
        result.fingerprint
    };

    let fingerprint1 = fingerprint_for_recorded_target("old-target");
    let fingerprint2 = fingerprint_for_recorded_target("new-target");

    assert_ne!(
        fingerprint1, fingerprint2,
        "fingerprint must reflect the removed symlink's recorded target"
    );
}

/// A removed dir and a removed file at the same path must not share a
/// fingerprint.
///
/// This is the observable kind-difference guarantee. It is deliberately weak
/// as a variant-tag check: the file payload's own field bytes would keep the
/// two fingerprints distinct even if the `removed_*` variant tags were
/// dropped from hashing. The tag itself is pinned by
/// `test_removed_dir_payload_contributes_its_variant_tag` below, which works
/// at the payload-hashing level where the tag is the only material.
#[test]
fn test_removed_dir_fingerprint_bound_to_entry_kind() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    let fingerprint_for_entry = |ward_entry: WardEntry| {
        let mut entries = BTreeMap::new();
        entries.insert("removed".to_string(), ward_entry);
        create_ward_file(root, entries);

        let result = compute_status(
            root,
            ChecksumPolicy::Never,
            StatusMode::Interesting,
            StatusPurpose::Display,
            DiffMode::None,
        )
        .unwrap();

        assert_eq!(result.statuses.len(), 1);
        assert_eq!(result.statuses[0].status_type(), StatusType::Removed);
        assert_eq!(result.statuses[0].path(), "removed");
        result.fingerprint
    };

    let dir_fingerprint = fingerprint_for_entry(WardEntry::Dir {});
    let file_fingerprint = fingerprint_for_entry(WardEntry::File {
        sha256: "a".repeat(64),
        mtime_nanos: 1000,
        size: 5,
    });

    assert_ne!(
        dir_fingerprint, file_fingerprint,
        "fingerprint must reflect the removed entry kind"
    );
}

/// The `removed_dir` variant tag itself must be hashed.
///
/// `WardEntry::Dir` has no fields, so a removed dir's entire payload
/// contribution is its variant tag. Hashing the payload must therefore change
/// the digest relative to hashing nothing — an implementation that dropped
/// the `removed_*` tags from `hash_fingerprint_payload` would contribute zero
/// bytes here and fail this test, which no end-to-end fingerprint comparison
/// can detect (every other payload variant carries field bytes of its own).
#[test]
fn test_removed_dir_payload_contributes_its_variant_tag() {
    let mut with_payload = Sha256::new();
    hash_fingerprint_payload(
        &mut with_payload,
        &FingerprintPayload::Removed {
            ward_entry: WardEntry::Dir {},
        },
    );

    let empty = Sha256::new();

    assert_ne!(
        with_payload.finalize(),
        empty.finalize(),
        "removed_dir payload must contribute its variant tag to the hash"
    );
}

#[test]
fn test_mtime_to_nanos_rejects_pre_epoch() {
    let pre_epoch = UNIX_EPOCH - std::time::Duration::from_secs(1);
    let err = mtime_to_nanos(&pre_epoch, Path::new("some/old.txt")).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("before the UNIX epoch") && msg.contains("some/old.txt"),
        "error must explain the limitation and name the file: {}",
        msg
    );
}

#[test]
fn test_mtime_to_nanos_rejects_toml_integer_overflow() {
    let boundary = UNIX_EPOCH
        + std::time::Duration::from_secs(9_223_372_036)
        + std::time::Duration::from_nanos(854_775_807);
    assert_eq!(
        mtime_to_nanos(&boundary, Path::new("some/boundary.txt")).unwrap(),
        i64::MAX as u64
    );

    let far_future = UNIX_EPOCH + std::time::Duration::from_secs(9_223_372_037);
    let err = mtime_to_nanos(&far_future, Path::new("some/future.txt")).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("i64 nanoseconds since epoch") && msg.contains("some/future.txt"),
        "error must explain the limitation and name the file: {}",
        msg
    );
}

/// A real file with a pre-epoch mtime must surface the conversion error through
/// `compute_status` rather than being silently skipped or misreported.
#[test]
fn test_status_errors_on_pre_epoch_mtime() {
    use filetime::{FileTime, set_file_mtime};

    let temp = TempDir::new().unwrap();
    let root = temp.path();

    create_ward_file(root, BTreeMap::new());
    fs::write(root.join("old.txt"), "content").unwrap();
    set_file_mtime(root.join("old.txt"), FileTime::from_unix_time(-1, 0)).unwrap();

    let err = compute_status(
        root,
        ChecksumPolicy::Never,
        StatusMode::Interesting,
        StatusPurpose::Display,
        DiffMode::None,
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("before the UNIX epoch") && msg.contains("old.txt"),
        "error must explain the limitation and name the file: {}",
        msg
    );
}

/// WARNING: This test verifies that non-UTF-8 paths are rejected rather than
/// silently converted. Do not change this behavior without extreme care!
///
/// The fingerprint mechanism relies on paths being converted to bytes for
/// hashing. If we used lossy UTF-8 conversion (to_string_lossy), different
/// non-UTF-8 byte sequences could map to the same replacement character
/// sequence, causing distinct file sets to produce identical fingerprints.
/// This would break the TOCTOU protection that fingerprints provide.
///
/// If we need to support non-UTF-8 paths in the future, we must use a
/// collision-free encoding (e.g., raw OS bytes via OsStrExt on Unix).
#[test]
#[cfg(unix)]
fn test_non_utf8_path_rejected() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let temp = TempDir::new().unwrap();
    let root = temp.path();

    // Create a filename with invalid UTF-8: 0xFF is not valid in any UTF-8 sequence
    let invalid_utf8_name =
        OsStr::from_bytes(&[0x66, 0x69, 0x6c, 0x65, 0xFF, 0x2e, 0x74, 0x78, 0x74]); // "file\xFF.txt"
    let invalid_path = root.join(invalid_utf8_name);

    // Linux (ext4, etc.) allows non-UTF-8 filenames. Other platforms like macOS
    // (APFS/HFS+) enforce UTF-8 at the filesystem level. If file creation fails,
    // test path_to_str directly on the in-memory path object. This is more prone
    // to errors since we are testing a specific function that may or
    // may not actually be used in the logic that is relevant.
    // However, it is better than nothing given that we cannot actually
    // create the real files on these platforms.
    if fs::write(&invalid_path, "content").is_err() {
        #[cfg(target_os = "linux")]
        panic!("expected non-UTF-8 filename to be allowed on Linux");

        #[cfg(not(target_os = "linux"))]
        {
            let result = path_to_str(&invalid_path);
            assert!(result.is_err());
            let err_msg = result.unwrap_err().to_string();
            assert!(
                err_msg.contains("non-UTF-8"),
                "Error should mention non-UTF-8: {}",
                err_msg
            );
            return;
        }
    }

    create_ward_file(root, BTreeMap::new());

    let result = compute_status(
        root,
        ChecksumPolicy::Never,
        StatusMode::Interesting,
        StatusPurpose::Display,
        DiffMode::None,
    );

    assert!(result.is_err());
    let err = result.unwrap_err();
    let err_msg = err.to_string();
    assert!(
        err_msg.contains("non-UTF-8"),
        "Error should mention non-UTF-8: {}",
        err_msg
    );
}
