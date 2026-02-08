//! Canonical hashing helpers for stable fingerprints.
//!
//! Provides canonical encodings for byte fields, integers, and paths used by
//! fingerprint construction.

use sha2::{Digest, Sha256};
use std::path::Path;

/// Hashes a byte field with an explicit length prefix.
///
/// Length-prefixing avoids delimiter ambiguities (for example embedded `|` or
/// newlines) that can otherwise make distinct data serialize to identical byte
/// streams before hashing.
pub(crate) fn hash_field(hasher: &mut Sha256, bytes: &[u8]) {
    let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    hasher.update(len.to_be_bytes());
    hasher.update(bytes);
}

/// Hashes a fixed-width integer field.
pub(crate) fn hash_u64_field(hasher: &mut Sha256, value: u64) {
    hasher.update(value.to_be_bytes());
}

/// Hashes a path-like value while preserving platform identity semantics.
///
/// On Unix we hash raw OS bytes so distinct non-UTF-8 paths remain distinct.
/// On non-Unix platforms we fall back to string form, matching current
/// portability assumptions in this codebase.
pub(crate) fn hash_path_field(hasher: &mut Sha256, path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        hash_field(hasher, path.as_os_str().as_bytes());
    }
    #[cfg(not(unix))]
    {
        hash_field(hasher, path.to_string_lossy().as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest_hex(hasher: Sha256) -> String {
        format!("{:x}", hasher.finalize())
    }

    #[test]
    fn hash_field_matches_explicit_length_prefix_encoding() {
        let payload = b"a|b\nc";

        let mut via_helper = Sha256::new();
        hash_field(&mut via_helper, payload);

        let mut manual = Sha256::new();
        manual.update((payload.len() as u64).to_be_bytes());
        manual.update(payload);

        assert_eq!(digest_hex(via_helper), digest_hex(manual));
    }

    #[test]
    fn hash_field_prevents_boundary_collision() {
        let mut split_one = Sha256::new();
        hash_field(&mut split_one, b"a");
        hash_field(&mut split_one, b"bc");

        let mut split_two = Sha256::new();
        hash_field(&mut split_two, b"ab");
        hash_field(&mut split_two, b"c");

        assert_ne!(digest_hex(split_one), digest_hex(split_two));
    }

    #[test]
    fn hash_u64_field_matches_manual_big_endian_bytes() {
        let value = 0x0123_4567_89ab_cdef_u64;

        let mut via_helper = Sha256::new();
        hash_u64_field(&mut via_helper, value);

        let mut manual = Sha256::new();
        manual.update(value.to_be_bytes());

        assert_eq!(digest_hex(via_helper), digest_hex(manual));
    }

    #[test]
    fn hash_path_field_matches_platform_specific_encoding() {
        let path = Path::new("dir/a|b\nc");

        let mut via_helper = Sha256::new();
        hash_path_field(&mut via_helper, path);

        let mut manual = Sha256::new();
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            hash_field(&mut manual, path.as_os_str().as_bytes());
        }
        #[cfg(not(unix))]
        {
            hash_field(&mut manual, path.to_string_lossy().as_bytes());
        }

        assert_eq!(digest_hex(via_helper), digest_hex(manual));
    }

    #[test]
    fn hash_path_field_distinguishes_different_paths() {
        let mut one = Sha256::new();
        hash_path_field(&mut one, Path::new("dir/file-a"));

        let mut two = Sha256::new();
        hash_path_field(&mut two, Path::new("dir/file-b"));

        assert_ne!(digest_hex(one), digest_hex(two));
    }
}
