use crate::checksum::{ChecksumError, checksum_file};
use crate::dir_list::{DirListError, FsEntry, list_directory};
use crate::util::hashing;
use crate::ward_file::{WardEntry, WardFile, WardFileError};
use base64::Engine;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf, StripPrefixError};
use std::time::UNIX_EPOCH;
use tracing::info;

#[derive(Debug, thiserror::Error)]
pub enum StatusError {
    #[error("Ward file error: {0}")]
    WardFile(#[from] WardFileError),
    #[error("Directory listing error: {0}")]
    DirList(#[from] DirListError),
    #[error("Checksum error: {0}")]
    Checksum(#[from] ChecksumError),
    #[error("Path error: {0}")]
    StripPrefix(#[from] StripPrefixError),
    #[error("{0}")]
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusType {
    Added,
    Removed,
    /// File metadata differs but a content change has NOT been confirmed
    /// for reporting purposes (even if checksummed for ward updates).
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
/// Some variants carry `ward_entry` and/or `old_ward_entry` fields:
///
/// - `ward_entry: Option<WardEntry>` - The current/new ward data for this entry.
///   Populated when `StatusPurpose::WardUpdate` is used and, for differing entries,
///   when `DiffMode::Capture` is enabled to show new values in diffs. When present,
///   it is always complete (never contains placeholder data).
///
/// - `old_ward_entry: Option<WardEntry>` - The original ward data before the change.
///   Only populated when `DiffMode::Capture` is used, to enable displaying what
///   changed (e.g., old size vs new size, old checksum vs new checksum).
///
/// # Variants
///
/// - `Added`: Entry exists in filesystem but not in ward. The `ward_entry` contains
///   the new entry data to be written (if `WardUpdate` purpose). No `old_ward_entry`
///   since there was no previous ward data.
///
/// - `Removed`: Entry exists in ward but not in filesystem. The `old_ward_entry`
///   contains the original ward data (if `DiffMode::Capture`). No `ward_entry`
///   since the entry should be removed.
///
/// - `Modified`: Entry exists in both but content differs. The `ward_entry` contains
///   the updated entry data when either `WardUpdate` purpose or `DiffMode::Capture`
///   is used. The `old_ward_entry` contains the original ward data
///   (if `DiffMode::Capture`).
///
/// - `PossiblyModified`: Metadata differs but content was not checksummed for status
///   reporting purposes (only occurs with `ChecksumPolicy::Never`). When building
///   ward updates or capturing diffs, content may still be checksummed to populate
///   `ward_entry`; with `DiffMode::Capture`, `old_ward_entry` contains the original
///   ward data.
///
/// - `Unchanged`: Entry exists in both and matches. The `ward_entry` contains the
///   current entry data (if `WardUpdate` purpose), which may have updated metadata
///   even if content is unchanged. No `old_ward_entry` since nothing changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusEntry {
    Added {
        path: String,
        ward_entry: Option<WardEntry>,
    },
    Removed {
        path: String,
        /// The original ward entry (for diff display)
        old_ward_entry: Option<WardEntry>,
    },
    Modified {
        path: String,
        ward_entry: Option<WardEntry>,
        /// The original ward entry (for diff display)
        old_ward_entry: Option<WardEntry>,
    },
    PossiblyModified {
        path: String,
        ward_entry: Option<WardEntry>,
        /// The original ward entry (for diff display)
        old_ward_entry: Option<WardEntry>,
    },
    Unchanged {
        path: String,
        ward_entry: Option<WardEntry>,
    },
}

impl StatusEntry {
    pub fn path(&self) -> &str {
        match self {
            StatusEntry::Added { path, .. } => path,
            StatusEntry::Removed { path, .. } => path,
            StatusEntry::Modified { path, .. } => path,
            StatusEntry::PossiblyModified { path, .. } => path,
            StatusEntry::Unchanged { path, .. } => path,
        }
    }

    pub fn ward_entry(&self) -> Option<&WardEntry> {
        match self {
            StatusEntry::Added { ward_entry, .. } => ward_entry.as_ref(),
            StatusEntry::Modified { ward_entry, .. } => ward_entry.as_ref(),
            StatusEntry::Unchanged { ward_entry, .. } => ward_entry.as_ref(),
            StatusEntry::PossiblyModified { ward_entry, .. } => ward_entry.as_ref(),
            StatusEntry::Removed { .. } => None,
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
    /// `StatusEntry` variants will usually have `ward_entry: None`. When
    /// `DiffMode::Capture` is enabled, modified entries include `ward_entry`
    /// to show new values in diffs. Checksumming is controlled entirely by
    /// `ChecksumPolicy` (to determine Modified vs PossiblyModified), but the
    /// checksum result is not retained otherwise.
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

/// Controls whether `StatusEntry` variants include diff data (old ward entry values).
///
/// When diff mode is enabled, Modified, PossiblyModified, and Removed variants
/// will include the original ward entry data for comparison with current filesystem state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiffMode {
    /// Don't capture diff data (current default)
    #[default]
    None,
    /// Capture old ward entry for diff display
    Capture,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusResult {
    pub statuses: Vec<StatusEntry>,
    /// A unique fingerprint representing the entire changeset.
    ///
    /// This is currently a Base64-encoded SHA-256 but it could change
    /// in the future.
    ///
    /// See --fingerprint flag for more information.
    pub fingerprint: String,
}

/// Canonicalized fingerprint input for one status entry.
///
/// We decouple fingerprint construction from `StatusEntry` so hashing can use
/// normalized, policy-aware payloads without affecting user-facing status output.
///
/// Example: if `status` reports `M? notes.txt`, then `notes.txt` is edited again
/// before `update --fingerprint`, path and status class may still be `notes.txt + M?`.
/// By carrying per-entry payload in this record, the second edit changes fingerprint input.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FingerprintRecord {
    path: String,
    status_type: StatusType,
    payload: FingerprintPayload,
}

/// Additional state material hashed into the fingerprint.
///
/// `path + status` alone is insufficient for TOCTOU detection: a file can be edited
/// repeatedly while still remaining in the same status class. This payload captures
/// enough state to bind a fingerprint to the exact reviewed snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FingerprintPayload {
    /// Present for Added/Modified/PossiblyModified files.
    File {
        /// Current filesystem mtime at status computation time.
        mtime_nanos: u64,
        /// Current filesystem size at status computation time.
        size: u64,
        // Present only when status determination was checksum-based.
        sha256: Option<String>,
    },
    /// Present for Added directories and type changes to directories.
    Dir { mtime_nanos: u64 },
    /// Present for Added/Modified symlinks and type changes to symlinks.
    Symlink { symlink_target: PathBuf },
    /// Present for Removed entries (captures prior ward state).
    ///
    /// Removed entries have no filesystem-side object to hash, so the previous ward
    /// data is the only stable identity for what was reviewed. Capturing it prevents
    /// path-only `R` entries from masking ward-state drift between status and update.
    Removed { ward_entry: WardEntry },
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
/// * `diff_mode` - Controls whether to capture old ward entry data for diff display:
///   - `None`: Don't capture diff data (default)
///   - `Capture`: Include old ward entry in Modified, PossiblyModified, and Removed variants
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
    diff_mode: DiffMode,
) -> Result<StatusResult, StatusError> {
    let root = root.to_path_buf();

    let mut statuses = Vec::new();
    let mut fingerprint_records = Vec::new();

    walk_directory(
        &root,
        &root,
        &mut statuses,
        &mut fingerprint_records,
        policy,
        mode,
        purpose,
        diff_mode,
    )?;

    statuses.sort_by(|a, b| a.path().cmp(b.path()));
    // Keep fingerprint deterministic even if traversal order changes in the future.
    fingerprint_records.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then_with(|| status_type_code(a.status_type).cmp(status_type_code(b.status_type)))
    });

    let fingerprint = compute_fingerprint(&fingerprint_records);

    Ok(StatusResult {
        statuses,
        fingerprint,
    })
}

#[allow(clippy::too_many_arguments)]
fn walk_directory(
    tree_root: &Path,
    current_dir: &Path,
    statuses: &mut Vec<StatusEntry>,
    fingerprint_records: &mut Vec<FingerprintRecord>,
    policy: ChecksumPolicy,
    mode: StatusMode,
    purpose: StatusPurpose,
    diff_mode: DiffMode,
) -> Result<(), StatusError> {
    info!("Entering directory {}", current_dir.display());

    let ward_path = current_dir.join(".treeward");
    let ward_file = WardFile::load_if_exists(&ward_path)?;
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
        fingerprint_records,
        policy,
        mode,
        purpose,
        diff_mode,
    )?;

    for (name, entry) in &fs_entries {
        if matches!(entry, FsEntry::Dir { .. }) {
            let child_path = current_dir.join(name);
            walk_directory(
                tree_root,
                &child_path,
                statuses,
                fingerprint_records,
                policy,
                mode,
                purpose,
                diff_mode,
            )?;
        }
    }

    for (name, entry) in &ward_entries {
        if matches!(entry, WardEntry::Dir {}) && !fs_entries.contains_key(name) {
            let child_path = current_dir.join(name);
            walk_directory(
                tree_root,
                &child_path,
                statuses,
                fingerprint_records,
                policy,
                mode,
                purpose,
                diff_mode,
            )?;
        }
    }

    Ok(())
}

fn mtime_to_nanos(mtime: &std::time::SystemTime) -> Result<u64, StatusError> {
    let nanos = mtime
        .duration_since(UNIX_EPOCH)
        .map_err(|_| StatusError::Other("mtime is before UNIX epoch".to_string()))?
        .as_nanos();
    nanos.try_into().map_err(|_| {
        StatusError::Other("timestamp overflow: nanoseconds exceeds u64 range".to_string())
    })
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
    fingerprint_records: &mut Vec<FingerprintRecord>,
    policy: ChecksumPolicy,
    mode: StatusMode,
    purpose: StatusPurpose,
    diff_mode: DiffMode,
) -> Result<(), StatusError> {
    for name in fs_entries.keys() {
        if !ward_entries.contains_key(name) {
            let relative_path = make_relative_path(tree_root, current_dir, name)?;
            let fs_entry = &fs_entries[name];
            let fingerprint_payload = fingerprint_payload_from_fs_entry(fs_entry, None)?;

            let ward_entry = if purpose == StatusPurpose::WardUpdate {
                Some(build_ward_entry_from_fs(current_dir, name, fs_entry)?)
            } else {
                None
            };

            statuses.push(StatusEntry::Added {
                path: relative_path.clone(),
                ward_entry,
            });
            fingerprint_records.push(FingerprintRecord {
                path: relative_path,
                status_type: StatusType::Added,
                payload: fingerprint_payload,
            });
        }
    }

    for name in ward_entries.keys() {
        if !fs_entries.contains_key(name) {
            let relative_path = make_relative_path(tree_root, current_dir, name)?;
            let removed_ward_entry = ward_entries[name].clone();
            let old_ward_entry = if diff_mode == DiffMode::Capture {
                Some(removed_ward_entry.clone())
            } else {
                None
            };
            statuses.push(StatusEntry::Removed {
                path: relative_path.clone(),
                old_ward_entry,
            });
            fingerprint_records.push(FingerprintRecord {
                path: relative_path,
                status_type: StatusType::Removed,
                payload: FingerprintPayload::Removed {
                    ward_entry: removed_ward_entry,
                },
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
                fingerprint_records,
                policy,
                mode,
                purpose,
                diff_mode,
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
    fingerprint_records: &mut Vec<FingerprintRecord>,
    policy: ChecksumPolicy,
    mode: StatusMode,
    purpose: StatusPurpose,
    diff_mode: DiffMode,
) -> Result<(), StatusError> {
    let relative_path = make_relative_path(tree_root, current_dir, name)?;
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
            let need_checksum_for_diff = diff_mode == DiffMode::Capture && metadata_differs;
            let need_checksum =
                need_checksum_for_status || need_checksum_for_ward || need_checksum_for_diff;

            let (sha256_differs, new_checksum) = if need_checksum {
                let checksum = checksum_file(&absolute_path)?;
                (checksum.sha256 != *ward_sha, Some(checksum))
            } else {
                (false, None)
            };

            let new_ward_entry =
                if purpose == StatusPurpose::WardUpdate || diff_mode == DiffMode::Capture {
                    Some(match &new_checksum {
                        Some(c) => WardEntry::File {
                            sha256: c.sha256.clone(),
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

            // Capture old_ward_entry when diff mode is enabled and the entry differs
            // (either metadata or checksum - for --always-verify detecting silent corruption)
            let old_ward_entry =
                if diff_mode == DiffMode::Capture && (metadata_differs || sha256_differs) {
                    Some(ward_entry.clone())
                } else {
                    None
                };

            // Fingerprint should reflect file state at status-time, not just path/status.
            let fingerprint_payload = FingerprintPayload::File {
                mtime_nanos: fs_mtime_nanos,
                size: *fs_size,
                sha256: if need_checksum_for_status {
                    // Include hash only when status policy was checksum-driven.
                    // This preserves fingerprint parity between `status` and `update`
                    // when the same verify flags are used.
                    new_checksum.as_ref().map(|c| c.sha256.clone())
                } else {
                    None
                },
            };

            if metadata_differs && !need_checksum_for_status {
                // Policy says don't checksum for status reporting, so report
                // PossiblyModified regardless of whether we checksummed for ward
                // building. This ensures fingerprint consistency between status
                // and ward commands when using the same --verify/--always-verify flags.
                statuses.push(StatusEntry::PossiblyModified {
                    path: relative_path.clone(),
                    ward_entry: new_ward_entry,
                    old_ward_entry,
                });
                fingerprint_records.push(FingerprintRecord {
                    path: relative_path,
                    status_type: StatusType::PossiblyModified,
                    payload: fingerprint_payload,
                });
            } else if sha256_differs {
                statuses.push(StatusEntry::Modified {
                    path: relative_path.clone(),
                    ward_entry: new_ward_entry,
                    old_ward_entry,
                });
                fingerprint_records.push(FingerprintRecord {
                    path: relative_path,
                    status_type: StatusType::Modified,
                    payload: fingerprint_payload,
                });
            } else if mode == StatusMode::All || purpose == StatusPurpose::WardUpdate {
                statuses.push(StatusEntry::Unchanged {
                    path: relative_path,
                    ward_entry: new_ward_entry,
                });
            }
        }
        (WardEntry::Dir {}, FsEntry::Dir { .. }) => {
            if mode == StatusMode::All || purpose == StatusPurpose::WardUpdate {
                let new_ward_entry = if purpose == StatusPurpose::WardUpdate {
                    Some(WardEntry::Dir {})
                } else {
                    None
                };
                statuses.push(StatusEntry::Unchanged {
                    path: relative_path,
                    ward_entry: new_ward_entry,
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
            let new_ward_entry =
                if purpose == StatusPurpose::WardUpdate || diff_mode == DiffMode::Capture {
                    Some(WardEntry::Symlink {
                        symlink_target: fs_target.clone(),
                    })
                } else {
                    None
                };

            if ward_target != fs_target {
                let old_ward_entry = if diff_mode == DiffMode::Capture {
                    Some(ward_entry.clone())
                } else {
                    None
                };
                statuses.push(StatusEntry::Modified {
                    path: relative_path.clone(),
                    ward_entry: new_ward_entry,
                    old_ward_entry,
                });
                fingerprint_records.push(FingerprintRecord {
                    path: relative_path,
                    status_type: StatusType::Modified,
                    payload: FingerprintPayload::Symlink {
                        symlink_target: fs_target.clone(),
                    },
                });
            } else if mode == StatusMode::All || purpose == StatusPurpose::WardUpdate {
                statuses.push(StatusEntry::Unchanged {
                    path: relative_path,
                    ward_entry: new_ward_entry,
                });
            }
        }
        _ => {
            // Type change (e.g., file -> symlink)
            let new_ward_entry =
                if purpose == StatusPurpose::WardUpdate || diff_mode == DiffMode::Capture {
                    Some(build_ward_entry_from_fs(current_dir, name, fs_entry)?)
                } else {
                    None
                };
            let old_ward_entry = if diff_mode == DiffMode::Capture {
                Some(ward_entry.clone())
            } else {
                None
            };
            let fingerprint_payload = fingerprint_payload_from_fs_entry(fs_entry, None)?;
            statuses.push(StatusEntry::Modified {
                path: relative_path.clone(),
                ward_entry: new_ward_entry,
                old_ward_entry,
            });
            fingerprint_records.push(FingerprintRecord {
                path: relative_path,
                status_type: StatusType::Modified,
                payload: fingerprint_payload,
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

/// Constructs a relative path string from tree_root, current_dir, and entry name.
///
/// Returns the path as a validated UTF-8 String suitable for use in StatusEntry.
fn make_relative_path(
    tree_root: &Path,
    current_dir: &Path,
    name: &str,
) -> Result<String, StatusError> {
    let relative_dir = current_dir.strip_prefix(tree_root)?;
    let relative_path = relative_dir.join(name);
    path_to_str(&relative_path).map(|s| s.to_string())
}

fn fingerprint_payload_from_fs_entry(
    fs_entry: &FsEntry,
    file_sha256: Option<String>,
) -> Result<FingerprintPayload, StatusError> {
    match fs_entry {
        FsEntry::File { mtime, size } => Ok(FingerprintPayload::File {
            mtime_nanos: mtime_to_nanos(mtime)?,
            size: *size,
            sha256: file_sha256,
        }),
        FsEntry::Dir { mtime } => Ok(FingerprintPayload::Dir {
            mtime_nanos: mtime_to_nanos(mtime)?,
        }),
        FsEntry::Symlink { symlink_target } => Ok(FingerprintPayload::Symlink {
            symlink_target: symlink_target.clone(),
        }),
    }
}

/// Stable short code used both in terminal output and fingerprint records.
fn status_type_code(status_type: StatusType) -> &'static str {
    match status_type {
        StatusType::Added => "A",
        StatusType::Removed => "R",
        StatusType::PossiblyModified => "M?",
        StatusType::Modified => "M",
        StatusType::Unchanged => ".",
    }
}

/// Hashes payload-specific fingerprint material.
///
/// Variant tags are included explicitly to prevent cross-variant collisions
/// (for example, a removed file payload never collides with a live file payload
/// that happens to contain the same scalar values).
fn hash_fingerprint_payload(hasher: &mut Sha256, payload: &FingerprintPayload) {
    match payload {
        FingerprintPayload::File {
            mtime_nanos,
            size,
            sha256,
        } => {
            hasher.update(b"file");
            hashing::hash_u64_field(hasher, *mtime_nanos);
            hashing::hash_u64_field(hasher, *size);
            match sha256 {
                Some(sha) => {
                    hasher.update([1u8]);
                    hashing::hash_field(hasher, sha.as_bytes());
                }
                None => {
                    hasher.update([0u8]);
                }
            }
        }
        FingerprintPayload::Dir { mtime_nanos } => {
            hasher.update(b"dir");
            hashing::hash_u64_field(hasher, *mtime_nanos);
        }
        FingerprintPayload::Symlink { symlink_target } => {
            hasher.update(b"symlink");
            hashing::hash_path_field(hasher, symlink_target);
        }
        FingerprintPayload::Removed { ward_entry } => match ward_entry {
            WardEntry::File {
                sha256,
                mtime_nanos,
                size,
            } => {
                hasher.update(b"removed_file");
                hashing::hash_field(hasher, sha256.as_bytes());
                hashing::hash_u64_field(hasher, *mtime_nanos);
                hashing::hash_u64_field(hasher, *size);
            }
            WardEntry::Dir {} => {
                hasher.update(b"removed_dir");
            }
            WardEntry::Symlink { symlink_target } => {
                hasher.update(b"removed_symlink");
                hashing::hash_path_field(hasher, symlink_target);
            }
        },
    }
}

/// Computes the fingerprint for all interesting status entries.
///
/// Unchanged entries are intentionally excluded because fingerprints are used to
/// guard the reviewed change set for init/update acceptance.
fn compute_fingerprint(records: &[FingerprintRecord]) -> String {
    let mut hasher = Sha256::new();

    for record in records {
        hashing::hash_field(&mut hasher, record.path.as_bytes());
        hashing::hash_field(&mut hasher, status_type_code(record.status_type).as_bytes());
        hash_fingerprint_payload(&mut hasher, &record.payload);
    }

    let hash_bytes = hasher.finalize();
    base64::engine::general_purpose::STANDARD.encode(hash_bytes)
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
                let entry_path = Path::new(entry.path());
                let parent_dir = root.join(entry_path.parent().unwrap_or(Path::new("")));
                let filename = entry_path
                    .file_name()
                    .ok_or_else(|| {
                        StatusError::DirList(DirListError::Io(std::io::Error::new(
                            ErrorKind::InvalidInput,
                            format!("Path has no filename: {}", entry.path()),
                        )))
                    })?
                    .to_str()
                    .expect("path is already validated as UTF-8")
                    .to_string();

                dir_entries
                    .entry(parent_dir.clone())
                    .or_default()
                    .insert(filename, ward_entry.clone());

                if matches!(ward_entry, WardEntry::Dir {}) {
                    let dir_path = root.join(entry_path);
                    dir_entries.entry(dir_path).or_default();
                }
            }
            None => {
                if !matches!(entry, StatusEntry::Removed { .. }) {
                    return Err(StatusError::Other(format!(
                        "missing ward_entry for non-Removed status: {}",
                        entry.path()
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

#[cfg(test)]
mod tests;
