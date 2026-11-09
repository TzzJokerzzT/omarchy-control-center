# Maintenance

What lives where, so recovering this project never depends on remembering a chat.

## Three layers

| Layer | Contents | Survives deleting the local clone | Survives deleting the GitHub repository |
| --- | --- | --- | --- |
| **GitHub** | branch protection, issues, pull requests, releases, CI runs | yes | **no** |
| **The repository** | code, `.github/CODEOWNERS`, `.github/workflows/ci.yml`, `install.sh`, `CONTRIBUTING.md`, docs, history | no, but a fresh clone brings it back | no |
| **Your machine** | the installed binary and `control-center` link in `$XDG_BIN_HOME`, one marked block in your shell rc, and `~/.local/state/omarchy-control-center/` (log, operation history, quarantine) | **yes** | **yes** |

The launcher is a compiled copy, not a link into the clone: deleting the project folder
does not break `control-center`, and the operation history and quarantine area live under
`$XDG_STATE_HOME`, outside the repository.

## Restoring a clone

```sh
git clone https://github.com/TzzJokerzzT/omarchy-control-center.git
cd omarchy-control-center
./install.sh
```

Two things do not come back on their own, because they are local to a clone:

* your commit identity — `git config user.name "…"` and `git config user.email "…"`, or
  set them globally once;
* any branch that was never pushed.

## Restoring the repository

Branch protection is **server-side configuration**, not a file, so a deleted repository
comes back unprotected. Everything else returns with the code, including `CODEOWNERS` and
the CI workflow.

```sh
scripts/protect-main.sh               # the base rules; keeps any required CI checks as they are
scripts/protect-main.sh --require-ci  # the base rules plus the CI check, but only when it
                                      # has already reported a green run
```

`protect-main.sh` refuses to run when `.github/CODEOWNERS` is missing or has errors: a
code-owner requirement without a resolvable owner blocks every pull request permanently,
which is worse than no protection at all. It also refuses `--require-ci` until the `ci`
check has succeeded once, because a required check that never reports blocks every pull
request forever — and it preserves existing required checks when run without the flag, so
re-running it can never silently drop a CI requirement.

## What the rules are, and why

| Rule | Why |
| --- | --- |
| Pull request required | nobody pushes to `main` directly, so every change is reviewable |
| One approval, from the code owner | only the maintainer approves; contributors fork and open a PR |
| Dismiss stale approvals | a changed pull request is reviewed again |
| Resolve conversations first | a review comment cannot be ignored by merging |
| No force push, no deleting `main` | the history of a distributed program stays intact |
| The `ci` check is required, with a strict branch | a pull request cannot be merged with a failing build or on top of an outdated `main` |
| Owner **not** restricted | with `enforce_admins`, the owner could neither push nor merge their own PRs, because GitHub forbids approving your own |

Inviting a collaborator with the **Write** role lets them merge an already-approved pull
request. Keep contributions fork-based, or invite with **Read**.

## Routine checks

```sh
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

The CI job runs exactly these, plus the installer's syntax and a dry run.
