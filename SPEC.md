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
  sequences (OSC/CSI) into the listing. Diagnostic logging (`-v`) and error messages on stderr are NOT covered by this
  guarantee and may contain raw names.

- A child entry that vanishes between listing a directory and inspecting it is a fatal error (concurrent modification);
  it is never silently treated as removed. This includes a directory that disappears between being listed and being
  walked. A directory that was already absent when its parent was listed is reported as removed by its parent.

- A `.treeward` file whose `sha256` fields are not exactly 64 lowercase hex characters is rejected as corrupt with a
  fatal error at load time.

- Written `.treeward` files get standard umask-derived permissions (0666 masked by the process umask), like any normally
  created file — not owner-only modes that would break `verify` for other users in group-shared trees. NOTE: a readable
  ward file discloses the sha256, size, and mtime of every sibling entry, including files whose own permissions are more
  restrictive. Anyone needing those checksums kept private must rely on directory permissions or a stricter umask.
