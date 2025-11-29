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
pub enum StatusType {
    Added,
    Removed,
    /// File metadata differs but a content time has NOT been confirmed
    /// through checksumming.
    PossiblyModified,
    Modified,
    Unchanged,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub path: PathBuf,
    pub status_type: StatusType,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksumPolicy {
    /// Never compute checksums. Files with differing metadata will be
    /// reported as PossiblyModified.
    Never,

    /// Compute checksums only for files that appear possibly modified
    /// (mtime or size differs from ward). This upgrades PossiblyModified
    /// to either Modified (checksum differs) or no change (checksum matches).
    WhenPossiblyModified,

    /// Always compute checksums for all files in the ward, even if metadata
    /// matches. This can detect silent corruption or metadata manipulation.
    Always,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusMode {
    /// Only include files with interesting changes (added, removed, modified, possibly modified)
    Interesting,

    /// Include all files, including unchanged ones
    All,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusResult {
    pub statuses: Vec<Status>,
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
/// * `policy` - Controls when checksums are computed:
///   - `Never`: Only compare metadata (mtime/size)
///   - `WhenPossiblyModified`: Checksum files with differing metadata
///   - `Always`: Checksum all files (detects silent corruption)
/// * `mode` - Controls which files are included in results:
///   - `Interesting`: Only include changed files
///   - `All`: Include all files, even unchanged ones
///
/// # Returns
///
/// A `StatusResult` containing:
/// * `statuses` - Sorted list of file statuses (by path)
/// * `fingerprint` - Unique identifier for this set of changes
///
/// # Change Detection
///
/// * `Added` - Entry exists in filesystem but not in ward
/// * `Removed` - Entry exists in ward but not in filesystem
/// * `PossiblyModified` - Metadata differs (only with `ChecksumPolicy::Never`)
/// * `Modified` - Content differs (checksum mismatch, symlink target changed,
///   or type changed)
/// * `Unchanged` - Entry exists in both and matches (only with `StatusMode::All`)
///
/// # Errors
///
/// Returns error if:
/// * Ward files are corrupted or have unsupported versions
/// * Permission denied accessing files or directories
/// * File modified during checksumming
#[allow(dead_code)]
pub fn compute_status(
    root: &Path,
    policy: ChecksumPolicy,
    mode: StatusMode,
) -> Result<StatusResult, StatusError> {
    let root = root.canonicalize().map_err(|e| {
        if e.kind() == ErrorKind::PermissionDenied {
            StatusError::DirList(DirListError::PermissionDenied(root.to_path_buf()))
        } else {
            StatusError::DirList(DirListError::Io(e))
        }
    })?;

    let mut statuses = Vec::new();

    walk_directory(&root, &root, &mut statuses, policy, mode)?;

    statuses.sort_by(|a, b| a.path.cmp(&b.path));

    let fingerprint = compute_fingerprint(&statuses);

    Ok(StatusResult {
        statuses,
        fingerprint,
    })
}

fn walk_directory(
    tree_root: &Path,
    current_dir: &Path,
    statuses: &mut Vec<Status>,
    policy: ChecksumPolicy,
    mode: StatusMode,
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
        statuses,
        policy,
        mode,
    )?;

    for (name, entry) in &fs_entries {
        if matches!(entry, FsEntry::Dir { .. }) {
            let child_path = current_dir.join(name);
            walk_directory(tree_root, &child_path, statuses, policy, mode)?;
        }
    }

    for (name, entry) in &ward_entries {
        if matches!(entry, WardEntry::Dir {}) && !fs_entries.contains_key(name) {
            let child_path = current_dir.join(name);
            walk_directory(tree_root, &child_path, statuses, policy, mode)?;
        }
    }

    Ok(())
}

fn compare_entries(
    tree_root: &Path,
    current_dir: &Path,
    ward_entries: &BTreeMap<String, WardEntry>,
    fs_entries: &BTreeMap<String, FsEntry>,
    statuses: &mut Vec<Status>,
    policy: ChecksumPolicy,
    mode: StatusMode,
) -> Result<(), StatusError> {
    for name in fs_entries.keys() {
        if !ward_entries.contains_key(name) {
            let relative_path = current_dir.strip_prefix(tree_root).unwrap().join(name);
            statuses.push(Status {
                path: relative_path,
                status_type: StatusType::Added,
            });
        }
    }

    for name in ward_entries.keys() {
        if !fs_entries.contains_key(name) {
            let relative_path = current_dir.strip_prefix(tree_root).unwrap().join(name);
            statuses.push(Status {
                path: relative_path,
                status_type: StatusType::Removed,
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
                statuses,
                policy,
                mode,
            )?;
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn check_modification(
    tree_root: &Path,
    current_dir: &Path,
    name: &str,
    ward_entry: &WardEntry,
    fs_entry: &FsEntry,
    statuses: &mut Vec<Status>,
    policy: ChecksumPolicy,
    mode: StatusMode,
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
            let metadata_differs = fs_mtime_nanos != *ward_mtime_nanos || fs_size != ward_size;

            let should_checksum = match policy {
                ChecksumPolicy::Never => false,
                ChecksumPolicy::WhenPossiblyModified => metadata_differs,
                ChecksumPolicy::Always => true,
            };

            if should_checksum {
                let checksum = checksum_file(&absolute_path)?;
                if &checksum.sha256 != ward_sha {
                    statuses.push(Status {
                        path: relative_path,
                        status_type: StatusType::Modified,
                    });
                } else if mode == StatusMode::All {
                    statuses.push(Status {
                        path: relative_path,
                        status_type: StatusType::Unchanged,
                    });
                }
            } else if metadata_differs {
                statuses.push(Status {
                    path: relative_path,
                    status_type: StatusType::PossiblyModified,
                });
            } else if mode == StatusMode::All {
                statuses.push(Status {
                    path: relative_path,
                    status_type: StatusType::Unchanged,
                });
            }
        }
        (WardEntry::Dir {}, FsEntry::Dir { .. }) => {
            if mode == StatusMode::All {
                statuses.push(Status {
                    path: relative_path,
                    status_type: StatusType::Unchanged,
                });
            }
        }
        (
            WardEntry::Symlink {
                symlink_target: ward_target,
            },
            FsEntry::Symlink {
                symlink_target: fs_target,
            },
        ) => {
            if ward_target != fs_target {
                statuses.push(Status {
                    path: relative_path,
                    status_type: StatusType::Modified,
                });
            } else if mode == StatusMode::All {
                statuses.push(Status {
                    path: relative_path,
                    status_type: StatusType::Unchanged,
                });
            }
        }
        _ => {
            statuses.push(Status {
                path: relative_path,
                status_type: StatusType::Modified,
            });
        }
    }

    Ok(())
}

fn compute_fingerprint(statuses: &[Status]) -> String {
    let mut hasher = Sha256::new();

    for status in statuses {
        if status.status_type == StatusType::Unchanged {
            continue;
        }

        // TODO: re-consider lossy here and what to do instead
        hasher.update(status.path.to_string_lossy().as_bytes());
        hasher.update(b"|");

        let status_type_str = match status.status_type {
            StatusType::Added => "A",
            StatusType::Removed => "R",
            StatusType::PossiblyModified => "M?",
            StatusType::Modified => "M",
            StatusType::Unchanged => unreachable!(),
        };
        hasher.update(status_type_str.as_bytes());
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

        let result = compute_status(root, ChecksumPolicy::Never, StatusMode::Interesting).unwrap();
        assert_eq!(result.statuses.len(), 0);

        let result2 = compute_status(root, ChecksumPolicy::Never, StatusMode::Interesting).unwrap();
        assert_eq!(result.fingerprint, result2.fingerprint);
    }

    #[test]
    fn test_added_files() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        create_ward_file(root, BTreeMap::new());

        fs::write(root.join("file1.txt"), "content1").unwrap();

        let result = compute_status(root, ChecksumPolicy::Never, StatusMode::Interesting).unwrap();
        assert_eq!(result.statuses.len(), 1);
        assert_eq!(result.statuses[0].path, PathBuf::from("file1.txt"));
        assert_eq!(result.statuses[0].status_type, StatusType::Added);
    }

    #[test]
    fn test_added_directory() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        create_ward_file(root, BTreeMap::new());

        fs::create_dir(root.join("dir1")).unwrap();
        fs::write(root.join("dir1/file1.txt"), "content").unwrap();

        let result = compute_status(root, ChecksumPolicy::Never, StatusMode::Interesting).unwrap();
        assert_eq!(result.statuses.len(), 2);

        let paths: Vec<PathBuf> = result.statuses.iter().map(|c| c.path.clone()).collect();
        assert!(paths.contains(&PathBuf::from("dir1")));
        assert!(paths.contains(&PathBuf::from("dir1/file1.txt")));

        for change in &result.statuses {
            assert_eq!(change.status_type, StatusType::Added);
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

        let result = compute_status(root, ChecksumPolicy::Never, StatusMode::Interesting).unwrap();
        assert_eq!(result.statuses.len(), 1);
        assert_eq!(result.statuses[0].path, PathBuf::from("file1.txt"));
        assert_eq!(result.statuses[0].status_type, StatusType::Removed);
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

        let result = compute_status(root, ChecksumPolicy::Never, StatusMode::Interesting).unwrap();
        assert_eq!(result.statuses.len(), 1);

        assert_eq!(result.statuses[0].path, PathBuf::from("dir1"));
        assert_eq!(result.statuses[0].status_type, StatusType::Removed);
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

        let result = compute_status(root, ChecksumPolicy::Never, StatusMode::Interesting).unwrap();
        assert_eq!(result.statuses.len(), 1);
        assert_eq!(result.statuses[0].path, PathBuf::from("file1.txt"));
        assert_eq!(result.statuses[0].status_type, StatusType::PossiblyModified);
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
        )
        .unwrap();
        assert_eq!(result.statuses.len(), 1);
        assert_eq!(result.statuses[0].path, PathBuf::from("file1.txt"));
        assert_eq!(result.statuses[0].status_type, StatusType::Modified);
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
        )
        .unwrap();
        assert_eq!(result.statuses.len(), 0);
    }

    #[test]
    fn test_fingerprint_deterministic() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        create_ward_file(root, BTreeMap::new());

        fs::write(root.join("file1.txt"), "content").unwrap();

        let result1 = compute_status(root, ChecksumPolicy::Never, StatusMode::Interesting).unwrap();
        let result2 = compute_status(root, ChecksumPolicy::Never, StatusMode::Interesting).unwrap();

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

        let result1 =
            compute_status(root1, ChecksumPolicy::Never, StatusMode::Interesting).unwrap();
        let result2 =
            compute_status(root2, ChecksumPolicy::Never, StatusMode::Interesting).unwrap();

        assert_ne!(result1.fingerprint, result2.fingerprint);
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

        let result = compute_status(root, ChecksumPolicy::Never, StatusMode::Interesting).unwrap();
        assert_eq!(result.statuses.len(), 3);

        let change_types: BTreeMap<PathBuf, StatusType> = result
            .statuses
            .iter()
            .map(|c| (c.path.clone(), c.status_type))
            .collect();

        assert_eq!(
            change_types.get(&PathBuf::from("file1.txt")),
            Some(&StatusType::PossiblyModified)
        );
        assert_eq!(
            change_types.get(&PathBuf::from("file2.txt")),
            Some(&StatusType::Removed)
        );
        assert_eq!(
            change_types.get(&PathBuf::from("file4.txt")),
            Some(&StatusType::Added)
        );

        let paths: Vec<PathBuf> = result.statuses.iter().map(|c| c.path.clone()).collect();
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

        let result = compute_status(root, ChecksumPolicy::Never, StatusMode::Interesting).unwrap();

        let link_change = result
            .statuses
            .iter()
            .find(|c| c.path == PathBuf::from("link"));
        assert!(link_change.is_some());
        assert_eq!(link_change.unwrap().status_type, StatusType::Modified);
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

        let result = compute_status(root, ChecksumPolicy::Never, StatusMode::Interesting).unwrap();

        let file_change = result
            .statuses
            .iter()
            .find(|c| c.path == PathBuf::from("dir1/dir2/dir3/file.txt"));
        assert!(file_change.is_some());
        assert_eq!(
            file_change.unwrap().status_type,
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

        let result = compute_status(root, ChecksumPolicy::Never, StatusMode::Interesting).unwrap();
        assert_eq!(result.statuses.len(), 3);

        for change in &result.statuses {
            assert_eq!(change.status_type, StatusType::Added);
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

        let result = compute_status(root, ChecksumPolicy::Never, StatusMode::Interesting).unwrap();

        let item_change = result
            .statuses
            .iter()
            .find(|c| c.path == PathBuf::from("item"));
        assert!(item_change.is_some());
        assert_eq!(item_change.unwrap().status_type, StatusType::Modified);
    }

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

        let result = compute_status(root, ChecksumPolicy::Never, StatusMode::Interesting).unwrap();
        assert_eq!(result.statuses.len(), 1);
        assert_eq!(result.statuses[0].path, PathBuf::from("file1.txt"));
        assert_eq!(result.statuses[0].status_type, StatusType::PossiblyModified);
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
        )
        .unwrap();
        assert_eq!(result.statuses.len(), 1);
        assert_eq!(result.statuses[0].path, PathBuf::from("file1.txt"));
        assert_eq!(result.statuses[0].status_type, StatusType::Modified);
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

        let result = compute_status(root, ChecksumPolicy::Always, StatusMode::Interesting).unwrap();
        assert_eq!(result.statuses.len(), 1);
        assert_eq!(result.statuses[0].path, PathBuf::from("file1.txt"));
        assert_eq!(result.statuses[0].status_type, StatusType::Modified);
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

        let result = compute_status(root, ChecksumPolicy::Always, StatusMode::Interesting).unwrap();
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
        )
        .unwrap();
        assert_eq!(result.statuses.len(), 0);
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

        let result = compute_status(root, ChecksumPolicy::Never, StatusMode::All).unwrap();
        assert_eq!(result.statuses.len(), 2);

        for change in &result.statuses {
            assert_eq!(change.status_type, StatusType::Unchanged);
        }

        let paths: Vec<PathBuf> = result.statuses.iter().map(|c| c.path.clone()).collect();
        assert!(paths.contains(&PathBuf::from("file1.txt")));
        assert!(paths.contains(&PathBuf::from("file2.txt")));
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

        let result = compute_status(root, ChecksumPolicy::Never, StatusMode::All).unwrap();
        assert_eq!(result.statuses.len(), 4);

        let change_types: BTreeMap<PathBuf, StatusType> = result
            .statuses
            .iter()
            .map(|c| (c.path.clone(), c.status_type))
            .collect();

        assert_eq!(
            change_types.get(&PathBuf::from("unchanged.txt")),
            Some(&StatusType::Unchanged)
        );
        assert_eq!(
            change_types.get(&PathBuf::from("modified.txt")),
            Some(&StatusType::PossiblyModified)
        );
        assert_eq!(
            change_types.get(&PathBuf::from("added.txt")),
            Some(&StatusType::Added)
        );
        assert_eq!(
            change_types.get(&PathBuf::from("removed.txt")),
            Some(&StatusType::Removed)
        );
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

        let result_interesting =
            compute_status(root, ChecksumPolicy::Never, StatusMode::Interesting).unwrap();
        let result_all = compute_status(root, ChecksumPolicy::Never, StatusMode::All).unwrap();

        assert_eq!(result_interesting.statuses.len(), 0);
        assert_eq!(result_all.statuses.len(), 1);
        assert_eq!(result_all.statuses[0].status_type, StatusType::Unchanged);

        assert_eq!(result_interesting.fingerprint, result_all.fingerprint);
    }
}
