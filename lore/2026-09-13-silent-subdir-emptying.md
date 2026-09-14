# Silent emptying of a tracked subdirectory

NOTE: Historical artifact, written 2026-09-13 from a code review finding against commit df05b13. Records a known,
unfixed gap and the options considered. Nothing described here was implemented at the time of writing; check `SPEC.md`
and the code for what actually happened since.

## The problem

A parent `.treeward` records a subdirectory as a bare presence marker, `Dir {}`, with no information about what is
inside it. The child's own `.treeward` is excluded from scans like every ward file. Put those two facts together and
`rm -rf sub; mkdir sub` is invisible: the parent sees "directory still present", the recreated `sub` has no ward file
and no children so it has nothing to compare, and every read command reports clean.

Reproduced on the built binary at df05b13. After `init` on a tree containing `sub/child.txt`, wipe and recreate `sub`,
then:

```
$ treeward status    # exit 0, no output
$ treeward verify    # exit 0, no output
$ treeward update    # exit 0, rewrites sub/.treeward with an empty [entries] table
```

The `update` step is the nasty part: it seals the loss, and afterwards there is no record anywhere that `child.txt` ever
existed. Deleting the ward file of an already-empty tracked directory is silent in the same way. Two neighbouring cases
do work: deleting a child ward while its files remain shows the files as added, and removing the whole subdirectory is
reported as removed.

## Why this is not obviously a bug

The non-recursive per-directory model is a deliberate design choice, stated in the README, the help text, and
`AGENTS.md`: a directory is a self-contained unit that can be moved without disturbing its ancestors, and a parent knows
nothing about grandchildren. Under that model "the parent cannot tell that the child was emptied" is a direct
consequence, not an oversight. The reviewer's headline fix, recording a hash (or entry count) of the child's ward file
in the parent's `Dir` entry, would quietly abandon the model: any update inside a subdirectory would dirty every
ancestor up to the root, which is exactly the coupling the design avoids. It is also an on-disk format change.

## The narrower observation that survives

There is a state the tool can distinguish today but does not: "tracked directory with no `.treeward` at all" versus
"tracked directory with an empty ward". Every directory that `init` or `update` visits gets a ward file written, so a
child listed as `Dir` in its parent was warded at the time. Finding it later without a ward is an anomaly (someone
deleted the ward, or deleted and recreated the directory), and today that anomaly is treated as identical to a
legitimately empty directory.

Reporting a missing child ward as its own finding in `status` and `verify` catches both scenarios above without any
format change and without leaving the non-recursive model. It does not catch a wipe where the attacker also drops in a
forged empty ward file. That is content forgery, and defending against it genuinely requires binding child state into
the parent, which is the design question again.

## Options as understood at the time

Narrow fix: treat a missing ward file in a tracked subdirectory as a distinct reported condition in `status` and
`verify`, probably with `update` refusing to seal it without an explicit flag, plus a `SPEC.md` bullet. This was the
option I leaned toward.

Spec only: add a `SPEC.md` bullet stating that a child directory's integrity is judged solely by its own ward file and
that a missing ward in a tracked directory is treated as empty, so the gap is documented and future reviews stop
re-raising it.

Design change: bind child ward state into the parent entry, with a ward format version bump and migration. Rejected for
now because it contradicts the stated model; recorded here so the reasoning does not have to be rediscovered.

Do nothing. Recorded as an option, not recommended, since `verify` is the command people will build monitoring on.
