# Omarchy Control Center

A terminal inventory of everything installed on an Omarchy / Arch Linux system —
packages, AUR packages, Omarchy shell plugins, flatpak applications, Neovim plugins,
Zsh plugins, Hyprland configuration, Cargo and Python tools, systemd services, fonts,
themes and icon themes — with lifecycle operations that are safe by construction: every destructive action
shows exactly what will run, what else could be affected, what could not be
determined, and refuses to proceed without the exact name typed back.

The interface never runs as root and never reads a password. It is also not a skin
over `pacman`: adapters own their ecosystem, an unavailable tool is reported as an
unavailable integration instead of an error, and no value is ever invented.

## Requirements

| Tool | Needed for | Notes |
| --- | --- | --- |
| Rust 1.88+ (edition 2024) | building | developed against `rustc 1.99.1` |
| `pacman` | the pacman adapter | read-only queries need no privileges |
| `yay` or `paru` | the AUR adapter | optional; without one, AUR is reported unavailable |
| `omarchy` | the plugin adapter | lists, inspects and removes shell plugins |
| `flatpak` | the flatpak adapter | **optional by design**; the program is fully usable without it |
| `nvim` + `lazy-lock.json` | the Neovim adapter | reads files only; no command is run |
| `zsh` + oh-my-zsh | the Zsh adapter | reads `.zshrc` only; it is never modified |
| `systemctl` | the services adapter | inspection only |
| `cargo` | the Cargo tools adapter | removal goes through `cargo uninstall` |
| `uv` or `pipx` | the Python tools adapter | a lazy-install shim is refused, never executed |
| `fc-list` | the resources adapter | fonts; themes and icons are read from their roots |

Not implemented, and reported as such rather than simulated: AppImage candidates,
inventory comparison between runs, and mutation of anything under `~/.config`
(Hyprland and Zsh are inspected, never edited).

## What it inventories

| Ecosystem | Source of truth | Can remove? |
| --- | --- | --- |
| pacman packages | `pacman -Q` / `-Qi` / `-Qm` / `-Qdtq` | yes, official packages only |
| AUR packages | the pacman foreign set, enriched by `yay -Si` | yes, through the helper |
| Omarchy shell plugins | `omarchy plugin list --json` | yes, third-party only |
| flatpak apps and runtimes | `flatpak list` | yes, when flatpak is installed |
| Neovim plugins | `lazy-lock.json` + `~/.local/share/nvim/lazy` | no — the config still references them |
| Zsh plugins | `plugins=(...)` in `.zshrc` + oh-my-zsh | no — `.zshrc` is never written |
| Hyprland configuration | `~/.config/hypr/*.lua` and `*.conf` | no — a reference is not ownership |
| Cargo tools | `cargo install --list` | yes, through `cargo uninstall` |
| Python tools | `uv tool list` / `pipx list --json` | no — ownership cannot be proven yet |
| systemd services | `systemctl list-units` + `list-unit-files` | no — stop, disable and remove are different actions |
| Fonts, themes, icons | `fc-list` + the theme and icon roots | no — shared resources |

On the machine this was developed on that is 2666 components, 0 failures and 0 duplicate
identities, including 3 Neovim plugins reported as `missing` because the lock file lists
them and no directory exists.

## Build and run

```sh
cargo build --release
./target/release/omarchy-control-center
```

Read-only session — explores everything, offers no destructive action at all:

```sh
./target/release/omarchy-control-center --read-only
```

Headless, no terminal needed (useful over ssh and for scripted inspection):

```sh
# the whole inventory as JSON
omarchy-control-center --print-inventory

# what a removal would run, exactly, without running it
omarchy-control-center --print-plan btop
omarchy-control-center --print-plan some-aur-package
```

## Keyboard

| Key | Action |
| --- | --- |
| `↑` `↓`, `k` `j` | move inside the focused pane |
| `Tab`, `←` `→` | switch between the category sidebar and the list |
| `Enter` | details of the selected component; from details, go back |
| `/` | search by name, identifier, description or source (`Enter` keeps, `Esc` clears) |
| `d` | preview a removal (only when the adapter supports it) |
| `u` | preview an update (flatpak only — see limitations) |
| `H` | operation history |
| `a` | adapter status and per-adapter issues |
| `?` | help, including what every state means |
| `q`, `Q`, `Esc` | quit; never interrupted while a command is running |

`Enter` on a row only opens details. It never removes anything.

## What the interface guarantees

* **Nothing destructive on navigation.** Opening, searching and browsing never runs a
  mutating command. The only mutating commands are the ones in a confirmed plan.
* **Every removal is previewed.** Exact argv, whether root is required, whether the
  terminal is handed over, affected dependents, files owned by the package, and an
  explicit "not determined" section.
* **Confirmation is not a `y`.** You type the component's native identifier
  (`btop`). Anything else, including `yes`, does nothing.
* **Stale plans do not execute.** The plan is recomputed right before execution and
  compared by fingerprint; a changed or vanished component cancels the action.
* **A broken adapter is isolated.** Each adapter reports its own availability, and a
  missing tool or a panicking integration never stops the others.
* **No invented facts.** Absent values render as `No disponible`, and every component
  names the exact command that detected it.

## Configuration

Optional file at `~/.config/omarchy-control-center/config.toml` (see
`config.example.toml`). An invalid file is reported and the defaults are used, so a
typo never prevents the inventory from opening.

## Files it writes

| Path | Contents | Mode |
| --- | --- | --- |
| `~/.config/omarchy-control-center/config.toml` | your configuration (only if you create it) | — |
| `~/.local/state/omarchy-control-center/occ.log` | diagnostic log: commands, exit codes, errors | `0600` |
| `~/.local/state/omarchy-control-center/history.jsonl` | one line per executed operation | `0600` |

The history stores timestamp, adapter, component, operation, result, exit code,
command and error summary. It never stores passwords, tokens, environment variables
or configuration content.

## Documentation

* [`docs/architecture.md`](docs/architecture.md) — layers, adapter contract, operation lifecycle.
* [`docs/adapters.md`](docs/adapters.md) — every adapter, every command it runs and why.
* [`docs/security.md`](docs/security.md) — threat model, privilege handling, and the automated tests behind each guarantee.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test          # 137 tests, no system modification, no root
```

Tests use fake executables and canned command output; nothing in the suite installs,
removes or upgrades a package.

## Known limitations

These are real, and the program does not pretend otherwise.

* **Removal is offered for four ecosystems only** — pacman, the AUR helper, Omarchy
  plugins and Cargo — because those have a manager that owns the files. Everything else
  is inspection: deleting a plugin directory or a font because a name matched is exactly
  what this program refuses to do.
* **Updates are only offered for flatpak.** Arch does not support partial upgrades, so
  a per-package update would be a lie; a whole-system upgrade is a different operation
  with different consent, and is not in this version.
* **Removals never cascade.** If anything requires a package, the operation is blocked
  and the dependents are listed. `-Rns`, `-Rdd` and orphan batches are not offered.
* **The interface waits during a privileged command.** A removal hands the terminal to
  `sudo`, then `pacman`; the interface is unavailable until that finishes. This is the
  price of never reading a password ourselves.
* **A command that forks a long-lived grandchild may outlive its deadline.** The child
  is killed and the interface stops waiting at the deadline, but the reader threads are
  detached rather than joined, so the pipe can stay open server-side.
* **flatpak's present path is verified through fixtures only** on the machine this was
  developed on (no flatpak installed there). Its unavailable path is verified live.
* **A lazy-install shim is never executed.** `uv` behind mise would install itself on
  first run, so the Python adapter reports the integration as unavailable with that
  reason instead of triggering an install. On the reference machine that is exactly what
  happens.
* **No visual verification in a headless environment.** The TUI needs a real terminal;
  without a TTY the program exits with a clear message instead of corrupting the shell.
