# Security

The program can uninstall software and, through its package manager, modify the system.
A defect here can leave a machine unusable. Security is therefore part of the design, not
a later task, and every claim below maps to code and to an automated test.

## Threat model

| Threat | Control |
| --- | --- |
| A crafted package name reaches a shell | No shell exists: program plus separate arguments only, and identifiers must satisfy an allowlist |
| A displayed name is trusted as an argument | Names shown in the interface are never used as arguments; the native identifier from the manager is, and it is re-validated at execution time |
| The interface runs privileged and a bug deletes system files | The interface refuses to start as root; privileged commands get the terminal, one at a time |
| A stale preview removes something else | The plan is recomputed right before execution and compared by fingerprint |
| An operation reports success when it failed | Success is `exit code == Some(0)`, nothing else; stderr and the code are kept verbatim |
| A heuristic deletes files | Nothing is deleted by path. No `rm`, no recursive delete, no symlink following, no orphan batch removal |
| Secrets leak into logs or history | Neither logs nor history contain environment variables, passwords, tokens or configuration content |
| A tool hangs and blocks the interface | Every captured command has a deadline and is killed at it |

## Privilege

* The program **refuses to run as root** (`guard_not_root`). A privileged interface means
  every renderer bug and every stray keystroke has root's reach.
* A privileged operation (a system removal) suspends the interface, releases the terminal,
  and hands the TTY to `sudo` for **exactly the confirmed argv**. The command owns the
  terminal while it runs; the interface repaints fully afterwards.
* The program never prompts for, reads, stores or forwards a password. It does not set
  `SUDO_ASKPASS`, does not read environment variables from the command, and does not
  reuse credentials between operations.
* If the terminal cannot be taken over at all, the program exits with a clear message
  rather than drawing into an unknown terminal state.

## Process execution

All process execution lives in `src/infrastructure/process.rs`. It enforces:

* **No shell, ever.** `Command::new(program).args(args)` — there is no `sh -c`, no
  string joining, no `shell_words`. A test passes `"$(touch /tmp/occ-should-not-exist)"`,
  `"a;b"`, `` "`id`" `` and `"a|b"` as arguments and asserts each arrives verbatim and
  that no file was created.
* **Program names are bare names.** A name containing `/` or a NUL byte is rejected
  (`InvalidProgram`), so a name cannot choose its own executable. Resolution walks
  `PATH` and **ignores empty and relative entries**, which a directory-local binary would
  otherwise exploit.
* **Every command has a deadline.** A captured command is killed when its deadline
  expires and reported as `Timeout`; the caller never waits forever. Interactive commands
  (the `sudo` handoff) have no deadline because a human is typing, and they own the
  terminal for the duration.
* **Exit codes are data.** `CommandOutput::success()` is exactly `exit_code == Some(0)`.
  A non-zero exit can never become success anywhere in the program.
* `LC_ALL=C` is set for captured commands so field labels and dates we parse are stable
  across locales; nothing else is added to or removed from the environment.

### Identifier allowlists

* pacman names: `[A-Za-z0-9@._+-]`, no leading `-`, no whitespace, no control characters,
  255 bytes max.
* flatpak refs: the same set plus `/`, and additionally **no empty, `.` or `..` path
  segment**, so a reference cannot name a path outside the flatpak store.

Both reject `""`, `"  "`, `"a;b"`, `"$(...)"`, `` "`id`" ``, `"-Rns"`, `"a>b"`, quotes,
backslashes and newlines. Tests assert the rejection and, where a process could otherwise
have started, assert that **nothing ran**.

## Destructive operations

### Mandatory preview

Before anything executes, the plan shows:

1. the exact argv, and whether root and the terminal are needed;
2. what else could be affected — dependents, files owned by the package;
3. what was **blocked** and why, when the operation must not run;
4. what could **not be determined** — never a guess in place of a fact;
5. whether the operation is reversible, and if not, that it is not.

### Explicit confirmation

The human types the component's native identifier. `Enter` in the list only opens
details. `Esc` cancels and executes nothing. `q` inside the confirmation field types a
`q` instead of quitting. The decision is a value (`ConfirmDecision`), so
"not confirmed" cannot flow into "execute".

### No cascades, no heuristics

* Removal is `pacman -R <pkg>`. If dependents exist the operation is **blocked** and they
  are listed; `-Rns`, `-Rdd`, `-Rsc` are never proposed (a test asserts none of them can
  appear in a planned command).
* Orphaned dependencies are a **label**, never a queue to delete.
* No file is deleted by path. A plugin or configuration that is still referenced is never
  removed on the strength of its name.
* flatpak removals use `--no-related` so a shared runtime is not pulled out from under
  another application, and a runtime removal states that dependent applications are not
  reported by the data we read.

### Drift and identity

The plan fingerprint covers the command argv, the affected components and the blocking
reasons. Immediately before execution the plan is recomputed against a fresh read of the
system:

* a component that disappeared → `Vanished`, nothing runs;
* a new dependent → `Drift`, nothing runs;
* a newly blocked operation → `Blocked`, nothing runs;
* a changed command → `Drift`, nothing runs.

The executor also refuses a plan that belongs to another adapter, or whose program is not
the tool that adapter owns — even if such a plan were constructed by a bug elsewhere.

### Failure handling

The program does not "fix" a failed operation with another command. It stops at the first
non-zero exit, presents the real exit code and the error output, and appends the outcome
to the history. A failed history write is reported next to the outcome, because an audit
gap must be visible.

## Data protection

* History and log are created with mode `0600`; an existing file with looser permissions
  is tightened instead of trusted.
* The history record contains: timestamp, adapter, component, operation, result, exit
  code, command, error summary, and the confirmation that was typed. Nothing else.
* Configuration files are never modified by the program. There is no path in the code that
  writes to `~/.config`, `~/.zshrc` or `~/.config/hypr`.
* Nothing from command output is ever interpreted as an instruction or executed. Output is
  parsed into typed models; unknown or malformed rows are skipped and logged, never
  guessed.

## Automated security tests

| Requirement | Where |
| --- | --- |
| Arguments with spaces, quotes, dashes and special characters cannot inject a command | `infrastructure::process::tests::hostile_arguments_arrive_verbatim_and_are_never_interpreted_by_a_shell` |
| Malicious or unexpected paths are rejected | `validate::tests::rejects_injection_shaped_names`, `rejects_flatpak_traversal_and_options`, `process::tests::a_program_path_is_rejected_instead_of_executed`, `relative_or_empty_path_entries_are_ignored` |
| Nothing destructive happens at startup or on selection | `pacman::tests`, `flatpak::tests` (inspection-only paths), `operations::tests::read_only_mode_never_builds_a_plan`, plus the fact that only `execute` runs a mutating command |
| A cancelled confirmation executes nothing | `operations::tests::an_unconfirmed_phrase_never_decides_to_execute`, `a_blocked_plan_decides_blocked_even_with_the_right_phrase` |
| A plan change after the preview cancels execution | `operations::tests::a_plan_that_changed_after_the_preview_is_refused`, `a_component_that_vanished_is_refused` |
| A missing adapter does not break the program | `registry::tests::a_missing_tool_does_not_stop_the_other_adapters`, `an_erroring_adapter_does_not_prevent_the_others_from_loading`, `a_panicking_adapter_is_contained`, `adapters::tests::a_machine_without_any_tool_degrades_instead_of_failing` |
| A non-zero exit is never reported as success | `process::tests::a_non_zero_exit_code_is_never_success`, `pacman::removal_tests::a_failing_removal_is_never_reported_as_success`, `flatpak::tests::a_failing_uninstall_is_never_reported_as_success` |
| No unauthorised recursive deletion | `pacman::removal_tests::the_plan_is_exactly_one_non_cascading_removal`, `a_plan_with_dependents_is_blocked_and_names_them` |
| The interface never runs as root | `system::tests::refuses_to_run_privileged` |
| Another adapter's plan, or another program, cannot be executed | `pacman::removal_tests::a_tampered_plan_cannot_make_this_adapter_run_another_program`, `a_plan_from_another_adapter_is_refused`, `aur::tests::execution_refuses_a_program_that_is_not_a_known_helper` |

Acceptance rule applied to this version: no destructive operation counts as implemented
until it has a preview, validation, explicit confirmation, error handling and automated
tests. That is why pacman offers removal and no update, and why nothing outside pacman,
the AUR helper and flatpak can be modified at all.

## Residual risks

Stated plainly, because hiding them would be the real vulnerability.

* A command that forks a **grandchild** can outlive its deadline: the direct child is
  killed, the readers are detached rather than joined, and the interface stops waiting.
  The command is never retried automatically.
* During a privileged handoff the interface is not reading keys. `Ctrl-C` reaches the
  package manager, which may leave work pending — the package manager's own recovery rules
  apply.
* `sudo` policy is the system's, not ours. The program does not attempt to bypass it, and
  an account without the right to install packages simply cannot complete a system
  removal.
* A malicious executable earlier in `PATH` is a general system concern, not something this
  program can fully solve; it mitigates the specific case of empty and relative `PATH`
  entries.
