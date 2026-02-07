pub(super) const ROOT_LONG_ABOUT: &str = "\
File integrity tool for checksumming and verifying trees

Treeward helps you detect changes in directory trees by maintaining SHA-256 checksums
and metadata for all files, directories, and symlinks. It uses a distributed approach
where each directory contains a .treeward file tracking its immediate children - this
ensures directories can be moved around as self-contained units.

CORE CONCEPTS:

  .treeward files:
    Each directory has a .treeward TOML file containing checksums and metadata for
    its immediate children (files, subdirectories, symlinks). This non-recursive
    per-directory model allows directories to be moved independently while maintaining
    integrity information.

  Ward operations:
    - init: Create .treeward files for the first time
    - update: Update existing .treeward files with current state
    - status: Show what has changed since last ward operation
    - verify: Comprehensive integrity check (automation-friendly)

TYPICAL WORKFLOW:

  1. Initialize a directory tree:
     $ cd /path/to/project
     $ treeward init

     Or without changing directory:
     $ treeward -C /path/to/project init

  2. Make changes to your files
     $ # ... edit, add, remove files ...

  3. Check what changed:
     $ treeward status

  4. Update ward files to record new state:
     $ treeward update

  5. Periodically verify integrity:
     $ treeward verify

COMMANDS:

  init
    Initialize .treeward files in a directory tree for the first time.
    Checksums all files and creates ward metadata. Fails if already initialized.
    Use this when setting up treeward for the first time.

  update
    Update existing .treeward files to reflect current state.
    Only checksums new or modified files (efficient for incremental changes).
    Fails if not initialized - use 'init' first, or 'update --allow-init'.

  status
    Show what has changed: added, removed, or modified files.
    Fast metadata-only check by default, optional checksumming with --verify.
    Produces fingerprints for safe update workflows.

  verify
    Comprehensive integrity check - checksums all files and exits with
    status code 0 if everything matches. Designed for automation, monitoring,
    and CI/CD pipelines.

GLOBAL OPTIONS:

  -C <DIRECTORY>
    Change to directory before operating (like git -C or make -C).
    Defaults to current directory if not specified.

COMMON USE CASES:

  Track changes in a project:
    $ cd /my/project
    $ treeward init
    $ # ... work on project ...
    $ treeward status --verify

  Safe update workflow (prevents TOCTOU):
    $ treeward status > review.txt
    $ cat review.txt  # Review changes
    $ FP=$(grep '^Fingerprint:' review.txt | cut -d' ' -f2)
    $ treeward update --fingerprint $FP

  Detect data corruption:
    $ treeward -C /critical/data status --always-verify

  Automated integrity monitoring:
    $ treeward -C /data verify || alert_admin

  CI/CD artifact verification:
    $ # ... build process ...
    $ treeward -C ./dist update --allow-init
    $ treeward -C ./dist verify

  Idempotent scripting:
    $ treeward update --allow-init  # Works whether initialized or not

KEY FEATURES:

  Efficient incremental updates:
    By default, only checksums files that are new or have changed metadata (mtime/size).
    Subsequent updates after initialization are very fast.

  Fingerprint validation:
    Prevents time-of-check-time-of-use (TOCTOU) race conditions by validating
    that the exact changes you reviewed are what gets recorded.

  Multiple verification modes:
    - Fast metadata checks (default)
    - Selective checksumming (--verify)
    - Full integrity audits (--always-verify)

  Distributed ward model:
    Each directory tracks only its immediate children, allowing independent
    movement and verification of subdirectories.

  Dry run support:
    Preview what would be changed without writing any files.

EXAMPLES:

  # Initialize current directory
  $ treeward init

  # Initialize specific directory (without changing to it)
  $ treeward -C /path/to/project init

  # Check what changed (fast)
  $ treeward status

  # Check what changed (verify checksums)
  $ treeward status --verify

  # Update ward files
  $ treeward update

  # Comprehensive integrity check
  $ treeward verify

  # Automated monitoring of specific directory
  $ treeward -C /data verify || echo 'Integrity check failed!'

For detailed help on any command, use:
  treeward <command> --help

For example:
  treeward init --help
  treeward update --help
  treeward status --help
  treeward verify --help
";

pub(super) const UPDATE_LONG_ABOUT: &str = "\
Update ward files with current state

This command updates existing .treeward files in a directory tree to reflect the current
state of all files, subdirectories, and symlinks. It computes SHA-256 checksums for files
that have changed and updates the ward metadata accordingly.

USAGE MODES:

  Normal update (default):
    treeward update
    treeward -C /path/to/project update

    Updates .treeward files in the current (or specified) directory and subdirectories.
    Fails if the root directory has no .treeward file (use 'treeward init' first).

  Update with --allow-init:
    treeward update --allow-init

    Updates .treeward files, creating them if missing. This makes 'update --allow-init'
    behave idempotently - it works whether or not ward files already exist. Useful for
    scripts and automation where you want to ensure files are warded without checking
    initialization status first.

EFFICIENCY:

The update process meant to be efficient for incremental changes:

  - By default, only checksums files that are new or have changed metadata (mtime/size)
  - Files with matching metadata reuse checksums from existing ward files
  - Only rewrites .treeward files if their contents actually changed
  - Preserves mtimes of unchanged ward files

This makes repeated updates very fast - only changed files are checksummed.

FINGERPRINT VALIDATION (--fingerprint):

Fingerprints prevent time-of-check-time-of-use (TOCTOU) race conditions:

  1. Run 'treeward status' to review changes and get a fingerprint
  2. Review the changes shown by status
  3. Run 'treeward update --fingerprint <FINGERPRINT>' to apply those exact changes

If any files change between the status check and update, the fingerprint won't match
and the update will fail without writing any ward files. This ensures you're updating
exactly what you reviewed.

Example workflow:
  $ treeward status > review.txt
  $ cat review.txt  # Review changes
  # Extract fingerprint from review.txt
  $ treeward update --fingerprint abc123def456...

DRY RUN (--dry-run):

Preview what would be updated without writing any files:

  $ treeward update --dry-run

This shows which .treeward files would be written and how many files would be checksummed,
but performs no writes. Useful for understanding the impact before committing to an update.

INITIALIZATION vs UPDATE:

  'treeward init'                - Initialize a new directory (fails if already initialized)
  'treeward update'              - Update existing ward files (fails if not initialized)
  'treeward update --allow-init' - Update or initialize (always succeeds, idempotent)

The --allow-init flag is for users who want idempotent behavior and don't need the safety
check that distinguishes first-time initialization from incremental updates.

EXAMPLES:

  # Update ward files in current directory
  $ treeward update

  # Update ward files in specific directory (without cd)
  $ treeward -C /path/to/project update

  # Preview what would be updated
  $ treeward update --dry-run

  # Update with fingerprint validation (safe workflow)
  $ FP=$(treeward status | grep '^Fingerprint:' | cut -d' ' -f2)
  $ treeward update --fingerprint $FP

  # Update or initialize (idempotent, for scripts)
  $ treeward update --allow-init
";

pub(super) const INIT_LONG_ABOUT: &str = "\
Initialize ward files in a directory

This command performs the first-time initialization of .treeward files in a directory tree.
It recursively traverses the directory, computes SHA-256 checksums for all files, and creates
.treeward metadata files to record the initial state.

USAGE:

  treeward init
  treeward -C /path/to/project init

Initializes ward files in the current (or specified) directory. This command will FAIL if
the root directory already has a .treeward file - use 'treeward update' for subsequent
changes after initialization.

PURPOSE:

The init command exists as a safety mechanism to distinguish between:
  - First-time setup (init): Creating ward files for the first time
  - Incremental updates (update): Updating existing ward files with changes

This separation prevents accidentally initializing a directory when you meant to update it,
and vice versa. If you want idempotent behavior that works either way, use:
  treeward update --allow-init

WHAT HAPPENS DURING INITIALIZATION:

1. Traverses the entire directory tree recursively
2. Computes SHA-256 checksums for every file
3. Records metadata for files (checksum, mtime, size)
4. Records metadata for directories (just their presence)
5. Records metadata for symlinks (their target paths)
6. Creates a .treeward file in each directory containing metadata for immediate children

The .treeward files use a non-recursive per-directory model - each directory only tracks
its immediate children, not grandchildren. This allows directories to be moved independently
while maintaining integrity information.

FINGERPRINT VALIDATION (--fingerprint):

Like 'update', init supports fingerprint validation to prevent TOCTOU issues:

  1. Run 'treeward status' on an uninitialized directory to see what would be recorded
  2. Review the output
  3. Run 'treeward init --fingerprint <FINGERPRINT>' to initialize with exactly that state

If files change between status and init, the fingerprint won't match and initialization
will fail without writing any ward files.

Note: On uninitialized directories, 'treeward status' shows all files as 'Added'.

DRY RUN (--dry-run):

Preview what would be created during initialization:

  $ treeward init --dry-run

This shows:
  - How many files would be checksummed
  - Which .treeward files would be created
  - No actual files are written

Useful for understanding the scope before committing to initialization.

WHEN TO USE INIT vs UPDATE:

  Use 'init':
    - First time setting up treeward in a directory
    - You want explicit safety against re-initializing
    - You want clear intent in scripts/documentation

  Use 'update --allow-init':
    - You want idempotent behavior (works whether initialized or not)
    - Writing automation that should \"just work\"
    - You don't care about distinguishing first-time vs incremental

PERFORMANCE:

Initial checksumming of a large directory tree can take time since every file must be
read and checksummed. Subsequent updates with 'treeward update' are much faster because
they only checksum changed files.

For very large trees, consider:
  - Using --dry-run first to estimate scope
  - Initializing subdirectories incrementally
  - Running on fast storage or with warm filesystem caches

EXAMPLES:

  # Initialize current directory
  $ treeward init

  # Initialize specific directory (without cd)
  $ treeward -C /path/to/project init

  # Preview initialization without writing files
  $ treeward init --dry-run

  # Safe initialization with fingerprint validation
  $ FP=$(treeward status | grep '^Fingerprint:' | cut -d' ' -f2)
  $ treeward init --fingerprint $FP

  # Initialize, then use update for changes
  $ treeward init
  $ # ... make changes ...
  $ treeward update
";

pub(super) const STATUS_LONG_ABOUT: &str = "\
Show status of files (added, removed, modified)

This command compares the current filesystem state against existing .treeward files to
detect changes. It reports what has been added, removed, or modified since the last
ward operation.

USAGE:

  treeward status                         # Fast metadata-only check (default)
  treeward status --verify                # Checksum files with changed metadata
  treeward status --always-verify         # Checksum all files (detect silent corruption)
  treeward status --diff                  # Show detailed diff of changes (implies --verify)
  treeward -C /path/to/project status     # Check specific directory

CHANGE TYPES:

The status command reports four types of changes:

  Added: New files, directories, or symlinks not in the ward
  Removed: Entries in the ward that no longer exist on filesystem
  PossiblyModified: Files whose metadata (mtime/size) differs from ward
  Modified: Content differs (checksum mismatch when verified), symlink target changed, or entry type changed

VERIFICATION MODES:

By default, status uses a fast metadata-only check:

  $ treeward status

This compares file modification times and sizes against the ward. Files with differing
metadata are reported as 'PossiblyModified'. This is very fast but doesn't detect:
  - Content changes that preserve mtime/size (rare but possible)
  - Silent data corruption that doesn't change metadata

To verify file contents, use one of the verification flags:

  --verify (recommended):
    $ treeward status --verify

    Checksums only files that appear possibly modified (differing metadata). This upgrades
    'PossiblyModified' entries to either 'Modified' (content changed) or removes them from
    the report (content unchanged, only metadata touched).

    This is the recommended mode for verification as it's efficient - only changed files
    are checksummed.

  --always-verify (thorough):
    $ treeward status --always-verify

    Checksums ALL files in the tree, even those with matching metadata. This detects:
      - Silent data corruption (bitrot, disk errors)
      - Sophisticated attacks that preserve metadata
      - Any content changes regardless of metadata

    This is slower but provides the highest assurance. Useful for:
      - Periodic integrity audits
      - High-value data verification
      - Detecting hardware-level corruption

DIFF MODE:

The --diff flag shows detailed information about what changed for each entry:

  $ treeward status --diff

For modified files, this shows old and new values for any changed fields (size, mtime, sha256):

  M  data.json
     size: 1.2 KB -> 1.5 KB
     mtime: 2024-01-15 10:30:45.123 -> 2024-01-16 14:22:10.456
     sha256: abc123def456... -> 789xyz012345...

For removed entries, it shows what was recorded in the ward:

  R  oldfile.txt
     was: file (256 bytes, sha256: abc123def456...)

For type changes (e.g., file replaced with directory), both old and new types are shown.

The --diff flag implies --verify, since showing sha256 differences requires checksumming.

FINGERPRINTS:

Every status check produces a unique fingerprint representing the exact changeset:

  Fingerprint: abc123def456...

This fingerprint is a Base64-encoded SHA-256 hash of all detected changes. Use it with
'treeward init --fingerprint' or 'treeward update --fingerprint' to ensure you're
applying exactly the changes you reviewed:

  $ treeward status > review.txt
  $ cat review.txt  # Review changes
  $ FP=$(grep '^Fingerprint:' review.txt | cut -d' ' -f2)
  $ treeward update --fingerprint $FP

If any files change between status and init/update, the fingerprint won't match and
the operation will fail. This prevents time-of-check-time-of-use (TOCTOU) issues.

UNINITIALIZED DIRECTORIES:

Status works on uninitialized directories (those without .treeward files):

  $ treeward -C /path/to/uninitialized/dir status

All files will be reported as 'Added' since there's no existing ward to compare against.
This is useful for previewing what would be recorded during initialization.

UNDERSTANDING OUTPUT:

The output shows a status code and path for each changed entry:

  A  newfile.txt
  R  oldfile.txt
  M? data.json
  M  config.yaml

  Fingerprint: abc123...

Status codes:
  A   Added - new entry not in ward
  R   Removed - entry in ward no longer exists
  M?  PossiblyModified - metadata differs, content not verified
  M   Modified - content verified as changed

Paths are relative to the root directory being checked. For recursive checks, subdirectory
paths are shown with their full relative path.

With --diff, each entry also shows what changed (see DIFF MODE above).

PERFORMANCE:

  Metadata-only (default): Very fast, only reads directory listings and .treeward files
  --verify: Fast for incremental changes, only checksums modified files
  --always-verify: Slower, reads and checksums every file in the tree

For large trees with few changes, --verify is nearly as fast as the default mode.

INTEGRATION WITH INIT/UPDATE:

Common workflows:

  # Check status before updating
  $ treeward status
  $ treeward update

  # Safe workflow with fingerprint validation
  $ treeward status --verify > review.txt
  $ # Review the changes
  $ FP=$(grep '^Fingerprint:' review.txt | cut -d' ' -f2)
  $ treeward update --fingerprint $FP

  # Periodic integrity check
  $ treeward status --always-verify
  # If no changes shown, all files match their checksums

EXAMPLES:

  # Quick metadata check of current directory
  $ treeward status

  # Verify changed files in specific directory (without cd)
  $ treeward -C /path/to/project status --verify

  # Full integrity audit (check all checksums)
  $ treeward status --always-verify

  # Show detailed diff of what changed
  $ treeward status --diff

  # Preview what would be initialized
  $ treeward -C /path/to/uninitialized/dir status

  # Automated verification (exit code 0 if no changes)
  $ treeward status --always-verify || echo \"Changes detected!\"
";

pub(super) const VERIFY_LONG_ABOUT: &str = "\
Verify consistency of the ward, exit with success if no inconsistency

This command verifies the integrity of all files in a directory tree by checksumming
every file and comparing against the ward. It's designed for automated verification and
monitoring where you want a simple success/failure result.

USAGE:

  treeward verify
  treeward -C /path/to/project verify

Verifies all files in the current (or specified) directory. This is equivalent to:

  treeward status --always-verify

The key difference is the intended use case and output format:
  - 'verify' is for automation and monitoring where any differences are
    considered errors and are logged as such
  - 'status --always-verify' is for interactive review and detailed reporting
    where discrepancies are not necessarily errors

BEHAVIOR:

The verify command:

1. Recursively traverses the directory tree
2. Checksums EVERY file, regardless of metadata
3. Compares checksums against .treeward files
4. Reports any inconsistencies found
5. Exits with status code 0 if everything matches
6. Exits with non-zero status code if any changes detected

This comprehensive check detects:
  - Modified files (content differs from ward)
  - Added files (exist on filesystem but not in ward)
  - Removed files (in ward but missing from filesystem)
  - Silent data corruption (bitrot, disk errors)
  - Metadata manipulation attacks

WHAT IT DOESN'T DO:

Unlike 'treeward status', verify does NOT support:
  - --verify or --always-verify flags (always checksums everything)
  - Fingerprint output (it's a pass/fail check)
  - Metadata-only checks (always reads file contents)

The command is intentionally simple and focused on one task: comprehensive verification.

USE CASES:

**Automated monitoring:**
  $ treeward -C /critical/data verify || alert_admin

**Cron jobs:**
  0 2 * * * /usr/local/bin/treeward -C /data verify || mail -s \"Integrity check failed\" admin@example.com

**Pre-deployment verification:**
  $ treeward -C /app/build verify || exit 1
  $ deploy_to_production

**Periodic integrity audits:**
  $ treeward -C /archive verify
  $ echo $?  # 0 = all good, non-zero = issues found

**CI/CD pipeline checks:**
  - name: Verify build artifacts
    run: treeward -C ./dist verify

EXIT CODES:

  0: All files match their wards (success)
  Non-zero: Changes detected or errors encountered (failure)

The specific non-zero exit code may vary based on the type of error (changes vs I/O errors),
but scripts should simply check for zero (success) vs non-zero (failure).

PERFORMANCE:

Verify reads and checksums every file in the tree, so it's slower than:
  - 'treeward status' (metadata-only check)
  - 'treeward status --verify' (only checksums changed files)

For very large trees, consider:
  - Running during low-usage periods (cron at night)
  - Parallelizing across subdirectories if needed
  - Using fast storage for better I/O performance

The checksumming speed depends on:
  - Disk I/O performance
  - File sizes and count
  - CPU speed (SHA-256 computation)
  - Filesystem cache state

COMPARISON WITH OTHER COMMANDS:

  'treeward verify':
    - Always checksums all files
    - Designed for automation
    - Simple pass/fail exit code
    - Minimal output

  'treeward status':
    - Fast metadata-only check by default
    - Designed for interactive use
    - Detailed change reporting
    - Produces fingerprints

  'treeward status --always-verify':
    - Same verification as 'verify'
    - Designed for interactive review
    - Detailed output with fingerprints
    - Can feed into 'update --fingerprint'

Choose based on your use case:
  - Automation/monitoring → use 'verify'
  - Interactive review → use 'status --always-verify'
  - Quick check before update → use 'status'

EXAMPLES:

  # Verify current directory
  $ treeward verify
  $ echo $?  # Check exit code

  # Verify specific directory (without cd)
  $ treeward -C /path/to/data verify

  # Use in shell script
  #!/bin/bash
  if treeward -C /critical/data verify; then
    echo \"Integrity check passed\"
  else
    echo \"WARNING: Integrity check failed!\"
    exit 1
  fi

  # Verify before deploying
  $ treeward -C ./build verify && deploy.sh

  # Cron job with email alert
  0 2 * * * /usr/local/bin/treeward -C /data verify || echo \"Integrity failure\" | mail admin

  # CI/CD pipeline
  - name: Verify artifacts
    run: |
      treeward -C ./artifacts verify
      if [ $? -ne 0 ]; then
        echo \"Artifact integrity check failed\"
        exit 1
      fi
";
