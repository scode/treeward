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
    #[error("{0}")]
    Other(String),
}

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

/// Represents the status of a single filesystem entry compared to its ward state.
///
/// `StatusEntry` is the result of comparing a filesystem entry against the corresponding
/// `.treeward` file. Each variant captures a different relationship between the current
/// filesystem state and the recorded ward state.
///
/// # Relationship to `WardEntry`
///
/// Some variants carry an `Option<WardEntry>` field. This represents the complete ward
/// data (checksum, metadata) for the entry, but it is only populated when the caller
/// needs it - specifically when `StatusPurpose::WardUpdate` is used.
///
/// The `Option` exists because computing a `WardEntry` for files requires checksumming,
/// which is expensive. When status is computed for display purposes only
/// (`StatusPurpose::Display`), we skip checksumming and set `ward_entry: None`.
/// When status is computed to update ward files (`StatusPurpose::WardUpdate`), we
/// compute the full checksum and populate `ward_entry: Some(...)`.
///
/// This ensures that `WardEntry` is always complete when present - it never
/// contains placeholder data like empty checksums.
///
/// # Variants
///
/// - `Added`: Entry exists in filesystem but not in ward. The `ward_entry` contains
///   the new entry data to be written (if `WardUpdate` purpose).
///
/// - `Removed`: Entry exists in ward but not in filesystem. No `ward_entry` is needed
///   since the entry should be removed from the ward file.
///
/// - `Modified`: Entry exists in both but differs. The `ward_entry` contains the
///   updated entry data reflecting the current filesystem state (if `WardUpdate` purpose).
///
/// - `PossiblyModified`: Metadata differs but content was not checksummed (only occurs
///   with `ChecksumPolicy::Never`). No `ward_entry` since we don't know the true state.
///
/// - `Unchanged`: Entry exists in both and matches. The `ward_entry` contains the
///   current entry data (if `WardUpdate` purpose), which may have updated metadata
///   even if content is unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusEntry {
    Added {
        path: PathBuf,
        ward_entry: Option<WardEntry>,
    },
    Removed {
        path: PathBuf,
    },
    Modified {
        path: PathBuf,
        ward_entry: Option<WardEntry>,
    },
    PossiblyModified {
        path: PathBuf,
    },
    Unchanged {
        path: PathBuf,
        ward_entry: Option<WardEntry>,
    },
}

impl StatusEntry {
    pub fn path(&self) -> &Path {
        match self {
            StatusEntry::Added { path, .. } => path,
            StatusEntry::Removed { path } => path,
            StatusEntry::Modified { path, .. } => path,
            StatusEntry::PossiblyModified { path } => path,
            StatusEntry::Unchanged { path, .. } => path,
        }
    }

    pub fn ward_entry(&self) -> Option<&WardEntry> {
        match self {
            StatusEntry::Added { ward_entry, .. } => ward_entry.as_ref(),
            StatusEntry::Modified { ward_entry, .. } => ward_entry.as_ref(),
            StatusEntry::Unchanged { ward_entry, .. } => ward_entry.as_ref(),
            StatusEntry::Removed { .. } => None,
            StatusEntry::PossiblyModified { .. } => None,
        }
    }

    pub fn status_type(&self) -> StatusType {
        match self {
            StatusEntry::Added { .. } => StatusType::Added,
            StatusEntry::Removed { .. } => StatusType::Removed,
            StatusEntry::Modified { .. } => StatusType::Modified,
            StatusEntry::PossiblyModified { .. } => StatusType::PossiblyModified,
            StatusEntry::Unchanged { .. } => StatusType::Unchanged,
        }
    }
}

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

/// Controls whether `StatusEntry` variants include complete `WardEntry` data.
///
/// This is orthogonal to `ChecksumPolicy` - the policy controls *when* checksums
/// are computed, while purpose controls *whether* to populate `ward_entry` fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusPurpose {
    /// Display status to user.
    ///
    /// `StatusEntry` variants will have `ward_entry: None`. Checksumming is
    /// controlled entirely by `ChecksumPolicy` (to determine Modified vs
    /// PossiblyModified), but the checksum result is not retained.
    Display,

    /// Generate ward files - populate `ward_entry` with complete data.
    ///
    /// All `StatusEntry` variants that can carry a `WardEntry` will have
    /// `ward_entry: Some(...)` with complete data (including checksums).
    ///
    /// Checksum computation still respects `ChecksumPolicy`:
    /// - `Always`: Checksum every file (detects silent corruption)
    /// - `WhenPossiblyModified`: Checksum only if metadata differs, otherwise
    ///   reuse the existing checksum from the ward file
    /// - `Never`: Checksum only if metadata differs (to get correct data for
    ///   the ward file), otherwise reuse existing checksum
    ///
    /// The checksum reuse optimization only applies when metadata matches,
    /// allowing efficient incremental updates while still detecting changes.
    WardUpdate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusMode {
    /// Only include files with interesting changes (added, removed, modified, possibly modified)
    Interesting,

    /// Include all files, including unchanged ones
    All,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusResult {
    pub statuses: Vec<StatusEntry>,
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
/// * `purpose` - Controls whether to generate complete ward entries:
///   - `Display`: Only checksum based on policy (for user display)
///   - `WardUpdate`: Always provide complete ward entries, reusing checksums when possible
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
pub fn compute_status(
    root: &Path,
    policy: ChecksumPolicy,
    mode: StatusMode,
    purpose: StatusPurpose,
) -> Result<StatusResult, StatusError> {
    let root = root.canonicalize().map_err(|e| {
        if e.kind() == ErrorKind::PermissionDenied {
            StatusError::DirList(DirListError::PermissionDenied(root.to_path_buf()))
        } else {
            StatusError::DirList(DirListError::Io(e))
        }
    })?;

    let mut statuses = Vec::new();

    walk_directory(&root, &root, &mut statuses, policy, mode, purpose)?;

    statuses.sort_by(|a, b| a.path().cmp(b.path()));

    let fingerprint = compute_fingerprint(&statuses)?;

    Ok(StatusResult {
        statuses,
        fingerprint,
    })
}

fn walk_directory(
    tree_root: &Path,
    current_dir: &Path,
    statuses: &mut Vec<StatusEntry>,
    policy: ChecksumPolicy,
    mode: StatusMode,
    purpose: StatusPurpose,
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
        purpose,
    )?;

    for (name, entry) in &fs_entries {
        if matches!(entry, FsEntry::Dir { .. }) {
            let child_path = current_dir.join(name);
            walk_directory(tree_root, &child_path, statuses, policy, mode, purpose)?;
        }
    }

    for (name, entry) in &ward_entries {
        if matches!(entry, WardEntry::Dir {}) && !fs_entries.contains_key(name) {
            let child_path = current_dir.join(name);
            walk_directory(tree_root, &child_path, statuses, policy, mode, purpose)?;
        }
    }

    Ok(())
}

fn mtime_to_nanos(mtime: &std::time::SystemTime) -> Result<u64, StatusError> {
    mtime
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .map_err(|_| StatusError::Other("mtime is before UNIX epoch".to_string()))
}

fn build_ward_entry_from_fs(
    dir: &Path,
    name: &str,
    fs_entry: &FsEntry,
) -> Result<WardEntry, StatusError> {
    match fs_entry {
        FsEntry::File { .. } => {
            let path = dir.join(name);
            let checksum = checksum_file(&path)?;

            Ok(WardEntry::File {
                sha256: checksum.sha256,
                mtime_nanos: mtime_to_nanos(&checksum.mtime)?,
                size: checksum.size,
            })
        }
        FsEntry::Dir { .. } => Ok(WardEntry::Dir {}),
        FsEntry::Symlink { symlink_target, .. } => Ok(WardEntry::Symlink {
            symlink_target: symlink_target.clone(),
        }),
    }
}

#[allow(clippy::too_many_arguments)]
fn compare_entries(
    tree_root: &Path,
    current_dir: &Path,
    ward_entries: &BTreeMap<String, WardEntry>,
    fs_entries: &BTreeMap<String, FsEntry>,
    statuses: &mut Vec<StatusEntry>,
    policy: ChecksumPolicy,
    mode: StatusMode,
    purpose: StatusPurpose,
) -> Result<(), StatusError> {
    for name in fs_entries.keys() {
        if !ward_entries.contains_key(name) {
            let relative_path = current_dir.strip_prefix(tree_root).unwrap().join(name);
            let fs_entry = &fs_entries[name];

            let ward_entry = if purpose == StatusPurpose::WardUpdate {
                Some(build_ward_entry_from_fs(current_dir, name, fs_entry)?)
            } else {
                None
            };

            statuses.push(StatusEntry::Added {
                path: relative_path,
                ward_entry,
            });
        }
    }

    for name in ward_entries.keys() {
        if !fs_entries.contains_key(name) {
            let relative_path = current_dir.strip_prefix(tree_root).unwrap().join(name);
            statuses.push(StatusEntry::Removed {
                path: relative_path,
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
                purpose,
            )?;
        }
    }

    Ok(())
}

/// Compares a single entry that exists in both the ward file and filesystem.
///
/// Determines whether the entry is Modified, PossiblyModified, or Unchanged by
/// comparing the ward entry against the current filesystem state. For files,
/// this involves metadata comparison and optionally checksumming based on policy.
/// For directories and symlinks, comparison is simpler (dirs always match,
/// symlinks compare targets).
///
/// Type changes (e.g., file becoming symlink) are always reported as Modified.
///
/// Appends the resulting `StatusEntry` to `statuses` based on `mode` and `purpose`.
#[allow(clippy::too_many_arguments)]
fn check_modification(
    tree_root: &Path,
    current_dir: &Path,
    name: &str,
    ward_entry: &WardEntry,
    fs_entry: &FsEntry,
    statuses: &mut Vec<StatusEntry>,
    policy: ChecksumPolicy,
    mode: StatusMode,
    purpose: StatusPurpose,
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
            let fs_mtime_nanos = mtime_to_nanos(fs_mtime)?;
            let metadata_differs = fs_mtime_nanos != *ward_mtime_nanos || fs_size != ward_size;

            let need_checksum_for_status = match policy {
                ChecksumPolicy::Never => false,
                ChecksumPolicy::WhenPossiblyModified => metadata_differs,
                ChecksumPolicy::Always => true,
            };
            let need_checksum_for_ward = purpose == StatusPurpose::WardUpdate && metadata_differs;
            let need_checksum = need_checksum_for_status || need_checksum_for_ward;

            let (sha256_differs, new_checksum) = if need_checksum {
                let checksum = checksum_file(&absolute_path)?;
                (checksum.sha256 != *ward_sha, Some(checksum))
            } else {
                (false, None)
            };

            let ward_entry = if purpose == StatusPurpose::WardUpdate {
                Some(match new_checksum {
                    Some(c) => WardEntry::File {
                        sha256: c.sha256,
                        mtime_nanos: mtime_to_nanos(&c.mtime)?,
                        size: c.size,
                    },
                    None => WardEntry::File {
                        sha256: ward_sha.clone(),
                        mtime_nanos: fs_mtime_nanos,
                        size: *fs_size,
                    },
                })
            } else {
                None
            };

            if sha256_differs {
                statuses.push(StatusEntry::Modified {
                    path: relative_path,
                    ward_entry,
                });
            } else if metadata_differs && !need_checksum_for_status {
                statuses.push(StatusEntry::PossiblyModified {
                    path: relative_path,
                });
            } else if mode == StatusMode::All || purpose == StatusPurpose::WardUpdate {
                statuses.push(StatusEntry::Unchanged {
                    path: relative_path,
                    ward_entry,
                });
            }
        }
        (WardEntry::Dir {}, FsEntry::Dir { .. }) => {
            if mode == StatusMode::All || purpose == StatusPurpose::WardUpdate {
                let ward_entry = if purpose == StatusPurpose::WardUpdate {
                    Some(WardEntry::Dir {})
                } else {
                    None
                };
                statuses.push(StatusEntry::Unchanged {
                    path: relative_path,
                    ward_entry,
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
            let ward_entry = if purpose == StatusPurpose::WardUpdate {
                Some(WardEntry::Symlink {
                    symlink_target: fs_target.clone(),
                })
            } else {
                None
            };

            if ward_target != fs_target {
                statuses.push(StatusEntry::Modified {
                    path: relative_path,
                    ward_entry,
                });
            } else if mode == StatusMode::All || purpose == StatusPurpose::WardUpdate {
                statuses.push(StatusEntry::Unchanged {
                    path: relative_path,
                    ward_entry,
                });
            }
        }
        _ => {
            let ward_entry = if purpose == StatusPurpose::WardUpdate {
                Some(build_ward_entry_from_fs(current_dir, name, fs_entry)?)
            } else {
                None
            };
            statuses.push(StatusEntry::Modified {
                path: relative_path,
                ward_entry,
            });
        }
    }

    Ok(())
}

/// Converts a path to a UTF-8 string, returning an error if the path contains
/// non-UTF-8 bytes.
///
/// Non-UTF-8 paths are unsupported because handling them portably is complex:
/// Unix paths are arbitrary byte sequences (except NUL), while Windows paths
/// are UTF-16 (potentially unpaired surrogates). Using lossy conversion would
/// risk fingerprint collisions where different invalid paths map to the same
/// replacement characters. Platform-specific raw byte access (OsStrExt on Unix)
/// would work but complicates cross-platform support. Since non-UTF-8 filenames
/// are rare in practice, we require valid UTF-8 for now. If this is fixed in the
/// future it must be done carefully and correctly (e.g. don't just use lossy conversion
/// which would create potential collisions).
fn path_to_str(path: &Path) -> Result<&str, StatusError> {
    path.to_str()
        .ok_or_else(|| StatusError::Other(format!("non-UTF-8 path not supported: {:?}", path)))
}

fn compute_fingerprint(statuses: &[StatusEntry]) -> Result<String, StatusError> {
    let mut hasher = Sha256::new();

    for entry in statuses {
        if matches!(entry.status_type(), StatusType::Unchanged) {
            continue;
        }

        hasher.update(path_to_str(entry.path())?.as_bytes());
        hasher.update(b"|");

        let status_type_str = match entry.status_type() {
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
    Ok(base64::engine::general_purpose::STANDARD.encode(hash_bytes))
}

/// Build WardFile objects from a StatusResult.
///
/// Groups status entries by their containing directory and constructs complete
/// WardFile objects ready to be saved to disk.
///
/// This function expects the StatusResult to come from `compute_status()` called
/// with `StatusPurpose::WardUpdate`, which ensures all entries (except `Removed`)
/// have complete ward_entry data. `Removed` entries are intentionally skipped
/// since they should not appear in the new ward files.
///
/// This function also ensures that all directories that exist on the filesystem
/// get .treeward files, even if they are empty (have no child entries).
///
/// # Arguments
///
/// * `root` - The root directory path (canonicalized)
/// * `status_result` - The result from compute_status() containing status entries
///
/// # Returns
///
/// A map from directory paths to their corresponding WardFile objects.
///
/// # Errors
///
/// Returns error if a path cannot be parsed or converted.
pub fn build_ward_files(
    root: &Path,
    status_result: &StatusResult,
) -> Result<BTreeMap<PathBuf, WardFile>, StatusError> {
    let mut dir_entries: BTreeMap<PathBuf, BTreeMap<String, WardEntry>> = BTreeMap::new();

    for entry in &status_result.statuses {
        match entry.ward_entry() {
            Some(ward_entry) => {
                let parent_dir = root.join(entry.path().parent().unwrap_or(Path::new("")));
                let filename = entry
                    .path()
                    .file_name()
                    .ok_or_else(|| {
                        StatusError::DirList(DirListError::Io(std::io::Error::new(
                            ErrorKind::InvalidInput,
                            format!("Path has no filename: {}", entry.path().display()),
                        )))
                    })?
                    .to_string_lossy()
                    .to_string();

                dir_entries
                    .entry(parent_dir.clone())
                    .or_default()
                    .insert(filename, ward_entry.clone());

                if matches!(ward_entry, WardEntry::Dir {}) {
                    let dir_path = root.join(entry.path());
                    dir_entries.entry(dir_path).or_default();
                }
            }
            None => {
                if !matches!(entry, StatusEntry::Removed { .. }) {
                    return Err(StatusError::Other(format!(
                        "missing ward_entry for non-Removed status: {}",
                        entry.path().display()
                    )));
                }
            }
        }
    }

    Ok(dir_entries
        .into_iter()
        .map(|(path, entries)| (path, WardFile::new(entries)))
        .collect())
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

        let result = compute_status(
            root,
            ChecksumPolicy::Never,
            StatusMode::Interesting,
            StatusPurpose::Display,
        )
        .unwrap();
        assert_eq!(result.statuses.len(), 0);

        let result2 = compute_status(
            root,
            ChecksumPolicy::Never,
            StatusMode::Interesting,
            StatusPurpose::Display,
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
        )
        .unwrap();
        assert_eq!(result.statuses.len(), 1);
        assert_eq!(result.statuses[0].path(), PathBuf::from("file1.txt"));
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
        )
        .unwrap();
        assert_eq!(result.statuses.len(), 2);

        let paths: Vec<PathBuf> = result
            .statuses
            .iter()
            .map(|c| c.path().to_path_buf())
            .collect();
        assert!(paths.contains(&PathBuf::from("dir1")));
        assert!(paths.contains(&PathBuf::from("dir1/file1.txt")));

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
        )
        .unwrap();
        assert_eq!(result.statuses.len(), 1);
        assert_eq!(result.statuses[0].path(), PathBuf::from("file1.txt"));
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
        )
        .unwrap();
        assert_eq!(result.statuses.len(), 1);

        assert_eq!(result.statuses[0].path(), PathBuf::from("dir1"));
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
        )
        .unwrap();
        assert_eq!(result.statuses.len(), 1);
        assert_eq!(result.statuses[0].path(), PathBuf::from("file1.txt"));
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
        )
        .unwrap();
        assert_eq!(result.statuses.len(), 1);
        assert_eq!(result.statuses[0].path(), PathBuf::from("file1.txt"));
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

        let result1 = compute_status(
            root,
            ChecksumPolicy::Never,
            StatusMode::Interesting,
            StatusPurpose::Display,
        )
        .unwrap();
        let result2 = compute_status(
            root,
            ChecksumPolicy::Never,
            StatusMode::Interesting,
            StatusPurpose::Display,
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
        )
        .unwrap();
        let result2 = compute_status(
            root2,
            ChecksumPolicy::Never,
            StatusMode::Interesting,
            StatusPurpose::Display,
        )
        .unwrap();

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

        let result = compute_status(
            root,
            ChecksumPolicy::Never,
            StatusMode::Interesting,
            StatusPurpose::Display,
        )
        .unwrap();
        assert_eq!(result.statuses.len(), 3);

        let change_types: BTreeMap<PathBuf, StatusType> = result
            .statuses
            .iter()
            .map(|c| (c.path().to_path_buf(), c.status_type()))
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

        let paths: Vec<PathBuf> = result
            .statuses
            .iter()
            .map(|c| c.path().to_path_buf())
            .collect();
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

        let result = compute_status(
            root,
            ChecksumPolicy::Never,
            StatusMode::Interesting,
            StatusPurpose::Display,
        )
        .unwrap();

        let link_change = result
            .statuses
            .iter()
            .find(|c| c.path() == PathBuf::from("link"));
        assert!(link_change.is_some());
        assert_eq!(link_change.unwrap().status_type(), StatusType::Modified);
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
        )
        .unwrap();

        let file_change = result
            .statuses
            .iter()
            .find(|c| c.path() == PathBuf::from("dir1/dir2/dir3/file.txt"));
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
        )
        .unwrap();
        assert_eq!(result.statuses.len(), 3);

        for change in &result.statuses {
            assert_eq!(change.status_type(), StatusType::Added);
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

        let result = compute_status(
            root,
            ChecksumPolicy::Never,
            StatusMode::Interesting,
            StatusPurpose::Display,
        )
        .unwrap();

        let item_change = result
            .statuses
            .iter()
            .find(|c| c.path() == PathBuf::from("item"));
        assert!(item_change.is_some());
        assert_eq!(item_change.unwrap().status_type(), StatusType::Modified);
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

        let result = compute_status(
            root,
            ChecksumPolicy::Never,
            StatusMode::Interesting,
            StatusPurpose::Display,
        )
        .unwrap();
        assert_eq!(result.statuses.len(), 1);
        assert_eq!(result.statuses[0].path(), PathBuf::from("file1.txt"));
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
        )
        .unwrap();
        assert_eq!(result.statuses.len(), 1);
        assert_eq!(result.statuses[0].path(), PathBuf::from("file1.txt"));
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
        )
        .unwrap();
        assert_eq!(result.statuses.len(), 1);
        assert_eq!(result.statuses[0].path(), PathBuf::from("file1.txt"));
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

        let result = compute_status(
            root,
            ChecksumPolicy::Never,
            StatusMode::All,
            StatusPurpose::Display,
        )
        .unwrap();
        assert_eq!(result.statuses.len(), 2);

        for change in &result.statuses {
            assert_eq!(change.status_type(), StatusType::Unchanged);
        }

        let paths: Vec<PathBuf> = result
            .statuses
            .iter()
            .map(|c| c.path().to_path_buf())
            .collect();
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

        let result = compute_status(
            root,
            ChecksumPolicy::Never,
            StatusMode::All,
            StatusPurpose::Display,
        )
        .unwrap();
        assert_eq!(result.statuses.len(), 4);

        let change_types: BTreeMap<PathBuf, StatusType> = result
            .statuses
            .iter()
            .map(|c| (c.path().to_path_buf(), c.status_type()))
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

        let result_interesting = compute_status(
            root,
            ChecksumPolicy::Never,
            StatusMode::Interesting,
            StatusPurpose::Display,
        )
        .unwrap();
        let result_all = compute_status(
            root,
            ChecksumPolicy::Never,
            StatusMode::All,
            StatusPurpose::Display,
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
}
