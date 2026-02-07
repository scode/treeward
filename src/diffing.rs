use crate::status;
use crate::ward_file::WardEntry;

pub fn print_statuses(statuses: &[status::StatusEntry], show_diff: bool) {
    for entry in statuses {
        let status_code = match entry.status_type() {
            status::StatusType::Added => "A",
            status::StatusType::Removed => "R",
            status::StatusType::PossiblyModified => "M?",
            status::StatusType::Modified => "M",
            status::StatusType::Unchanged => ".",
        };

        println!("{:<2} {}", status_code, entry.path());

        if show_diff {
            print_diff(entry);
        }
    }
}

fn print_diff(entry: &status::StatusEntry) {
    let lines = format_diff_lines(entry);
    for line in lines {
        println!("{}", line);
    }
}

fn format_diff_lines(entry: &status::StatusEntry) -> Vec<String> {
    match entry {
        status::StatusEntry::Added { .. } | status::StatusEntry::Unchanged { .. } => Vec::new(),
        status::StatusEntry::Removed { old_ward_entry, .. } => old_ward_entry
            .as_ref()
            .map(|old| vec![format_was_entry(old)])
            .unwrap_or_default(),
        status::StatusEntry::Modified {
            ward_entry,
            old_ward_entry,
            ..
        }
        | status::StatusEntry::PossiblyModified {
            ward_entry,
            old_ward_entry,
            ..
        } => match (old_ward_entry, ward_entry) {
            (Some(old), Some(new)) => format_entry_diff(old, new),
            (Some(old), None) => vec![format_was_entry_verbose(old)],
            _ => Vec::new(),
        },
    }
}

fn format_was_entry(entry: &WardEntry) -> String {
    match entry {
        WardEntry::File { sha256, size, .. } => {
            format!(
                "   was: file ({}, sha256: {})",
                format_size(*size),
                truncate_sha256(sha256)
            )
        }
        WardEntry::Dir {} => "   was: directory".to_string(),
        WardEntry::Symlink { symlink_target } => {
            format!("   was: symlink -> {}", symlink_target.display())
        }
    }
}

fn format_was_entry_verbose(entry: &WardEntry) -> String {
    match entry {
        WardEntry::File {
            sha256,
            size,
            mtime_nanos,
        } => {
            format!(
                "   was: file ({}, mtime: {}, sha256: {})",
                format_size(*size),
                format_mtime(*mtime_nanos),
                truncate_sha256(sha256)
            )
        }
        WardEntry::Dir {} => "   was: directory".to_string(),
        WardEntry::Symlink { symlink_target } => {
            format!("   was: symlink -> {}", symlink_target.display())
        }
    }
}

#[cfg(test)]
fn format_diff(entry: &status::StatusEntry) -> String {
    let lines = format_diff_lines(entry);
    if lines.is_empty() {
        String::new()
    } else {
        lines.join("\n") + "\n"
    }
}

fn format_entry_diff(old: &WardEntry, new: &WardEntry) -> Vec<String> {
    let mut lines = Vec::new();

    match (old, new) {
        (
            WardEntry::File {
                sha256: old_sha,
                mtime_nanos: old_mtime,
                size: old_size,
            },
            WardEntry::File {
                sha256: new_sha,
                mtime_nanos: new_mtime,
                size: new_size,
            },
        ) => {
            if old_size != new_size {
                lines.push(format!(
                    "   size: {} -> {}",
                    format_size(*old_size),
                    format_size(*new_size)
                ));
            }
            if old_mtime != new_mtime {
                lines.push(format!(
                    "   mtime: {} -> {}",
                    format_mtime(*old_mtime),
                    format_mtime(*new_mtime)
                ));
            }
            if old_sha != new_sha {
                lines.push(format!(
                    "   sha256: {} -> {}",
                    truncate_sha256(old_sha),
                    truncate_sha256(new_sha)
                ));
            }
        }
        (
            WardEntry::Symlink {
                symlink_target: old_target,
            },
            WardEntry::Symlink {
                symlink_target: new_target,
            },
        ) => {
            if old_target != new_target {
                lines.push(format!(
                    "   target: {} -> {}",
                    old_target.display(),
                    new_target.display()
                ));
            }
        }
        _ => {
            lines.push(format!("   was: {}", format_entry_type(old)));
            lines.push(format!("   now: {}", format_entry_type(new)));
        }
    }

    lines
}

fn format_entry_type(entry: &WardEntry) -> String {
    match entry {
        WardEntry::File { sha256, size, .. } => {
            format!(
                "file ({}, sha256: {})",
                format_size(*size),
                truncate_sha256(sha256)
            )
        }
        WardEntry::Dir {} => "directory".to_string(),
        WardEntry::Symlink { symlink_target } => {
            format!("symlink -> {}", symlink_target.display())
        }
    }
}

fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * 1024 * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} bytes", bytes)
    }
}

fn format_mtime(nanos: u64) -> String {
    use std::time::{Duration, UNIX_EPOCH};

    let duration = Duration::from_nanos(nanos);
    let system_time = UNIX_EPOCH + duration;

    let datetime: chrono::DateTime<chrono::Local> = system_time.into();
    datetime.format("%Y-%m-%d %H:%M:%S%.3f").to_string()
}

fn truncate_sha256(sha256: &str) -> String {
    if sha256.len() > 12 {
        format!("{}...", &sha256[..12])
    } else {
        sha256.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_file(size: u64, sha256: &str) -> WardEntry {
        WardEntry::File {
            sha256: sha256.to_string(),
            mtime_nanos: 1704067200_000_000_000,
            size,
        }
    }

    fn make_file_with_mtime(size: u64, sha256: &str, mtime_nanos: u64) -> WardEntry {
        WardEntry::File {
            sha256: sha256.to_string(),
            mtime_nanos,
            size,
        }
    }

    #[test]
    fn diff_removed_file() {
        let entry = status::StatusEntry::Removed {
            path: "deleted.txt".into(),
            old_ward_entry: Some(make_file(
                2048,
                "abc123def456abc123def456abc123def456abc123def456abc123def456abc12345",
            )),
        };

        assert_eq!(
            format_diff(&entry),
            "   was: file (2.0 KB, sha256: abc123def456...)\n"
        );
    }

    #[test]
    fn diff_removed_directory() {
        let entry = status::StatusEntry::Removed {
            path: "old_dir".into(),
            old_ward_entry: Some(WardEntry::Dir {}),
        };

        assert_eq!(format_diff(&entry), "   was: directory\n");
    }

    #[test]
    fn diff_removed_symlink() {
        let entry = status::StatusEntry::Removed {
            path: "old_link".into(),
            old_ward_entry: Some(WardEntry::Symlink {
                symlink_target: PathBuf::from("/usr/bin/python3"),
            }),
        };

        assert_eq!(format_diff(&entry), "   was: symlink -> /usr/bin/python3\n");
    }

    #[test]
    fn diff_added_produces_no_output() {
        let entry = status::StatusEntry::Added {
            path: "new_file.txt".into(),
            ward_entry: None,
        };

        assert_eq!(format_diff(&entry), "");
    }

    #[test]
    fn diff_unchanged_produces_no_output() {
        let entry = status::StatusEntry::Unchanged {
            path: "stable.txt".into(),
            ward_entry: None,
        };

        assert_eq!(format_diff(&entry), "");
    }

    #[test]
    fn diff_modified_file_size_change() {
        let old = make_file(
            1024,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        );
        let new = make_file(
            2048,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        );

        let entry = status::StatusEntry::Modified {
            path: "growing.txt".into(),
            ward_entry: Some(new),
            old_ward_entry: Some(old),
        };

        assert_eq!(format_diff(&entry), "   size: 1.0 KB -> 2.0 KB\n");
    }

    #[test]
    fn diff_modified_file_content_change() {
        let old = make_file(
            1024,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        );
        let new = make_file(
            1024,
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        );

        let entry = status::StatusEntry::Modified {
            path: "changed.txt".into(),
            ward_entry: Some(new),
            old_ward_entry: Some(old),
        };

        assert_eq!(
            format_diff(&entry),
            "   sha256: aaaaaaaaaaaa... -> bbbbbbbbbbbb...\n"
        );
    }

    #[test]
    fn diff_modified_file_size_and_content_change() {
        let old = make_file(
            100,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        );
        let new = make_file(
            200,
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        );

        let entry = status::StatusEntry::Modified {
            path: "multi.txt".into(),
            ward_entry: Some(new),
            old_ward_entry: Some(old),
        };

        assert_eq!(
            format_diff(&entry),
            "   size: 100 bytes -> 200 bytes\n   sha256: aaaaaaaaaaaa... -> bbbbbbbbbbbb...\n"
        );
    }

    #[test]
    fn diff_modified_file_mtime_change() {
        let old_mtime: u64 = 1000000000_000_000_000;
        let new_mtime: u64 = 1100000000_000_000_000;

        let old = make_file_with_mtime(
            100,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            old_mtime,
        );
        let new = make_file_with_mtime(
            100,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            new_mtime,
        );

        let entry = status::StatusEntry::Modified {
            path: "touched.txt".into(),
            ward_entry: Some(new),
            old_ward_entry: Some(old),
        };

        // mtime format is "YYYY-MM-DD HH:MM:SS.mmm" in local timezone
        let expected = format!(
            "   mtime: {} -> {}\n",
            format_mtime(old_mtime),
            format_mtime(new_mtime)
        );
        assert_eq!(format_diff(&entry), expected);
    }

    #[test]
    fn diff_type_change_file_to_directory() {
        let old = make_file(
            512,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        );
        let new = WardEntry::Dir {};

        let entry = status::StatusEntry::Modified {
            path: "was_file".into(),
            ward_entry: Some(new),
            old_ward_entry: Some(old),
        };

        assert_eq!(
            format_diff(&entry),
            "   was: file (512 bytes, sha256: aaaaaaaaaaaa...)\n   now: directory\n"
        );
    }

    #[test]
    fn diff_type_change_directory_to_symlink() {
        let old = WardEntry::Dir {};
        let new = WardEntry::Symlink {
            symlink_target: PathBuf::from("../other"),
        };

        let entry = status::StatusEntry::Modified {
            path: "was_dir".into(),
            ward_entry: Some(new),
            old_ward_entry: Some(old),
        };

        assert_eq!(
            format_diff(&entry),
            "   was: directory\n   now: symlink -> ../other\n"
        );
    }

    #[test]
    fn diff_symlink_target_change() {
        let old = WardEntry::Symlink {
            symlink_target: PathBuf::from("/old/target"),
        };
        let new = WardEntry::Symlink {
            symlink_target: PathBuf::from("/new/target"),
        };

        let entry = status::StatusEntry::Modified {
            path: "link".into(),
            ward_entry: Some(new),
            old_ward_entry: Some(old),
        };

        assert_eq!(
            format_diff(&entry),
            "   target: /old/target -> /new/target\n"
        );
    }

    #[test]
    fn diff_possibly_modified_shows_same_as_modified() {
        let old = make_file(
            1024,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        );
        let new = make_file(
            2048,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        );

        let entry = status::StatusEntry::PossiblyModified {
            path: "maybe.txt".into(),
            ward_entry: Some(new),
            old_ward_entry: Some(old),
        };

        assert_eq!(format_diff(&entry), "   size: 1.0 KB -> 2.0 KB\n");
    }
}
