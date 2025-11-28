use crate::checksum::{ChecksumError, checksum_file};
use crate::dir_list::{DirListError, FsEntry, list_directory};
use crate::ward_file::{WardEntry, WardFile, WardFileError};
use base64::Engine;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Debug, thiserror::Error)]
pub enum StatusError {
    #[error("Ward file error: {0}")]
    WardFile(#[from] WardFileError),
    #[error("Directory listing error: {0}")]
    DirList(#[from] DirListError),
    #[error("Checksum error: {0}")]
    Checksum(#[from] ChecksumError),
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeType {
    Added,
    Removed,
    /// File metadata differs but a content time has NOT been confirmed
    /// through checksumming.
    PossiblyModified,
    Modified,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub path: PathBuf,
    pub change_type: ChangeType,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusResult {
    pub changes: Vec<Change>,
    /// A unique fingerprint representing the entire changeset.
    ///
    /// This is currentlyly a Base64-encoded SHA-256 but it could change
    /// in the future.
    ///
    /// See --fingerprint flag for more information.
    pub fingerprint: String,
}

/// Compare filesystem state against ward files to detect changes.
///
/// Recursively walks the directory tree starting from `root`, comparing the
/// current filesystem state against `.treeward` files to identify additions,
/// removals, and modifications.
///
/// # Arguments
///
/// * `root` - Directory to analyze (will be canonicalized)
/// * `verify_checksums` - If true, compute SHA-256 checksums to definitively
///   identify modified files. If false, only compare metadata (mtime/size).
///
/// # Returns
///
/// A `StatusResult` containing:
/// * `changes` - Sorted list of detected changes (by path)
/// * `fingerprint` - Unique identifier for this set of changes
///
/// # Change Detection
///
/// * `Added` - Entry exists in filesystem but not in ward
/// * `Removed` - Entry exists in ward but not in filesystem
/// * `PossiblyModified` - Metadata differs (only when `verify_checksums=false`)
/// * `Modified` - Content differs (checksum mismatch, symlink target changed,
///   or type changed)
///
/// # Errors
///
/// Returns error if:
/// * Ward files are corrupted or have unsupported versions
/// * Permission denied accessing files or directories
/// * File modified during checksumming (with `verify_checksums=true`)
#[allow(dead_code)]
pub fn compute_status(root: &Path, verify_checksums: bool) -> Result<StatusResult, StatusError> {
    let root = root.canonicalize().map_err(|e| {
        if e.kind() == ErrorKind::PermissionDenied {
            StatusError::DirList(DirListError::PermissionDenied(root.to_path_buf()))
        } else {
            StatusError::DirList(DirListError::Io(e))
        }
    })?;

    let mut changes = Vec::new();

    walk_directory(&root, &root, &mut changes, verify_checksums)?;

    changes.sort_by(|a, b| a.path.cmp(&b.path));

    let fingerprint = compute_fingerprint(&changes);

    Ok(StatusResult {
        changes,
        fingerprint,
    })
}

fn walk_directory(
    tree_root: &Path,
    current_dir: &Path,
    changes: &mut Vec<Change>,
    verify_checksums: bool,
) -> Result<(), StatusError> {
    let ward_path = current_dir.join(".treeward");
    let ward_file = try_load_ward_file(&ward_path)?;
    let ward_entries = ward_file.map(|wf| wf.entries).unwrap_or_else(BTreeMap::new);

    let fs_entries = match list_directory(current_dir) {
        Ok(entries) => entries,
        Err(DirListError::Io(e)) if e.kind() == ErrorKind::NotFound => BTreeMap::new(),
        Err(e) => return Err(StatusError::DirList(e)),
    };

    compare_entries(
        tree_root,
        current_dir,
        &ward_entries,
        &fs_entries,
        changes,
        verify_checksums,
    )?;

    for (name, entry) in &fs_entries {
        if matches!(entry, FsEntry::Dir { .. }) {
            let child_path = current_dir.join(name);
            walk_directory(tree_root, &child_path, changes, verify_checksums)?;
        }
    }

    for (name, entry) in &ward_entries {
        if matches!(entry, WardEntry::Dir {}) && !fs_entries.contains_key(name) {
            let child_path = current_dir.join(name);
            walk_directory(tree_root, &child_path, changes, verify_checksums)?;
        }
    }

    Ok(())
}

fn compare_entries(
    tree_root: &Path,
    current_dir: &Path,
    ward_entries: &BTreeMap<String, WardEntry>,
    fs_entries: &BTreeMap<String, FsEntry>,
    changes: &mut Vec<Change>,
    verify_checksums: bool,
) -> Result<(), StatusError> {
    for name in fs_entries.keys() {
        if !ward_entries.contains_key(name) {
            let relative_path = current_dir.strip_prefix(tree_root).unwrap().join(name);
            changes.push(Change {
                path: relative_path,
                change_type: ChangeType::Added,
            });
        }
    }

    for name in ward_entries.keys() {
        if !fs_entries.contains_key(name) {
            let relative_path = current_dir.strip_prefix(tree_root).unwrap().join(name);
            changes.push(Change {
                path: relative_path,
                change_type: ChangeType::Removed,
            });
        }
    }

    for (name, ward_entry) in ward_entries {
        if let Some(fs_entry) = fs_entries.get(name) {
            check_modification(
                tree_root,
                current_dir,
                name,
                ward_entry,
                fs_entry,
                changes,
                verify_checksums,
            )?;
        }
    }

    Ok(())
}

fn check_modification(
    tree_root: &Path,
    current_dir: &Path,
    name: &str,
    ward_entry: &WardEntry,
    fs_entry: &FsEntry,
    changes: &mut Vec<Change>,
    verify_checksums: bool,
) -> Result<(), StatusError> {
    let relative_path = current_dir.strip_prefix(tree_root).unwrap().join(name);
    let absolute_path = current_dir.join(name);

    match (ward_entry, fs_entry) {
        (
            WardEntry::File {
                sha256: ward_sha,
                mtime_nanos: ward_mtime_nanos,
                size: ward_size,
            },
            FsEntry::File {
                mtime: fs_mtime,
                size: fs_size,
            },
        ) => {
            let fs_mtime_nanos = fs_mtime.duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64;

            if fs_mtime_nanos != *ward_mtime_nanos || fs_size != ward_size {
                if verify_checksums {
                    let checksum = checksum_file(&absolute_path)?;
                    if &checksum.sha256 != ward_sha {
                        changes.push(Change {
                            path: relative_path,
                            change_type: ChangeType::Modified,
                        });
                    }
                } else {
                    changes.push(Change {
                        path: relative_path,
                        change_type: ChangeType::PossiblyModified,
                    });
                }
            }
        }
        (WardEntry::Dir {}, FsEntry::Dir { .. }) => {}
        (
            WardEntry::Symlink {
                symlink_target: ward_target,
            },
            FsEntry::Symlink {
                symlink_target: fs_target,
            },
        ) => {
            if ward_target != fs_target {
                changes.push(Change {
                    path: relative_path,
                    change_type: ChangeType::Modified,
                });
            }
        }
        _ => {
            changes.push(Change {
                path: relative_path,
                change_type: ChangeType::Modified,
            });
        }
    }

    Ok(())
}

fn compute_fingerprint(changes: &[Change]) -> String {
    let mut hasher = Sha256::new();

    for change in changes {
        // TODO: re-consider lossy here and what to do instead
        hasher.update(change.path.to_string_lossy().as_bytes());
        hasher.update(b"|");

        let change_type_str = match change.change_type {
            ChangeType::Added => "A",
            ChangeType::Removed => "R",
            ChangeType::PossiblyModified => "M?",
            ChangeType::Modified => "M",
        };
        hasher.update(change_type_str.as_bytes());
        hasher.update(b"\n");
    }

    let hash_bytes = hasher.finalize();
    base64::engine::general_purpose::STANDARD.encode(hash_bytes)
}

fn try_load_ward_file(path: &Path) -> Result<Option<WardFile>, WardFileError> {
    match WardFile::load(path) {
        Ok(wf) => Ok(Some(wf)),
        Err(WardFileError::Io(e)) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ward_file::WardEntry;
    use std::fs;
    use std::os::unix;
    use tempfile::TempDir;

    fn create_ward_file(dir: &Path, entries: BTreeMap<String, WardEntry>) {
        let ward = WardFile::new(entries);
        ward.save(&dir.join(".treeward")).unwrap();
    }

    #[test]
    fn test_no_changes() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        fs::write(root.join("file1.txt"), "content1").unwrap();
        fs::create_dir(root.join("dir1")).unwrap();
        fs::write(root.join("dir1/file2.txt"), "content2").unwrap();

        let checksum1 = checksum_file(&root.join("file1.txt")).unwrap();
        let checksum2 = checksum_file(&root.join("dir1/file2.txt")).unwrap();

        let mut root_entries = BTreeMap::new();
        root_entries.insert(
            "file1.txt".to_string(),
            WardEntry::File {
                sha256: checksum1.sha256.clone(),
                mtime_nanos: checksum1
                    .mtime
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos() as u64,
                size: checksum1.size,
            },
        );
        root_entries.insert("dir1".to_string(), WardEntry::Dir {});
        create_ward_file(root, root_entries);

        let mut dir1_entries = BTreeMap::new();
        dir1_entries.insert(
            "file2.txt".to_string(),
            WardEntry::File {
                sha256: checksum2.sha256.clone(),
                mtime_nanos: checksum2
                    .mtime
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos() as u64,
                size: checksum2.size,
            },
        );
        create_ward_file(&root.join("dir1"), dir1_entries);

        let result = compute_status(root, false).unwrap();
        assert_eq!(result.changes.len(), 0);

        let result2 = compute_status(root, false).unwrap();
        assert_eq!(result.fingerprint, result2.fingerprint);
    }

    #[test]
    fn test_added_files() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        create_ward_file(root, BTreeMap::new());

        fs::write(root.join("file1.txt"), "content1").unwrap();

        let result = compute_status(root, false).unwrap();
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].path, PathBuf::from("file1.txt"));
        assert_eq!(result.changes[0].change_type, ChangeType::Added);
    }

    #[test]
    fn test_added_directory() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        create_ward_file(root, BTreeMap::new());

        fs::create_dir(root.join("dir1")).unwrap();
        fs::write(root.join("dir1/file1.txt"), "content").unwrap();

        let result = compute_status(root, false).unwrap();
        assert_eq!(result.changes.len(), 2);

        let paths: Vec<PathBuf> = result.changes.iter().map(|c| c.path.clone()).collect();
        assert!(paths.contains(&PathBuf::from("dir1")));
        assert!(paths.contains(&PathBuf::from("dir1/file1.txt")));

        for change in &result.changes {
            assert_eq!(change.change_type, ChangeType::Added);
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

        let result = compute_status(root, false).unwrap();
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].path, PathBuf::from("file1.txt"));
        assert_eq!(result.changes[0].change_type, ChangeType::Removed);
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

        let result = compute_status(root, false).unwrap();
        assert_eq!(result.changes.len(), 1);

        assert_eq!(result.changes[0].path, PathBuf::from("dir1"));
        assert_eq!(result.changes[0].change_type, ChangeType::Removed);
    }

    #[test]
    fn test_modified_file_without_verify() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        fs::write(root.join("file1.txt"), "content1").unwrap();
        let checksum = checksum_file(&root.join("file1.txt")).unwrap();

        let mut entries = BTreeMap::new();
        entries.insert(
            "file1.txt".to_string(),
            WardEntry::File {
                sha256: checksum.sha256,
                mtime_nanos: 1000,
                size: checksum.size,
            },
        );
        create_ward_file(root, entries);

        let result = compute_status(root, false).unwrap();
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].path, PathBuf::from("file1.txt"));
        assert_eq!(result.changes[0].change_type, ChangeType::PossiblyModified);
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

        let result = compute_status(root, true).unwrap();
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].path, PathBuf::from("file1.txt"));
        assert_eq!(result.changes[0].change_type, ChangeType::Modified);
    }

    #[test]
    fn test_modified_file_with_verify_unchanged() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        fs::write(root.join("file1.txt"), "content1").unwrap();
        let checksum = checksum_file(&root.join("file1.txt")).unwrap();

        let mut entries = BTreeMap::new();
        entries.insert(
            "file1.txt".to_string(),
            WardEntry::File {
                sha256: checksum.sha256,
                mtime_nanos: 1000,
                size: checksum.size,
            },
        );
        create_ward_file(root, entries);

        let result = compute_status(root, true).unwrap();
        assert_eq!(result.changes.len(), 0);
    }

    #[test]
    fn test_fingerprint_deterministic() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        create_ward_file(root, BTreeMap::new());

        fs::write(root.join("file1.txt"), "content").unwrap();

        let result1 = compute_status(root, false).unwrap();
        let result2 = compute_status(root, false).unwrap();

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

        let result1 = compute_status(root1, false).unwrap();
        let result2 = compute_status(root2, false).unwrap();

        assert_ne!(result1.fingerprint, result2.fingerprint);
    }

    #[test]
    fn test_mixed_changes() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        fs::write(root.join("file1.txt"), "content1").unwrap();
        fs::write(root.join("file4.txt"), "new file").unwrap();

        let checksum1 = checksum_file(&root.join("file1.txt")).unwrap();

        let mut entries = BTreeMap::new();
        entries.insert(
            "file1.txt".to_string(),
            WardEntry::File {
                sha256: checksum1.sha256,
                mtime_nanos: 1000,
                size: checksum1.size,
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

        let result = compute_status(root, false).unwrap();
        assert_eq!(result.changes.len(), 3);

        let change_types: BTreeMap<PathBuf, ChangeType> = result
            .changes
            .iter()
            .map(|c| (c.path.clone(), c.change_type))
            .collect();

        assert_eq!(
            change_types.get(&PathBuf::from("file1.txt")),
            Some(&ChangeType::PossiblyModified)
        );
        assert_eq!(
            change_types.get(&PathBuf::from("file2.txt")),
            Some(&ChangeType::Removed)
        );
        assert_eq!(
            change_types.get(&PathBuf::from("file4.txt")),
            Some(&ChangeType::Added)
        );

        let paths: Vec<PathBuf> = result.changes.iter().map(|c| c.path.clone()).collect();
        assert_eq!(
            paths,
            vec![
                PathBuf::from("file1.txt"),
                PathBuf::from("file2.txt"),
                PathBuf::from("file4.txt")
            ]
        );
    }

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

        let result = compute_status(root, false).unwrap();

        let link_change = result
            .changes
            .iter()
            .find(|c| c.path == PathBuf::from("link"));
        assert!(link_change.is_some());
        assert_eq!(link_change.unwrap().change_type, ChangeType::Modified);
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

        let mut dir3_entries = BTreeMap::new();
        dir3_entries.insert(
            "file.txt".to_string(),
            WardEntry::File {
                sha256: checksum.sha256,
                mtime_nanos: 1000,
                size: checksum.size,
            },
        );
        create_ward_file(&root.join("dir1/dir2/dir3"), dir3_entries);

        let result = compute_status(root, false).unwrap();

        let file_change = result
            .changes
            .iter()
            .find(|c| c.path == PathBuf::from("dir1/dir2/dir3/file.txt"));
        assert!(file_change.is_some());
        assert_eq!(
            file_change.unwrap().change_type,
            ChangeType::PossiblyModified
        );
    }

    #[test]
    fn test_uninitialized_directory() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        fs::write(root.join("file1.txt"), "content1").unwrap();
        fs::write(root.join("file2.txt"), "content2").unwrap();
        fs::create_dir(root.join("dir1")).unwrap();

        let result = compute_status(root, false).unwrap();
        assert_eq!(result.changes.len(), 3);

        for change in &result.changes {
            assert_eq!(change.change_type, ChangeType::Added);
        }
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

        let result = compute_status(root, false).unwrap();

        let item_change = result
            .changes
            .iter()
            .find(|c| c.path == PathBuf::from("item"));
        assert!(item_change.is_some());
        assert_eq!(item_change.unwrap().change_type, ChangeType::Modified);
    }
}
