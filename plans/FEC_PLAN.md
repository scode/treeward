# Plan: Add Forward Error Correction (FEC) to Treeward

## Overview

Add file repair capability using RaptorQ forward error correction. Users can protect files against data loss/corruption
with configurable tolerance levels.

## Key Design Decisions

- **Storage**: FEC data in `.treeward-fec/` directory, content-addressed files
- **Configuration**: `--loss-tolerance N` (percentage of data loss to protect against, default 10%)
- **Persistence**: Protection settings stored per-directory in `.treeward` metadata (version 2)
- **Automatic updates**: Protected files have FEC regenerated automatically during `update`
- **Commands**: `protect`, `unprotect`, `repair`

## File Format Changes

### `.treeward` v2 Metadata

```toml
[metadata]
version = 2

[metadata.protection]
enabled = true
loss_tolerance = 10 # percentage
```

### `.treeward-fec/<sha256>.fec` Format

Binary format with segmented FEC for large file support (never loads entire file into memory):

```
[Header]
  4 bytes   Magic: "TWFC"
  4 bytes   Format version (1)
  8 bytes   Original file size
  4 bytes   Segment size (default 64MB, configurable for testing)
  4 bytes   Number of segments

[Per-segment, repeated N times]
  8 bytes   Segment offset in original file
  4 bytes   Segment length (may be < segment_size for final segment)
  12 bytes  ObjectTransmissionInformation for this segment
  4 bytes   Number of repair packets
  [packets, each with 4-byte length prefix]
  32 bytes  SHA-256 of this segment's FEC data (offset through packets)

[Footer]
  32 bytes  SHA-256 of entire file (header + all segments)
```

Each segment is processed independently, allowing:

- Streaming generation without loading full file
- Per-segment repair (only read corrupted portions)
- Per-segment FEC integrity verification

## New Module: `src/fec.rs`

Thin wrapper around raptorq with streaming support for large files:

```rust
pub const DEFAULT_SEGMENT_SIZE: usize = 64 * 1024 * 1024;  // 64MB

pub struct FecConfig {
    pub loss_tolerance_percent: u8,  // 1-100
    pub segment_size: usize,         // default 64MB, smaller for tests
}

pub struct SegmentFec {
    pub offset: u64,
    pub length: u32,
    pub oti: ObjectTransmissionInformation,
    pub repair_packets: Vec<EncodingPacket>,
}

pub struct FecFile {
    pub original_size: u64,
    pub segment_size: u32,
    pub segments: Vec<SegmentFec>,
}

// Streaming FEC generation - processes file in segments, never loads full file
pub fn generate_fec_streaming<R: Read + Seek>(
    reader: &mut R,
    file_size: u64,
    config: &FecConfig,
) -> Result<FecFile, FecError>;

// Repair a single segment (used internally)
pub fn repair_segment(
    corrupted_data: &[u8],
    segment_fec: &SegmentFec,
) -> Result<Vec<u8>, FecError>;

// Reads file segment by segment, repairs corrupted segments, writes back in place
pub fn repair_file_streaming<F: Read + Write + Seek>(
    file: &mut F,
    original_size: u64,
    fec: &FecFile,
) -> Result<RepairResult, FecError>;

// Binary serialization with embedded checksums
pub fn write_fec_file<W: Write>(fec: &FecFile, writer: &mut W) -> Result<(), FecError>;
pub fn read_fec_file<R: Read>(reader: &mut R) -> Result<FecFile, FecError>;
```

## Implementation Steps

### Step 1: Add raptorq dependency

- Add `raptorq = "2"` to `Cargo.toml`

### Step 2: Create `src/fec.rs` module

- Implement `FecConfig`, `SegmentFec`, `FecFile`, `FecError` types
- Implement `generate_fec_streaming()`: read file in segments, generate FEC per-segment
- Implement `repair_segment()`: reconstruct single segment from corrupted data + FEC
- Implement `repair_file_streaming()`: identify corrupted segments, repair in place
- Implement `write_fec_file()`/`read_fec_file()`: binary format with per-segment and overall checksums
- Calculate repair symbol count from loss_tolerance_percent
- Unit tests with small segment sizes for fast execution

### Step 3: Update `src/ward_file.rs` for v2

- Add `ProtectionSettings` struct to metadata
- Update `SUPPORTED_VERSION` to 2
- Add migration: v1 files parsed and auto-upgraded on write
- Keep backward compatibility: v1 files readable, written as v2

### Step 4: Create `src/protection.rs` module

- Manage `.treeward-fec/` directory
- FEC file naming: `<sha256>.fec` (content-addressed)
- Functions:
  - `ensure_protected(path, sha256, data, config)` - generate/update FEC
  - `remove_protection(sha256)` - delete FEC file
  - `load_fec(sha256)` - load FEC data for repair
  - `repair_file(path, sha256)` - attempt repair, return success/failure

### Step 5: Update `src/update.rs`

- After checksumming a file, if directory has protection enabled:
  - Generate FEC data
  - Write to `.treeward-fec/<sha256>.fec`
- Clean up orphaned FEC files (sha256 no longer in ward)

### Step 6: Update `src/status.rs`

- When protection enabled, check FEC file exists for each protected file
- New status: `MissingFec` (file exists, protection on, but no FEC data)
- Verify FEC file checksum during `--always-verify`

### Step 7: Add CLI commands in `src/cli.rs` and `src/main.rs`

**`treeward protect [--loss-tolerance N]`**

- Enable protection on current directory (recursive by default)
- Set `metadata.protection.enabled = true` and `loss_tolerance = N`
- Generate FEC for all existing files
- Log warning if subdirectories have different loss_tolerance

**`treeward unprotect`**

- Disable protection on current directory
- Delete `.treeward-fec/` directory
- Set `metadata.protection.enabled = false`

**`treeward repair [--dry-run]`**

- For each file marked as Modified/corrupted by status check:
  - Load FEC data
  - Attempt repair
  - Verify repaired file matches expected sha256
  - Replace corrupted file atomically
- Report: repaired, repair failed, no FEC available

### Step 8: Update `src/dir_list.rs`

- Exclude `.treeward-fec/` from directory listings (like `.treeward`)

### Step 9: Automatic v1→v2 migration

- Any write operation (`init`, `update`, `protect`) upgrades v1 to v2
- Read operations accept both v1 and v2

## Files to Modify

| File                | Changes                                   |
| ------------------- | ----------------------------------------- |
| `Cargo.toml`        | Add `raptorq = "2"`                       |
| `src/lib.rs`        | Add `mod fec; mod protection;`            |
| `src/fec.rs`        | **New** - RaptorQ wrapper                 |
| `src/protection.rs` | **New** - FEC file management             |
| `src/ward_file.rs`  | v2 format, protection settings, migration |
| `src/dir_list.rs`   | Exclude `.treeward-fec/`                  |
| `src/status.rs`     | Check FEC existence, MissingFec status    |
| `src/update.rs`     | Auto-generate FEC for protected files     |
| `src/cli.rs`        | Add protect/unprotect/repair commands     |
| `src/main.rs`       | Command handlers                          |

## Testing Strategy

1. **`fec.rs` unit tests**: Use small segment sizes (e.g., 1KB) for fast tests
   - Round-trip encode/decode single segment
   - Multi-segment files (verify segment boundaries handled correctly)
   - Repair with simulated corruption at various positions
   - Edge cases: empty file, file exactly at segment boundary, single-byte file
   - 100% loss tolerance (full reconstruction from FEC only)
2. **`protection.rs` tests**: FEC file I/O, content-addressing, orphan cleanup
3. **`ward_file.rs` tests**: v1→v2 migration, protection settings serialization
4. **Integration tests**: Full protect→corrupt→repair cycle, status reporting with FEC

## Repair Semantics

- Repair works with whatever source data is available (partial file, truncated, or empty)
- With `--loss-tolerance N`, user can recover from up to N% loss of combined (file + FEC) data
- Special case: `--loss-tolerance 100` stores enough repair data to reconstruct from zero source bytes
- Same code path handles all cases; no separate "full backup" feature needed

## Open Questions Resolved

- Loss tolerance: user-facing percentage (1-100), we calculate repair symbols internally
- Storage: `.treeward-fec/<sha256>.fec` (content-addressed)
- Automatic: protected files stay protected through updates
- Verification: `verify` reports, never repairs; explicit `repair` command needed
- FEC integrity: embedded SHA-256 checksum in FEC file format
- Full file loss: supported if user chooses `--loss-tolerance 100`
