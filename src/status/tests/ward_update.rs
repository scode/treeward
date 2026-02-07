use super::*;

/// Verifies that `StatusPurpose::Display` with `DiffMode::None` does not
/// populate `ward_entry`.
///
/// When computing status for display (e.g., `treeward status`), we don't need
/// the full ward entry data - we only care about whether files changed. Populating
/// `ward_entry` would require checksumming files that don't need it, wasting CPU.
///
/// This test creates files in all status states (added, unchanged, modified, removed)
/// and verifies that every `StatusEntry` has `ward_entry=None` when using
/// Display purpose without diff capture.
#[test]
fn test_display_purpose_without_diff_ward_entry_is_none() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(root.join("added.txt"), "new").unwrap();
    fs::write(root.join("unchanged.txt"), "unchanged").unwrap();
    fs::write(root.join("modified.txt"), "modified content").unwrap();

    let checksum_unchanged = checksum_file(&root.join("unchanged.txt")).unwrap();
    let metadata_unchanged = std::fs::metadata(root.join("unchanged.txt")).unwrap();

    let mut entries = BTreeMap::new();
    entries.insert(
        "unchanged.txt".to_string(),
        WardEntry::File {
            sha256: checksum_unchanged.sha256,
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
            sha256: "old_checksum".to_string(),
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

    let result = compute_status(
        root,
        ChecksumPolicy::WhenPossiblyModified,
        StatusMode::All,
        StatusPurpose::Display,
        DiffMode::None,
    )
    .unwrap();

    for status in &result.statuses {
        assert!(
            status.ward_entry().is_none(),
            "Display + DiffMode::None should have ward_entry=None for {}, got {:?}",
            status.path(),
            status.ward_entry()
        );
    }
}

/// Verifies that `StatusPurpose::WardUpdate` populates `ward_entry` for all non-Removed entries.
///
/// When computing status to update ward files (e.g., `treeward update`), we need complete
/// `WardEntry` data including checksums for every file that will be written to `.treeward`.
/// This data comes from `ward_entry` in each `StatusEntry`.
///
/// - Added files: must have freshly computed checksums
/// - Modified files: must have freshly computed checksums reflecting current content
/// - Unchanged files: must have ward_entry (checksum may be reused, tested separately)
/// - Removed files: no ward_entry needed (they're being deleted from the ward file)
///
/// This test verifies the presence of ward_entry and correctness of checksums for
/// added and modified files.
#[test]
fn test_ward_update_purpose_ward_entry_is_some() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(root.join("added.txt"), "new content").unwrap();
    fs::write(root.join("unchanged.txt"), "unchanged").unwrap();
    fs::write(root.join("modified.txt"), "modified content").unwrap();

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
            sha256: "old_checksum".to_string(),
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

    let result = compute_status(
        root,
        ChecksumPolicy::WhenPossiblyModified,
        StatusMode::All,
        StatusPurpose::WardUpdate,
        DiffMode::None,
    )
    .unwrap();

    for status in &result.statuses {
        match status {
            StatusEntry::Removed { .. } => {
                assert!(
                    status.ward_entry().is_none(),
                    "Removed entries should have no ward_entry"
                );
            }
            _ => {
                assert!(
                    status.ward_entry().is_some(),
                    "WardUpdate purpose should have ward_entry=Some for {}, status={:?}",
                    status.path(),
                    status.status_type()
                );
            }
        }
    }

    let added = result
        .statuses
        .iter()
        .find(|s| s.path() == "added.txt")
        .unwrap();
    let added_checksum = checksum_file(&root.join("added.txt")).unwrap();
    match added.ward_entry().unwrap() {
        WardEntry::File { sha256, size, .. } => {
            assert_eq!(sha256, &added_checksum.sha256);
            assert_eq!(*size, added_checksum.size);
        }
        _ => panic!("Expected File entry"),
    }

    let modified = result
        .statuses
        .iter()
        .find(|s| s.path() == "modified.txt")
        .unwrap();
    let modified_checksum = checksum_file(&root.join("modified.txt")).unwrap();
    match modified.ward_entry().unwrap() {
        WardEntry::File { sha256, size, .. } => {
            assert_eq!(sha256, &modified_checksum.sha256);
            assert_eq!(*size, modified_checksum.size);
        }
        _ => panic!("Expected File entry"),
    }
}

/// Verifies the checksum reuse optimization when metadata is unchanged.
///
/// When `StatusPurpose::WardUpdate` is used with `ChecksumPolicy::WhenPossiblyModified`
/// (the default), files whose metadata (mtime, size) matches the ward file should NOT
/// be re-checksummed. Instead, the existing checksum from the ward file is reused.
/// This optimization makes incremental updates fast - only changed files are checksummed.
///
/// This test proves the optimization works by storing a fake checksum in the ward file
/// with matching metadata. If the code incorrectly re-checksummed the file, we'd get
/// the real checksum back. Instead, we verify we get the fake checksum, proving reuse.
#[test]
fn test_ward_update_checksum_reuse_when_metadata_unchanged() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(root.join("file.txt"), "content").unwrap();
    let metadata = std::fs::metadata(root.join("file.txt")).unwrap();
    let mtime_nanos = metadata
        .modified()
        .unwrap()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;

    // Use a fake checksum that differs from the real one. If checksum reuse
    // works, we'll get this fake one back. If not, we'd get the real checksum.
    let fake_checksum = "fake_checksum_proving_reuse_works".to_string();

    let mut entries = BTreeMap::new();
    entries.insert(
        "file.txt".to_string(),
        WardEntry::File {
            sha256: fake_checksum.clone(),
            mtime_nanos,
            size: metadata.len(),
        },
    );
    create_ward_file(root, entries);

    let result = compute_status(
        root,
        ChecksumPolicy::WhenPossiblyModified,
        StatusMode::All,
        StatusPurpose::WardUpdate,
        DiffMode::None,
    )
    .unwrap();

    assert_eq!(result.statuses.len(), 1);
    let status = &result.statuses[0];
    assert_eq!(status.status_type(), StatusType::Unchanged);

    match status.ward_entry().unwrap() {
        WardEntry::File { sha256, .. } => {
            assert_eq!(
                sha256, &fake_checksum,
                "Checksum should be reused from ward file when metadata matches"
            );
        }
        _ => panic!("Expected File entry"),
    }
}

/// Verifies that `ChecksumPolicy::Always` checksums files even when metadata matches.
///
/// Silent data corruption (bit rot) can change file contents without updating mtime.
/// The `--always-verify` flag uses `ChecksumPolicy::Always` to detect this by
/// checksumming every file regardless of metadata.
///
/// This test simulates corruption by storing a wrong checksum in the ward file with
/// matching metadata. With the default policy, this would be detected as Unchanged
/// (metadata matches, so no checksum computed). With `Always`, the checksum mismatch
/// is detected and reported as Modified.
///
/// Also verifies that `ward_entry` contains the correct (freshly computed) checksum,
/// not the corrupted one from the ward file.
#[test]
fn test_checksum_policy_always_forces_checksum_even_when_metadata_matches() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(root.join("file.txt"), "content").unwrap();
    let metadata = std::fs::metadata(root.join("file.txt")).unwrap();
    let mtime_nanos = metadata
        .modified()
        .unwrap()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;

    let mut entries = BTreeMap::new();
    entries.insert(
        "file.txt".to_string(),
        WardEntry::File {
            sha256: "wrong_checksum_simulating_corruption".to_string(),
            mtime_nanos,
            size: metadata.len(),
        },
    );
    create_ward_file(root, entries);

    // With Always policy, even though metadata matches, it should detect the mismatch
    let result = compute_status(
        root,
        ChecksumPolicy::Always,
        StatusMode::Interesting,
        StatusPurpose::WardUpdate,
        DiffMode::None,
    )
    .unwrap();

    assert_eq!(result.statuses.len(), 1);
    assert_eq!(result.statuses[0].status_type(), StatusType::Modified);

    // The ward_entry should have the correct (freshly computed) checksum
    let real_checksum = checksum_file(&root.join("file.txt")).unwrap();
    match result.statuses[0].ward_entry().unwrap() {
        WardEntry::File { sha256, .. } => {
            assert_eq!(sha256, &real_checksum.sha256);
        }
        _ => panic!("Expected File entry"),
    }
}

/// Verifies that `ChecksumPolicy::Never` + `StatusPurpose::WardUpdate` still checksums
/// files with changed metadata, but reports them as `PossiblyModified`.
///
/// This tests a subtle interaction: ChecksumPolicy controls status *reporting*, while
/// StatusPurpose controls whether to *populate ward_entry*. When building ward files,
/// we must have correct checksums even if the policy says "don't checksum for status".
///
/// The behavior:
/// - Status is `PossiblyModified` (policy=Never means we don't confirm via checksum
///   for fingerprint/reporting purposes)
/// - But `ward_entry` contains freshly computed checksum (needed to write .treeward)
///
/// This ensures fingerprint consistency: running `status` then `ward` with the same
/// flags produces matching fingerprints, even though ward internally does more work.
#[test]
fn test_checksum_policy_never_with_ward_update_still_checksums_modified() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(root.join("file.txt"), "content").unwrap();
    let real_checksum = checksum_file(&root.join("file.txt")).unwrap();

    // Same checksum but different mtime - metadata differs but content same
    let mut entries = BTreeMap::new();
    entries.insert(
        "file.txt".to_string(),
        WardEntry::File {
            sha256: real_checksum.sha256.clone(),
            mtime_nanos: 1000,
            size: real_checksum.size,
        },
    );
    create_ward_file(root, entries);

    let result = compute_status(
        root,
        ChecksumPolicy::Never,
        StatusMode::Interesting,
        StatusPurpose::WardUpdate,
        DiffMode::None,
    )
    .unwrap();

    assert_eq!(result.statuses.len(), 1);
    // Status is PossiblyModified because ChecksumPolicy::Never means we
    // don't confirm content via checksumming for status reporting purposes
    assert_eq!(
        result.statuses[0].status_type(),
        StatusType::PossiblyModified
    );

    // But ward_entry should have the freshly computed checksum and updated mtime
    match result.statuses[0].ward_entry().unwrap() {
        WardEntry::File {
            sha256,
            size,
            mtime_nanos,
        } => {
            assert_eq!(sha256, &real_checksum.sha256);
            assert_eq!(*size, real_checksum.size);
            // mtime should be updated to current value, not the old 1000
            assert_ne!(*mtime_nanos, 1000);
        }
        _ => panic!("Expected File entry"),
    }
}

/// Verifies fingerprint consistency when both metadata AND content differ.
///
/// This is the critical test for the bug where `ChecksumPolicy::Never` +
/// `StatusPurpose::WardUpdate` would incorrectly report `Modified` instead of
/// `PossiblyModified` when content actually changed.
///
/// The scenario:
/// - `treeward status` (Display, Never): metadata differs → PossiblyModified (M?)
/// - `treeward update` (WardUpdate, Never): checksums for ward, finds content differs
///
/// Without the fix, ward would report Modified (M) because sha256_differs was
/// checked before the policy condition. This would cause fingerprint mismatch
/// between status and ward, breaking `--fingerprint` validation flows.
///
/// The fix ensures policy controls *reporting* regardless of whether we checksummed
/// internally for ward building purposes.
#[test]
fn test_checksum_policy_never_with_ward_update_content_also_differs() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    // File has different content than what's in the ward
    fs::write(root.join("file.txt"), "new content").unwrap();

    // Ward has old checksum AND old mtime - both metadata and content differ
    let mut entries = BTreeMap::new();
    entries.insert(
        "file.txt".to_string(),
        WardEntry::File {
            sha256: "old_checksum_that_doesnt_match".to_string(),
            mtime_nanos: 1000,
            size: 50,
        },
    );
    create_ward_file(root, entries);

    // With Display purpose: would report PossiblyModified (no checksumming)
    let display_result = compute_status(
        root,
        ChecksumPolicy::Never,
        StatusMode::Interesting,
        StatusPurpose::Display,
        DiffMode::None,
    )
    .unwrap();
    assert_eq!(display_result.statuses.len(), 1);
    assert_eq!(
        display_result.statuses[0].status_type(),
        StatusType::PossiblyModified
    );

    // With WardUpdate purpose: must ALSO report PossiblyModified for fingerprint consistency
    let ward_result = compute_status(
        root,
        ChecksumPolicy::Never,
        StatusMode::Interesting,
        StatusPurpose::WardUpdate,
        DiffMode::None,
    )
    .unwrap();
    assert_eq!(ward_result.statuses.len(), 1);
    assert_eq!(
        ward_result.statuses[0].status_type(),
        StatusType::PossiblyModified,
        "WardUpdate with policy=Never must report PossiblyModified even when content differs"
    );

    // Fingerprints must match
    assert_eq!(
        display_result.fingerprint, ward_result.fingerprint,
        "Fingerprints must match between Display and WardUpdate with same policy"
    );

    // But ward_entry should still have the correct (freshly computed) checksum
    let real_checksum = checksum_file(&root.join("file.txt")).unwrap();
    match ward_result.statuses[0].ward_entry().unwrap() {
        WardEntry::File { sha256, size, .. } => {
            assert_eq!(sha256, &real_checksum.sha256);
            assert_eq!(*size, real_checksum.size);
        }
        _ => panic!("Expected File entry"),
    }
}
