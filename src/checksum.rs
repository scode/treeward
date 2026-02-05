use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

#[derive(Debug, thiserror::Error)]
pub enum ChecksumError {
    #[error("IO error: {0}")]
    Io(std::io::Error),
    #[error("Permission denied: {0}")]
    PermissionDenied(PathBuf),
    #[error("File modified during checksumming: {0}")]
    ConcurrentModification(PathBuf),
}

pub struct FileChecksum {
    /// Hex encoded.
    pub sha256: String,
    /// Modification time captured after checksumming.
    pub mtime: std::time::SystemTime,
    /// File size in bytes.
    pub size: u64,
}

/// Computes the SHA-256 checksum of a file with concurrent modification detection.
///
/// # Behavior
/// - Records the file's modification time before reading
/// - Reads the file in chunks and computes SHA-256
/// - Verifies the modification time hasn't changed after reading
/// - Returns an error if the file was modified during checksumming
///
/// # Errors (may be changed in the future)
/// - `ChecksumError::Io`: File doesn't exist or other I/O errors
/// - `ChecksumError::PermissionDenied`: Insufficient permissions to read the file
/// - `ChecksumError::ConcurrentModification`: File was detected as being modified while
///   checksumming. Note that the absence of this error is *not* a guarantee that the
///   file was *not* modified.
pub fn checksum_file(path: &Path) -> Result<FileChecksum, ChecksumError> {
    info!("Checksumming {}", path.display());

    let metadata_before = std::fs::metadata(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            ChecksumError::PermissionDenied(path.to_path_buf())
        } else {
            ChecksumError::Io(e)
        }
    })?;
    let mtime_before = metadata_before.modified().map_err(ChecksumError::Io)?;

    let mut file = File::open(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            ChecksumError::PermissionDenied(path.to_path_buf())
        } else {
            ChecksumError::Io(e)
        }
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];

    loop {
        let bytes_read = file.read(&mut buffer).map_err(ChecksumError::Io)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }

    let metadata_after = std::fs::metadata(path).map_err(ChecksumError::Io)?;
    let mtime_after = metadata_after.modified().map_err(ChecksumError::Io)?;

    if mtime_before != mtime_after {
        return Err(ChecksumError::ConcurrentModification(path.to_path_buf()));
    }

    let hash_bytes = hasher.finalize();
    let sha256 = format!("{:x}", hash_bytes);

    debug!("Checksum of {} is {}", path.display(), sha256);

    Ok(FileChecksum {
        sha256,
        mtime: mtime_after,
        size: metadata_after.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_checksum_simple_file() {
        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(b"Hello, world!").unwrap();
        temp_file.flush().unwrap();

        let result = checksum_file(temp_file.path()).unwrap();

        assert_eq!(
            result.sha256,
            "315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3"
        );
    }

    #[test]
    fn test_checksum_empty_file() {
        let temp_file = NamedTempFile::new().unwrap();

        let result = checksum_file(temp_file.path()).unwrap();

        assert_eq!(
            result.sha256,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn test_checksum_large_file() {
        let mut temp_file = NamedTempFile::new().unwrap();

        let content = vec![b'A'; 1024 * 1024];
        temp_file.write_all(&content).unwrap();
        temp_file.flush().unwrap();

        let result = checksum_file(temp_file.path()).unwrap();

        assert_eq!(result.sha256.len(), 64);
    }

    #[test]
    fn test_checksum_nonexistent_file() {
        let result = checksum_file(Path::new("/nonexistent/file.txt"));

        assert!(result.is_err());
        match result {
            Err(ChecksumError::Io(_)) => {}
            _ => panic!("Expected IO error for nonexistent file"),
        }
    }

    #[test]
    fn test_checksum_deterministic() {
        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(b"test content").unwrap();
        temp_file.flush().unwrap();

        let result1 = checksum_file(temp_file.path()).unwrap();
        let result2 = checksum_file(temp_file.path()).unwrap();

        assert_eq!(result1.sha256, result2.sha256);
    }

    #[test]
    #[cfg(unix)]
    fn test_checksum_permission_denied() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(b"test content").unwrap();
        temp_file.flush().unwrap();

        let mut perms = fs::metadata(temp_file.path()).unwrap().permissions();
        perms.set_mode(0o000);
        fs::set_permissions(temp_file.path(), perms).unwrap();

        let result = checksum_file(temp_file.path());

        assert!(result.is_err());
        match result {
            Err(ChecksumError::PermissionDenied(_)) => {}
            _ => panic!("Expected PermissionDenied error for permission denied"),
        }
    }

    #[test]
    fn test_checksum_concurrent_modification() {
        // This test is inherently non-deterministic and may occasionally fail due to timing.
        // The concurrent modification detection requires the mtime to change between the
        // pre-read and post-read metadata checks, which we achieve by racing a background
        // thread against the checksum operation. A deterministic test would require
        // refactoring checksum_file to accept an injected reader or hook, which adds
        // complexity to production code for test-only benefit. In practice, with a 5MB
        // file and 100 attempts, failure is extremely unlikely.
        use filetime::{FileTime, set_file_mtime};
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::thread;
        use std::time::Duration;

        let mut temp_file = NamedTempFile::new().unwrap();
        let content = vec![b'X'; 5 * 1024 * 1024];
        temp_file.write_all(&content).unwrap();
        temp_file.flush().unwrap();

        let path = temp_file.path().to_path_buf();
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_flag_clone = stop_flag.clone();

        let modifier_handle = thread::spawn(move || {
            let mut counter = 0u64;
            while !stop_flag_clone.load(Ordering::Relaxed) {
                counter = counter.wrapping_add(1);
                let mtime = FileTime::from_unix_time(1_000_000_000 + (counter as i64), 0);
                let _ = set_file_mtime(&path, mtime);
            }
        });

        let mut got_concurrent_modification = false;
        for _ in 0..100 {
            match checksum_file(temp_file.path()) {
                Err(ChecksumError::ConcurrentModification(_)) => {
                    got_concurrent_modification = true;
                    break;
                }
                Ok(_) => {
                    thread::sleep(Duration::from_millis(1));
                }
                Err(e) => panic!("Unexpected error: {}", e),
            }
        }

        stop_flag.store(true, Ordering::Relaxed);
        modifier_handle.join().unwrap();

        assert!(
            got_concurrent_modification,
            "Expected to detect concurrent modification at least once in 100 attempts"
        );
    }
}
