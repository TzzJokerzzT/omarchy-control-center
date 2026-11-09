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
| Pull requests open to collaborators only | a stranger cannot open a pull request, so a contribution arrives as an issue with a diff |
| Pull request required | nobody pushes to `main` directly, so every change is reviewable |
| One approval, from the code owner | only the maintainer approves; a collaborator cannot approve their own pull request |
| Dismiss stale approvals | a changed pull request is reviewed again |
| Resolve conversations first | a review comment cannot be ignored by merging |
| No force push, no deleting `main` | the history of a distributed program stays intact |
| The `ci` check is required, with a strict branch | a pull request cannot be merged with a failing build or on top of an outdated `main` |
| Owner **not** restricted | with `enforce_admins`, the owner could neither push nor merge their own PRs, because GitHub forbids approving your own |

Pull requests can only be **opened by collaborators** (`pull_request_creation_policy:
collaborators_only`), so an outside contributor cannot open one even with a fork: the flow is
an issue with the patch attached, which `CONTRIBUTING.md` describes. Inviting someone with the
**Write** role also lets them merge an already-approved pull request, so invite with **Read** if
you only want them to review.

That policy is **not** applied by `scripts/protect-main.sh`, which only writes
`/branches/{branch}/protection`. A repository recreated from this source would come back with
pull requests open to everyone; set it after running the script:

```sh
gh api --method PATCH "repos/$REPO" -f pull_request_creation_policy=collaborators_only
```

## Routine checks

```sh
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

The CI job runs exactly these, plus the installer's syntax and a dry run.
