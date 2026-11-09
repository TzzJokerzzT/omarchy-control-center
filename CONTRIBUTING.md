# Contributing

Thanks for taking the time. This project has a strict safety model, so a few things are
asked of every change.

## Reporting a problem

Open an issue. A useful report says what you ran, what you expected, what happened, and
the output of `omarchy-control-center --print-inventory > inventory.json` when the problem
is about what the inventory shows (that file lists package names, so skim it first).
Never paste a password, a token, or the contents of a configuration file.

## Sending a change

`main` is protected: nobody pushes to it directly, including the maintainer's own work
when it needs review. The flow is:

1. Fork the repository and work on a branch in your fork.
2. Keep the change focused. One behaviour per pull request.
3. Run the checks locally — they are the same ones the maintainer runs:
   ```sh
   cargo fmt --check
   cargo clippy --all-targets -- -D warnings
   cargo test
   ```
4. Open the pull request against `main`. A CODEOWNERS rule means it needs the
   maintainer's review; only the maintainer can approve and merge.

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
