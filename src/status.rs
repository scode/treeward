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
    #[cfg(unix)]
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
}
