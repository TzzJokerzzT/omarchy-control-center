# Changelog

Every notable change, grouped by kind. This project follows [Semantic
Versioning](https://semver.org/): the version lives in `Cargo.toml`, the tag is `vX.Y.Z`,
and the rules for bumping it are in [`docs/versioning.md`](docs/versioning.md).

## [Unreleased]

Nothing yet.

## [0.2.0] — themes, cleanup and packaging

### Added

- **Themes**: six palettes (`zelda`, `cyberpunk`, `rei`, `monokai`, `matrix`, `cviper`) plus
  a `default` that keeps the terminal's own colours, a selector window (`t`) with live
  preview, and support for user theme files in `~/.config/omarchy-control-center/themes/`.
  A rejected theme file is reported with the exact field, never silently ignored.
- **Quarantine cleanup** (`C`, `V`): leftovers nobody owns are moved into a quarantine area
  that records their original path, so a restore is a real operation; purging is a separate,
  explicit step. Omarchy plugin removal backups and Hyprland backup copies are reclaimable.
- **Omarchy shell plugins** (`omarchy plugin list --json`): list, detail and removal of
  third-party plugins through Omarchy's own manager, with the plan stating whether the
  manager deletes the clone or moves a plain directory aside.
- **Etapa 5 integrations**: lazy.nvim, Zsh, Hyprland inspection, systemd services, Cargo
  tools, Python tools (uv/pipx) and fonts/themes/icon themes.
- **Installer** (`install.sh`): builds, installs, verifies, adds the `control-center` short
  name, offers to place the prefix on `PATH` with a marked block, and installs the Omarchy
  bar widget. `--dry-run`, `--uninstall` and `--purge-state` included.
- **Omarchy bar widget** (`omarchy-plugin/`): a bar button that opens the control center or
  its installation instructions.
- **CI**: formatting, lints and the full suite on every push and pull request.
- **Branch protection as code**: `scripts/protect-main.sh` re-applies the `main` rules,
  including the required `ci` check, and refuses when its prerequisites are missing.
- **Documentation**: architecture, adapters, security, maintenance, roadmap, contributing,
  and screenshots generated from the real renderer.

### Changed

- `--read-only` now refuses `--print-plan` before reading the inventory, so the answer no
  longer depends on what happens to be installed.
- The draw loop never walks the filesystem: cleanup sizes are measured once when the view
  opens.

### Fixed

- Font rows are collapsed per file: fontconfig lists one row per style and language variant,
  which produced duplicate identities (`84` on the reference machine).
- Hyprland comment stripping is quote-aware, so `".../fix-ultrawide-mode --now"` is not
  truncated at `--`.
- A lazy-install shim (`uv` behind mise) is refused instead of executed: running it installs
  the tool as a side effect.
- The test suite no longer depends on the ambient `PATH` or on a package manager.

## [0.1.0] — first published version

The initial published state: pacman, AUR and flatpak inventories, Omarchy plugins, operation
plans with a typed confirmation and drift detection, controlled removal, read-only mode, and
the operation history.
