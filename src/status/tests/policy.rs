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
