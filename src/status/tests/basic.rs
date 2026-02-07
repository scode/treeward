use super::*;

#[test]
fn test_no_changes() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(root.join("file1.txt"), "content1").unwrap();
    fs::create_dir(root.join("dir1")).unwrap();
    fs::write(root.join("dir1/file2.txt"), "content2").unwrap();

    let checksum1 = checksum_file(&root.join("file1.txt")).unwrap();
    let checksum2 = checksum_file(&root.join("dir1/file2.txt")).unwrap();

    let metadata1 = std::fs::metadata(root.join("file1.txt")).unwrap();
    let metadata2 = std::fs::metadata(root.join("dir1/file2.txt")).unwrap();

    let mut root_entries = BTreeMap::new();
    root_entries.insert(
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
    root_entries.insert("dir1".to_string(), WardEntry::Dir {});
    create_ward_file(root, root_entries);

    let mut dir1_entries = BTreeMap::new();
    dir1_entries.insert(
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
    create_ward_file(&root.join("dir1"), dir1_entries);

    let result = compute_status(
        root,
        ChecksumPolicy::Never,
        StatusMode::Interesting,
        StatusPurpose::Display,
        DiffMode::None,
    )
    .unwrap();
    assert_eq!(result.statuses.len(), 0);

    let result2 = compute_status(
        root,
        ChecksumPolicy::Never,
        StatusMode::Interesting,
        StatusPurpose::Display,
        DiffMode::None,
    )
    .unwrap();
    assert_eq!(result.fingerprint, result2.fingerprint);
}

#[test]
fn test_added_files() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    create_ward_file(root, BTreeMap::new());

    fs::write(root.join("file1.txt"), "content1").unwrap();

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
    assert_eq!(result.statuses[0].status_type(), StatusType::Added);
}

#[test]
fn test_added_directory() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    create_ward_file(root, BTreeMap::new());

    fs::create_dir(root.join("dir1")).unwrap();
    fs::write(root.join("dir1/file1.txt"), "content").unwrap();

    let result = compute_status(
        root,
        ChecksumPolicy::Never,
        StatusMode::Interesting,
        StatusPurpose::Display,
        DiffMode::None,
    )
    .unwrap();
    assert_eq!(result.statuses.len(), 2);

    let paths: Vec<&str> = result.statuses.iter().map(|c| c.path()).collect();
    assert!(paths.contains(&"dir1"));
    assert!(paths.contains(&"dir1/file1.txt"));

    for change in &result.statuses {
        assert_eq!(change.status_type(), StatusType::Added);
    }
}

#[test]
fn test_removed_files() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    let mut entries = BTreeMap::new();
    entries.insert(
        "file1.txt".to_string(),
        WardEntry::File {
            sha256: "abc123".to_string(),
            mtime_nanos: 1000,
            size: 100,
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
    assert_eq!(result.statuses[0].path(), "file1.txt");
    assert_eq!(result.statuses[0].status_type(), StatusType::Removed);
}

#[test]
fn test_removed_directory() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::create_dir(root.join("dir1")).unwrap();

    let mut root_entries = BTreeMap::new();
    root_entries.insert("dir1".to_string(), WardEntry::Dir {});
    create_ward_file(root, root_entries);

    let mut dir1_entries = BTreeMap::new();
    dir1_entries.insert(
        "file1.txt".to_string(),
        WardEntry::File {
            sha256: "abc123".to_string(),
            mtime_nanos: 1000,
            size: 100,
        },
    );
    create_ward_file(&root.join("dir1"), dir1_entries);

    fs::remove_dir_all(root.join("dir1")).unwrap();

    let result = compute_status(
        root,
        ChecksumPolicy::Never,
        StatusMode::Interesting,
        StatusPurpose::Display,
        DiffMode::None,
    )
    .unwrap();
    assert_eq!(result.statuses.len(), 1);

    assert_eq!(result.statuses[0].path(), "dir1");
    assert_eq!(result.statuses[0].status_type(), StatusType::Removed);
}

#[test]
fn test_modified_file_without_verify() {
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
            mtime_nanos: 1000,
            size: metadata.len(),
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
    assert_eq!(result.statuses[0].path(), "file1.txt");
    assert_eq!(
        result.statuses[0].status_type(),
        StatusType::PossiblyModified
    );
}

#[test]
fn test_modified_file_with_verify_changed() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(root.join("file1.txt"), "content1").unwrap();

    let mut entries = BTreeMap::new();
    entries.insert(
        "file1.txt".to_string(),
        WardEntry::File {
            sha256: "wrong_checksum".to_string(),
            mtime_nanos: 1000,
            size: 8,
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
fn test_modified_file_with_verify_unchanged() {
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

#[test]
fn test_mixed_changes() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(root.join("file1.txt"), "content1").unwrap();
    fs::write(root.join("file4.txt"), "new file").unwrap();

    let checksum1 = checksum_file(&root.join("file1.txt")).unwrap();
    let metadata1 = std::fs::metadata(root.join("file1.txt")).unwrap();

    let mut entries = BTreeMap::new();
    entries.insert(
        "file1.txt".to_string(),
        WardEntry::File {
            sha256: checksum1.sha256,
            mtime_nanos: 1000,
            size: metadata1.len(),
        },
    );
    entries.insert(
        "file2.txt".to_string(),
        WardEntry::File {
            sha256: "abc".to_string(),
            mtime_nanos: 1000,
            size: 100,
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
    assert_eq!(result.statuses.len(), 3);

    let change_types: BTreeMap<&str, StatusType> = result
        .statuses
        .iter()
        .map(|c| (c.path(), c.status_type()))
        .collect();

    assert_eq!(
        change_types.get("file1.txt"),
        Some(&StatusType::PossiblyModified)
    );
    assert_eq!(change_types.get("file2.txt"), Some(&StatusType::Removed));
    assert_eq!(change_types.get("file4.txt"), Some(&StatusType::Added));

    let paths: Vec<&str> = result.statuses.iter().map(|c| c.path()).collect();
    assert_eq!(paths, vec!["file1.txt", "file2.txt", "file4.txt"]);
}

#[test]
fn test_nested_directories() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::create_dir(root.join("dir1")).unwrap();
    fs::create_dir(root.join("dir1/dir2")).unwrap();
    fs::create_dir(root.join("dir1/dir2/dir3")).unwrap();
    fs::write(root.join("dir1/dir2/dir3/file.txt"), "content").unwrap();

    let checksum = checksum_file(&root.join("dir1/dir2/dir3/file.txt")).unwrap();

    let mut root_entries = BTreeMap::new();
    root_entries.insert("dir1".to_string(), WardEntry::Dir {});
    create_ward_file(root, root_entries);

    let mut dir1_entries = BTreeMap::new();
    dir1_entries.insert("dir2".to_string(), WardEntry::Dir {});
    create_ward_file(&root.join("dir1"), dir1_entries);

    let mut dir2_entries = BTreeMap::new();
    dir2_entries.insert("dir3".to_string(), WardEntry::Dir {});
    create_ward_file(&root.join("dir1/dir2"), dir2_entries);

    let metadata = std::fs::metadata(root.join("dir1/dir2/dir3/file.txt")).unwrap();

    let mut dir3_entries = BTreeMap::new();
    dir3_entries.insert(
        "file.txt".to_string(),
        WardEntry::File {
            sha256: checksum.sha256,
            mtime_nanos: 1000,
            size: metadata.len(),
        },
    );
    create_ward_file(&root.join("dir1/dir2/dir3"), dir3_entries);

    let result = compute_status(
        root,
        ChecksumPolicy::Never,
        StatusMode::Interesting,
        StatusPurpose::Display,
        DiffMode::None,
    )
    .unwrap();

    let file_change = result
        .statuses
        .iter()
        .find(|c| c.path() == "dir1/dir2/dir3/file.txt");
    assert!(file_change.is_some());
    assert_eq!(
        file_change.unwrap().status_type(),
        StatusType::PossiblyModified
    );
}

#[test]
fn test_uninitialized_directory() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(root.join("file1.txt"), "content1").unwrap();
    fs::write(root.join("file2.txt"), "content2").unwrap();
    fs::create_dir(root.join("dir1")).unwrap();

    let result = compute_status(
        root,
        ChecksumPolicy::Never,
        StatusMode::Interesting,
        StatusPurpose::Display,
        DiffMode::None,
    )
    .unwrap();
    assert_eq!(result.statuses.len(), 3);

    for change in &result.statuses {
        assert_eq!(change.status_type(), StatusType::Added);
    }
}

#[test]
fn test_malformed_ward_file_is_error() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(root.join("file.txt"), "content").unwrap();
    fs::write(root.join(".treeward"), "this is not valid TOML {{{").unwrap();

    let result = compute_status(
        root,
        ChecksumPolicy::Never,
        StatusMode::Interesting,
        StatusPurpose::Display,
        DiffMode::None,
    );

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        matches!(err, StatusError::WardFile(WardFileError::TomlParse(_))),
        "Expected TomlParse error, got: {:?}",
        err
    );
}
