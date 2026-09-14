# SPEC

This file specifies treeward's user-observable behavior: command output, exit codes, and on-disk formats. It does not
describe implementation choices — those belong in code and doc comments.

The spec is the contract. When behavior is intentionally changed, the change and the spec update land together.
Otherwise, the implementation conforms to what is written here; divergence is an implementation bug.

NOTE: This spec is bootstrapped empty and populated incrementally as behavior is intentionally introduced or changed.
Absence of an entry means the behavior is not yet specified, not that it is unspecified by design.

## Behaviors

- The entry listing printed to stdout by `status` and `verify` never emits raw control characters from scanned file
  names, symlink targets, or ward-file fields. Control characters (including C1 controls such as the single-byte CSI)
  are rendered as backslash escapes (`\n`, `\t`, `\u{1b}`, ...), and literal backslashes are doubled so escaped output
  is unambiguous; all other Unicode is printed unchanged. This prevents crafted names from injecting terminal escape
  sequences (OSC/CSI) into the listing.

- Diagnostic logging (`-v`) and error messages printed to stderr never emit raw control characters (including C1
  controls), so crafted names cannot inject terminal escape sequences through diagnostics either. Control characters are
  rendered in an escaped textual form; the exact rendering is not specified and may differ from the stdout listing's
  escapes. Command-line usage errors reported while arguments are still being parsed are outside this guarantee: they
  echo the offending argument verbatim, and the argument came from the invoker rather than from a scanned tree or ward
  file.

- A child entry that vanishes between listing a directory and inspecting it is a fatal error (concurrent modification);
  it is never silently treated as removed. This includes a directory that disappears between being listed and being
  walked. A directory that was already absent when its parent was listed is reported as removed by its parent.

- A file whose size or modification time changes between the listing of its directory and the moment it is checksummed
  is likewise a fatal concurrent-modification error, in every command that checksums it. In particular, `update` never
  records content that differs from what the fingerprint it validated was computed over. A command that never reads the
  file (for example `status` under the default metadata-only policy) is unaffected and reports from the listing alone.

- The fingerprint printed by `status` and validated by `init --fingerprint`/`update --fingerprint` binds exactly what
  the chosen checksum policy observes for each changed entry: its path and status class, plus mtime and size for files
  and the target for symlinks, and the content checksum only when the policy read the file (`--verify` for
  metadata-differing files, `--always-verify` for all). Under the default policy an edit that preserves a file's mtime
  and size does not change the fingerprint. The fingerprint is only meaningful when `status` and `init`/`update` run
  under the same policy; with different `--verify`/`--always-verify` flags the two never match.

- A `.treeward` file whose `sha256` fields are not exactly 64 lowercase hex characters is rejected as corrupt with a
  fatal error at load time.

- A `.treeward` file containing an entry whose name could not have come from scanning a directory — a name with a path
  separator, `.`, `..`, a NUL byte, the reserved name `.treeward` itself, or a name that differs from `.treeward` only
  by ASCII case — is rejected as corrupt with a fatal error at load time.

- A directory entry whose name differs from `.treeward` only by ASCII case (`.TREEWARD`, `.Treeward`, ...) is refused by
  `init`/`status`/`update`/`verify` with a fatal error naming it, on every platform. On a case-insensitive filesystem
  such an entry is the ward file's own path; refusing it everywhere keeps ward files portable and rules out reading or
  overwriting the user's file under the ward file's name.

- Written `.treeward` files get standard umask-derived permissions (0666 masked by the process umask), like any normally
  created file — not owner-only modes that would break `verify` for other users in group-shared trees. NOTE: a readable
  ward file discloses the sha256, size, and mtime of every sibling entry, including files whose own permissions are more
  restrictive. Anyone needing those checksums kept private must rely on directory permissions or a stricter umask.

- Checksumming a path that is no longer a regular file (e.g. swapped for a FIFO or device mid-run) is a fatal error; it
  never blocks waiting on the object.

- On Unix, when `init`/`update` reports success, written `.treeward` files are durable: file contents and the rename
  into place are both flushed (including a parent-directory fsync) before success is reported. On filesystems that do
  not support directory fsync (some FUSE and network mounts), and on non-Unix platforms, the rename flush is skipped and
  durability of the rename is best-effort.

- `update` reports "Not initialized" only when nothing at all exists at the root `.treeward` path, and `init` reports
  "Already initialized" only when a regular file does. If anything else occupies that path (a directory, or a symlink
  whether or not it resolves), `init` and `update` (including `--allow-init`) abort with a fatal error naming the path
  and leave the entry untouched: they never follow it, read through it, or replace it.

- `init`/`update` write each `.treeward` by first writing a temporary file named `.treeward.tmp-` followed by random
  characters in the same directory, then renaming it into place. An interrupted run (crash, kill) can leave such a file
  behind. It is reported by `status`/`verify` as an ordinary added entry and is never silently excluded from scans, so
  nothing can be hidden from `verify` under that name. Deleting a leftover is safe; the next `init`/`update` creates a
  fresh one.

- `init`/`update` write ward files one directory at a time and are not atomic as a set. If a write fails partway, the
  command exits with a fatal error stating how many of the changed ward files were written, and the tree is left
  partially updated. In that state no written `.treeward` lists a subdirectory whose own `.treeward` is missing or
  stale, so `status` still reports the unfinished changes at the level the user reviewed them. Re-running `init` or
  `update` after fixing the cause writes only the remainder. A fingerprint taken before the failed run no longer
  matches, since the committed ward files removed entries from the pending changeset; `status` must be re-run to obtain
  a fresh one. A `.treeward` that already matches its new content is never rewritten.

- Entry names and symlink targets that are not valid UTF-8 are not supported: `init`/`status`/`update`/`verify`
  (including `--dry-run`) abort with a fatal error naming the offending entry (for a symlink, the link itself) before
  any `.treeward` file is written. This is a deliberate limitation of the TOML on-disk format, which is UTF-8 only.

- Regular files with modification times before the Unix epoch (pre-1970) or above `i64::MAX` nanoseconds since the epoch
  (~year 2262) are not supported: `init`/`status`/`update`/`verify` abort with a fatal error naming the offending file.
  This is a deliberate limitation of the TOML `mtime_nanos` on-disk format: TOML integers are `i64`. Directory and
  symlink modification times are never recorded or examined, so out-of-range values on those are not an error.
