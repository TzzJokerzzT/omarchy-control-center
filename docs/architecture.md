# Architecture

## Shape

Ports and adapters (hexagonal), with one rule that the module tree enforces: the
dependency arrow only points inward.

```text
ui/            presentation          draws only, never runs a command
  ↓
app/           interface process     state, keys, draw loop, background threads
  ↓
application/   use cases             registry, operations, search, history record
  ↓
domain/        models and rules      no process, no terminal, no filesystem
  ↑
adapters/      integrations          pacman, aur, flatpak — one ecosystem each
  ↓
infrastructure/ outside world        process runner, XDG paths, logging, history
```

`domain` depends on nothing. `adapters` depend on `domain` and `infrastructure`, never
on `ui` or `app`. Adding an integration touches `adapters/` and one line in
`adapters::build_registry`; the interface and the application layer do not change.

## Domain

* `ComponentId { adapter, native_id }` — the composite identity. `firefox` in pacman
  and `firefox` in flatpak are different components, and no name is ever used as a
  global key.
* `Component` — name, version, description, category, source, status, confidence,
  capabilities, the detection method, size, location, install date, dependencies,
  dependents and extra source-specific fields.
* Optional facts are `Option<T>` and render through `UNAVAILABLE` (`"No disponible"`).
  A source that does not provide a value never gets one invented for it.
* `DetectionMethod` records the exact program and arguments that produced the fact, so
  a claim can be audited instead of trusted.
* `OperationPlan` — the only value an executor accepts: exact commands (program plus
  separate arguments), affected components, blocking reasons, unknown facts, notes, and
  a fingerprint of everything decision-relevant.

## Adapter contract

```rust
fn id(&self) -> &'static str;
fn capabilities(&self) -> Vec<ComponentCapability>;
fn status(&self) -> AdapterStatus;                       // availability, tool, commands
fn list(&self) -> Result<Vec<Component>, AdapterError>;
fn details(&self, native_id: &str) -> Result<Option<Component>, AdapterError>;
fn enrich(&self, component: &Component) -> Option<Vec<DetailField>>;   // default: None
fn supports(&self, component: &Component, kind: OperationKind) -> bool;
fn plan_operation(&self, request, component) -> Result<OperationPlan, AdapterError>;
fn execute(&self, plan: &OperationPlan) -> Result<OperationOutcome, AdapterError>;
```

Adapters are **synchronous**. Each one is a `Command`-in / typed-model-out translation
that must be testable without a runtime, and the application layer decides when to run
it in the background. There is no async runtime in the dependency set.

`details()` returning `Ok(None)` means "this component no longer exists", which cancels
any pending operation instead of proceeding.

## Registry

The registry is the only place that knows which integrations exist, and it enforces:

* **Isolation** — an unavailable adapter contributes nothing; an adapter that returns an
  error or panics is recorded as a failure attributed to that adapter and stage, and the
  remaining adapters still load.
* **No duplicate identity** — two adapters reporting the same `ComponentId` is recorded
  as a conflict; the first claim is kept and the conflict is displayed.
* **One operation provider** — for a given `(component, kind)` pair exactly one adapter
  may claim support. Zero claims means the action is not offered; two claims is reported
  as ambiguous rather than resolved by guessing.
* **Enrichment is additive and safe** — secondary sources (AUR metadata for a foreign
  pacman package) can contribute fields, and a failing or panicking enricher contributes
  nothing.

This is how one package is never shown twice: pacman owns every pacman-managed
component, and the AUR adapter contributes metadata and operation routing instead of a
second list.

## Operation lifecycle

```text
prepare   → read-only refused · provider resolved · plan built (re-reads the system)
preview   → exact argv, root/tty needs, affected, blocking, unknown, notes
confirm   → the human types the native identifier
execute   → plan recomputed · fingerprint compared · adapter executes that argv exactly
record    → real exit code, stderr tail and outcome appended to the history
```

* `prepare` refuses to build a plan at all in read-only mode, so no destructive path
  exists in such a session.
* The drift check compares a fingerprint of the commands, affected components and
  blocking reasons. A newly reported dependent, a changed command or a vanished
  component all change it, and the operation is cancelled with "nothing was executed".
* `ConfirmDecision` is a value (`Execute` / `Blocked` / `PhraseMismatch`), so "not
  confirmed" cannot be silently treated as "execute anyway".

## Concurrency

The draw loop never awaits a process:

* inventory loads and metadata lookups run on `std::thread` and arrive as messages over
  `std::sync::mpsc`; the loop polls with a 120 ms timeout so keys stay responsive;
* every non-interactive command has a deadline and is killed when it expires;
* the one deliberate exception is a command that needs the controlling terminal. The
  terminal is released, the command owns it, and the interface repaints fully afterwards
  (`needs_full_redraw`), because the alternate-screen contents are gone.

## Presentation

Three panes: category sidebar, component list, details. Below 74 columns the sidebar is
dropped and below 22 rows the details pane is stacked away, so a small terminal loses
detail instead of becoming unreadable. Every state is also conveyed by text, not only by
colour.

## Decisions

| Id | Decision |
| --- | --- |
| D1 | pacman owns every pacman-managed component; AUR enriches and routes, never lists |
| D2 | adapters are synchronous; the application layer chooses the threading |
| D3 | no `rusqlite`, `directories`, `which`, `fuzzy-matcher` or `shell-words` |
| D4 | the interface never runs privileged; privileged commands get the TTY |
| D5 | removal is `pacman -R <pkg>` only; dependents block instead of cascading |
| D6 | flatpak is optional: compiled in, auto-detected, never required |
| D7 | configuration and history are plain files under XDG, mode 0600 |
| D8 | confirming means typing the native identifier |
| D9 | no async runtime: deadlines and kill are owned by the command runner |
| D10 | a package's repository is read lazily from one `pacman -Si`, not guessed |
