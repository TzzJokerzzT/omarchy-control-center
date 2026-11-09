# Roadmap

What is deliberately not built yet, why, and what it would take. Nothing here is a
promise: each line names its blocker, because an item without one is just a wish.

## Operations

| Item | Why it is not done | What it needs |
| --- | --- | --- |
| Enable / disable Omarchy plugins from the interface | The state is already read (`enabled`, `active`, `canDisable`); only the action is missing | `OperationKind::Enable`/`Disable`, plans for `omarchy plugin enable/disable`, and the same confirmation path |
| Pacman updates | Arch does not support partial upgrades, so a per-package update would be a lie | A whole-system upgrade as its own operation, with its own consent and its own preview |
| Flatpak present-path verification | No flatpak is installed on the machine this was built on | A host with flatpak, or a container, to run the tested fixtures against the real tool |
| Review queue for AUR/pacman orphans | Removing dependencies in batch is exactly what the specification forbids | A read-only review screen with sizes and dependents, then per-package removal through the manager |
| systemd actions (stop / disable / remove) | Three different actions with three different consequences | Three distinct operations, each with its own plan; "remove" additionally needs unit-file ownership |
| Zsh `.zshrc` editing | Writing a shell configuration without a diff is unacceptable | A diff view, a backup, an atomic write, and explicit authorisation per change |
| Assisted font/theme removal | Shared resources; deleting by name is the failure mode this project exists to avoid | Path review per item, ownership proof, and a restore path |
| Purge-on-schedule for the quarantine | Deleting automatically is a policy decision the user must make | A retention setting, a visible count, and an explicit opt-in |

## Integrations

| Item | Why it is not done | What it needs |
| --- | --- | --- |
| mise tools | On the reference machine `uv` is a mise shim whose execution *installs* the tool; the adapter refuses to trigger that | A mise adapter that reads `mise ls --json` instead of executing shims |
| AppImage candidates | Detection is a guess about intent; the specification asks for candidates, not ownership | A configured directory list, a confidence label, and no removal |
| Omarchy themes as first-class components | They are listed today under resources, but the active theme and switching are Omarchy's | Read `omarchy theme current`, and never switch from here |
| Inventory comparison between runs | Needs a stable snapshot format first | Versioned JSON export plus a diff command |
| Removable-media and user-scope duplication | The same package can exist in two scopes; the model supports it, no adapter uses it yet | An adapter that reports both scopes instead of merging them |

## Interface

| Item | Why it is not done | What it needs |
| --- | --- | --- |
| Mouse support | Keyboard-only was an explicit requirement | A click map per pane and a documented fallback |
| Colours from the active Omarchy theme | The palette is fixed because a wrong guess is worse than a neutral default | Reading the theme's `colors.toml` and degrading to the current palette |
| Rendering snapshot tests | The renderer is only smoke-tested today | `ratatui` `TestBackend` with golden buffers |
| Progress view for long operations | Removal hands the terminal over, so there is nothing to draw during it | Requires running the command detached with a log tail, which changes the privilege model |

## Distribution

| Item | Why it is not done | What it needs |
| --- | --- | --- |
| CI workflow | Not added yet | `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test` on push |
| AUR package | The project is not packaged | A `PKGBUILD` building from the tagged source; `install.sh` covers the source install in the meantime |
| `omarchy plugin add <url>` for the widget | The manager clones a repository whose **root** is the plugin; ours is a subfolder of a Rust project | A separate plugin repository, or a root manifest (which would pollute the crate) |
| Release artifacts | The workflow exists; no release has been published yet | Push a `v*` tag |
| Automatic update | Installing without being asked is the one thing this program must not do | A decision to make only if the checksum requirement stays |
| Shell completions | `install.sh` installs the binary and the widget, nothing else | Generated completions per shell, installed next to the binary |
| Uninstall from the interface | The installer undoes an install; the program cannot uninstall itself | Needs a decision about what "uninstall" means while the program is running |

## Quality

| Item | Why it is not done | What it needs |
| --- | --- | --- |
| Property-based tests for the parsers | The fixtures cover real shapes; adversarial shapes are not enumerated | A generator per parser, asserting invariants rather than outputs |
| Differential runs against real tools | Same blocker as flatpak verification | A container image with pacman, yay, flatpak and systemd |
| Privilege via polkit | The TTY handoff works and never touches a password; polkit would keep the interface alive during the operation | A polkit agent integration and a fallback when none is running |

## Reading this list

Three rules held while building the current version, and they apply to everything above:

1. **Inspection before modification.** A new integration starts as inspection and earns
   actions one at a time, each with a preview, a confirmation and tests.
2. **A manager owns its data.** If an ecosystem has a manager, the operation goes through
   it. This program never edits the manager's files behind its back.
3. **No deletion without a way back.** Leftovers are moved into quarantine, never deleted
   on the strength of a name, and a permanent delete only accepts entries a ledger
   already knows.
