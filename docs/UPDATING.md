# Updating the fork from upstream

The repository has two remotes:

- `origin`: `git@github.com:ocque41/grok-build.git`, the user-owned fork;
- `upstream`: `git@github.com:xai-org/grok-build.git`, the read-only source.

The upstream remote has no usable push URL. Updates flow from upstream into the
fork and never in the other direction.

## Assumptions

- The checked-out branch is `main`, has no staged, modified, or untracked
  files, and matches `origin/main` before the update starts.
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

1. validates the clean branch and exact remote identities;
2. fetches both `origin` and `upstream`;
3. creates a temporary candidate branch/worktree from the fork's current
   `main`;
4. merges `upstream/main` into that isolated candidate without rewriting
   history;
5. validates the OpenAI profile, focused tests, mock end-to-end flow, and
   release build inside the candidate;
6. only after every gate passes, pushes the exact tested candidate SHA to
   `origin/main` with a normal, non-force push;
7. only after that push succeeds, fast-forwards local `main` to the same tested
   commit and installs the validated build through `install-openai.sh`;
8. only after the installation succeeds, removes the temporary candidate
   worktree and branch.

If fetch fails before a candidate exists, the script exits non-zero and leaves
all state unchanged. If merge, test, build, or another pre-publication gate
fails, the real `main`, installed `grok-openai`, and remote fork remain
unchanged; the failed candidate worktree and branch are intentionally retained,
and their locations are reported so the failure can be inspected without
recreating it. There is no force push, destructive reset, xAI release download,
shell startup edit, or push to upstream.

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
resolve the retained candidate directly, or reproduce the conflict in a normal
review branch rather than weakening the updater:

```sh
git switch -c review/upstream-update origin/main
git merge upstream/main
```

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
