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

## Install

One command, no root, nothing outside your home:

```sh
git clone https://github.com/TzzJokerzzT/omarchy-control-center.git
cd omarchy-control-center
./install.sh
```

It builds the release binary, installs it into `$XDG_BIN_HOME` (or `~/.local/bin`),
verifies the installed binary runs, installs the short name **`control-center`**, makes
sure that directory is on your `PATH`, and offers to add the Omarchy bar widget. Every
step is printed before it happens; `--dry-run` prints the whole plan and changes nothing.

After that, from any directory and in any terminal:

```sh
control-center                 # the inventory
control-center --read-only     # explore without any destructive action
```

`control-center` is a **link** to the same binary, not a shell alias. An alias would only
exist in an interactive shell that sourced your rc file; a link works from scripts, from
the bar widget, and from `omarchy-launch-tui control-center` too.

```sh
./install.sh --dry-run        # show every step, touch nothing
./install.sh --no-plugin      # binary only
./install.sh --plugin         # binary plus the Omarchy bar widget, no question asked
./install.sh --prefix DIR     # install somewhere else
./install.sh --path           # add the prefix to PATH without asking
./install.sh --no-path        # never touch the shell configuration
./install.sh --help           # all options
```

Requirements: `cargo` and `rustc` (Rust 1.88+; on Arch: `sudo pacman -S --needed rust`).
An Arch-based system is not required, but the pacman, AUR and Omarchy adapters report
themselves unavailable elsewhere.

| What the installer writes | Where |
| --- | --- |
| the program | `<prefix>/omarchy-control-center` |
| the short name | `<prefix>/control-center` (a link to the program) |
| one marked block, only when the prefix is not already on `PATH` | your shell rc file, chosen from `$SHELL` |
| the bar widget (optional) | `~/.config/omarchy/plugins/tzzjokerzzt.omarchy-control-center/` |

The PATH block is written between `# >>> omarchy-control-center >>>` and
`# <<< omarchy-control-center <<<`, so it is added once no matter how often you install,
and removed exactly — the rest of the file is left byte-for-byte as it was. It also
avoids adding a duplicate entry when the file is sourced twice. If you prefer to manage
`PATH` yourself, `--no-path` keeps the installer out of your shell configuration.

The widget is copied only after Omarchy's own validator accepts it, and it is enabled
with `omarchy plugin enable` so the shell state stays consistent.

### Manual install

If you would rather not run a script:

```sh
cargo build --release --locked
install -Dm755 target/release/omarchy-control-center ~/.local/bin/omarchy-control-center
ln -sfn omarchy-control-center ~/.local/bin/control-center    # the short name

# optional bar widget
omarchy plugin validate omarchy-plugin
cp -r omarchy-plugin ~/.config/omarchy/plugins/tzzjokerzzt.omarchy-control-center
omarchy plugin enable tzzjokerzzt.omarchy-control-center
```

## Uninstall

```sh
./install.sh --uninstall                # the program, the short name, the PATH block, the widget
./install.sh --uninstall --purge-state  # …and your configuration, history and quarantine
```

The uninstall removes the `control-center` link and the PATH block it added, restoring
your shell rc file to exactly what it was before the install.

By default an uninstall **keeps your data**, and says where it is:

| Path | Contents |
| --- | --- |
| `~/.config/omarchy-control-center/config.toml` | your configuration |
| `~/.local/state/omarchy-control-center/occ.log` | diagnostic log |
| `~/.local/state/omarchy-control-center/history.jsonl` | every operation you ran |
| `~/.local/state/omarchy-control-center/quarantine/` | files moved out of the way by a cleanup |

`--purge-state` deletes those too, after warning that quarantined files are the only
copy left: the cleanup screen moves things there instead of deleting them, so removing
that directory is the step that makes them unrecoverable. It asks before doing it;
`--yes` skips the question for scripted use.

The widget is removed through `omarchy plugin remove`, so the shell's own bookkeeping is
updated instead of a directory being deleted behind its back.

## Build and run

Building from source without installing anything:

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
| `C` | cleanup: leftovers that can be moved into quarantine |
| `V` | quarantine: restore what was moved, or purge it for good |
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

## Cleaning up leftovers (`C`)

Orphans are not one thing, and the difference decides what may happen to them.

| Kind | What this program does |
| --- | --- |
| A leftover directory or file nobody owns — a plugin removal backup, a Hyprland `*.bak` copy | **Moved into quarantine.** You get a preview with the exact paths and the total size, then a phrase to type. It can be restored. |
| A package the package manager no longer needs (AUR / pacman orphan) | **Not in the cleanup list.** It appears in the inventory with the `orphan` state and is removed through the manager that owns it, one package at a time. |
| A systemd service | Inspection only. Stopping, disabling and removing are three different actions. |

On the reference machine the cleanup screen finds 62 leftovers (1.4 MiB): 23 plugin
removal backups that `omarchy plugin list` hides and 39 Hyprland backup copies.

Two steps, on purpose:

1. **Quarantine** (`C`, select, `Enter`) moves the item to
   `~/.local/state/omarchy-control-center/quarantine/<stamp>/` and records the original
   path in a ledger. Restore it from `V`. Nothing is deleted, and the space is not free
   yet.
2. **Purge** (`V`, `p`, type the exact phrase) deletes a quarantined entry for good. It
   only accepts what the ledger recorded, so it can never be pointed at an arbitrary
   path.

Why not one "delete all orphans" button: a path is verified before it moves (direct child
of a root the adapter owns, canonicalised, never a symlink, size unchanged since the
preview), and a bulk delete would throw that away for the one case that matters.

## Omarchy bar widget

`omarchy-plugin/` contains a real Omarchy shell plugin: a bar button that opens the
control center, or the installation instructions when the binary is not on `PATH`.

```sh
./install.sh --plugin    # validates, copies and enables the widget
```

Left click opens the inventory in a terminal, right click reuses an open window, and an
uninstalled binary opens the *Build and run* section of this README in the browser.

Note: `omarchy plugin add <git-url>` clones a repository whose **root** is the plugin, so
it cannot install from a subfolder of this project. Copying the folder is the honest
instruction until the widget lives in its own repository — see
[`docs/roadmap.md`](docs/roadmap.md).

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
* [`docs/roadmap.md`](docs/roadmap.md) — every deferred item, with its blocker.

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

* **Cleanup moves things; it does not delete them.** Freeing the space takes a second,
  explicit purge step, and that is the only step that deletes anything this program did
  not create.
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
