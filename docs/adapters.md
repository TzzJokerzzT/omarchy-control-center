# Adapters

Every adapter is independent, detects its own tool, and states its availability. A
missing tool is an **unavailable integration** with a reason, never an error, and never
a reason for the rest of the program to fail.

Every command runs without a shell: program and arguments are separate `execve` entries,
the executable is resolved through a validated `PATH`, and every identifier is checked
against an allowlist before it becomes an argument.

---

## pacman — system packages

**Detects:** everything pacman administers, official and foreign.

| Command | Why |
| --- | --- |
| `pacman --version` | tool identity in the adapters view |
| `pacman -Q` | one line per installed package: the authoritative list |
| `pacman -Qi` | every field for every package in **one** invocation (description, size, dependencies, dependents, install date, install reason). One call per package would mean 1200+ processes |
| `pacman -Qm` | packages not owned by a synced repository ("foreign") |
| `pacman -Qdtq` | orphaned dependencies, used only as a status label |
| `pacman -Si <package>` | the repository of an official package, read lazily on the details path because `-Qi` does not report it |
| `pacman -Ql <package>` | how many files a removal would delete |

**Capabilities:** inspect; remove for packages from a synced repository.

**Removal plan:** `pacman -R <name>`. Never `-Rns`, `-Rdd`, `-Rsc` or any cascade. If
anything requires the package the plan is **blocked** and the dependents are listed.

**Deliberate limits:**

* No update. Arch does not support partial upgrades, so a per-package update would be
  misleading; a full `-Syu` is a different operation with different consent.
* The repository stays `No disponible` until `pacman -Si` is asked, rather than being
  guessed from the package name.
* No orphan batch deletion. Orphans are labelled, never removed automatically.
* Files are counted, not listed exhaustively; the exact list is one `pacman -Ql` away.

---

## AUR — helper detection, metadata, removal routing

**Detects:** which AUR helper is present, preferring `yay` then `paru`.

| Command | Why |
| --- | --- |
| `yay --version` / `paru --version` | helper identity and version |
| `yay -Si <package>` | the AUR record (version, maintainer, votes, popularity, last modified) for a foreign package, read lazily for the details view |
| `yay -Qi <package>` / `yay -Ql <package>` | the installed facts of one foreign package, asked through the helper so the answer comes from the same owner that will act on it |
| `yay -R <package>` | removal of a foreign package, delegated to the helper |

**Capabilities:** inspect; remove for foreign packages, and only when a helper exists.

**It deliberately lists no components.** AUR packages are foreign *pacman-owned*
components: pacman lists them, pacman owns their identity, and this adapter only
enriches them and routes their operations. Listing them again would show the same
package twice under one name — the exact failure the composite identity exists to
prevent.

**Why removal is delegated** and not run as `pacman -R`: the helper built the package,
so the helper's view of its record is the one that must be consulted. Mixing the two
would mean one database owner per decision and another per operation.

**Enrichment is not authoritative.** Votes, popularity and maintainer come from remote
AUR metadata, so they never claim to be detection of what is installed. A broken helper
contributes nothing to the details view instead of breaking it.

---

## flatpak — optional by design

**Detects:** applications and runtimes, per installation scope.

| Command | Why |
| --- | --- |
| `flatpak --version` | tool identity |
| `flatpak list --app --columns=ref,application,version,origin,installation,size` | installed applications |
| `flatpak list --runtime --columns=…` | installed runtimes |
| `flatpak info <ref>` | details of one reference |
| `flatpak uninstall --user\|--system --noninteractive --no-related <ref>` | removal |
| `flatpak update --user\|--system --noninteractive <ref>` | update of one reference |

**Optional, not required.** Flatpak is compiled in and auto-detected. The program is
fully usable without it: on a machine without flatpak the adapter reports
`unavailable — flatpak was not found in the executable search path` and every other
adapter keeps working. It is not a build dependency, an install dependency or a
prerequisite.

**Identity:** `flatpak:<ref>`, e.g. `flatpak:app/org.mozilla.firefox/x86_64/stable`.

**Scope is read, never guessed.** `--user` or `--system` comes from the component's own
`installation` column. A row whose scope cannot be named is skipped, not assumed.

**Removal is scope-explicit and non-recursive:** `--no-related` so shared runtimes are
never dragged along, and `--noninteractive` because the confirmation already happened in
the interface. A user installation needs no privileges; a system installation does.

**What the plan states openly:** per-application data under `~/.var/app` is not removed;
related runtimes are kept; and for a runtime, *which applications depend on it* is not
reported by the columns we read, so the plan says so instead of implying safety.

**Verified how:** the unavailable path is verified live on a host without flatpak. The
present path is verified through fixtures (`tests/fixtures/flatpak/`) and must not be
described as verified against a real flatpak installation.

---


---

## Omarchy shell plugins — list, detail and removal

**Detects:** every shell plugin and bar widget Omarchy knows about, first-party and
third-party.

| Command | Why |
| --- | --- |
| `omarchy plugin list --json` | Omarchy's own catalogue: id, name, kinds, enabled, active, first-party flag, clone origin |
| `omarchy plugin remove <id> --yes` | removal, delegated to the manager that owns the plugin directory |

The inventory comes from the catalogue rather than from a directory scan, so it cannot
disagree with `omarchy plugin list`. `--yes` only suppresses the manager's own prompt:
the confirmation that matters happens in the interface, with the exact command on screen.

**What the plan states, and why it matters:** `omarchy plugin remove` runs `rm -rf` for a
git clone and moves a plain directory to a hidden `.${id}.bak.<timestamp>` directory. The
plan says which of the two will happen, because one is irreversible and the other is not.

**First-party plugins are refused.** They ship inside the `omarchy` package; removing one
would be undone by the next package update, and the package manager owns that file.

**Hidden leftovers are listed as orphans.** The catalogue excludes `.*` directories, so
the `.bak` directories produced by earlier removals are invisible to `omarchy plugin
list` while still occupying disk. They are shown with low confidence and are **not**
removable: no manager owns them, and deleting by name is what this program refuses to do.

---

## Lazy.nvim — Neovim plugins

**Reads only, runs no command:** `~/.config/nvim/lazy-lock.json` (name, branch, commit) and
`~/.local/share/nvim/lazy/<plugin>`.

A lock entry with a directory is installed; a lock entry without one is **`missing`** —
configured, never downloaded. A directory with no lock entry is not listed at all: it is
not part of the configuration, and inventing a component for it would be a guess.

Removal is not offered: there is no manager to delegate to, and the configuration still
references the plugin, so deleting the directory would leave a broken lock file (S17).

---

## Zsh — plugins

**Reads only:** the `plugins=(...)` assignment from `~/.zshrc` (or `$ZDOTDIR/.zshrc`), the
oh-my-zsh framework root, and `custom/plugins`.

Each configured plugin is classified by where it actually lives: `oh-my-zsh` when it ships
with the framework, `custom clone` when it is a clone in `custom/plugins`, `missing` when
it is configured but present in neither. `.zshrc` is never written — no diff preview exists
yet, so no modification exists either.

---

## Hyprland — configuration inspection

**Reads only:** `~/.config/hypr/*.lua` and `*.conf`.

Three kinds of component, all inspection-only:

* **config modules** — the files that make up the active configuration;
* **launch targets** — `o.launch_on_start("…")`, `o.exec_on_start("…")` and legacy
  `exec-once = …`, each with the file and line it comes from and whether the program
  resolves;
* **backup files** — leftover `*.bak.*`, `*.lacquer-backup-*` and
  `*.omarchy-upgrade-to-*` copies, reported as orphans.

Comment stripping is quote-aware: the real configuration starts
`"/usr/bin/fix-ultrawide-mode --now"`, and a naive split on `--` would truncate the
argument and lose the component. A configuration line is a **reference**, never a package:
nothing here claims ownership and nothing is removable.

---

## systemd — services

| Command | Why |
| --- | --- |
| `systemctl [--user] list-units --type=service --all --no-pager --no-legend` | what is loaded, and whether it is active |
| `systemctl [--user] list-unit-files --type=service --no-pager --no-legend` | whether it is enabled |
| `systemctl [--user] show -p FragmentPath,ActiveState,UnitFileState,Description <unit>` | details of one unit, on demand |

Two bulk calls per scope, never one call per unit. Identity is `<scope>:<unit>` because the
same unit name can exist in both scopes. A scope whose command fails is logged and the
other scope still loads. A unit known only from its unit file reports its enabled or
disabled state rather than "unknown".

**Inspection only.** Stopping, disabling and removing are three different actions with
three different consequences, and this version implements none of them; the adapter says
so in its own status.

---

## Cargo — installed tools

| Command | Why |
| --- | --- |
| `cargo install --list` | every crate installed in this Cargo home, with its binaries |
| `cargo uninstall <crate>` | removal, delegated to Cargo |

Cargo tools are a separate ecosystem from the distribution's Rust packages: this adapter
never claims a package pacman owns. Removal goes through `cargo uninstall` so Cargo's own
bookkeeping stays consistent — deleting `~/.cargo/bin/<tool>` by hand would not.

The plan states that a same-named binary from another installation is not reported by
Cargo, and that a `CARGO_HOME` pointing inside a system directory would need privileges
(this adapter does not detect that yet).

---

## Python — tools installed by uv or pipx

| Command | Why |
| --- | --- |
| `uv tool list` | tools installed with `uv tool install` |
| `pipx list --json` | the same for pipx, in its documented machine-readable shape |
| `<manager> --version` | tool identity, only when the manager is a real executable |

`uv` is preferred; `pipx` is the fallback. Two deliberate refusals:

* **`pip` is never consulted.** It can modify the Python the distribution owns, and the
  specification forbids touching that without verifying the environment.
* **A lazy-install shim is refused, not executed.** On the reference machine `uv` resolves
  to `~/.local/share/mise/shims/uv`, and invoking it makes mise *install* uv. An inventory
  must never install software as a side effect, so the adapter reports the integration as
  unavailable and says exactly why.

No removal is offered yet: proving which environment owns an executable is not
implemented, and guessing it would be the kind of inference this program refuses.

---

## Fonts, themes and icon themes — informative

| Source | What it gives |
| --- | --- |
| `fc-list` | every font fontconfig knows: file, family, style |
| `~/.config/omarchy/themes`, `/usr/share/omarchy/themes` | installed Omarchy themes, user and packaged |
| `~/.local/share/icons`, `~/.icons`, `/usr/share/icons` | icon themes |

Rows are collapsed **per file**: fontconfig lists one row per style and language variant,
and one file is one font, so the same path would otherwise appear dozens of times (84
duplicates on the reference machine before the fix). The extra fields show how many
fontconfig entries and how many files a family has.

Themes keep user and packaged scope in their identity, so a user theme with the same name
as a packaged one is two components, not one. The active theme is decided by
`omarchy theme set`; this adapter never switches or removes one — these files are shared
resources and deleting them by name is exactly what the specification forbids.

Missing `fc-list` does not hide the rest: themes and icon themes still load.

## Not implemented (reported as unavailable, never simulated)

AppImage candidates, inventory comparison between runs, and every form of mutation
outside the four ecosystems that have a manager (pacman, the AUR helper, Omarchy plugins
and Cargo). Nothing under `~/.config` is ever written: Hyprland and Zsh are inspected,
never edited.

Two findings from the reference machine shape that work:

* Hyprland on Omarchy 4.5 is configured in **Lua** (`~/.config/hypr/*.lua`), not only
  `.conf`, so a future Hyprland adapter must read both instead of assuming the old
  layout.
* `uv` was present only as a mise shim with no installed tool, which is exactly the case
  where a heuristic would have invented an integration. It is reported as unavailable.

## Etapa 5 verdicts, in one place

| Integration | Removal | Why |
| --- | --- | --- |
| Neovim plugins | no | the configuration still references them; no manager exists |
| Zsh plugins | no | writing `.zshrc` would need a diff and an explicit authorisation |
| Hyprland references | no | a reference is not ownership |
| Hyprland backup copies | **reclaim** | nobody reads them; they move to quarantine, never to `/dev/null` |
| Omarchy removal backups | **reclaim** | the catalogue hides them and no manager owns them |
| systemd services | no | stop, disable and remove are different actions |
| Python tools | no | the owning environment cannot be proven yet |
| Fonts and themes | no | shared resources, owned by the distribution or by Omarchy |

## Installing this project

`./install.sh` builds and installs the binary, verifies it runs, and can add the Omarchy
bar widget. `./install.sh --uninstall` reverses it and keeps your data unless you pass
`--purge-state`. See the README for the full option list and the manual equivalent.

## Adding an adapter

1. Create `src/adapters/<id>/mod.rs` implementing `ComponentAdapter`.
2. Declare `status()` honestly, including every command the adapter may run.
3. Validate identifiers before they become arguments.
4. Return `Ok(None)` from `details()` when the component vanished.
5. Claim `supports()` only for what this adapter may actually run.
6. Register it in `adapters::build_registry`.
7. Add unit tests with `FakeRunner` and fixtures; add no dependency for it.
