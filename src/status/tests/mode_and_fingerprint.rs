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
            sha256: "wrong_checksum".to_string(),
            mtime_nanos: 1000,
            size: 8,
        },
    );
    entries.insert(
        "removed.txt".to_string(),
        WardEntry::File {
            sha256: "abc".to_string(),
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

#[test]
fn test_unchanged_not_in_fingerprint() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

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

    assert_eq!(result_interesting.statuses.len(), 0);
    assert_eq!(result_all.statuses.len(), 1);
    assert_eq!(result_all.statuses[0].status_type(), StatusType::Unchanged);

    assert_eq!(result_interesting.fingerprint, result_all.fingerprint);
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
