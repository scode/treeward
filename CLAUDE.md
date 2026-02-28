# CLAUDE.md

Guidance for agents editing this repository.

## Style And Intent

- Place Rust doc comments (`///` and `//!`) before attribute directives such as `#[derive(...)]`, `#[cfg(...)]`, and
  serde attributes.
- Prefer comments that explain intent, tradeoffs, and context over comments that restate obvious behavior.
- Do not add trivial inline comments.

## Domain Constraints (Non-Obvious)

- Keep the non-recursive model: each directory has its own `.treeward` file containing only immediate children.
- Keep entry naming and shape consistent across runtime and persisted representations:
  - Field name is `symlink_target` (not `target`).
  - Entry variants are `File`, `Dir`, and `Symlink`.
  - Persisted file entries use `mtime_nanos` (`u64`) and `size`, plus `sha256` in ward files.
- Use deterministic maps (`BTreeMap`) for entry collections to preserve stable TOML output.
- Keep `#[serde(deny_unknown_fields)]` on persisted types to fail fast on unexpected input.

## Error And Integrity Policy

- Use `thiserror` for typed library errors; `anyhow` is acceptable at CLI boundaries.
- Corrupted/unreadable `.treeward` files and permission failures are fatal errors.
- Do not silently skip filesystem problems.
- Preserve concurrent-modification checks when checksumming files (compare mtime before and after read; no retries).

## Filesystem Semantics

- Symlinks are tracked but never followed.
- Use `symlink_metadata()` (not `metadata()`) when type-dispatching filesystem entries.
- Use `read_link()` for symlink targets.
- Broken symlinks are valid tracked entries.

## Time Representation

- Runtime metadata uses `SystemTime`.
- Persisted metadata uses nanoseconds since `UNIX_EPOCH` in `mtime_nanos` (`u64`).
- Use nanosecond-precision conversion when writing ward files.

## Recursive Operations

- For tree walks, compare both ward entries and filesystem entries (union traversal).
- Visit subdirectories found in either source.
- Canonicalize the root path before recursive operations.

## Validation Before Finishing

- `cargo test`
- `cargo clippy -- -D warnings`
- `cargo fmt`
- `dprint fmt`
