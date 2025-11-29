//! Non-recursive directory listing for the treeward checksumming tool.
//!
//! This module provides functionality to list the immediate children of a directory,
//! collecting filesystem metadata (mtime, size, symlink targets) for each entry.
//! The listing is non-recursive - each directory has its own `.treeward` file
//! containing only its immediate children, allowing directories to be moved
//! independently while maintaining their integrity information.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const TREEWARD_FILENAME: &str = ".treeward";

#[derive(Debug, thiserror::Error)]
pub enum DirListError {
    #[error("IO error: {0}")]
    Io(std::io::Error),
    #[error("Permission denied: {0}")]
    PermissionDenied(PathBuf),
    #[error("non-UTF-8 path not supported: {0:?}")]
    NonUtf8Path(PathBuf),
    #[error("unsupported file type (not a regular file, directory, or symlink): {0}")]
    UnsupportedFileType(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsEntry {
    File { mtime: SystemTime, size: u64 },
    Dir { mtime: SystemTime },
    Symlink { symlink_target: PathBuf },
}

pub fn list_directory(root: &Path) -> Result<BTreeMap<String, FsEntry>, DirListError> {
    let read_dir = std::fs::read_dir(root).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            DirListError::PermissionDenied(root.to_path_buf())
        } else {
            DirListError::Io(e)
        }
    })?;

    let mut entries = BTreeMap::new();

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

        let filename = path
            .file_name()
            .ok_or_else(|| DirListError::Io(std::io::Error::other("Failed to get filename")))?
            .to_str()
            .ok_or_else(|| DirListError::NonUtf8Path(path.clone()))?
            .to_string();

        let file_type = metadata.file_type();

        let fs_entry = if file_type.is_symlink() {
            let symlink_target = std::fs::read_link(&path).map_err(|e| {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    DirListError::PermissionDenied(path.clone())
                } else {
                    DirListError::Io(e)
                }
            })?;
            FsEntry::Symlink { symlink_target }
        } else if file_type.is_dir() {
            let mtime = metadata.modified().map_err(DirListError::Io)?;
            FsEntry::Dir { mtime }
        } else if file_type.is_file() {
            let mtime = metadata.modified().map_err(DirListError::Io)?;
            let size = metadata.len();
            FsEntry::File { mtime, size }
        } else {
            return Err(DirListError::UnsupportedFileType(path));
        };

        entries.insert(filename, fs_entry);
    }

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

        assert!(entries.contains_key("dir1"));
        assert!(matches!(entries.get("dir1").unwrap(), FsEntry::Dir { .. }));

        assert!(entries.contains_key("file1.txt"));
        assert!(matches!(
            entries.get("file1.txt").unwrap(),
            FsEntry::File { .. }
        ));

        assert!(entries.contains_key("file2.txt"));
        assert!(matches!(
            entries.get("file2.txt").unwrap(),
            FsEntry::File { .. }
        ));

        let subdir_entries = list_directory(&root.join("dir1")).unwrap();
        assert_eq!(subdir_entries.len(), 1);
        assert!(subdir_entries.contains_key("file3.txt"));
        assert!(matches!(
            subdir_entries.get("file3.txt").unwrap(),
            FsEntry::File { .. }
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

        assert!(entries.contains_key("link.txt"));
        let link_entry = entries.get("link.txt").unwrap();
        assert!(matches!(link_entry, FsEntry::Symlink { .. }));
        if let FsEntry::Symlink { symlink_target } = link_entry {
            assert!(symlink_target.ends_with("target.txt"));
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
        assert!(entries.contains_key("dir1"));
        assert!(matches!(entries.get("dir1").unwrap(), FsEntry::Dir { .. }));

        let entries = list_directory(&root.join("dir1")).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries.contains_key("dir2"));
        assert!(matches!(entries.get("dir2").unwrap(), FsEntry::Dir { .. }));

        let entries = list_directory(&root.join("dir1/dir2")).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries.contains_key("dir3"));
        assert!(matches!(entries.get("dir3").unwrap(), FsEntry::Dir { .. }));

        let entries = list_directory(&root.join("dir1/dir2/dir3")).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries.contains_key("file.txt"));
        assert!(matches!(
            entries.get("file.txt").unwrap(),
            FsEntry::File { .. }
        ));
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
        assert!(!entries.contains_key(TREEWARD_FILENAME));

        let subdir_entries = list_directory(&root.join("dir1")).unwrap();
        assert_eq!(subdir_entries.len(), 1);
        assert!(!subdir_entries.contains_key(TREEWARD_FILENAME));
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
        assert!(entries.contains_key("broken_link"));
        let entry = entries.get("broken_link").unwrap();
        assert!(matches!(entry, FsEntry::Symlink { .. }));
        if let FsEntry::Symlink { symlink_target } = entry {
            assert_eq!(symlink_target, &PathBuf::from("/nonexistent/target"));
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

        assert!(entries.contains_key("link"));
        let link_entry = entries.get("link").unwrap();
        assert!(matches!(link_entry, FsEntry::Symlink { .. }));
        if let FsEntry::Symlink { symlink_target } = link_entry {
            assert!(symlink_target.ends_with("target.txt"));
        }

        assert!(entries.contains_key("restricted"));
        let restricted_entry = entries.get("restricted").unwrap();
        assert!(matches!(restricted_entry, FsEntry::Dir { .. }));
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

        let keys: Vec<&String> = entries1.keys().collect();
        assert_eq!(keys[0], "apple.txt");
        assert_eq!(keys[1], "banana.txt");
        assert_eq!(keys[2], "zebra.txt");

        assert_eq!(entries1, entries2);
    }

    #[test]
    fn test_metadata_collection_files() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        fs::write(root.join("test.txt"), "content").unwrap();

        let entries = list_directory(root).unwrap();

        assert_eq!(entries.len(), 1);
        assert!(entries.contains_key("test.txt"));
        let entry = entries.get("test.txt").unwrap();
        assert!(matches!(entry, FsEntry::File { .. }));
        if let FsEntry::File { size, .. } = entry {
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
        assert!(entries.contains_key("testdir"));
        assert!(matches!(
            entries.get("testdir").unwrap(),
            FsEntry::Dir { .. }
        ));
    }

    #[test]
    #[cfg(unix)]
    fn test_metadata_collection_symlinks() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        std::os::unix::fs::symlink("/some/target", root.join("link")).unwrap();

        let entries = list_directory(root).unwrap();

        assert_eq!(entries.len(), 1);
        assert!(entries.contains_key("link"));
        let entry = entries.get("link").unwrap();
        assert!(matches!(entry, FsEntry::Symlink { .. }));
        if let FsEntry::Symlink { symlink_target } = entry {
            assert_eq!(symlink_target, &PathBuf::from("/some/target"));
        }
    }

    #[test]
    #[cfg(unix)]
    fn test_symlink_cycle_self_referential() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create a self-referential symlink: link -> link
        std::os::unix::fs::symlink("self", root.join("self")).unwrap();

        // list_directory should succeed because symlinks are not followed
        let entries = list_directory(root).unwrap();

        assert_eq!(entries.len(), 1);
        assert!(entries.contains_key("self"));
        let entry = entries.get("self").unwrap();
        match entry {
            FsEntry::Symlink { symlink_target } => {
                assert_eq!(symlink_target, &PathBuf::from("self"));
            }
            _ => panic!("Expected Symlink entry"),
        }
    }

    #[test]
    #[cfg(unix)]
    fn test_symlink_cycle_mutual() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create mutual symlinks: a -> b, b -> a
        std::os::unix::fs::symlink("b", root.join("a")).unwrap();
        std::os::unix::fs::symlink("a", root.join("b")).unwrap();

        // list_directory should succeed because symlinks are not followed
        let entries = list_directory(root).unwrap();

        assert_eq!(entries.len(), 2);

        assert!(entries.contains_key("a"));
        match entries.get("a").unwrap() {
            FsEntry::Symlink { symlink_target } => {
                assert_eq!(symlink_target, &PathBuf::from("b"));
            }
            _ => panic!("Expected Symlink entry for 'a'"),
        }

        assert!(entries.contains_key("b"));
        match entries.get("b").unwrap() {
            FsEntry::Symlink { symlink_target } => {
                assert_eq!(symlink_target, &PathBuf::from("a"));
            }
            _ => panic!("Expected Symlink entry for 'b'"),
        }
    }

    #[test]
    #[cfg(unix)]
    fn test_symlink_to_parent_directory() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        fs::create_dir(root.join("subdir")).unwrap();
        // Create a symlink in subdir pointing to parent (would cause cycle if followed)
        std::os::unix::fs::symlink("..", root.join("subdir/parent")).unwrap();

        let entries = list_directory(&root.join("subdir")).unwrap();

        assert_eq!(entries.len(), 1);
        assert!(entries.contains_key("parent"));
        match entries.get("parent").unwrap() {
            FsEntry::Symlink { symlink_target } => {
                assert_eq!(symlink_target, &PathBuf::from(".."));
            }
            _ => panic!("Expected Symlink entry"),
        }
    }

    #[test]
    #[cfg(unix)]
    fn test_unsupported_file_type_fifo() {
        use nix::sys::stat;
        use nix::unistd;

        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        let fifo_path = root.join("test_fifo");
        unistd::mkfifo(&fifo_path, stat::Mode::S_IRWXU).unwrap();

        let result = list_directory(root);

        assert!(result.is_err());
        match result {
            Err(DirListError::UnsupportedFileType(path)) => {
                assert_eq!(path, fifo_path);
            }
            _ => panic!("Expected UnsupportedFileType error, got {:?}", result),
        }
    }

    #[test]
    #[cfg(unix)]
    fn test_unsupported_file_type_socket() {
        use std::os::unix::net::UnixListener;

        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        let socket_path = root.join("test_socket");
        let _listener = UnixListener::bind(&socket_path).unwrap();

        let result = list_directory(root);

        assert!(result.is_err());
        match result {
            Err(DirListError::UnsupportedFileType(path)) => {
                assert_eq!(path, socket_path);
            }
            _ => panic!("Expected UnsupportedFileType error, got {:?}", result),
        }
    }

    /// Hard links should be treated as separate files, not deduplicated.
    #[test]
    #[cfg(unix)]
    fn test_hard_links_treated_as_separate_files() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        fs::write(root.join("original.txt"), "content").unwrap();
        fs::hard_link(root.join("original.txt"), root.join("hardlink.txt")).unwrap();

        let entries = list_directory(root).unwrap();

        assert_eq!(entries.len(), 2);
        assert!(entries.contains_key("original.txt"));
        assert!(entries.contains_key("hardlink.txt"));

        let original = entries.get("original.txt").unwrap();
        let hardlink = entries.get("hardlink.txt").unwrap();

        match (original, hardlink) {
            (
                FsEntry::File {
                    mtime: mtime1,
                    size: size1,
                },
                FsEntry::File {
                    mtime: mtime2,
                    size: size2,
                },
            ) => {
                assert_eq!(size1, size2);
                assert_eq!(mtime1, mtime2);
            }
            _ => panic!("Expected both entries to be files"),
        }
    }
}
