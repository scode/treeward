# TODO

Open work items that are known but not yet scheduled. Each item points at where the context lives (a lore entry, an
issue, a spec bullet) rather than repeating it here. Remove an item when the work lands or the decision is made not to
do it.

## Bugs

- Emptying a tracked subdirectory (`rm -rf sub; mkdir sub`) is invisible to `status`, `verify`, and `update`, and
  `update` seals the loss. Options and reasoning: `lore/2026-09-13-silent-subdir-emptying.md`.
