//! Ward-file write pipeline built on top of status computation.
//!
//! Runs status traversal in ward-update mode, validates optional fingerprints,
//! builds new per-directory ward snapshots, and writes changed `.treeward`
//! files.

use crate::checksum::ChecksumError;
use crate::dir_list::DirListError;
use crate::status::{
    ChecksumPolicy, DiffMode, StatusEntry, StatusError, StatusMode, StatusPurpose,
    build_ward_files, compute_status,
};
#[cfg(test)]
use crate::ward_file::WardEntry;
use crate::ward_file::{WardFile, WardFileError};
use std::path::{Path, PathBuf, StripPrefixError};

#[derive(Debug, thiserror::Error)]
pub enum WardError {
    #[error("Ward file error: {0}")]
    WardFile(#[from] WardFileError),
    #[error("Status error: {0}")]
    Status(#[from] StatusError),
    #[error("Directory listing error: {0}")]
    DirList(#[from] DirListError),
    #[error("Checksum error: {0}")]
    Checksum(#[from] ChecksumError),
    #[error("Path error: {0}")]
    StripPrefix(#[from] StripPrefixError),
    #[error("Not initialized (use treeward init to initialize)")]
    NotInitialized,
    #[error("Already initialized (use treeward update instead)")]
    AlreadyInitialized,
    #[error(
        "Fingerprint mismatch: expected {expected}, got {actual}. Ensure --verify/--always-verify flags match between status and init/update commands."
    )]
    FingerprintMismatch { expected: String, actual: String },
}

pub struct WardOptions {
    pub init: bool,
    pub allow_init: bool,
    pub fingerprint: Option<String>,
    pub dry_run: bool,
    pub checksum_policy: ChecksumPolicy,
}

#[derive(Debug)]
pub struct WardResult {
    pub files_warded: usize,
    pub ward_files_updated: Vec<PathBuf>,
}

/// Create or update `.treeward` files to record the current state of a directory tree.
///
/// Recursively traverses the directory tree starting from `root`, computing checksums
/// for files and creating/updating `.treeward` files in each directory to record
/// the current state.
///
/// # Arguments
///
/// * `root` - Directory to ward (will be canonicalized)
/// * `options` - Configuration options controlling the ward operation:
///   - `init`: Allow warding when no `.treeward` exists in root (required for first ward)
///   - `fingerprint`: Optional fingerprint from `compute_status()` to validate before writing
///   - `dry_run`: Preview what would be updated without writing any files
///
/// # Behavior
///
/// **Efficient checksumming:**
/// - Only checksums files that are new or have changed metadata (mtime/size)
/// - Files with matching metadata reuse checksums from existing ward files
/// - This makes incremental warding very fast (only checksums what changed)
///
/// **Initialization:**
/// - If `!options.init` and no `.treeward` in root, returns `NotInitialized` error
/// - The `init` flag only applies to the root directory - subdirectories can always
///   have `.treeward` files created without `init`
///
/// **Fingerprint validation:**
/// - If `options.fingerprint` is provided, validates current changes match the fingerprint
/// - Fails with `FingerprintMismatch` error if changes don't match
/// - This prevents TOCTOU issues where files change between `status` and `ward`
/// - **No ward files are written if fingerprint doesn't match**
///
/// **Selective writing:**
/// - Only rewrites `.treeward` files if their contents actually changed
/// - Avoids unnecessary disk writes and preserves mtimes of unchanged ward files
///
/// **Dry run:**
/// - If `options.dry_run`, computes what would be updated but writes no files
/// - Returns what would have been updated in `ward_files_updated`
///
/// # Returns
///
/// * `files_warded` - Number of files that required checksumming for ward entries (added,
///   modified, or possibly modified; excludes unchanged files and directories/symlinks).
///   Unchanged files may still be checksummed when using `--always-verify`.
/// * `ward_files_updated` - Relative paths of `.treeward` files that were written (or
///   would be written in dry-run mode)
pub fn ward_directory(root: &Path, options: WardOptions) -> Result<WardResult, WardError> {
    let root = root.to_path_buf();

    let ward_path = root.join(".treeward");

    if !options.init && !options.allow_init && !ward_path.exists() {
        return Err(WardError::NotInitialized);
    }

    if options.init && !options.allow_init && ward_path.exists() {
        return Err(WardError::AlreadyInitialized);
    }

    // Compute status with WardUpdate purpose to get complete ward entries.
    // The checksum policy must match what was used with `status` command
    // for fingerprint validation to work correctly.
    let status = compute_status(
        &root,
        options.checksum_policy,
        StatusMode::All,
        StatusPurpose::WardUpdate,
        DiffMode::None,
    )?;

    // Build ward files in memory from status result
    let mut ward_files = build_ward_files(&root, &status)?;

    // Ensure root directory always has a ward file (even if empty)
    ward_files
        .entry(root.clone())
        .or_insert_with(|| WardFile::new(std::collections::BTreeMap::new()));

    // Intentionally validating fingerprint AFTER generating ward
    // to avoid TOCTOU conditions.
    if let Some(expected_fingerprint) = &options.fingerprint
        && &status.fingerprint != expected_fingerprint
    {
        return Err(WardError::FingerprintMismatch {
            expected: expected_fingerprint.clone(),
            actual: status.fingerprint,
        });
    }

    // Write ward files - only changed ones.
    let mut ward_files_updated = Vec::new();
    for (dir_path, ward_file) in &ward_files {
        let ward_path = dir_path.join(".treeward");
        let existing = WardFile::load_if_exists(&ward_path)?;

        if existing.as_ref() != Some(ward_file) {
            if !options.dry_run {
                ward_file.save(&ward_path)?;
            }
            ward_files_updated.push(ward_path.strip_prefix(&root)?.to_path_buf());
        }
    }

    // Count files that were checksummed for the ward file. This includes Added, Modified,
    // and PossiblyModified (which are checksummed for ward building even though the status
    // is reported as PossiblyModified for fingerprint consistency with ChecksumPolicy::Never).
    let files_warded = status
        .statuses
        .iter()
        .filter(|s| match s {
            StatusEntry::Added { ward_entry, .. }
            | StatusEntry::Modified { ward_entry, .. }
            | StatusEntry::PossiblyModified { ward_entry, .. } => {
                matches!(ward_entry, Some(crate::ward_file::WardEntry::File { .. }))
            }
            _ => false,
        })
        .count();

    Ok(WardResult {
        files_warded,
        ward_files_updated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checksum::checksum_file;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix;
    use tempfile::TempDir;

    #[test]
    fn test_initial_ward_with_init() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        fs::write(root.join("file1.txt"), "content1").unwrap();
        fs::create_dir(root.join("dir1")).unwrap();
        fs::write(root.join("dir1/file2.txt"), "content2").unwrap();

        let options = WardOptions {
            init: true,
            allow_init: false,
            fingerprint: None,
            dry_run: false,
            checksum_policy: ChecksumPolicy::Never,
        };

        let result = ward_directory(root, options).unwrap();

        assert_eq!(result.files_warded, 2);
        assert!(root.join(".treeward").exists());
        assert!(root.join("dir1/.treeward").exists());

        let root_ward = WardFile::load(&root.join(".treeward")).unwrap();
        assert!(root_ward.entries.contains_key("file1.txt"));
        assert!(root_ward.entries.contains_key("dir1"));

        let dir1_ward = WardFile::load(&root.join("dir1/.treeward")).unwrap();
        assert!(dir1_ward.entries.contains_key("file2.txt"));
    }

    #[test]
    fn test_ward_without_init_when_not_initialized() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        fs::write(root.join("file1.txt"), "content1").unwrap();

        let options = WardOptions {
            init: false,
            allow_init: false,
            fingerprint: None,
            dry_run: false,
            checksum_policy: ChecksumPolicy::Never,
        };

        let result = ward_directory(root, options);

        assert!(result.is_err());
        match result {
            Err(WardError::NotInitialized) => {}
            _ => panic!("Expected NotInitialized error"),
        }
    }

    #[test]
    fn test_ward_with_init_when_already_initialized() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        fs::write(root.join("file1.txt"), "content1").unwrap();

        let init_options = WardOptions {
            init: true,
            allow_init: false,
            fingerprint: None,
            dry_run: false,
            checksum_policy: ChecksumPolicy::Never,
        };

        ward_directory(root, init_options).unwrap();

        fs::write(root.join("file2.txt"), "content2").unwrap();

        let update_options = WardOptions {
            init: true,
            allow_init: false,
            fingerprint: None,
            dry_run: false,
            checksum_policy: ChecksumPolicy::Never,
        };

        let result = ward_directory(root, update_options);
        match result {
            Err(WardError::AlreadyInitialized) => {}
            _ => panic!("Expected AlreadyInitialized error"),
        }
    }

    #[test]
    fn test_fingerprint_validation_matching() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        fs::write(root.join("file1.txt"), "content1").unwrap();

        let init_options = WardOptions {
            init: true,
            allow_init: false,
            fingerprint: None,
            dry_run: false,
            checksum_policy: ChecksumPolicy::Never,
        };

        ward_directory(root, init_options).unwrap();

        fs::write(root.join("file2.txt"), "content2").unwrap();

        // Both status and update must use the same checksum policy for
        // fingerprint validation to work correctly.
        let status = compute_status(
            root,
            ChecksumPolicy::WhenPossiblyModified,
            StatusMode::Interesting,
            StatusPurpose::Display,
            DiffMode::None,
        )
        .unwrap();

        let options = WardOptions {
            init: false,
            allow_init: false,
            fingerprint: Some(status.fingerprint.clone()),
            dry_run: false,
            checksum_policy: ChecksumPolicy::WhenPossiblyModified,
        };

        let result = ward_directory(root, options);
        assert!(result.is_ok());
    }

    /// Tests that fingerprint from status with ChecksumPolicy::Never (the CLI
    /// default) correctly matches what update computes.
    ///
    /// This reproduces a bug where:
    /// 1. A file's mtime changes but content stays the same
    /// 2. status (default) reports it as M? (PossiblyModified) in fingerprint
    /// 3. update checksums it, finds it unchanged, computes different fingerprint
    #[test]
    fn test_fingerprint_validation_with_metadata_only_change() {
        use filetime::{FileTime, set_file_mtime};

        let temp = TempDir::new().unwrap();
        let root = temp.path();

        fs::write(root.join("file1.txt"), "content1").unwrap();

        let init_options = WardOptions {
            init: true,
            allow_init: false,
            fingerprint: None,
            dry_run: false,
            checksum_policy: ChecksumPolicy::Never,
        };

        ward_directory(root, init_options).unwrap();

        // Touch file1.txt to change mtime without changing content.
        // This simulates .DS_Store files that macOS updates frequently.
        set_file_mtime(
            root.join("file1.txt"),
            FileTime::from_unix_time(1000000000, 0),
        )
        .unwrap();

        // Status with Never policy (CLI default) - file appears as M?
        let status = compute_status(
            root,
            ChecksumPolicy::Never,
            StatusMode::Interesting,
            StatusPurpose::Display,
            DiffMode::None,
        )
        .unwrap();

        assert_eq!(status.statuses.len(), 1);
        assert!(matches!(
            status.statuses[0],
            StatusEntry::PossiblyModified { .. }
        ));

        // Update should accept this fingerprint
        let options = WardOptions {
            init: false,
            allow_init: false,
            fingerprint: Some(status.fingerprint.clone()),
            dry_run: false,
            checksum_policy: ChecksumPolicy::Never,
        };

        let result = ward_directory(root, options);
        assert!(
            result.is_ok(),
            "Fingerprint from status (Never policy) should match update: {:?}",
            result
        );
    }

    #[test]
    fn test_fingerprint_validation_mismatch() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        fs::write(root.join("file1.txt"), "content1").unwrap();

        let init_options = WardOptions {
            init: true,
            allow_init: false,
            fingerprint: None,
            dry_run: false,
            checksum_policy: ChecksumPolicy::Never,
        };

        ward_directory(root, init_options).unwrap();

        fs::write(root.join("file2.txt"), "content2").unwrap();

        let options = WardOptions {
            init: false,
            allow_init: false,
            fingerprint: Some("wrong_fingerprint".to_string()),
            dry_run: false,
            checksum_policy: ChecksumPolicy::Never,
        };

        let result = ward_directory(root, options);

        assert!(result.is_err());
        match result {
            Err(WardError::FingerprintMismatch { expected, actual }) => {
                assert_eq!(expected, "wrong_fingerprint");
                assert_ne!(actual, "wrong_fingerprint");
            }
            _ => panic!("Expected FingerprintMismatch error"),
        }

        assert!(!root.join("file2.txt").join(".treeward").exists());
    }

    /// Tests TOCTOU protection: if a new file appears between `status` and `update`,
    /// the fingerprint mismatch is caught and update fails without writing.
    ///
    /// The fingerprint captures the set of changes (path + status type). If the
    /// filesystem state changes between status and update (e.g., new file added),
    /// the fingerprint won't match and update fails atomically.
    ///
    /// This validates the intentional ordering where fingerprint validation happens
    /// AFTER computing the new ward state.
    #[test]
    fn test_fingerprint_catches_new_file_between_status_and_update() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        fs::write(root.join("file1.txt"), "content1").unwrap();

        let init_options = WardOptions {
            init: true,
            allow_init: false,
            fingerprint: None,
            dry_run: false,
            checksum_policy: ChecksumPolicy::Never,
        };
        ward_directory(root, init_options).unwrap();

        // Modify file1 - this is the change the user will review with status
        fs::write(root.join("file1.txt"), "modified").unwrap();

        // User runs status and gets fingerprint (shows file1 as Modified)
        let status = compute_status(
            root,
            ChecksumPolicy::WhenPossiblyModified,
            StatusMode::Interesting,
            StatusPurpose::Display,
            DiffMode::None,
        )
        .unwrap();
        assert_eq!(status.statuses.len(), 1);
        let fingerprint_at_status_time = status.fingerprint.clone();

        // NEW file appears between status and update (simulating race condition)
        fs::write(root.join("file2.txt"), "sneaky new file").unwrap();

        // Update with the fingerprint from earlier status should FAIL
        // because a new file appeared
        let update_options = WardOptions {
            init: false,
            allow_init: false,
            fingerprint: Some(fingerprint_at_status_time),
            dry_run: false,
            checksum_policy: ChecksumPolicy::WhenPossiblyModified,
        };

        let result = ward_directory(root, update_options);
        assert!(
            matches!(result, Err(WardError::FingerprintMismatch { .. })),
            "Update should fail when new file appeared after status: {:?}",
            result
        );

        // Verify the ward file still has the OLD state - file2.txt should NOT be tracked
        let ward = WardFile::load(&root.join(".treeward")).unwrap();
        assert!(
            !ward.entries.contains_key("file2.txt"),
            "New file should not be in ward after failed update"
        );
        // file1.txt should still have original checksum (no writes occurred)
        // Compute expected checksum using a temp file
        let temp_for_checksum = TempDir::new().unwrap();
        let checksum_path = temp_for_checksum.path().join("temp");
        fs::write(&checksum_path, "content1").unwrap();
        let original_checksum = checksum_file(&checksum_path).unwrap();

        match ward.entries.get("file1.txt").unwrap() {
            WardEntry::File { sha256, .. } => {
                assert_eq!(
                    sha256, &original_checksum.sha256,
                    "file1.txt should still have original checksum after failed update"
                );
            }
            _ => panic!("Expected File entry"),
        }
    }

    /// Tests TOCTOU protection: if a file is deleted between `status` and `update`,
    /// the fingerprint mismatch is caught and update fails without writing.
    #[test]
    fn test_fingerprint_catches_deleted_file_between_status_and_update() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        fs::write(root.join("file1.txt"), "content1").unwrap();
        fs::write(root.join("file2.txt"), "content2").unwrap();

        let init_options = WardOptions {
            init: true,
            allow_init: false,
            fingerprint: None,
            dry_run: false,
            checksum_policy: ChecksumPolicy::Never,
        };
        ward_directory(root, init_options).unwrap();

        // Modify both files - user will review this with status
        fs::write(root.join("file1.txt"), "modified1").unwrap();
        fs::write(root.join("file2.txt"), "modified2").unwrap();

        // User runs status and gets fingerprint (shows both files as Modified)
        let status = compute_status(
            root,
            ChecksumPolicy::WhenPossiblyModified,
            StatusMode::Interesting,
            StatusPurpose::Display,
            DiffMode::None,
        )
        .unwrap();
        assert_eq!(status.statuses.len(), 2);
        let fingerprint_at_status_time = status.fingerprint.clone();

        // file2 is DELETED between status and update (simulating race condition)
        fs::remove_file(root.join("file2.txt")).unwrap();

        // Update with the fingerprint from earlier status should FAIL
        let update_options = WardOptions {
            init: false,
            allow_init: false,
            fingerprint: Some(fingerprint_at_status_time),
            dry_run: false,
            checksum_policy: ChecksumPolicy::WhenPossiblyModified,
        };

        let result = ward_directory(root, update_options);
        assert!(
            matches!(result, Err(WardError::FingerprintMismatch { .. })),
            "Update should fail when file deleted after status: {:?}",
            result
        );

        // Verify ward file unchanged - file2.txt should still be tracked
        let ward = WardFile::load(&root.join(".treeward")).unwrap();
        assert!(
            ward.entries.contains_key("file2.txt"),
            "Deleted file should still be in ward after failed update"
        );
    }

    #[test]
    fn test_dry_run() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        fs::write(root.join("file1.txt"), "content1").unwrap();

        let options = WardOptions {
            init: true,
            allow_init: false,
            fingerprint: None,
            dry_run: true,
            checksum_policy: ChecksumPolicy::Never,
        };

        let result = ward_directory(root, options).unwrap();

        assert_eq!(result.files_warded, 1);
        assert_eq!(result.ward_files_updated, vec![PathBuf::from(".treeward")]);

        assert!(!root.join(".treeward").exists());
    }

    #[test]
    fn test_dry_run_reports_all_pending_writes() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        fs::create_dir(root.join("dir")).unwrap();
        fs::write(root.join("file1.txt"), "content1").unwrap();
        fs::write(root.join("dir/file2.txt"), "content2").unwrap();

        let options = WardOptions {
            init: true,
            allow_init: false,
            fingerprint: None,
            dry_run: true,
            checksum_policy: ChecksumPolicy::Never,
        };

        let result = ward_directory(root, options).unwrap();

        let mut reported = result.ward_files_updated.clone();
        reported.sort();
        assert_eq!(
            reported,
            vec![PathBuf::from(".treeward"), PathBuf::from("dir/.treeward")]
        );

        assert!(!root.join(".treeward").exists());
        assert!(!root.join("dir/.treeward").exists());
    }

    #[test]
    fn test_only_modified_ward_files_written() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        fs::create_dir(root.join("dir1")).unwrap();
        fs::write(root.join("dir1/file1.txt"), "content1").unwrap();
        fs::create_dir(root.join("dir2")).unwrap();
        fs::write(root.join("dir2/file2.txt"), "content2").unwrap();

        let init_options = WardOptions {
            init: true,
            allow_init: false,
            fingerprint: None,
            dry_run: false,
            checksum_policy: ChecksumPolicy::Never,
        };

        ward_directory(root, init_options).unwrap();

        let dir1_ward_mtime_before = fs::metadata(root.join("dir1/.treeward"))
            .unwrap()
            .modified()
            .unwrap();

        fs::write(root.join("dir2/file3.txt"), "content3").unwrap();

        let update_options = WardOptions {
            init: false,
            allow_init: false,
            fingerprint: None,
            dry_run: false,
            checksum_policy: ChecksumPolicy::Never,
        };

        let result = ward_directory(root, update_options).unwrap();

        // We rely on the OS having high precision mtimes. Otherwise we'd
        // need to sleep or mock the file system.
        let dir1_ward_mtime_after = fs::metadata(root.join("dir1/.treeward"))
            .unwrap()
            .modified()
            .unwrap();

        assert_eq!(dir1_ward_mtime_before, dir1_ward_mtime_after);

        assert_eq!(result.ward_files_updated.len(), 1);
        assert!(
            result
                .ward_files_updated
                .contains(&PathBuf::from("dir2/.treeward"))
        );
        assert!(
            !result
                .ward_files_updated
                .contains(&PathBuf::from(".treeward"))
        );
        assert!(
            !result
                .ward_files_updated
                .contains(&PathBuf::from("dir1/.treeward"))
        );
    }

    #[test]
    fn test_complex_directory_tree() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        fs::write(root.join("file1.txt"), "content1").unwrap();
        fs::create_dir(root.join("dir1")).unwrap();
        fs::write(root.join("dir1/file2.txt"), "content2").unwrap();
        fs::create_dir(root.join("dir1/dir2")).unwrap();
        fs::write(root.join("dir1/dir2/file3.txt"), "content3").unwrap();
        unix::fs::symlink("file1.txt", root.join("link1")).unwrap();

        let options = WardOptions {
            init: true,
            allow_init: false,
            fingerprint: None,
            dry_run: false,
            checksum_policy: ChecksumPolicy::Never,
        };

        let result = ward_directory(root, options).unwrap();

        assert_eq!(result.files_warded, 3);

        assert!(root.join(".treeward").exists());
        assert!(root.join("dir1/.treeward").exists());
        assert!(root.join("dir1/dir2/.treeward").exists());

        let root_ward = WardFile::load(&root.join(".treeward")).unwrap();
        assert!(root_ward.entries.contains_key("file1.txt"));
        assert!(root_ward.entries.contains_key("dir1"));
        assert!(root_ward.entries.contains_key("link1"));

        match root_ward.entries.get("link1").unwrap() {
            WardEntry::Symlink { symlink_target } => {
                assert_eq!(symlink_target, &PathBuf::from("file1.txt"));
            }
            _ => panic!("Expected symlink entry"),
        }
    }

    #[test]
    fn test_incremental_ward_efficiency() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        fs::write(root.join("file1.txt"), "content1").unwrap();
        fs::write(root.join("file2.txt"), "content2").unwrap();
        fs::write(root.join("file3.txt"), "content3").unwrap();

        let init_options = WardOptions {
            init: true,
            allow_init: false,
            fingerprint: None,
            dry_run: false,
            checksum_policy: ChecksumPolicy::Never,
        };

        let init_result = ward_directory(root, init_options).unwrap();
        assert_eq!(init_result.files_warded, 3);

        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::write(root.join("file2.txt"), "modified").unwrap();

        let update_options = WardOptions {
            init: false,
            allow_init: false,
            fingerprint: None,
            dry_run: false,
            checksum_policy: ChecksumPolicy::Never,
        };

        let update_result = ward_directory(root, update_options).unwrap();

        assert_eq!(update_result.files_warded, 1);
        assert_eq!(update_result.ward_files_updated.len(), 1);
    }

    #[test]
    fn test_empty_directory() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        let options = WardOptions {
            init: true,
            allow_init: false,
            fingerprint: None,
            dry_run: false,
            checksum_policy: ChecksumPolicy::Never,
        };

        let result = ward_directory(root, options).unwrap();

        assert_eq!(result.files_warded, 0);
        assert!(root.join(".treeward").exists());

        let ward = WardFile::load(&root.join(".treeward")).unwrap();
        assert_eq!(ward.entries.len(), 0);
    }

    /// The init option should only be required when the top-level directory
    /// operated upon lacks a .treeward file. This ensures we do not fail
    /// when subdirectories are new.
    #[test]
    fn test_new_subdirectory_without_init() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        fs::write(root.join("file1.txt"), "content1").unwrap();

        let init_options = WardOptions {
            init: true,
            allow_init: false,
            fingerprint: None,
            dry_run: false,
            checksum_policy: ChecksumPolicy::Never,
        };

        ward_directory(root, init_options).unwrap();

        fs::create_dir(root.join("newdir")).unwrap();
        fs::write(root.join("newdir/file2.txt"), "content2").unwrap();

        let options = WardOptions {
            init: false,
            allow_init: false,
            fingerprint: None,
            dry_run: false,
            checksum_policy: ChecksumPolicy::Never,
        };

        let result = ward_directory(root, options);

        assert!(result.is_ok());
        assert!(root.join("newdir/.treeward").exists());

        let newdir_ward = WardFile::load(&root.join("newdir/.treeward")).unwrap();
        assert!(newdir_ward.entries.contains_key("file2.txt"));
    }

    #[test]
    fn test_ward_write_permission_denied() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let root = temp.path();

        fs::write(root.join("file1.txt"), "content1").unwrap();

        let mut perms = fs::metadata(root).unwrap().permissions();
        perms.set_mode(0o555);
        fs::set_permissions(root, perms.clone()).unwrap();

        let options = WardOptions {
            init: true,
            allow_init: false,
            fingerprint: None,
            dry_run: false,
            checksum_policy: ChecksumPolicy::Never,
        };

        let result = ward_directory(root, options);

        perms.set_mode(0o755);
        fs::set_permissions(root, perms).unwrap();

        assert!(result.is_err());
        match result {
            Err(WardError::WardFile(crate::ward_file::WardFileError::PermissionDenied(_))) => {}
            other => panic!("Expected WardFile(PermissionDenied) error, got {:?}", other),
        }

        assert!(!root.join(".treeward").exists());
    }

    #[test]
    fn test_ward_write_permission_denied_subdirectory() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let root = temp.path();

        fs::write(root.join("file1.txt"), "content1").unwrap();

        let init_options = WardOptions {
            init: true,
            allow_init: false,
            fingerprint: None,
            dry_run: false,
            checksum_policy: ChecksumPolicy::Never,
        };

        ward_directory(root, init_options).unwrap();

        // Create a new subdirectory and immediately make it read-only.
        // This prevents the .treeward file being created.
        fs::create_dir(root.join("newsubdir")).unwrap();
        fs::write(root.join("newsubdir/file2.txt"), "content2").unwrap();

        let mut perms = fs::metadata(root.join("newsubdir")).unwrap().permissions();
        perms.set_mode(0o555);
        fs::set_permissions(root.join("newsubdir"), perms.clone()).unwrap();

        let options = WardOptions {
            init: false,
            allow_init: false,
            fingerprint: None,
            dry_run: false,
            checksum_policy: ChecksumPolicy::Never,
        };

        let result = ward_directory(root, options);

        perms.set_mode(0o755);
        fs::set_permissions(root.join("newsubdir"), perms).unwrap();

        assert!(result.is_err());
        assert!(
            matches!(
                result,
                Err(WardError::WardFile(
                    crate::ward_file::WardFileError::PermissionDenied(_)
                ))
            ),
            "Expected WardFile(PermissionDenied) error, got {:?}",
            result
        );
    }
}
