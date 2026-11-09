#!/usr/bin/env bash
# Omarchy Control Center — installer.
#
# Installs the binary into the user's own bin directory, optionally adds the Omarchy
# bar widget, and can undo all of it. It never uses sudo, never touches anything
# outside the paths it prints, and never deletes state silently: the operation history
# and the quarantine area survive an uninstall unless you ask for them to go.
#
# Every step is echoed before it happens. Pass --dry-run to see the whole plan without
# changing anything.
set -euo pipefail

REPO_URL="https://github.com/TzzJokerzzT/omarchy-control-center"
BIN_NAME="omarchy-control-center"
PLUGIN_ID="tzzjokerzzt.omarchy-control-center"

PREFIX="${XDG_BIN_HOME:-$HOME/.local/bin}"
PLUGIN=ask
DRY_RUN=0
UNINSTALL=0
PURGE_STATE=0
ASSUME_YES=0

usage() {
  cat <<'EOF'
Omarchy Control Center — installer

Usage:
  ./install.sh [options]

Options:
  --prefix DIR      Where to put the binary (default: $XDG_BIN_HOME or ~/.local/bin)
  --plugin          Also install the Omarchy bar widget
  --no-plugin       Skip the bar widget even if Omarchy is present
  --dry-run         Print every step and change nothing
  --uninstall       Remove the binary and the bar widget
  --purge-state     With --uninstall: also delete config, history and quarantine
  --yes             Do not ask for confirmation (for scripted use)
  -h, --help        This text

What it writes:
  <prefix>/omarchy-control-center                                   the program
  ~/.config/omarchy/plugins/tzzjokerzzt.omarchy-control-center/     the bar widget (optional)

What it leaves alone unless you ask:
  ~/.config/omarchy-control-center/          your configuration
  ~/.local/state/omarchy-control-center/     log, operation history, quarantine area
EOF
}

say() { printf '%s\n' "$*"; }
step() { printf '\n==> %s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

# Runs a command, or prints it when --dry-run is set.
run() {
  if [ "$DRY_RUN" = 1 ]; then
    printf '    [dry-run] %s\n' "$*"
    return 0
  fi
  printf '    %s\n' "$*"
  "$@"
}

confirm() {
  [ "$ASSUME_YES" = 1 ] && return 0
  local answer
  printf '%s [y/N] ' "$1"
  read -r answer || answer=""
  case "$answer" in
    y|Y|yes|YES) return 0 ;;
    *) return 1 ;;
  esac
}

parse_args() {
  while [ $# -gt 0 ]; do
    case "$1" in
      --prefix)
        [ $# -ge 2 ] || die "--prefix needs a directory"
        PREFIX="$2"
        shift 2
        ;;
      --plugin) PLUGIN=yes; shift ;;
      --no-plugin) PLUGIN=no; shift ;;
      --dry-run) DRY_RUN=1; shift ;;
      --uninstall) UNINSTALL=1; shift ;;
      --purge-state) PURGE_STATE=1; shift ;;
      --yes|-y) ASSUME_YES=1; shift ;;
      -h|--help) usage; exit 0 ;;
      *) die "unknown option: $1 (try --help)" ;;
    esac
  done
}

# Paths the program owns. Kept in one place so uninstall cannot guess.
state_dir() { printf '%s' "${XDG_STATE_HOME:-$HOME/.local/state}/omarchy-control-center"; }
config_dir() { printf '%s' "${XDG_CONFIG_HOME:-$HOME/.config}/omarchy-control-center"; }
plugins_dir() { printf '%s' "${XDG_CONFIG_HOME:-$HOME/.config}/omarchy/plugins"; }
plugin_dir() { printf '%s/%s' "$(plugins_dir)" "$PLUGIN_ID"; }

is_arch_like() {
  [ -r /etc/os-release ] || return 1
  # shellcheck disable=SC1091
  . /etc/os-release
  case "${ID:-}:${ID_LIKE:-}" in
    *arch*) return 0 ;;
    *) return 1 ;;
  esac
}

need_cargo() {
  command -v cargo >/dev/null 2>&1 && command -v rustc >/dev/null 2>&1 &&
    command -v install >/dev/null 2>&1 && return 0
  say "This needs a Rust toolchain (cargo, rustc) and coreutils 'install'."
  say "On Arch and Omarchy:"
  say "    sudo pacman -S --needed rust"
  return 1
}

do_install() {
  step "Checking the machine"
  if is_arch_like; then
    say "    Arch-based system detected: pacman adapters will work."
  else
    warn "this is not an Arch-based system; the pacman, AUR and Omarchy plugin"
    warn "adapters will report themselves unavailable. The program still runs."
  fi
  need_cargo || die "missing build requirements"
  say "    cargo: $(cargo --version)"

  step "Building the release binary"
  run cargo build --release --locked

  step "Installing into $PREFIX"
  run mkdir -p "$PREFIX"
  run install -m 755 target/release/"$BIN_NAME" "$PREFIX/$BIN_NAME"

  step "Verifying the installed binary"
  if [ "$DRY_RUN" = 1 ]; then
    say "    [dry-run] $PREFIX/$BIN_NAME --version"
  else
    "$PREFIX/$BIN_NAME" --version >/dev/null ||
      die "the installed binary does not run; nothing else was changed"
    say "    $("$PREFIX/$BIN_NAME" --version)"
  fi

  install_plugin

  step "Done"
  case ":$PATH:" in
    *":$PREFIX:"*) say "    Run it:      $BIN_NAME" ;;
    *)
      warn "$PREFIX is not on your PATH"
      say "    Add it to your shell configuration, or run the full path:"
      say "      $PREFIX/$BIN_NAME"
      ;;
  esac
  say "    Read-only:   $BIN_NAME --read-only"
  say "    Inventory:   $BIN_NAME --print-inventory"
  say "    Uninstall:   ./install.sh --uninstall"
  say ""
  say "    The first run writes a log to $(state_dir)/occ.log"
  say "    Nothing is installed or removed without a preview and a typed confirmation."
}

install_plugin() {
  local wanted="$PLUGIN"
  if [ "$wanted" = ask ]; then
    if command -v omarchy >/dev/null 2>&1 && [ -d omarchy-plugin ]; then
      if confirm "Add the Omarchy bar widget?"; then wanted=yes; else wanted=no; fi
    else
      wanted=no
    fi
  fi
  [ "$wanted" = yes ] || return 0

  step "Installing the Omarchy bar widget"
  [ -d omarchy-plugin ] || die "omarchy-plugin/ is missing; run this from the repository root"

  if command -v omarchy >/dev/null 2>&1; then
    # Omarchy's own validator decides whether this is a usable plugin. Nothing is
    # copied when it refuses.
    if [ "$DRY_RUN" = 1 ]; then
      say "    [dry-run] omarchy plugin validate omarchy-plugin"
    else
      if omarchy plugin validate omarchy-plugin; then
        say "    the plugin validates against Omarchy's schema"
      else
        die "Omarchy rejected the plugin manifest; nothing was copied"
      fi
    fi
  else
    warn "the 'omarchy' command was not found: the widget is copied but cannot be enabled"
  fi

  run mkdir -p "$(plugins_dir)"
  run rm -rf "$(plugin_dir)"
  run cp -r omarchy-plugin "$(plugin_dir)"

  if command -v omarchy >/dev/null 2>&1; then
    run omarchy plugin enable "$PLUGIN_ID"
  fi
  say "    widget: $(plugin_dir)"
}

do_uninstall() {
  local binary="$PREFIX/$BIN_NAME"

  step "Removing the program"
  if [ -f "$binary" ]; then
    run rm -f "$binary"
    say "    removed $binary"
  else
    say "    $binary is not there; nothing to remove"
  fi

  step "Removing the Omarchy bar widget"
  if [ -d "$(plugin_dir)" ]; then
    if command -v omarchy >/dev/null 2>&1; then
      # The manager owns the plugin directory: let it do the removal.
      run omarchy plugin remove "$PLUGIN_ID" --yes
    else
      # Exact path, verified to be the directory this installer created.
      if [ "$(plugin_dir)" = "$(plugins_dir)/$PLUGIN_ID" ]; then
        run rm -rf "$(plugin_dir)"
      else
        warn "refusing to remove an unexpected path: $(plugin_dir)"
      fi
    fi
  else
    say "    the widget is not installed"
  fi

  step "Your data"
  say "    configuration : $(config_dir)"
  say "    state         : $(state_dir)"
  say "                    (log, operation history, quarantine area)"

  if [ "$PURGE_STATE" = 1 ]; then
    warn "this deletes your configuration, the operation history and anything sitting"
    warn "in the quarantine area. Quarantined files are the only copy: they will not"
    warn "become recoverable afterwards."
    if confirm "Delete $(config_dir) and $(state_dir)?"; then
      run rm -rf "$(config_dir)"
      run rm -rf "$(state_dir)"
    else
      say "    kept"
    fi
  else
    say ""
    say "    Left in place. To delete them too:"
    say "      ./install.sh --uninstall --purge-state"
  fi

  step "Done"
}

main() {
  parse_args "$@"
  if [ "$UNINSTALL" = 1 ]; then
    do_uninstall
  else
    do_install
  fi
}

main "$@"
