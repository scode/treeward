# UX plan

This will be a command line tool called `treeward`. It's goal is to provide checksumming (and in the future,
error correction) on trees of files.

It will be designed to work in a similar manner to an SCM like git or sapling, except there is no notion of committing.

For the following examples, assume we have a directory /foo containing:

  - dir1/d1file1.txt
  - dir1/d1file2.txt
  - dir1/d1file3.txt
  - file1.txt
  - file2.txt
  - file3.txt

## Initializing and warding the tree

```
cd /foo
treeward ward --init # initialize and ward for the first time
```

This will create /foo/.treeward and /foo/dir1/.treeward.

The `--init` flag is required when no `.treeward` file exists in the target directory. It's safe to pass `--init` 
even when already initialized - it will simply proceed normally. Without `--init`, `ward` will fail if not already 
initialized.

Each `.treeward` file is a TOML file. /foo/.treeward contains something like:

```
["file1.txt"]
type = "file"
sha256 = "...."

["dir1"]
type = "dir"
sha256 = "..."

...
```

Similarly for /foo/dir1/.treeward.

**Note on symbolic links:** Symlinks are tracked in the TOML file (their presence and target), not followed.

## Verifying the tree

```
cd /foo
treeward verify  # default directory is always .
```

This will traverse the entire tree and read through contents of files and verify the checksums.
For each file we will emit a green checkmark emoji (unless --quiet/-q is passed) when it matches,
and a red cross emoji when it does not.

At the end, a summary will be displayed showing the number of files verified and the number of files that failed verification.

**Flags:**
- `--quiet/-q`: Suppress per-file output, only show summary

## Status of the tree

```
cd /foo
treeward status  # default directory is always .
```

This won't actually read file contents - but it will traverse the entire tree looking for missing or added files/directories.
Suppose someone had run `touch /foo/file4.txt` and we ran it, we'd see:

```
A foo/file4.txt
```

Status codes:
- `A` - Added (file/directory not in ward)
- `R` - Removed (file/directory in ward but missing)
- `M?` - Possibly modified (based on last mod time/size)

**Flags:**
- `--verify`: Actually verify checksums to differentiate between "definitely modified" (show as `M`) vs "possibly modified" (`M?`)

At the end, it will print a message saying something like:

```
Run 'treeward ward --fingerprint <fingerprint>' to accept these changes and update the ward.
```

The fingerprint will be a base64 encoded string that represents a hash of the set of changes
detected. When later passed to the ward command, it will cause the ward command to only proceed if
the changes match what was previously displayed to the user in the status command.

## Updating the ward

`treeward ward [--fingerprint <fingerprint>]` will:

* Record any new files and deleted files
* Update the checksum for any modified file, and update metadata (last mod time, size, etc.)
* Only rewrite `.treeward` files if their contents actually changed
* Log changes made during the operation

**Flags:**
- `--init`: Allow warding even if not already initialized (required for first ward)
- `--fingerprint <fp>`: Only proceed if current changes match the given fingerprint from `status`
- `--dry-run`: Preview what would be updated without actually writing changes

**Behavior:**
- If `--fingerprint` is omitted, ward proceeds and applies changes
- If `--fingerprint` is provided:
  - The actual changes on disk are compared against the fingerprint **before any ward files are modified**
  - If changes don't match the fingerprint exactly, ward fails with an error and **no ward files are written**
  - This ensures we are resistant to changes made while the ward process is running
  - Only if changes match the fingerprint will any `.treeward` files be updated

**Concurrent modification detection:**

To detect concurrent modifications during checksumming:

1. **Before checksumming a file:**
   - Record the file's modification time (mtime)

2. **After checksumming a file:**
   - Check the file's mtime again
   - If it changed during our read, return an error indicating concurrent modification

3. This pattern applies to both `ward` and `verify` operations whenever we read file contents for checksumming

4. The operation will fail with an error if concurrent modification is detected - no retry logic or resilience attempts

## Error handling philosophy

**treeward follows a strict error handling policy:**
- Corrupted or unreadable `.treeward` files are fatal errors
- Permission errors when reading files are fatal errors  
- **Never** silently skip problems with warnings - all issues are reported as errors
- Invalid TOML is treated as a corrupted file (no attempt to recover)

## Scope and defaults

- All operations take a directory as a positional argument (defaults to `.`)
- All operations are **always recursive** - they apply to the entire tree under the given directory
- No filtering by file patterns or selective subdirectory operations (not initially supported)

# Implementation plan

* Programing language: Rust (2024 edition)
* Use latest version of clap for command line parsing
* Use thiserror for typed errors in library modules, anyhow is acceptable at the CLI level
* GitHub CI to include cargo fmt, cargo clippy, cargo test.

## Step-by-step implementation approach

Each step below represents a clean slate with full test coverage. We start from the bottom of the dependency graph and work our way up to the CLI layer.

### Step 0: Bootstrap repository

**Goal:** Set up a minimal Rust project with CI infrastructure.

**Deliverables:**
- `Cargo.toml` with basic metadata and dependencies (sha2, clap, toml, base64, thiserror, anyhow)
- `src/main.rs` with a basic "Hello, treeward!" placeholder
- `.github/workflows/ci.yml` with:
  - `cargo fmt --check`
  - `cargo clippy -- -D warnings`
  - `cargo test`
  - `cargo build --release`
- `.gitignore` for Rust projects
- All CI checks passing

**Tests:** No functional tests yet, just ensure CI runs successfully.

---

### Step 1: File checksumming with concurrent modification detection

**Goal:** Implement the core file checksumming logic with mtime-based protection against concurrent modifications.

**Module:** `src/checksum.rs`

**API:**
```rust
#[derive(Debug, thiserror::Error)]
pub enum ChecksumError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("File modified during checksumming: {0}")]
    ConcurrentModification(PathBuf),
    // ... other variants as needed
}

pub struct FileChecksum {
    pub sha256: String,  // hex encoded
    pub mtime: SystemTime,
    pub size: u64,
}

pub fn checksum_file(path: &Path) -> Result<FileChecksum, ChecksumError>;
```

**Behavior:**
- Before reading: record mtime
- Read file in chunks and compute SHA-256
- After reading: verify mtime hasn't changed, return error if it has
- Return checksum, final mtime, and file size

**Tests:**
- Happy path: checksum a simple file
- Large file checksumming (use tempfile with known content)
- Concurrent modification detection (file modified during checksum returns error)
- Permission errors are propagated as errors
- Non-existent file returns appropriate error

**CLI integration:** Update `main.rs` to be a stub that imports the module but doesn't expose functionality yet.

---

### Step 2: TOML ward file format (data structures and serialization)

**Goal:** Define the data structures for `.treeward` files and implement serialization/deserialization.

**Module:** `src/ward_file.rs`

**API:**
```rust
#[derive(Debug, thiserror::Error)]
pub enum WardFileError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("TOML parse error: {0}")]
    TomlParse(#[from] toml::de::Error),
    #[error("TOML serialization error: {0}")]
    TomlSerialize(#[from] toml::ser::Error),
    #[error("Corrupted ward file: {0}")]
    Corrupted(String),
    // ... other variants as needed
}

pub enum EntryType {
    File,
    Dir,
    Symlink,
}

pub struct WardEntry {
    pub name: String,
    pub entry_type: EntryType,
    pub sha256: Option<String>,  // None for symlinks
    pub mtime: Option<SystemTime>,
    pub size: Option<u64>,
    pub symlink_target: Option<PathBuf>,  // Only for symlinks
}

pub struct WardFile {
    pub entries: HashMap<String, WardEntry>,
}

impl WardFile {
    pub fn from_toml(content: &str) -> Result<Self, WardFileError>;
    pub fn to_toml(&self) -> Result<String, WardFileError>;
    pub fn load(path: &Path) -> Result<Self, WardFileError>;
    pub fn save(&self, path: &Path) -> Result<(), WardFileError>;
}
```

**Behavior:**
- Parse TOML into WardFile structure
- Serialize WardFile back to TOML (deterministic ordering for reproducibility)
- Invalid/corrupted TOML is a fatal error
- Handle all entry types: files, directories, symlinks

**Tests:**
- Round-trip serialization (WardFile -> TOML -> WardFile)
- Parse valid TOML examples matching the spec
- Corrupted TOML returns error (not panic)
- Missing required fields in TOML
- Load/save to actual filesystem (use tempfile)
- Deterministic TOML output (same input always produces same output)

**CLI integration:** Still a stub.

---

### Step 3: Filesystem traversal and entry metadata

**Goal:** Traverse a directory tree and collect metadata about files, directories, and symlinks without checksumming yet.

**Module:** `src/traverse.rs`

**API:**
```rust
#[derive(Debug, thiserror::Error)]
pub enum TraverseError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Permission denied: {0}")]
    PermissionDenied(PathBuf),
    // ... other variants as needed
}

pub struct FsEntry {
    pub relative_path: PathBuf,  // relative to traversal root
    pub entry_type: EntryType,
    pub mtime: Option<SystemTime>,
    pub size: Option<u64>,
    pub symlink_target: Option<PathBuf>,
}

pub fn traverse_directory(root: &Path) -> Result<Vec<FsEntry>, TraverseError>;
```

**Behavior:**
- Recursively walk directory tree
- Skip `.treeward` files themselves
- Collect metadata for files, dirs, symlinks
- Symlinks are NOT followed (just their target is recorded)
- Permission errors are fatal
- Sort results by path for deterministic ordering

**Tests:**
- Traverse simple directory structure (use tempfile)
- Handle symlinks correctly (create test symlinks)
- Empty directory
- Nested directories
- Permission errors are propagated
- `.treeward` files are excluded from results

**CLI integration:** Still a stub.

---

### Step 4: Change detection (status computation)

**Goal:** Compare filesystem state against ward files to detect additions, removals, and potential modifications.

**Module:** `src/status.rs`

**API:**
```rust
#[derive(Debug, thiserror::Error)]
pub enum StatusError {
    #[error("Ward file error: {0}")]
    WardFile(#[from] WardFileError),
    #[error("Traverse error: {0}")]
    Traverse(#[from] TraverseError),
    #[error("Checksum error: {0}")]
    Checksum(#[from] ChecksumError),
    // ... other variants as needed
}

pub enum ChangeType {
    Added,
    Removed,
    PossiblyModified,  // Based on mtime/size
    Modified,          // With --verify flag, after checksum
}

pub struct Change {
    pub path: PathBuf,
    pub change_type: ChangeType,
}

pub struct StatusResult {
    pub changes: Vec<Change>,
    pub fingerprint: String,  // base64 encoded hash of changes
}

pub fn compute_status(
    root: &Path,
    verify_checksums: bool,
) -> Result<StatusResult, StatusError>;
```

**Behavior:**
- Load all `.treeward` files in the tree
- Traverse filesystem
- Compare ward state vs filesystem state
- Generate fingerprint (hash of sorted change list)
- If `verify_checksums=true`, compute checksums for "possibly modified" files

**Tests:**
- No changes: empty change list
- Added files/directories
- Removed files/directories
- Modified files (mtime/size changed) without --verify
- Modified files with --verify (actual checksum comparison)
- Fingerprint is deterministic for same changes
- Different changes produce different fingerprints

**CLI integration:** Still a stub.

---

### Step 5: Ward update logic

**Goal:** Implement the logic to create/update `.treeward` files based on current filesystem state.

**Module:** `src/ward.rs`

**API:**
```rust
#[derive(Debug, thiserror::Error)]
pub enum WardError {
    #[error("Ward file error: {0}")]
    WardFile(#[from] WardFileError),
    #[error("Traverse error: {0}")]
    Traverse(#[from] TraverseError),
    #[error("Checksum error: {0}")]
    Checksum(#[from] ChecksumError),
    #[error("Not initialized (use --init to initialize)")]
    NotInitialized,
    #[error("Fingerprint mismatch: expected {expected}, got {actual}")]
    FingerprintMismatch { expected: String, actual: String },
    // ... other variants as needed
}

pub struct WardOptions {
    pub init: bool,
    pub fingerprint: Option<String>,
    pub dry_run: bool,
}

pub struct WardResult {
    pub files_warded: usize,
    pub ward_files_updated: Vec<PathBuf>,
}

pub fn ward_directory(
    root: &Path,
    options: WardOptions,
) -> Result<WardResult, WardError>;
```

**Behavior:**
- Traverse filesystem and checksum all files
- If `fingerprint` provided, verify changes match before writing anything
- Group entries by directory for `.treeward` files
- Only write `.treeward` files if contents changed
- If `!init` and no `.treeward` exists, return error
- In `dry_run` mode, return what would be updated without writing

**Tests:**
- Initial ward (--init) creates `.treeward` files
- Ward without --init when not initialized fails
- Ward with --init when already initialized succeeds
- Fingerprint validation: matching fingerprint succeeds
- Fingerprint validation: mismatched fingerprint fails, no files written
- Dry run doesn't write files but reports what would change
- Only modified `.treeward` files are rewritten
- Directory tree with files, subdirs, symlinks

**CLI integration:** Still a stub.

---

### Step 6: Verify logic

**Goal:** Implement tree verification against stored checksums.

**Module:** `src/verify.rs`

**API:**
```rust
#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error("Ward file error: {0}")]
    WardFile(#[from] WardFileError),
    #[error("Checksum error: {0}")]
    Checksum(#[from] ChecksumError),
    #[error("File not found in ward: {0}")]
    FileNotInWard(PathBuf),
    // ... other variants as needed
}

pub struct VerifyResult {
    pub files_checked: usize,
    pub files_failed: Vec<PathBuf>,
}

pub struct VerifyOptions {
    pub quiet: bool,
}

pub fn verify_directory(
    root: &Path,
    options: VerifyOptions,
) -> Result<VerifyResult, VerifyError>;
```

**Behavior:**
- Load all `.treeward` files
- For each file entry, recompute checksum
- Compare against stored checksum
- Emit per-file output (checkmark/cross) unless quiet
- Return summary statistics

**Tests:**
- All files verify successfully
- Some files fail verification (tampered content)
- Missing files are detected
- Extra files don't cause verification failure (they're just not in ward)
- Quiet mode suppresses per-file output
- Output formatting includes emoji and file paths

**CLI integration:** Still a stub.

---

### Step 7: CLI layer - basic structure

**Goal:** Wire up clap CLI parsing with subcommands but stub implementations.

**Module:** `src/main.rs`, `src/cli.rs`

**API:**
```rust
// In src/cli.rs
pub enum Command {
    Ward { path: PathBuf, init: bool, fingerprint: Option<String>, dry_run: bool },
    Status { path: PathBuf, verify: bool },
    Verify { path: PathBuf, quiet: bool },
}

pub fn parse_args() -> Command;
```

**Behavior:**
- Use clap to define all three subcommands with flags
- Default path is current directory (".")
- Parse command line and return structured Command enum

**Tests:**
- Argument parsing tests for all subcommands
- Default values are applied correctly
- Invalid arguments produce helpful errors

**CLI integration:** Update `main.rs` to call `parse_args()` but still stub out actual execution.

---

### Step 8: CLI layer - wire up ward command

**Goal:** Connect CLI to the ward module implementation.

**Updates:** `src/main.rs`

**Behavior:**
- `treeward ward [path] [--init] [--fingerprint FP] [--dry-run]`
- Call `ward_directory()` with parsed options
- Print user-friendly output and error messages
- Exit with appropriate status codes

**Tests:**
- Integration test: full CLI execution for ward command
- Use `assert_cmd` crate for CLI testing
- Test with temporary directories
- Verify `.treeward` files are created correctly
- Verify dry-run doesn't create files
- Verify fingerprint validation works end-to-end

---

### Step 9: CLI layer - wire up status command

**Goal:** Connect CLI to the status module implementation.

**Updates:** `src/main.rs`

**Behavior:**
- `treeward status [path] [--verify]`
- Call `compute_status()` with parsed options
- Print changes with status codes (A/R/M?/M)
- Print fingerprint suggestion message
- Exit with appropriate status codes

**Tests:**
- Integration test: full CLI execution for status command
- Verify output formatting matches spec
- Verify fingerprint is displayed
- Test --verify flag behavior

---

### Step 10: CLI layer - wire up verify command

**Goal:** Connect CLI to the verify module implementation.

**Updates:** `src/main.rs`

**Behavior:**
- `treeward verify [path] [--quiet]`
- Call `verify_directory()` with parsed options
- Print per-file results (unless --quiet)
- Print summary statistics
- Exit with non-zero code if verification fails

**Tests:**
- Integration test: full CLI execution for verify command
- Verify emoji output for success/failure
- Verify quiet mode behavior
- Verify exit codes

---

### Step 11: End-to-end integration tests

**Goal:** Full workflow testing across all commands.

**Module:** `tests/integration_test.rs`

**Test scenarios:**
- Complete workflow: init → modify files → status → ward with fingerprint → verify
- Multiple directories with nesting
- Symlink handling throughout workflow
- Error cases: permission errors, corrupted ward files
- Concurrent modification during ward (if feasible to simulate)

**Success criteria:** All tests pass, coverage of all major user workflows documented in UX plan.
