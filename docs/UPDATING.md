# Updating the fork from upstream

The repository has two remotes:

- `origin`: `git@github.com:ocque41/grok-build.git`, the user-owned fork;
- `upstream`: `git@github.com:xai-org/grok-build.git`, the read-only source.

The upstream remote has no usable push URL. Updates flow from upstream into the
fork and never in the other direction.

## Assumptions

- The checked-out branch is `main`, has no staged, modified, or untracked
  files, and matches `origin/main` before the update starts.
- `.grok-openai-upstream` is a non-executable regular tracked file, contains
  exactly one full lowercase commit SHA, names a valid commit, and that commit
  is an ancestor of the fork's current `HEAD`. It records the last upstream
  snapshot integrated by a successful update; do not edit it by hand.
- The current user is authenticated to push `origin/main` normally. The script
  never force-pushes and does not bypass branch protection.
- Rust, `protoc`, Git, and the repository's build dependencies are available.
- Upstream changes may conflict with the fork's provider-isolation patch. A
  conflict needs a deliberate code review; it is never auto-resolved by
  discarding either side.
- The validation suite is the release gate. A successful fetch or merge alone
  is not enough to publish an update.

## One-command update

From the repository root:

```sh
./scripts/update-from-upstream.sh
```

The script performs the following transaction:

1. validates the clean branch, exact remote identities, and trusted upstream
   marker;
2. fetches both `origin` and `upstream`;
3. creates a temporary candidate branch/worktree from the fork's current
   `main`;
4. integrates `upstream/main` in that isolated candidate using a normal merge,
   or the explicitly accepted append-only bridge described below if upstream
   was force-rewritten, rebased, rewound, or rolled back;
5. records the fetched upstream SHA in `.grok-openai-upstream` as part of the
   candidate merge commit;
6. validates the OpenAI profile, focused tests, mock end-to-end flow, and
   release build inside the candidate;
7. re-fetches both remotes and refuses publication if either `origin/main` or
   `upstream/main` moved during validation;
8. only after every gate passes, pushes the exact tested candidate SHA to
   `origin/main` with a normal, non-force push;
9. only after that push succeeds, fast-forwards local `main` to the same tested
   commit and installs the validated build through `install-openai.sh`;
10. only after the installation succeeds, removes the temporary candidate
   worktree and branch.

If fetch fails before a candidate exists, the script exits non-zero without
changing repository refs, the worktree, the installed `grok-openai`, or the
remote fork. On a first run it may still create or repair the read-only
`upstream` remote before that fetch. If merge, test, build, or another
pre-publication gate fails, the real `main`, installed `grok-openai`, and remote
fork remain unchanged; the failed candidate worktree and branch are
intentionally retained, and their locations are reported so the failure can be
inspected without recreating it. There is no force push, destructive reset, xAI
release download, shell startup edit, or push to upstream.

## If upstream rewrote, rebased, or rolled back history

The tracked `.grok-openai-upstream` marker lets the updater handle a fetched
`upstream/main` that no longer descends from the last integrated snapshot while
preserving append-only fork history. This test is deliberately stricter than
checking for any common ancestor: a rebase or rewind may share old history but
still omit the marker. The updater refuses this path unless the marker is a
full lowercase SHA for a valid commit already contained in fork `HEAD`.

On refusal, the updater prints the exact marker/fetched pair it observed. After
inspecting those two commits and deciding that the new upstream tree is the
intended source snapshot, rerun with that exact pair, for example:

```sh
./scripts/update-from-upstream.sh \
  --accept-upstream-rewrite=<previous-full-sha>..<fetched-full-sha>
```

The no-argument command remains the normal path. It will never silently accept
a rewrite or rollback. A stale/mismatched pair is rejected, as is supplying the
option for normal descendant history. The pin therefore accepts only the exact
snapshot that was inspected; it does not weaken any validation, race, push, or
installation gate.

Only inside the temporary candidate, the updater synthesizes a bridge commit:

- its tree is exactly the fetched rewritten upstream tree;
- its first parent is the marker's previously integrated upstream commit;
- its second parent is the fetched rewritten upstream commit.

Merging that bridge into the fork applies the tree delta from the last trusted
upstream snapshot to the rewritten snapshot, while making the new upstream
commit an ancestor of the result. The merge commit then advances the marker to
the fetched SHA. This adds commits; it does not force-push, reset, rebase,
stash, replace refs, or discard fork history.

The synthetic bridge is not created until the isolated candidate exists. Its
tree and both parent object IDs are verified immediately after creation. If
the delta conflicts with fork changes, the candidate and conflict are retained
for inspection while local `main`, `origin/main`, the marker on `main`, and the
installed binary remain unchanged. A malformed, missing, untracked, unknown,
or nonancestor marker stops before any candidate is created.

## Candidate-code trust boundary

The updater clears the ambient environment before every candidate-derived
script or executable, including candidate validation, release `--version`, the
staged installer, and the final validated installer. Prepublication processes
receive a disposable `HOME`, so API keys, bearer tokens, cloud variables, and
implicit user Git/auth configuration are not inherited. It explicitly supplies
only basic process paths plus the existing Cargo and rustup roots needed for the
public Rust toolchain and dependency cache. The final installer receives the
real installation paths but still does not inherit ambient credentials.

This is environment isolation, not an operating-system sandbox. Candidate
source, build scripts, proc macros, and test binaries still run as the local
user and can access files that user can read, including explicitly supplied
Cargo/rustup directories. Treat upstream source as executable code: inspect an
unexpected rewrite before accepting it, and run the updater in a stronger
external sandbox if the source itself is not trusted. Never weaken the updater
by exporting account credentials into the candidate gate.

If installation fails after the tested candidate has been published, local
`main` and `origin/main` already point to that candidate. The updater retains
the candidate worktree and branch and reports the failure. Diagnose the
installation error, then rerun `scripts/install-openai.sh` from the updated
`main`; do not assume the previous installed binary was replaced successfully.

## Before running it

Inspect the current state without changing it:

```sh
git status --short --branch
git remote -v
```

Commit or move intentional work to its own branch. Do not delete or stash files
you do not understand merely to satisfy the clean-tree precondition. The
updater refusing to proceed is the safe behavior.

## If upstream merge conflicts

The updater reports the conflict, keeps `main` untouched, and retains the
candidate worktree and branch at the paths shown in its output. Inspect and
resolve the retained candidate directly. For a normal related-history update,
you can instead reproduce the conflict in a review branch without weakening
the updater:

```sh
git switch -c review/upstream-update origin/main
git merge upstream/main
```

Do not use `--allow-unrelated-histories` when upstream was fully rewritten; it
uses no trusted base and therefore does not model the intended delta. Start
from the retained candidate/bridge or rerun the updater after deliberately
resolving the retained conflict.

Review every conflict, preserve `FORK-NOTICE.md`, `config/openai.toml`,
provider-isolation behavior, secret redaction, and the OpenAI install/update
scripts, then run the same validation gate. Merge the reviewed branch into
`main` and push only after it passes. If the change is not understood, stop and
leave the published fork on the last known-good commit.

## If validation fails

The candidate is not published and is retained for inspection. Read the first
failing command's output and the reported candidate path, then decide whether
upstream intentionally changed an API/config contract or the host lacks a
prerequisite. Fix the cause on the retained candidate or a review branch and
rerun the full gate. Do not skip tests just to move `main`.

A missing real `OPENAI_API_KEY` is not a validation failure: the automated
suite is keyless and uses a local mock. A paid live request remains a separate,
optional account-acceptance check.

## Confirm the result

After the updater succeeds:

```sh
git status --short --branch
git log -1 --oneline --decorate
git rev-parse main
git rev-parse origin/main
~/.local/bin/grok-openai --version
```

The two revisions must match and the tree must be clean. The successful normal
push, local fast-forward, validated installation, and successful candidate
cleanup together form the update stop line; further scanning is not required
unless the user asks for a new audit or a live OpenAI account check.
