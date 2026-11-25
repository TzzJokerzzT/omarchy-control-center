# Privileged removal: ask for the root password dynamically

The interface refuses to run as root, and its preview marks a command `[root]`, but nothing
ever elevates: `requires_root` is only displayed and asserted in tests. Every privileged
operation (`pacman -R`, AUR removal, system flatpak) runs unprivileged and fails.

Branch: `fix/privileged-removal`. Risk: item 2 (privilege escalation), item 3 (`process.rs`
and `CommandSpec` are shared by every adapter).

## Specs

- **S1**: «Hay algo que se debe arreglar y es que la app no solicita usuario root ni nada. Por lo que no puede eliminar paquetes.»
- **S2**: «Quiero que dinamicamente detecte cuando la app solicite constraseña root para eliminar paquetes, app entre otras cosas»

Derived requirement (from S1–S2, not a new one): a command whose plan carries
`requires_root: true` is invoked as `sudo -- <absolute program> <args verbatim>`; the
interface keeps refusing to run as root; the terminal is handed over whenever such a
command runs, so the password prompt is visible and the interface never reads a password.

## Tasks

- T1 — S1,S2 — `src/infrastructure/process.rs`: `Privilege`, `CommandSpec::planned_with`,
  sudo wrapping in `build`, no deadline for an elevated command — route: inline — commit C1
- T2 — S1,S2 — pacman / aur / flatpak / cargo_tools executors use `CommandSpec::planned`
  instead of duplicating the `requires_tty` branch — route: inline — commit C1
- T3 — S2 — `OperationPlan::requires_handover` + `app::confirm_operation` hands the
  terminal over for a root command that is not itself interactive — route: inline — commit C1
- T4 — S1,S2 — preview and history show the exact `sudo -- …` argv that runs
  (`PlannedCommand::display_effective`) — route: inline — commit C1
- T5 — S1,S2 — full `cargo test`, docs consistency (`docs/security.md` already promises the
  handoff), independent verification of Risk item 2/3 — route: inline + verify — commit C1
- T6 — S1,S2 — `PlannedCommand::self_elevating`: a helper that prompts for itself (`yay`,
  `paru`) is never wrapped in `sudo`; the interface elevates only a command that cannot
  elevate itself — route: inline — commit C1
- T7 — S1,S2 — correction of the native review findings (R1-001, R3-existing-failure-message,
  R3-plan-export-field, R4-001): a privileged program and `sudo` resolve only from trusted
  system directories, never from the session search path; the baseline failure message is
  restored; the elevation flag is not serialized — route: worker — commit C1

## Log

L1: «Hay algo que se debe arreglar y es que la app no solicita usuario root ni nada. Por lo
que no puede eliminar paquetes. Quiero que dinamicamente detecte cuando la app solicite
constraseña root para eliminar paquetes, app entre otras cosas»

E1: exploration — `requires_root` has no consumer outside `ui/mod.rs` (`[root]` tag) and
tests; `grep '"sudo"'` in `src/` returns only the doc comment in `system.rs`; `term.rs`
already implements `suspend`/`resume` and `app::confirm_operation` already uses it for
`plan.requires_tty()`.

E2: while reviewing the diff before committing — `requires_root` means "needs root" and the
plans rely on two different mechanisms: the AUR adapter delegates to `yay`/`paru`, which
prompt for `pacman` themselves (its `rationale` says the removal is delegated to the
helper), while `pacman` and a system `flatpak` install cannot. Wrapping the helper in `sudo`
would have turned a working removal into a root-run build helper, so the elevation decision
needs its own field instead of being inferred from `requires_root`.

E3: differential check, base commit vs the branch, on the commands the request did not name:
`--help`, a rejected flag (exit 2), `--print-inventory` (all, pacman, flatpak),
`--print-plan` remove/update/reclaim/unknown/read-only — identical stdout, stderr and exit
codes. The only intended export change is one additive `"self_elevating"` field in a plan.

L3: `SystemRunner` now carries two resolution lists. A test runner built with the test-only
`with_path` trusts no privileged directory at all, so a privileged test must inject one with
`with_trusted_paths`; that is what keeps the host's `/usr/bin/sudo` unreachable from the
suite. `ProcessError::NotFound` keeps its `executable search path` wording for the missing
`sudo` case, so a doc must not quote it as if the trusted directories were a `PATH`; the new
`Untrusted` variant is the one that names a program a privileged command may not run.
