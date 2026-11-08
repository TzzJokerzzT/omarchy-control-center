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

## Not implemented (reported as unavailable, never simulated)

Lazy.nvim / Neovim plugins, Zsh plugins, Hyprland configuration inspection, Cargo
tools, Python tools (`uv`/`pipx`/`pip`), systemd services, fonts, themes and icons,
AppImage candidates, and inventory export/compare beyond the JSON printing above.

Two findings from the reference machine shape that work:

* Hyprland on Omarchy 4.5 is configured in **Lua** (`~/.config/hypr/*.lua`), not only
  `.conf`, so a future Hyprland adapter must read both instead of assuming the old
  layout.
* `uv` was present only as a mise shim with no installed tool, which is exactly the case
  where a heuristic would have invented an integration. It is reported as unavailable.

## Adding an adapter

1. Create `src/adapters/<id>/mod.rs` implementing `ComponentAdapter`.
2. Declare `status()` honestly, including every command the adapter may run.
3. Validate identifiers before they become arguments.
4. Return `Ok(None)` from `details()` when the component vanished.
5. Claim `supports()` only for what this adapter may actually run.
6. Register it in `adapters::build_registry`.
7. Add unit tests with `FakeRunner` and fixtures; add no dependency for it.
