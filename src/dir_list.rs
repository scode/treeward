//! Non-recursive directory listing for the treeward checksumming tool.
//!
//! This module provides functionality to list the immediate children of a directory,
//! collecting filesystem metadata (mtime, size, symlink targets) for each entry.
//! The listing is non-recursive - each directory has its own `.treeward` file
//! containing only its immediate children, allowing directories to be moved
//! independently while maintaining their integrity information.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

const TREEWARD_FILENAME: &str = ".treeward";

#[derive(Debug, thiserror::Error)]
pub enum DirListError {
    #[error("IO error: {0}")]
    Io(std::io::Error),
    #[error("Permission denied: {0}")]
    PermissionDenied(PathBuf),
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsEntry {
    pub relative_path: PathBuf,
    pub metadata: EntryMetadata,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryMetadata {
    File { mtime: SystemTime, size: u64 },
    Dir { mtime: SystemTime },
    Symlink { target: PathBuf },
}

#[allow(dead_code)]
pub fn list_directory(root: &Path) -> Result<Vec<FsEntry>, DirListError> {
    let root = root.canonicalize().map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            DirListError::PermissionDenied(root.to_path_buf())
        } else {
            DirListError::Io(e)
        }
    })?;

    let read_dir = std::fs::read_dir(&root).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            DirListError::PermissionDenied(root.to_path_buf())
        } else {
            DirListError::Io(e)
        }
    })?;

    let mut entries = Vec::new();

    for entry in read_dir {
        let entry = entry.map_err(DirListError::Io)?;
        let path = entry.path();

        if path.file_name() == Some(std::ffi::OsStr::new(TREEWARD_FILENAME)) {
            continue;
        }

        let metadata = std::fs::symlink_metadata(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                DirListError::PermissionDenied(path.clone())
            } else {
                DirListError::Io(e)
            }
        })?;

        let relative_path = path
            .file_name()
            .ok_or_else(|| DirListError::Io(std::io::Error::other("Failed to get filename")))?
            .into();

        let file_type = metadata.file_type();

        let entry_metadata = if file_type.is_symlink() {
            let target = std::fs::read_link(&path).map_err(|e| {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    DirListError::PermissionDenied(path.clone())
                } else {
                    DirListError::Io(e)
                }
            })?;
            EntryMetadata::Symlink { target }
        } else if file_type.is_dir() {
            let mtime = metadata.modified().map_err(DirListError::Io)?;
            EntryMetadata::Dir { mtime }
        } else {
            let mtime = metadata.modified().map_err(DirListError::Io)?;
            let size = metadata.len();
            EntryMetadata::File { mtime, size }
        };

        entries.push(FsEntry {
            relative_path,
            metadata: entry_metadata,
        });
    }

    entries.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_traverse_simple_directory() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        fs::write(root.join("file1.txt"), "content1").unwrap();
        fs::write(root.join("file2.txt"), "content2").unwrap();
        fs::create_dir(root.join("dir1")).unwrap();
        fs::write(root.join("dir1/file3.txt"), "content3").unwrap();

        let entries = list_directory(root).unwrap();

        assert_eq!(entries.len(), 3);

        assert_eq!(entries[0].relative_path, Path::new("dir1"));
        assert!(matches!(entries[0].metadata, EntryMetadata::Dir { .. }));

        assert_eq!(entries[1].relative_path, Path::new("file1.txt"));
        assert!(matches!(entries[1].metadata, EntryMetadata::File { .. }));

        assert_eq!(entries[2].relative_path, Path::new("file2.txt"));
        assert!(matches!(entries[2].metadata, EntryMetadata::File { .. }));

        let subdir_entries = list_directory(&root.join("dir1")).unwrap();
        assert_eq!(subdir_entries.len(), 1);
        assert_eq!(subdir_entries[0].relative_path, Path::new("file3.txt"));
        assert!(matches!(
            subdir_entries[0].metadata,
            EntryMetadata::File { .. }
        ));
    }

    #[test]
    #[cfg(unix)]
    fn test_traverse_with_symlink() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        fs::write(root.join("target.txt"), "content").unwrap();
        std::os::unix::fs::symlink(root.join("target.txt"), root.join("link.txt")).unwrap();

        let entries = list_directory(root).unwrap();

        assert_eq!(entries.len(), 2);

        let link_entry = entries
            .iter()
            .find(|e| e.relative_path == Path::new("link.txt"))
            .unwrap();
        assert!(matches!(link_entry.metadata, EntryMetadata::Symlink { .. }));
        if let EntryMetadata::Symlink { target } = &link_entry.metadata {
            assert!(target.ends_with("target.txt"));
        }
    }

    #[test]
    fn test_traverse_empty_directory() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        let entries = list_directory(root).unwrap();

        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn test_traverse_nested_directories() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        fs::create_dir(root.join("dir1")).unwrap();
        fs::create_dir(root.join("dir1/dir2")).unwrap();
        fs::create_dir(root.join("dir1/dir2/dir3")).unwrap();
        fs::write(root.join("dir1/dir2/dir3/file.txt"), "content").unwrap();

        let entries = list_directory(root).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].relative_path, Path::new("dir1"));
        assert!(matches!(entries[0].metadata, EntryMetadata::Dir { .. }));

        let entries = list_directory(&root.join("dir1")).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].relative_path, Path::new("dir2"));
        assert!(matches!(entries[0].metadata, EntryMetadata::Dir { .. }));

        let entries = list_directory(&root.join("dir1/dir2")).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].relative_path, Path::new("dir3"));
        assert!(matches!(entries[0].metadata, EntryMetadata::Dir { .. }));

        let entries = list_directory(&root.join("dir1/dir2/dir3")).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].relative_path, Path::new("file.txt"));
        assert!(matches!(entries[0].metadata, EntryMetadata::File { .. }));
    }

    #[test]
    fn test_traverse_excludes_treeward_files() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        fs::write(root.join("file1.txt"), "content1").unwrap();
        fs::write(root.join(TREEWARD_FILENAME), "ward content").unwrap();
        fs::create_dir(root.join("dir1")).unwrap();
        fs::write(root.join("dir1/file2.txt"), "content2").unwrap();
        fs::write(root.join("dir1").join(TREEWARD_FILENAME), "ward content").unwrap();

        let entries = list_directory(root).unwrap();

        assert_eq!(entries.len(), 2);
        assert!(
            entries
                .iter()
                .all(|e| e.relative_path.file_name()
                    != Some(std::ffi::OsStr::new(TREEWARD_FILENAME)))
        );

        let subdir_entries = list_directory(&root.join("dir1")).unwrap();
        assert_eq!(subdir_entries.len(), 1);
        assert!(
            subdir_entries
                .iter()
                .all(|e| e.relative_path.file_name()
                    != Some(std::ffi::OsStr::new(TREEWARD_FILENAME)))
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_traverse_permission_denied() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        let restricted_dir = root.join("restricted");
        fs::create_dir(&restricted_dir).unwrap();

        let mut perms = fs::metadata(&restricted_dir).unwrap().permissions();
        perms.set_mode(0o000);
        fs::set_permissions(&restricted_dir, perms.clone()).unwrap();

        let result = list_directory(&restricted_dir);

        perms.set_mode(0o755);
        fs::set_permissions(&restricted_dir, perms).unwrap();

        assert!(result.is_err());
        match result {
            Err(DirListError::PermissionDenied(_)) => {}
            _ => panic!("Expected PermissionDenied error"),
        }
    }

    #[test]
    #[cfg(unix)]
    fn test_traverse_broken_symlink() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        std::os::unix::fs::symlink("/nonexistent/target", root.join("broken_link")).unwrap();

        let entries = list_directory(root).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].relative_path, Path::new("broken_link"));
        assert!(matches!(entries[0].metadata, EntryMetadata::Symlink { .. }));
        if let EntryMetadata::Symlink { target } = &entries[0].metadata {
            assert_eq!(target, &PathBuf::from("/nonexistent/target"));
        }
    }

    #[test]
    #[cfg(unix)]
    fn test_traverse_symlink_to_inaccessible_file() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        let restricted_dir = root.join("restricted");
        fs::create_dir(&restricted_dir).unwrap();
        fs::write(restricted_dir.join("target.txt"), "content").unwrap();

        let mut perms = fs::metadata(&restricted_dir).unwrap().permissions();
        perms.set_mode(0o000);
        fs::set_permissions(&restricted_dir, perms.clone()).unwrap();

        std::os::unix::fs::symlink(restricted_dir.join("target.txt"), root.join("link")).unwrap();

        let result = list_directory(root);

        perms.set_mode(0o755);
        fs::set_permissions(&restricted_dir, perms).unwrap();

        assert!(result.is_ok());
        let entries = result.unwrap();
        assert_eq!(entries.len(), 2);

        let link_entry = entries
            .iter()
            .find(|e| e.relative_path == Path::new("link"))
            .unwrap();
        assert!(matches!(link_entry.metadata, EntryMetadata::Symlink { .. }));
        if let EntryMetadata::Symlink { target } = &link_entry.metadata {
            assert!(target.ends_with("target.txt"));
        }

        let restricted_entry = entries
            .iter()
            .find(|e| e.relative_path == Path::new("restricted"))
            .unwrap();
        assert!(matches!(
            restricted_entry.metadata,
            EntryMetadata::Dir { .. }
        ));
    }

    #[test]
    fn test_traverse_deterministic_ordering() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        fs::write(root.join("zebra.txt"), "z").unwrap();
        fs::write(root.join("apple.txt"), "a").unwrap();
        fs::write(root.join("banana.txt"), "b").unwrap();

        let entries1 = list_directory(root).unwrap();
        let entries2 = list_directory(root).unwrap();

        assert_eq!(entries1.len(), 3);
        assert_eq!(entries1[0].relative_path, Path::new("apple.txt"));
        assert_eq!(entries1[1].relative_path, Path::new("banana.txt"));
        assert_eq!(entries1[2].relative_path, Path::new("zebra.txt"));

        assert_eq!(entries1, entries2);
    }

    #[test]
    fn test_metadata_collection_files() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        fs::write(root.join("test.txt"), "content").unwrap();

        let entries = list_directory(root).unwrap();

        assert_eq!(entries.len(), 1);
        assert!(matches!(entries[0].metadata, EntryMetadata::File { .. }));
        if let EntryMetadata::File { size, .. } = &entries[0].metadata {
            assert_eq!(*size, 7);
        }
    }

    #[test]
    fn test_metadata_collection_directories() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        fs::create_dir(root.join("testdir")).unwrap();

        let entries = list_directory(root).unwrap();

        assert_eq!(entries.len(), 1);
        assert!(matches!(entries[0].metadata, EntryMetadata::Dir { .. }));
    }

    #[test]
    #[cfg(unix)]
    fn test_metadata_collection_symlinks() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        std::os::unix::fs::symlink("/some/target", root.join("link")).unwrap();

        let entries = list_directory(root).unwrap();

        assert_eq!(entries.len(), 1);
        assert!(matches!(entries[0].metadata, EntryMetadata::Symlink { .. }));
        if let EntryMetadata::Symlink { target } = &entries[0].metadata {
            assert_eq!(target, &PathBuf::from("/some/target"));
        }
    }
}
