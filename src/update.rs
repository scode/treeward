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
use crate::ward_file::{WardEntry, WardFile, WardFileError};
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
    /// Something other than a regular file sits at the root `.treeward` path.
    ///
    /// Covers directories and symlinks (looping, dangling, or resolving).
    /// Refusing is the only safe answer: `init` must not replace whatever the
    /// user put there, and `update` must not follow a link to some other file
    /// or, via its atomic rename, quietly turn the link into a regular file.
    #[error(
        "root .treeward at {0} is not a regular file (directory or symlink); refusing to read or replace it"
    )]
    RootWardNotRegularFile(PathBuf),
    #[error(
        "Fingerprint mismatch: expected {expected}, got {actual}. Ensure --verify/--always-verify flags match between status and init/update commands."
    )]
    FingerprintMismatch { expected: String, actual: String },
    /// A ward file write failed during the write phase, possibly after other
    /// ward files in the same run were already committed.
    ///
    /// Every write-phase failure takes this variant, including one on the very
    /// first file (`written == 0`), so the user always learns how far the run
    /// got. Ward files are written one directory at a time with no journal or
    /// rollback, so a partially updated tree is reachable by design. Two
    /// guarantees still hold and are what make re-running a safe recovery:
    /// every written ward file is individually complete (each write is
    /// temp-file-plus-rename), and ward files are written deepest-first, so no
    /// committed ward vouches for a subdirectory whose own ward is stale or
    /// missing. The unwritten ancestors therefore still report the pending
    /// changes at the level the user reviewed them.
    ///
    /// `written` counts ward files committed before the failure; `total` is
    /// how many needed writing in this run. Re-running after fixing the cause
    /// writes only the remainder. A fingerprint taken before the failed run no
    /// longer matches, because the committed wards shrank the pending
    /// changeset; the user has to take a fresh one from `status`.
    #[error(
        "Ward file error: {source}. Wrote {written} of {total} changed ward files before failing. Those already \
         written are complete, and no directory's ward file was written before its subdirectories'. Fix the cause \
         and re-run to write the rest; a fingerprint from before this failure no longer matches, so re-run status \
         first if you use --fingerprint."
    )]
    PartialWrite {
        written: usize,
        total: usize,
        source: WardFileError,
    },
}

/// Orders pending ward writes so that no directory's ward file is written
/// before those of its descendants.
///
/// The only cross-file reference in the on-disk format points downward: a
/// parent's `Dir` entry asserts that the child directory is warded. Writing
/// deepest-first keeps that assertion true at every point during the write
/// phase, so a failure part way through never leaves a committed ward that
/// vouches for a stale or missing child ward. Ties at equal depth are broken
/// by path for deterministic output.
fn order_deepest_first(pending: &mut [(PathBuf, &WardFile)]) {
    pending.sort_by(|(a, _), (b, _)| {
        b.components()
            .count()
            .cmp(&a.components().count())
            .then_with(|| a.cmp(b))
    });
}

/// Reports whether a regular ward file occupies the root `.treeward` path,
/// refusing to proceed if anything else does.
///
/// `Path::exists()` was the wrong tool for the initialized/uninitialized
/// guards because it follows symlinks and answers `false` for *any* stat
/// failure. A `.treeward` symlink caught in a loop was diagnosed as "Not
/// initialized" and the user sent to `init`, which then failed with the raw
/// ELOOP error; a dangling `.treeward` symlink let `update` silently replace
/// the link with a regular file. Neither command should be guessing about a
/// ward path that is not a plain file, so anything else there (a directory,
/// or a symlink whether or not it resolves) is a fatal error naming the path.
/// The only outcomes are: regular file present, nothing present, or an error.
///
/// Inspecting with `symlink_metadata` means the link itself is judged, never
/// its target; treeward never follows symlinks, and a ward file reached
/// through one would be replaced rather than updated by the atomic rename in
/// `save` anyway. Only `NotFound` means absent; any other inspection failure
/// propagates as the I/O error it is instead of being read as a verdict.
fn root_ward_present(ward_path: &Path) -> Result<bool, WardError> {
    match std::fs::symlink_metadata(ward_path) {
        Ok(md) if md.is_file() => Ok(true),
        Ok(_) => Err(WardError::RootWardNotRegularFile(ward_path.to_path_buf())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(WardError::DirList(DirListError::Io(e))),
    }
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
///   - `init`: This is a first-time initialization; fails if a ward already exists
///   - `allow_init`: Accept both initialized and uninitialized roots (idempotent mode)
///   - `fingerprint`: Optional fingerprint from `compute_status()` to validate before writing
///   - `dry_run`: Preview what would be updated without writing any files
///   - `checksum_policy`: When to checksum files; affects reported status types and
///     therefore fingerprint validation (must match the policy used to produce the
///     fingerprint)
///
/// # Behavior
///
/// **Efficient checksumming:**
/// - Only checksums files that are new or have changed metadata (mtime/size)
/// - Files with matching metadata reuse checksums from existing ward files
/// - This makes incremental warding very fast (only checksums what changed)
///
/// **Initialization:**
/// - Without `init` or `allow_init`, a missing root `.treeward` is a `NotInitialized` error
/// - With `init` (and not `allow_init`), an existing root `.treeward` is an
///   `AlreadyInitialized` error - `init` asserts first-time use
/// - `allow_init` bypasses both checks, accepting either state
/// - Anything other than a regular file at the root `.treeward` path (a
///   directory, or a symlink whether or not it resolves) is a
///   `RootWardNotRegularFile` error regardless of these flags; see
///   `root_ward_present`
/// - These checks only apply to the root directory - subdirectories always
///   have `.treeward` files created as needed
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
/// **Failure partway through writing:**
/// - The write phase is not atomic across directories. Each ward file is
///   written atomically, deepest directories first, and the first failure
///   aborts with `WardError::PartialWrite` reporting how far it got.
/// - Ward files already written stay written; the unwritten ancestors still
///   report the pending changes. Re-running completes the update.
///
/// # Returns
///
/// * `files_warded` - Number of files that required checksumming for ward entries (added,
///   modified, or possibly modified; excludes unchanged files and directories/symlinks).
///   Unchanged files may still be checksummed when using `--always-verify`.
/// * `ward_files_updated` - Relative paths of `.treeward` files that were written (or
///   would be written in dry-run mode)
pub fn ward_directory(root: &Path, options: WardOptions) -> Result<WardResult, WardError> {
    let root = root.canonicalize().map_err(DirListError::Io)?;

    let ward_path = root.join(".treeward");

    let ward_present = root_ward_present(&ward_path)?;
    if !options.init && !options.allow_init && !ward_present {
        return Err(WardError::NotInitialized);
    }

    if options.init && !options.allow_init && ward_present {
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

    // Decide which ward files need writing before writing any of them, so the
    // write phase knows the total up front and can report exactly how far it
    // got if it fails partway.
    let mut pending: Vec<(PathBuf, &WardFile)> = Vec::new();
    for (dir_path, ward_file) in &ward_files {
        let ward_path = dir_path.join(".treeward");
        let existing = WardFile::load_if_exists(&ward_path)?;
        if existing.as_ref() != Some(ward_file) {
            pending.push((ward_path, ward_file));
        }
    }
    order_deepest_first(&mut pending);

    // Write phase. The set is not atomic (each file is, via temp+rename), so a
    // failure here leaves a partially updated tree by design; see
    // `WardError::PartialWrite` for the guarantees that still hold.
    let total = pending.len();
    if !options.dry_run {
        for (written, (ward_path, ward_file)) in pending.iter().enumerate() {
            // `written` can undercount by one: `save` reports failure if the
            // post-rename directory fsync fails, even though the rename itself
            // landed. That only makes the count conservative; a re-run finds
            // that ward already matching and skips it.
            if let Err(source) = ward_file.save(ward_path) {
                return Err(WardError::PartialWrite {
                    written,
                    total,
                    source,
                });
            }
        }
    }

    // Report in directory order regardless of write order: the deepest-first
    // order is a recovery property, not something a reader of the log needs to
    // see. Sorting by the containing directory (not the `.treeward` path)
    // reproduces the order the original per-directory map iterated in, so the
    // `-v` listing is unchanged.
    let mut ward_files_updated = pending
        .iter()
        .map(|(ward_path, _)| ward_path.strip_prefix(&root).map(Path::to_path_buf))
        .collect::<Result<Vec<_>, _>>()?;
    ward_files_updated.sort_by(|a, b| a.parent().cmp(&b.parent()));

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
                matches!(ward_entry, Some(WardEntry::File { .. }))
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
    #[cfg(unix)]
    fn test_ward_directory_accepts_symlinked_root() {
        let temp = TempDir::new().unwrap();
        let real_root = temp.path().join("real");
        let linked_root = temp.path().join("linked");
        fs::create_dir(&real_root).unwrap();
        unix::fs::symlink(&real_root, &linked_root).unwrap();

        fs::create_dir(real_root.join("dir")).unwrap();
        fs::write(real_root.join("dir/file.txt"), "content").unwrap();

        let options = WardOptions {
            init: true,
            allow_init: false,
            fingerprint: None,
            dry_run: false,
            checksum_policy: ChecksumPolicy::Never,
        };

        let result = ward_directory(&linked_root, options).unwrap();

        assert!(
            result
                .ward_files_updated
                .contains(&PathBuf::from(".treeward"))
        );
        assert!(
            result
                .ward_files_updated
                .contains(&PathBuf::from("dir/.treeward"))
        );
        assert!(real_root.join(".treeward").exists());
        assert!(real_root.join("dir/.treeward").exists());
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

    /// Anything other than a regular file at the root `.treeward` path must be
    /// a fatal error for both `init` and `update`, never a guard verdict and
    /// never something the commands write through or over. Before this was
    /// pinned, `Path::exists()` made a looping symlink read as "no ward file"
    /// (so `update` said "Not initialized" and `init` died with a raw ELOOP),
    /// and a dangling symlink let `update` silently replace the link with a
    /// regular file. Each case checks that the offending entry is untouched
    /// afterwards.
    #[test]
    #[cfg(unix)]
    fn test_non_regular_file_at_root_ward_path_is_refused() {
        type Setup<'a> = &'a dyn Fn(&Path);
        let cases: [(&str, Setup); 3] = [
            ("looping symlink", &|root| {
                unix::fs::symlink("loop", root.join(".treeward")).unwrap();
                unix::fs::symlink(".treeward", root.join("loop")).unwrap();
            }),
            ("dangling symlink", &|root| {
                unix::fs::symlink("nowhere", root.join(".treeward")).unwrap();
            }),
            ("directory", &|root| {
                fs::create_dir(root.join(".treeward")).unwrap();
            }),
        ];

        for (label, setup) in cases {
            let temp = TempDir::new().unwrap();
            let root = temp.path();
            fs::write(root.join("file1.txt"), "content1").unwrap();
            setup(root);
            let before = fs::symlink_metadata(root.join(".treeward")).unwrap();

            for (init, allow_init) in [(false, false), (true, false), (false, true)] {
                let result = ward_directory(
                    root,
                    WardOptions {
                        init,
                        allow_init,
                        fingerprint: None,
                        dry_run: false,
                        checksum_policy: ChecksumPolicy::Never,
                    },
                );
                match result {
                    Err(WardError::RootWardNotRegularFile(p)) => {
                        assert_eq!(p, root.canonicalize().unwrap().join(".treeward"))
                    }
                    other => panic!(
                        "{label}, init={init}, allow_init={allow_init}: expected \
                         RootWardNotRegularFile, got {:?}",
                        other
                    ),
                }
            }

            let after = fs::symlink_metadata(root.join(".treeward")).unwrap();
            assert_eq!(
                before.file_type(),
                after.file_type(),
                "{label}: entry was replaced"
            );
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
        let ward_path = root.join(".treeward");
        let ward_before = fs::read_to_string(&ward_path).unwrap();

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

        let ward_after = fs::read_to_string(&ward_path).unwrap();
        assert_eq!(
            ward_before, ward_after,
            "fingerprint mismatch must not rewrite the existing ward file"
        );
        assert!(
            !ward_after.contains("file2.txt"),
            "failed update must not accept the new file into the ward"
        );
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
    #[cfg(unix)]
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
    #[cfg(unix)]
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

        match result {
            Err(WardError::PartialWrite {
                written: 0,
                total: 1,
                source: crate::ward_file::WardFileError::PermissionDenied(_),
            }) => {}
            other => panic!(
                "expected PartialWrite(0 of 1) wrapping PermissionDenied, got {:?}",
                other
            ),
        }

        assert!(!root.join(".treeward").exists());
    }

    #[test]
    #[cfg(unix)]
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

        let root_ward_before = fs::read(root.join(".treeward")).unwrap();
        let result = ward_directory(root, options);

        perms.set_mode(0o755);
        fs::set_permissions(root.join("newsubdir"), perms).unwrap();

        // The child write fails first (deepest-first order), so nothing has
        // been written yet: the failure is reported as 0 of 2 and the root
        // ward must not have advanced to reference a subdirectory whose own
        // ward was never created.
        match result {
            Err(WardError::PartialWrite {
                written,
                total,
                source: crate::ward_file::WardFileError::PermissionDenied(_),
            }) => {
                assert_eq!((written, total), (0, 2));
            }
            other => panic!(
                "expected PartialWrite wrapping PermissionDenied, got {:?}",
                other
            ),
        }
        assert_eq!(fs::read(root.join(".treeward")).unwrap(), root_ward_before);
        assert!(!root.join("newsubdir/.treeward").exists());
    }

    /// The write order must put every directory after all of its descendants
    /// so a mid-run failure never leaves a committed parent vouching for a
    /// stale child. Equal depths sort by path for determinism. Checked on the
    /// pure ordering helper so the test does not depend on provoking a real
    /// write failure at each level.
    #[test]
    fn test_order_deepest_first() {
        let ward = WardFile::new(std::collections::BTreeMap::new());
        let mk = |p: &str| (PathBuf::from(p), &ward);
        let mut pending = vec![
            mk("/r/.treeward"),
            mk("/r/a/.treeward"),
            mk("/r/b/x/.treeward"),
            mk("/r/b/.treeward"),
            mk("/r/a/y/z/.treeward"),
            mk("/r/a/y/.treeward"),
        ];

        order_deepest_first(&mut pending);

        let order: Vec<&str> = pending.iter().map(|(p, _)| p.to_str().unwrap()).collect();
        assert_eq!(
            order,
            [
                "/r/a/y/z/.treeward",
                "/r/a/y/.treeward",
                "/r/b/x/.treeward",
                "/r/a/.treeward",
                "/r/b/.treeward",
                "/r/.treeward",
            ]
        );

        // Property form of the same guarantee, independent of the exact list:
        // nothing written after `p` may live inside `p`'s directory. (An
        // earlier version of this loop looked at entries written *before* `p`
        // and was tautologically true; it passed on a fully reversed order.)
        for (i, (p, _)) in pending.iter().enumerate() {
            let dir = p.parent().unwrap();
            for (q, _) in &pending[i + 1..] {
                assert!(
                    !q.starts_with(dir),
                    "{} was written before its descendant {}",
                    p.display(),
                    q.display()
                );
            }
        }
    }

    /// When the root write fails but a subdirectory's succeeds, the failure
    /// must report the partial progress and leave the subdirectory's ward
    /// committed. This is the case the deepest-first order is designed for:
    /// the root ward is stale, so the new subdirectory still shows up as
    /// pending at the root level and a re-run finishes the job.
    #[test]
    #[cfg(unix)]
    fn test_ward_write_root_failure_leaves_children_committed() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let root = temp.path();
        fs::write(root.join("file1.txt"), "content1").unwrap();
        fs::create_dir(root.join("sub")).unwrap();
        fs::write(root.join("sub/old.txt"), "old").unwrap();

        let options = |init: bool| WardOptions {
            init,
            allow_init: false,
            fingerprint: None,
            dry_run: false,
            checksum_policy: ChecksumPolicy::Never,
        };
        ward_directory(root, options(true)).unwrap();

        // Change both levels so both ward files need writing, then make only
        // the root unwritable.
        fs::write(root.join("file_new.txt"), "new").unwrap();
        fs::write(root.join("sub/new.txt"), "new").unwrap();
        let root_ward_before = fs::read(root.join(".treeward")).unwrap();
        let sub_ward_before = fs::read(root.join("sub/.treeward")).unwrap();

        let mut perms = fs::metadata(root).unwrap().permissions();
        perms.set_mode(0o555);
        fs::set_permissions(root, perms.clone()).unwrap();

        let result = ward_directory(root, options(false));

        perms.set_mode(0o755);
        fs::set_permissions(root, perms).unwrap();

        match result {
            Err(WardError::PartialWrite { written, total, .. }) => {
                assert_eq!((written, total), (1, 2));
            }
            other => panic!("expected PartialWrite, got {:?}", other),
        }
        assert_eq!(fs::read(root.join(".treeward")).unwrap(), root_ward_before);
        assert_ne!(
            fs::read(root.join("sub/.treeward")).unwrap(),
            sub_ward_before
        );

        // Re-running writes only the remainder and converges.
        let healed = ward_directory(root, options(false)).unwrap();
        assert_eq!(healed.ward_files_updated, vec![PathBuf::from(".treeward")]);
        assert!(
            ward_directory(root, options(false))
                .unwrap()
                .ward_files_updated
                .is_empty()
        );
    }
}
