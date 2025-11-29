use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum WardFileError {
    #[error("IO error: {0}")]
    Io(std::io::Error),
    #[error("Permission denied: {0}")]
    PermissionDenied(PathBuf),
    #[error("TOML parse error: {0}")]
    TomlParse(#[from] toml::de::Error),
    #[error("TOML serialization error: {0}")]
    TomlSerialize(#[from] toml::ser::Error),
    #[error("Unsupported ward file version: {0}")]
    UnsupportedVersion(u32),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum WardEntry {
    #[serde(rename = "file")]
    File {
        sha256: String,
        /// Modification time in nanoseconds since Unix epoch.
        /// Modern filesystems (ext4, APFS, etc.) support nanosecond precision.
        mtime_nanos: u64,
        size: u64,
    },
    #[serde(rename = "dir")]
    Dir {},
    #[serde(rename = "symlink")]
    Symlink { symlink_target: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    version: u32,
}

/// Helper struct to extract only the metadata section from a TOML file,
/// ignoring all other content. Used to check version before parsing the full file.
/// Note: We explicitly do NOT use deny_unknown_fields here, as this struct's
/// purpose is to ignore everything except metadata.
#[derive(Debug, Deserialize)]
struct MetadataOnly {
    metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WardFile {
    metadata: Metadata,
    pub entries: BTreeMap<String, WardEntry>,
}

impl WardFile {
    const SUPPORTED_VERSION: u32 = 1;

    /// Create a new WardFile with the current supported version
    pub fn new(entries: BTreeMap<String, WardEntry>) -> Self {
        WardFile {
            metadata: Metadata {
                version: Self::SUPPORTED_VERSION,
            },
            entries,
        }
    }

    /// Parse a TOML string into a WardFile structure
    pub fn from_toml(content: &str) -> Result<Self, WardFileError> {
        // First, extract only the metadata to check version. Otherwise
        // we would fail on unexpected *other* input (which could just be
        // due to a future version), without being able to provide a sensible
        // explanation.
        let metadata_only: MetadataOnly = toml::from_str(content)?;

        if metadata_only.metadata.version != Self::SUPPORTED_VERSION {
            return Err(WardFileError::UnsupportedVersion(
                metadata_only.metadata.version,
            ));
        }

        // Version is supported, now parse the full file
        let ward_file: WardFile = toml::from_str(content)?;
        Ok(ward_file)
    }

    /// Serialize a WardFile structure to TOML string
    pub fn to_toml(&self) -> Result<String, WardFileError> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Load a WardFile from the filesystem
    pub fn load(path: &Path) -> Result<Self, WardFileError> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                WardFileError::PermissionDenied(path.to_path_buf())
            } else {
                WardFileError::Io(e)
            }
        })?;

        Self::from_toml(&content)
    }

    /// Save a WardFile to the filesystem atomically.
    ///
    /// Writes to a temporary file, fsyncs it, then atomically renames it into place.
    pub fn save(&self, path: &Path) -> Result<(), WardFileError> {
        use std::io::Write;

        let content = self.to_toml()?;

        let parent = path.parent().unwrap_or(Path::new("."));

        let mut temp_file = tempfile::NamedTempFile::new_in(parent).map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                WardFileError::PermissionDenied(parent.to_path_buf())
            } else {
                WardFileError::Io(e)
            }
        })?;

        temp_file.write_all(content.as_bytes()).map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                WardFileError::PermissionDenied(path.to_path_buf())
            } else {
                WardFileError::Io(e)
            }
        })?;

        temp_file.as_file().sync_all().map_err(WardFileError::Io)?;

        temp_file.persist(path).map_err(|e| {
            if e.error.kind() == std::io::ErrorKind::PermissionDenied {
                WardFileError::PermissionDenied(path.to_path_buf())
            } else {
                WardFileError::Io(e.error)
            }
        })?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tempfile::NamedTempFile;

    #[test]
    fn test_parse_valid_toml_with_file() {
        let toml_content = r#"
[metadata]
version = 1

[entries."file1.txt"]
type = "file"
sha256 = "abc123"
mtime_nanos = 1234567890
size = 42
"#;

        let ward_file = WardFile::from_toml(toml_content).unwrap();
        assert_eq!(ward_file.entries.len(), 1);

        let entry = ward_file.entries.get("file1.txt").unwrap();
        match entry {
            WardEntry::File {
                sha256,
                mtime_nanos,
                size,
            } => {
                assert_eq!(sha256, "abc123");
                assert_eq!(*mtime_nanos, 1234567890);
                assert_eq!(*size, 42);
            }
            _ => panic!("Expected File entry"),
        }
    }

    #[test]
    fn test_parse_valid_toml_with_dir() {
        let toml_content = r#"
[metadata]
version = 1

[entries.dir1]
type = "dir"
"#;

        let ward_file = WardFile::from_toml(toml_content).unwrap();
        assert_eq!(ward_file.entries.len(), 1);

        let entry = ward_file.entries.get("dir1").unwrap();
        matches!(entry, WardEntry::Dir {});
    }

    #[test]
    fn test_parse_valid_toml_with_symlink() {
        let toml_content = r#"
[metadata]
version = 1

[entries.link1]
type = "symlink"
symlink_target = "/some/path"
"#;

        let ward_file = WardFile::from_toml(toml_content).unwrap();
        assert_eq!(ward_file.entries.len(), 1);

        let entry = ward_file.entries.get("link1").unwrap();
        match entry {
            WardEntry::Symlink { symlink_target } => {
                assert_eq!(symlink_target, Path::new("/some/path"));
            }
            _ => panic!("Expected Symlink entry"),
        }
    }

    #[test]
    fn test_corrupted_file_missing_sha256() {
        let toml_content = r#"
[metadata]
version = 1

[entries."file1.txt"]
type = "file"
mtime_nanos = 123
size = 456
"#;

        let result = WardFile::from_toml(toml_content);
        assert!(result.is_err());
        assert!(matches!(result, Err(WardFileError::TomlParse(_))));
    }

    #[test]
    fn test_corrupted_symlink_missing_target() {
        let toml_content = r#"
[metadata]
version = 1

[entries.link1]
type = "symlink"
"#;

        let result = WardFile::from_toml(toml_content);
        assert!(result.is_err());
        assert!(matches!(result, Err(WardFileError::TomlParse(_))));
    }

    #[test]
    fn test_dir_rejects_extra_fields() {
        let toml_content = r#"
[metadata]
version = 1

[entries.dir1]
type = "dir"
sha256 = "should_be_rejected"
"#;

        let result = WardFile::from_toml(toml_content);
        assert!(result.is_err());
        assert!(matches!(result, Err(WardFileError::TomlParse(_))));
    }

    #[test]
    fn test_round_trip_serialization() {
        let mut entries = BTreeMap::new();
        entries.insert(
            "file1.txt".to_string(),
            WardEntry::File {
                sha256: "abc123".to_string(),
                mtime_nanos: 1234567890,
                size: 42,
            },
        );
        entries.insert("dir1".to_string(), WardEntry::Dir {});

        let ward_file = WardFile {
            metadata: Metadata {
                version: WardFile::SUPPORTED_VERSION,
            },
            entries,
        };
        let toml_string = ward_file.to_toml().unwrap();
        let parsed = WardFile::from_toml(&toml_string).unwrap();

        assert_eq!(parsed.entries.len(), 2);
        assert!(parsed.entries.contains_key("file1.txt"));
        assert!(parsed.entries.contains_key("dir1"));
    }

    /// Ensure TOML output is sorted by file name (primarily to ensure output
    /// is stable, but also for the purpose of user convenience).
    #[test]
    fn test_sorted_output() {
        // Generate enough entries to ensure sufficient statistical power
        // given that we cannot prove stability in a black box test.
        //
        // (Yeah, seems unlikely it would not be stable given our use of
        // BTreeMap, but who knows what an implementation might do.)
        const NUM_ENTRIES: usize = 1000;
        let mut entries = BTreeMap::new();

        let mut names_with_keys: Vec<_> = (0..NUM_ENTRIES)
            .map(|i| {
                let name = format!("{}", i);
                let key = i ^ 0x5a5a5a5a; // Arbitrary XOR value to scramble order
                (name, key)
            })
            .collect();

        names_with_keys.sort_by_key(|(_, key)| *key);

        for (i, (name, _)) in names_with_keys.iter().enumerate() {
            entries.insert(
                format!("{}.txt", name),
                WardEntry::File {
                    sha256: format!("hash{}", i),
                    mtime_nanos: 1000 + i as u64,
                    size: 10 + i as u64,
                },
            );
        }

        let ward_file = WardFile {
            metadata: Metadata {
                version: WardFile::SUPPORTED_VERSION,
            },
            entries: entries.clone(),
        };

        let toml_string = ward_file.to_toml().unwrap();

        // Parse the output. Round-tripping to TOML and back would
        // be useless, since the BTreeMap would then be guaranteed to be sorted.
        let mut table_names = Vec::new();
        for line in toml_string.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                let name = trimmed.trim_start_matches('[').trim_end_matches(']');

                if name == "metadata" {
                    continue;
                }

                // Extract entry name from [entries.name] or [entries."name.with.dots"]
                // Note: entries.file1.txt would be parsed as nested tables in TOML,
                // so files with dots MUST be quoted: entries."file1.txt"
                if let Some(entry_name) = name.strip_prefix("entries.") {
                    // If it starts with a quote, it's a quoted key like "file1.txt"
                    let entry_name = if entry_name.starts_with('"') && entry_name.ends_with('"') {
                        &entry_name[1..entry_name.len() - 1]
                    } else {
                        // Unquoted key like dir1
                        entry_name
                    };
                    table_names.push(entry_name.to_string());
                }
            }
        }

        assert_eq!(
            table_names.len(),
            NUM_ENTRIES,
            "Expected {} table entries in TOML output",
            NUM_ENTRIES
        );

        let mut sorted_names = table_names.clone();
        sorted_names.sort();
        assert_eq!(
            table_names, sorted_names,
            "TOML table names are not in sorted order"
        );

        let toml_string2 = ward_file.to_toml().unwrap();
        assert_eq!(
            toml_string, toml_string2,
            "TOML serialization does not appear to preserve order"
        );
    }

    #[test]
    fn test_load_and_save() {
        let mut entries = BTreeMap::new();
        entries.insert(
            "test_file.txt".to_string(),
            WardEntry::File {
                sha256: "test_hash".to_string(),
                mtime_nanos: 9876543210,
                size: 100,
            },
        );
        entries.insert("test_dir".to_string(), WardEntry::Dir {});
        entries.insert(
            "test_link".to_string(),
            WardEntry::Symlink {
                symlink_target: PathBuf::from("/target/path"),
            },
        );

        let ward_file = WardFile {
            metadata: Metadata {
                version: WardFile::SUPPORTED_VERSION,
            },
            entries,
        };

        let temp_file = NamedTempFile::new().unwrap();
        ward_file.save(temp_file.path()).unwrap();

        let loaded = WardFile::load(temp_file.path()).unwrap();
        assert_eq!(loaded, ward_file);
    }

    #[test]
    fn test_invalid_toml_syntax() {
        // Missing closing bracket on table name
        let toml_content = r#"
[metadata]
version = 1

[entries.file1.txt
type = "file"
"#;

        let result = WardFile::from_toml(toml_content);
        assert!(result.is_err());
        match result {
            Err(WardFileError::TomlParse(_)) => {}
            _ => panic!("Expected TomlParse error"),
        }
    }

    #[test]
    fn test_unsupported_version() {
        let toml_content = r#"
[metadata]
version = 999

[entries.test]
type = "dir"
"#;

        let result = WardFile::from_toml(toml_content);
        assert!(result.is_err());
        match result {
            Err(WardFileError::UnsupportedVersion(999)) => {}
            _ => panic!("Expected UnsupportedVersion(999) error"),
        }
    }

    #[test]
    fn test_unsupported_version_with_invalid_entries() {
        // This test verifies that we check the version BEFORE trying to parse entries.
        // The entries section contains invalid data that would fail to parse if we tried.
        let toml_content = r#"
[metadata]
version = 999

[entries.test]
type = "unsupported-type-this-should-be-ignored"
some_future_field = "value"
another_field = 12345
"#;

        let result = WardFile::from_toml(toml_content);
        assert!(result.is_err());
        match result {
            Err(WardFileError::UnsupportedVersion(999)) => {}
            _ => panic!("Expected UnsupportedVersion(999) error, not a parse error"),
        }
    }

    #[test]
    fn test_unknown_field_in_metadata() {
        let toml_content = r#"
[metadata]
version = 1
unknown_field = "should_be_rejected"

[entries.test]
type = "dir"
"#;

        let result = WardFile::from_toml(toml_content);
        assert!(result.is_err());
        assert!(matches!(result, Err(WardFileError::TomlParse(_))));
    }

    #[test]
    fn test_unknown_top_level_section() {
        let toml_content = r#"
[metadata]
version = 1

[entries.test]
type = "dir"

[unknown_section]
field = "value"
"#;

        let result = WardFile::from_toml(toml_content);
        assert!(result.is_err());
        assert!(matches!(result, Err(WardFileError::TomlParse(_))));
    }

    #[test]
    fn test_unknown_field_in_file_entry() {
        let toml_content = r#"
[metadata]
version = 1

[entries."test.txt"]
type = "file"
sha256 = "abc123"
mtime_nanos = 1234567890
size = 42
unknown_field = "should_be_rejected"
"#;

        let result = WardFile::from_toml(toml_content);
        assert!(result.is_err());
        assert!(matches!(result, Err(WardFileError::TomlParse(_))));
    }
}
