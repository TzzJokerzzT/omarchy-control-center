# Contributing

Thanks for taking the time. This project has a strict safety model, so a few things are
asked of every change.

## Reporting a problem

Open an issue. A useful report says what you ran, what you expected, what happened, and
the output of `omarchy-control-center --print-inventory > inventory.json` when the problem
is about what the inventory shows (that file lists package names, so skim it first).
Never paste a password, a token, or the contents of a configuration file.

## Sending a change

Pull requests here can only be **opened by collaborators**: the repository uses GitHub's
"Restrict pull requests to collaborators" policy, so a fork does not by itself get you a pull
request. The path is an issue first:

1. **Open an issue** describing the change, or the bug it fixes. One behaviour per issue.
2. **Attach the patch if you already have it**: a `.patch` or `.diff` file, a branch in your
   fork, or the diff in the issue body. Small enough to read is small enough to review.
3. The maintainer either applies it, or invites you as a collaborator so you can open the
   pull request yourself and keep working on it.

If you are a collaborator, open the pull request against `main` and expect it to be
**squashed** on merge: the pull request title becomes the commit subject, so write it like a
commit (`feat(adapters): …`).

Either way, `main` is protected. It needs one approval, a CODEOWNERS review from the
maintainer, and a green `ci` run before it can merge; force pushes and branch deletion are
blocked, so nothing lands by going around it.

Run the checks locally first — they are the same ones `ci` runs:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

Two pipeline rules that fail a workflow rather than a test:

* **Every `uses:` must be a GitHub-owned action pinned to a full 40-character commit SHA.**
  Third-party actions and floating tags are rejected by the repository's policy, so a change
  that adds one has to pin it (or propose a different approach) before `ci` will run.
* **The lockfile is checked with `--locked`.** A change that needs a different dependency has
  to commit `Cargo.lock` with it, or CI fails on the lockfile step.

## What a change must respect

These are not style preferences; they are the reasons the program is safe to run.

* **A manager owns its data.** Integrations act through the manager that owns an
  ecosystem. Nothing edits a manager's files behind its back.
* **Inspection before modification.** A new integration starts read-only and earns
  actions one at a time, each with a preview, an explicit confirmation and tests.
* **No deletion by name.** Leftovers move into quarantine and can be restored. Nothing is
  deleted because a name matched, and a permanent delete only accepts entries a ledger
  already recorded.
* **No invented facts.** Absent values stay absent (`No disponible`), and every component
  names the exact command that detected it. A source that cannot provide a value never
  gets one filled in.
* **Every command is argv.** No shell string is ever built from data, and identifiers are
  validated against an allowlist before they become arguments.
* **Tests do not touch the system.** Adapters are tested with fixed command output; a test
  must never install, remove or upgrade anything.

Discussions about a design that changes these rules are welcome — as an issue first,
before the code.

## Code of conduct

Be direct about the code and generous with the person. Review comments here are about
what a change does to a user's machine.
