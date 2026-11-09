#!/usr/bin/env bash
# Omarchy Control Center — installer.
#
# Installs the binary into the user's own bin directory under two names
# (`omarchy-control-center` and the short `control-center`), makes sure that directory
# is on PATH, optionally adds the Omarchy bar widget, and can undo all of it.
#
# It never uses sudo, never touches anything outside the paths it prints, and never
# deletes state silently: the operation history and the quarantine area survive an
# uninstall unless you ask for them to go.
#
# Every step is echoed before it happens. Pass --dry-run to see the whole plan without
# changing anything.
set -euo pipefail

REPO_URL="https://github.com/TzzJokerzzT/omarchy-control-center"
BIN_NAME="omarchy-control-center"
SHORT_NAME="control-center"
PLUGIN_ID="tzzjokerzzt.omarchy-control-center"
MARK_BEGIN="# >>> omarchy-control-center >>>"
MARK_END="# <<< omarchy-control-center <<<"

PREFIX="${XDG_BIN_HOME:-$HOME/.local/bin}"
PLUGIN=ask
DRY_RUN=0
UNINSTALL=0
PURGE_STATE=0
ADD_TO_PATH=ask
ASSUME_YES=0

usage() {
  cat <<'EOF'
Omarchy Control Center — installer

Usage:
  ./install.sh [options]

Options:
  --prefix DIR      Where to put the binary (default: $XDG_BIN_HOME or ~/.local/bin)
  --path            Add the prefix to your shell configuration without asking
  --no-path         Never touch your shell configuration
  --plugin          Also install the Omarchy bar widget
  --no-plugin       Skip the bar widget even if Omarchy is present
  --dry-run         Print every step and change nothing
  --uninstall       Remove the binary, the short name and the bar widget
  --purge-state     With --uninstall: also delete config, history and quarantine
  --yes             Do not ask for confirmation (for scripted use)
  -h, --help        This text

What it writes:
  <prefix>/omarchy-control-center   the program
  <prefix>/control-center           a link to it, so the short name works from anywhere
  your shell rc file                one marked block that puts <prefix> on PATH,
                                    only when it is not there already
  ~/.config/omarchy/plugins/tzzjokerzzt.omarchy-control-center/   the bar widget (optional)

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
      --path) ADD_TO_PATH=yes; shift ;;
      --no-path) ADD_TO_PATH=no; shift ;;
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
short_link() { printf '%s/%s' "$PREFIX" "$SHORT_NAME"; }

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

on_path() {
  case ":$PATH:" in
    *":$1:"*) return 0 ;;
    *) return 1 ;;
  esac
}

# The shell configuration file that should gain the PATH block.
rc_file() {
  local shell_name
  shell_name="$(basename "${SHELL:-bash}")"
  case "$shell_name" in
    zsh) printf '%s' "${ZDOTDIR:-$HOME}/.zshrc" ;;
    bash) printf '%s' "$HOME/.bashrc" ;;
    fish) printf '%s' "${XDG_CONFIG_HOME:-$HOME/.config}/fish/config.fish" ;;
    *) printf '%s' "$HOME/.profile" ;;
  esac
}

rc_has_block() {
  local rc="$1"
  [ -f "$rc" ] && grep -Fqx "$MARK_BEGIN" "$rc"
}

# Writes the marked block. Idempotent: an existing block is replaced, never duplicated,
# and the block itself avoids adding a duplicate PATH entry when sourced twice.
write_path_block() {
  local rc="$1"
  local block
  if [ "$(basename "${SHELL:-bash}")" = fish ]; then
    block="$MARK_BEGIN
# Added by install.sh so 'control-center' works from any directory.
fish_add_path \"$PREFIX\"
$MARK_END"
  else
    block="$MARK_BEGIN
# Added by install.sh so 'control-center' works from any directory.
case \":\$PATH:\" in
  *\":$PREFIX:\"*) ;;
  *) export PATH=\"$PREFIX:\$PATH\" ;;
esac
$MARK_END"
  fi

  if [ "$DRY_RUN" = 1 ]; then
    printf '    [dry-run] append the PATH block to %s\n' "$rc"
    printf '%s\n' "$block" | sed 's/^/             /'
    return 0
  fi

  local directory
  directory="$(dirname "$rc")"
  mkdir -p "$directory"
  remove_path_block "$rc" quiet

  # A file that does not end with a newline would swallow the first marker line.
  if [ -s "$rc" ] && [ -n "$(tail -c 1 "$rc" 2>/dev/null)" ]; then
    printf '\n' >> "$rc"
  fi
  printf '%s\n' "$block" >> "$rc"
  printf '    added the PATH block to %s\n' "$rc"
}

# Removes exactly the block this script wrote, leaving the rest of the file untouched.
remove_path_block() {
  local rc="$1"
  local quiet="${2:-}"
  [ -f "$rc" ] || return 0
  rc_has_block "$rc" || return 0

  if [ "$DRY_RUN" = 1 ]; then
    printf '    [dry-run] remove the PATH block from %s\n' "$rc"
    return 0
  fi

  local temporary
  temporary="$(mktemp "$rc.occ.XXXXXX")"
  awk -v begin="$MARK_BEGIN" -v end="$MARK_END" '
    $0 == begin { skipping = 1 }
    !skipping { print }
    $0 == end { skipping = 0 }
  ' "$rc" >"$temporary"

  # Never leave the user with a truncated file: the rewrite must not contain markers.
  if grep -Fqx "$MARK_BEGIN" "$temporary"; then
    rm -f "$temporary"
    warn "refusing to rewrite $rc: the PATH block could not be isolated"
    return 1
  fi
  mv "$temporary" "$rc"
  [ -n "$quiet" ] || printf '    removed the PATH block from %s\n' "$rc"
}

install_short_name() {
  step "Installing the short name '$SHORT_NAME'"
  # A link, not a shell alias: a link works for scripts, launchers and the bar widget,
  # and it needs no shell configuration to be re-read.
  run ln -sfn "$BIN_NAME" "$(short_link)"
  if [ "$DRY_RUN" = 1 ]; then
    say "    [dry-run] $(short_link) --version"
  else
    "$(short_link)" --version >/dev/null || die "the '$SHORT_NAME' link does not run"
    say "    $SHORT_NAME -> $PREFIX/$BIN_NAME"
  fi
}

ensure_path() {
  step "Making '$SHORT_NAME' reachable from any directory"
  if on_path "$PREFIX"; then
    say "    $PREFIX is already on PATH"
    return 0
  fi

  local rc
  rc="$(rc_file)"
  say "    $PREFIX is not on PATH yet."

  if [ "$ADD_TO_PATH" = no ]; then
    warn "leaving your shell configuration alone (--no-path)"
    say "    add this yourself, then open a new terminal:"
    say "      export PATH=\"$PREFIX:\$PATH\""
    return 0
  fi

  if [ "$ADD_TO_PATH" = ask ] && ! confirm "Add $PREFIX to PATH in $rc?"; then
    say "    kept your shell configuration as it is"
    say "    to use the short name anyway, run: $PREFIX/$BIN_NAME"
    return 0
  fi

  write_path_block "$rc"
  say "    open a new terminal, or run: source $rc"
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

  install_short_name
  ensure_path
  install_plugin

  step "Done"
  if on_path "$PREFIX" || [ "$ADD_TO_PATH" = yes ]; then
    say "    Run it:      $SHORT_NAME"
  else
    say "    Run it:      $SHORT_NAME   (once $PREFIX is on your PATH)"
    say "                 $PREFIX/$BIN_NAME  works right now"
  fi
  say "    Read-only:   $SHORT_NAME --read-only"
  say "    Inventory:   $SHORT_NAME --print-inventory"
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

  step "Removing the short name"
  if [ -L "$(short_link)" ] || [ -e "$(short_link)" ]; then
    run rm -f "$(short_link)"
    say "    removed $(short_link)"
  else
    say "    $(short_link) is not there"
  fi

  step "Removing the PATH block"
  local rc
  rc="$(rc_file)"
  if rc_has_block "$rc"; then
    remove_path_block "$rc"
  else
    say "    $rc has no block from this installer"
  fi

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
