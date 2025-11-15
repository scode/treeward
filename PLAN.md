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

**Concurrent modification protection:**

To protect against concurrent modifications with high probability:

1. **Before checksumming a file:**
   - Check the file's modification time (mtime)
   - If mtime is not at least 1 second in the past, wait until it is
   - Record the mtime before starting to read

2. **After checksumming a file:**
   - Check the file's mtime again
   - If it changed during our read, log a message and retry the checksum
   - Before retrying, apply the same 1-second-in-the-past rule

3. This pattern applies to both `ward` and `verify` operations whenever we read file contents for checksumming

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

# Implementation Implementation

TODO
