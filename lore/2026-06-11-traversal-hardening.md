# Traversal hardening against directory-swap races

NOTE: This documents a known, unfixed weakness and the rough shape of the intended fix. Nothing described here is
implemented as of this writing.

## The problem

Security review finding F8 (SEC-DIRSWAP): a subdirectory swapped for a symlink between listing and recursion is followed
by the later `read_dir` or `.treeward` write.

The traversal is path-based. `walk_directory` classifies an entry as a directory from `symlink_metadata` taken inside
`list_directory`, then later recurses into `current_dir.join(name)`, where `std::fs::read_dir` follows a symlink at the
final component. A local attacker who swaps the directory for a symlink in that window gets foreign content recorded as
ward state.

The write side is worse. `ward_directory` builds all ward files in memory during the walk and only writes `.treeward`
files at the end, keyed by absolute path. The window between "this was a directory" and "write `.treeward` into it"
spans the entire checksum pass — seconds to minutes on a large tree. An attacker-controlled symlink at write time gets a
`.treeward` written into an arbitrary directory.

`checksum_file` already defends against exactly this class for regular files (`O_NOFOLLOW` plus a dev/ino re-check of
the held handle). Directories are the remaining gap.

## Why the file pattern does not transplant

The file defense is sound because the open handle pins the inode: `O_NOFOLLOW` blocks the swap at open time, and the
dev/ino re-check compares the held handle against a fresh `symlink_metadata`. For directories, std gives us no
equivalent — `std::fs::ReadDir` holds a dir handle internally but does not expose it, so a path-based "re-stat and
compare" after `read_dir` has no pinned identity to compare against. An attacker can swap, let the operation hit the
foreign directory, and swap back before the re-check. Detection-only hardening at the path level shrinks the window but
does not close it. Half-measures here are not worth doing.

## Shape of the fix: handle-relative traversal

The class goes away entirely if no operation after the root open ever re-traverses a path. That is `openat`-style
traversal:

- Open the canonicalized root once as a directory handle.
- List children via the handle, not via a path.
- Open each subdirectory relative to its parent handle with `O_NOFOLLOW | O_DIRECTORY`. A swapped-in symlink fails with
  `ELOOP`, which maps naturally onto the existing fatal concurrent-modification policy (no retries).
- Open files for checksumming via `openat(dirfd, name, O_NOFOLLOW)`. This also closes a residual gap in the file
  defense: today the path prefix leading to a file can still be swapped even though the final component cannot.
- Write `.treeward` via the directory handle (`openat` plus write, or tmp file plus `renameat` for atomicity).

One design wrinkle on the write side: keeping every dirfd alive from walk to write would mean one fd per directory,
which does not scale. Instead, record dev/ino per directory during the walk; at write time, re-open each target by
walking component-wise from the root handle with `O_NOFOLLOW | O_DIRECTORY` at every step, then verify the final
handle's dev/ino matches the walk-time value before writing through it. Component-wise `openat` is itself symlink-proof;
the dev/ino check additionally detects plain directory renames within the tree, consistent with the mtime-style
detection policy.

## Implementation options

cap-std is the recommended route. `cap_std::fs::Dir` is built for exactly this: all operations are handle-relative `*at`
syscalls, symlinks cannot escape the tree, and it works on Linux, macOS, and Windows (where it handles the reparse-point
equivalents currently hand-rolled in `checksum_file`). `list_directory` changes signature from `&Path` to a `Dir`,
`walk_directory` passes child `Dir` handles down, `checksum_file` takes an already-opened `File` (or a `Dir` plus name),
and `WardFile::save` writes through a `Dir`. Most of the platform-specific open code in `checksum.rs` disappears.

The alternative is hand-rolling with rustix/libc: same design, unix-only, with Windows falling back to current
path-based behavior. There is precedent for per-platform splits in `checksum.rs`, but that choice leaves the gap open on
Windows and means maintaining more unsafe-adjacent plumbing ourselves.

Either way it is a real refactor — `dir_list`, the `status` walk, the `update` write loop, and `checksum` all shift from
path-based to handle-based — but it is the difference between eliminating the class and narrowing it.

When implemented, SPEC.md should gain an entry along the lines of: directory contents and `.treeward` writes are never
redirected through symlinks; a directory replaced during an operation is a fatal error, not silently followed.
