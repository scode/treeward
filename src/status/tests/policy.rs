use super::*;

#[test]
fn test_checksum_policy_never() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(root.join("file1.txt"), "original content").unwrap();
    let original_checksum = checksum_file(&root.join("file1.txt")).unwrap();
    let original_metadata = std::fs::metadata(root.join("file1.txt")).unwrap();

    let mut entries = BTreeMap::new();
    entries.insert(
        "file1.txt".to_string(),
        WardEntry::File {
            sha256: original_checksum.sha256.clone(),
            mtime_nanos: 1000,
            size: original_metadata.len(),
        },
    );
    create_ward_file(root, entries);

    fs::write(root.join("file1.txt"), "modified content").unwrap();

    let result = compute_status(
        root,
        ChecksumPolicy::Never,
        StatusMode::Interesting,
        StatusPurpose::Display,
        DiffMode::None,
    )
    .unwrap();
    assert_eq!(result.statuses.len(), 1);
    assert_eq!(result.statuses[0].path(), "file1.txt");
    assert_eq!(
        result.statuses[0].status_type(),
        StatusType::PossiblyModified
    );
}

#[test]
fn test_checksum_policy_when_possibly_modified_metadata_differs() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(root.join("file1.txt"), "modified content").unwrap();

    let mut entries = BTreeMap::new();
    entries.insert(
        "file1.txt".to_string(),
        WardEntry::File {
            sha256: "wrong_checksum".to_string(),
            mtime_nanos: 1000,
            size: 16,
        },
    );
    create_ward_file(root, entries);

    let result = compute_status(
        root,
        ChecksumPolicy::WhenPossiblyModified,
        StatusMode::Interesting,
        StatusPurpose::Display,
        DiffMode::None,
    )
    .unwrap();
    assert_eq!(result.statuses.len(), 1);
    assert_eq!(result.statuses[0].path(), "file1.txt");
    assert_eq!(result.statuses[0].status_type(), StatusType::Modified);
}

#[test]
fn test_checksum_policy_when_possibly_modified_metadata_same() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(root.join("file1.txt"), "content").unwrap();
    let checksum = checksum_file(&root.join("file1.txt")).unwrap();
    let metadata = std::fs::metadata(root.join("file1.txt")).unwrap();
    let mtime_nanos = metadata
        .modified()
        .unwrap()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;

    let mut entries = BTreeMap::new();
    entries.insert(
        "file1.txt".to_string(),
        WardEntry::File {
            sha256: checksum.sha256,
            mtime_nanos,
            size: metadata.len(),
        },
    );
    create_ward_file(root, entries);

    let result = compute_status(
        root,
        ChecksumPolicy::WhenPossiblyModified,
        StatusMode::Interesting,
        StatusPurpose::Display,
        DiffMode::None,
    )
    .unwrap();
    assert_eq!(result.statuses.len(), 0);
}

#[test]
fn test_checksum_policy_always_with_corruption() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(root.join("file1.txt"), "content").unwrap();
    let metadata = std::fs::metadata(root.join("file1.txt")).unwrap();
    let mtime_nanos = metadata
        .modified()
        .unwrap()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;

    let mut entries = BTreeMap::new();
    entries.insert(
        "file1.txt".to_string(),
        WardEntry::File {
            sha256: "wrong_checksum_simulating_corruption".to_string(),
            mtime_nanos,
            size: metadata.len(),
        },
    );
    create_ward_file(root, entries);

    let result = compute_status(
        root,
        ChecksumPolicy::Always,
        StatusMode::Interesting,
        StatusPurpose::Display,
        DiffMode::None,
    )
    .unwrap();
    assert_eq!(result.statuses.len(), 1);
    assert_eq!(result.statuses[0].path(), "file1.txt");
    assert_eq!(result.statuses[0].status_type(), StatusType::Modified);
}

/// Diff capture must populate `old_ward_entry` when ONLY the checksum differs.
///
/// With matching metadata, `metadata_differs` is false and the capture condition
/// rests entirely on its `sha256_differs` half — the path that lets
/// `--diff --always-verify` display what silent corruption changed. Without this
/// test, that half of the condition could be deleted with no test failing.
#[test]
fn test_diff_capture_on_checksum_only_difference() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(root.join("file1.txt"), "content").unwrap();
    let actual_checksum = checksum_file(&root.join("file1.txt")).unwrap();

    let recorded_entry = WardEntry::File {
        sha256: "wrong_checksum_simulating_corruption".to_string(),
        mtime_nanos: mtime_to_nanos(&actual_checksum.mtime).unwrap(),
        size: actual_checksum.size,
    };
    let mut entries = BTreeMap::new();
    entries.insert("file1.txt".to_string(), recorded_entry.clone());
    create_ward_file(root, entries);

    let result = compute_status(
        root,
        ChecksumPolicy::Always,
        StatusMode::Interesting,
        StatusPurpose::Display,
        DiffMode::Capture,
    )
    .unwrap();

    assert_eq!(result.statuses.len(), 1);
    match &result.statuses[0] {
        StatusEntry::Modified {
            path,
            ward_entry,
            old_ward_entry,
        } => {
            assert_eq!(path, "file1.txt");
            assert_eq!(
                old_ward_entry.as_ref(),
                Some(&recorded_entry),
                "old ward entry must be captured for diff display"
            );
            match ward_entry {
                Some(WardEntry::File { sha256, .. }) => {
                    assert_eq!(sha256, &actual_checksum.sha256);
                }
                other => panic!("Expected new file ward entry, got {:?}", other),
            }
        }
        other => panic!("Expected Modified entry, got {:?}", other),
    }
}

/// `DiffMode::Capture` must force checksumming of metadata-differing files even
/// under `ChecksumPolicy::Never`.
///
/// The status class stays PossiblyModified — policy alone controls reporting —
/// but the captured `ward_entry` must carry the file's real current checksum so
/// the diff can show old vs new. If the diff-driven checksum term were dropped,
/// the entry would silently carry the stale recorded checksum instead.
#[test]
fn test_diff_capture_forces_checksum_under_policy_never() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(root.join("file1.txt"), "original content").unwrap();
    let old_checksum = checksum_file(&root.join("file1.txt")).unwrap();

    let recorded_entry = WardEntry::File {
        sha256: old_checksum.sha256.clone(),
        // Stale mtime so the entry is metadata-differing.
        mtime_nanos: 1000,
        size: old_checksum.size,
    };
    let mut entries = BTreeMap::new();
    entries.insert("file1.txt".to_string(), recorded_entry.clone());
    create_ward_file(root, entries);

    fs::write(root.join("file1.txt"), "modified content").unwrap();
    let new_checksum = checksum_file(&root.join("file1.txt")).unwrap();

    let result = compute_status(
        root,
        ChecksumPolicy::Never,
        StatusMode::Interesting,
        StatusPurpose::Display,
        DiffMode::Capture,
    )
    .unwrap();

    assert_eq!(result.statuses.len(), 1);
    match &result.statuses[0] {
        StatusEntry::PossiblyModified {
            path,
            ward_entry,
            old_ward_entry,
        } => {
            assert_eq!(path, "file1.txt");
            assert_eq!(old_ward_entry.as_ref(), Some(&recorded_entry));
            match ward_entry {
                Some(WardEntry::File { sha256, .. }) => {
                    assert_eq!(
                        sha256, &new_checksum.sha256,
                        "Capture must have checksummed the current content"
                    );
                }
                other => panic!("Expected new file ward entry, got {:?}", other),
            }
        }
        other => panic!("Expected PossiblyModified entry, got {:?}", other),
    }
}

#[test]
fn test_checksum_policy_always_without_corruption() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(root.join("file1.txt"), "content").unwrap();
    let checksum = checksum_file(&root.join("file1.txt")).unwrap();
    let metadata = std::fs::metadata(root.join("file1.txt")).unwrap();
    let mtime_nanos = metadata
        .modified()
        .unwrap()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;

    let mut entries = BTreeMap::new();
    entries.insert(
        "file1.txt".to_string(),
        WardEntry::File {
            sha256: checksum.sha256,
            mtime_nanos,
            size: metadata.len(),
        },
    );
    create_ward_file(root, entries);

    let result = compute_status(
        root,
        ChecksumPolicy::Always,
        StatusMode::Interesting,
        StatusPurpose::Display,
        DiffMode::None,
    )
    .unwrap();
    assert_eq!(result.statuses.len(), 0);
}

#[test]
fn test_metadata_changed_but_content_unchanged() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(root.join("file1.txt"), "content").unwrap();
    let checksum = checksum_file(&root.join("file1.txt")).unwrap();
    let metadata = std::fs::metadata(root.join("file1.txt")).unwrap();

    let mut entries = BTreeMap::new();
    entries.insert(
        "file1.txt".to_string(),
        WardEntry::File {
            sha256: checksum.sha256,
            mtime_nanos: 1000,
            size: metadata.len(),
        },
    );
    create_ward_file(root, entries);

    let result = compute_status(
        root,
        ChecksumPolicy::WhenPossiblyModified,
        StatusMode::Interesting,
        StatusPurpose::Display,
        DiffMode::None,
    )
    .unwrap();
    assert_eq!(result.statuses.len(), 0);
}
