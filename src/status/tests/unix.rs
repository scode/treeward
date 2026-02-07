use super::*;

#[test]
#[cfg(unix)]
fn test_symlink_target_changed() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(root.join("target1.txt"), "content").unwrap();
    fs::write(root.join("target2.txt"), "content").unwrap();
    unix::fs::symlink("target2.txt", root.join("link")).unwrap();

    let mut entries = BTreeMap::new();
    entries.insert(
        "link".to_string(),
        WardEntry::Symlink {
            symlink_target: PathBuf::from("target1.txt"),
        },
    );
    entries.insert(
        "target1.txt".to_string(),
        WardEntry::File {
            sha256: "abc".to_string(),
            mtime_nanos: 1000,
            size: 7,
        },
    );
    entries.insert(
        "target2.txt".to_string(),
        WardEntry::File {
            sha256: "abc".to_string(),
            mtime_nanos: 1000,
            size: 7,
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

    let link_change = result.statuses.iter().find(|c| c.path() == "link");
    assert!(link_change.is_some());
    assert_eq!(link_change.unwrap().status_type(), StatusType::Modified);
}

#[test]
#[cfg(unix)]
fn test_type_change() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(root.join("target.txt"), "content").unwrap();
    unix::fs::symlink("target.txt", root.join("item")).unwrap();

    let mut entries = BTreeMap::new();
    entries.insert(
        "item".to_string(),
        WardEntry::File {
            sha256: "abc123".to_string(),
            mtime_nanos: 1000,
            size: 100,
        },
    );
    entries.insert(
        "target.txt".to_string(),
        WardEntry::File {
            sha256: "abc".to_string(),
            mtime_nanos: 1000,
            size: 7,
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

    let item_change = result.statuses.iter().find(|c| c.path() == "item");
    assert!(item_change.is_some());
    assert_eq!(item_change.unwrap().status_type(), StatusType::Modified);
}

#[test]
#[cfg(unix)]
fn test_symlink_cycle_does_not_cause_infinite_loop() {
    use std::os::unix;

    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::create_dir(root.join("dir")).unwrap();
    fs::write(root.join("dir/file.txt"), "content").unwrap();
    // Symlink pointing back to parent - would cause infinite loop if followed
    unix::fs::symlink("..", root.join("dir/parent_link")).unwrap();
    // Self-referential symlink
    unix::fs::symlink("self", root.join("self")).unwrap();
    // Mutual symlinks
    unix::fs::symlink("b", root.join("a")).unwrap();
    unix::fs::symlink("a", root.join("b")).unwrap();

    create_ward_file(root, BTreeMap::new());

    // This should complete without hanging
    let result = compute_status(
        root,
        ChecksumPolicy::Never,
        StatusMode::Interesting,
        StatusPurpose::Display,
        DiffMode::None,
    );

    assert!(result.is_ok());
    let status = result.unwrap();

    // Verify symlinks are tracked as added entries
    let paths: Vec<_> = status.statuses.iter().map(|s| s.path()).collect();
    assert!(paths.iter().any(|p| p.ends_with("self")));
    assert!(paths.iter().any(|p| p.ends_with("a")));
    assert!(paths.iter().any(|p| p.ends_with("b")));
    assert!(paths.iter().any(|p| p.ends_with("parent_link")));
}
