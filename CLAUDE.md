# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**treeward** is a command-line file integrity tool for checksumming and verifying trees of files. It uses a distributed
approach where each directory contains a `.treeward` TOML file tracking its immediate children (non-recursive
per-directory model), allowing directories to be moved independently while maintaining integrity information.

**Language:** Rust 2024 edition

## Build and Test Commands

```bash
# Run all tests
cargo test

# Run tests for a specific module
cargo test checksum
cargo test ward_file
cargo test dir_list
cargo test status
cargo test ward

# Run with output visible (for debugging test failures)
cargo test -- --nocapture

# Format code
cargo fmt

# Lint
cargo clippy -- -D warnings

# Build
cargo build
cargo build --release
```

## Architecture

### Core Design Principles

1. **Non-recursive directory model**: Each directory has its own `.treeward` file containing metadata only for its
   immediate children (files, subdirectories, symlinks). This allows directories to be moved independently.

2. **BTreeMap for entries**: All entry collections use `BTreeMap<String, EntryType>` for deterministic ordering and
   consistent serialization. This ensures stable TOML output.

3. **Consistent naming across modules**:
   - Field name `symlink_target` (not `target`) for symlink destinations
   - Entry variants: `File`, `Dir`, `Symlink`
   - Files have `mtime_nanos`, `size`, and (in ward_file only) `sha256`
   - Directories have no additional fields beyond their presence
   - Symlinks have `symlink_target` only

4. **Error handling philosophy**:
   - Use `thiserror` for typed errors in library modules
   - `anyhow` is acceptable at CLI level
   - Corrupted/unreadable `.treeward` files are fatal errors
   - Permission errors are fatal errors
   - Never silently skip problems - all issues are reported as errors

5. **Concurrent modification detection**: Before and after checksumming a file, compare mtime to detect changes during
   the read operation. If detected, return an error (no retry logic).

6. **High-precision timestamps**: Use nanosecond-precision timestamps (`mtime_nanos` as `u64`) instead of `SystemTime`
   for accurate modification detection across all platforms.

### Module Structure

```
src/
├── checksum.rs    - SHA-256 file checksumming with concurrent modification detection
├── cli.rs         - CLI argument parsing (clap)
├── dir_list.rs    - Non-recursive directory listing with filesystem metadata
├── ward_file.rs   - TOML serialization/deserialization for .treeward files
├── status.rs      - Change detection by comparing filesystem vs ward state
├── update.rs      - Ward creation and update logic
└── main.rs        - CLI entry point and command handlers
```

### Key Data Flow

**ward_file.rs** (persistent format):

- `WardFile` contains `BTreeMap<String, WardEntry>`
- `WardEntry` enum variants: `File { sha256, mtime_nanos, size }`, `Dir {}`, `Symlink { symlink_target }`
- Serializes to/from TOML with deterministic ordering
- Version-checked parsing (current version: 1)

**dir_list.rs** (runtime representation):

- `list_directory()` returns `BTreeMap<String, FsEntry>`
- `FsEntry` enum variants: `File { mtime, size }`, `Dir { mtime }`, `Symlink { symlink_target }`
- Excludes `.treeward` files from listings
- No SHA-256 checksums (that's checksum.rs's job)

**checksum.rs**:

- `checksum_file()` computes SHA-256 with concurrent modification detection
- Records mtime before and after reading file contents
- Returns `FileChecksum { sha256, mtime, size }`

**status.rs** (change detection):

- `compute_status()` recursively compares filesystem state against ward files
- Returns `StatusResult { changes, fingerprint }`
- Supports three `ChecksumPolicy` modes:
  - `Never`: Only compare metadata (fast, reports `PossiblyModified`)
  - `WhenPossiblyModified`: Checksum files with differing metadata (default)
  - `Always`: Checksum all files (detects silent corruption)
- `ChangeType` variants: `Added`, `Removed`, `PossiblyModified`, `Modified`
- Fingerprint is SHA-256 hash of sorted changes (for TOCTOU protection)

**update.rs** (ward creation/update):

- `ward_directory()` creates or updates `.treeward` files
- Supports `WardOptions { init, fingerprint, dry_run }`
- Only checksums new or modified files (reuses checksums when metadata matches)
- Only writes `.treeward` files if contents actually changed
- Fingerprint validation prevents TOCTOU issues (validates before any writes)
- Returns `WardResult { files_warded, ward_files_updated }`

### Testing Strategy

- Each module has comprehensive unit tests in a `tests` submodule
- Use `tempfile` crate for filesystem-based tests
- Tests cover happy paths, error cases, edge cases, and platform-specific behavior (Unix symlinks)
- All tests must pass before merging changes

### Symlink Handling

Symlinks are tracked but NOT followed:

- `symlink_target` field stores the raw symlink target path
- Use `std::fs::symlink_metadata()` (not `metadata()`) to avoid following symlinks
- Use `std::fs::read_link()` to read symlink targets
- Broken symlinks are valid and tracked

## Important Conventions

### Data Consistency

When adding or modifying entry types, ensure consistency between:

1. `ward_file::WardEntry` (persistent TOML format)
2. `dir_list::FsEntry` (runtime representation)
3. Field names, especially `symlink_target` and `mtime_nanos`

### Time Representation

- **In memory (dir_list.rs)**: Use `SystemTime` for `mtime` field in `FsEntry`
- **On disk (ward_file.rs)**: Use `u64` nanoseconds since UNIX_EPOCH for `mtime_nanos` field in `WardEntry`
- **Conversion**: Use `.duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64` when converting SystemTime to nanos
- This provides nanosecond precision and avoids platform-specific serialization issues

### TOML Serialization

- Always use `BTreeMap` for entry collections to ensure stable output
- TOML files should have `[metadata]` section with `version = 1`
- Entries are stored as TOML tables: `[entries."filename"]`
- Use `#[serde(deny_unknown_fields)]` to catch forward-compatibility issues

### Recursive Tree Walking

Multiple modules perform recursive directory traversal:

- `status::walk_directory()`: Compares ward vs filesystem recursively
- `update::walk_and_ward()`: Updates ward files recursively

When implementing recursive operations:

- Traverse both ward entries (for removed items) and filesystem entries (for added items)
- Visit subdirectories found in either ward or filesystem
- Each directory has its own `.treeward` file (non-recursive per-directory model)
- Paths are canonicalized at the root level for consistent path operations

### Code Style

- Do not add trivial inline comments
- Only add comments for non-obvious logic or to explain "why" not "what"
