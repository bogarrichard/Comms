#!/bin/sh
# Install Comms (the `comms` binary) by building it from source with cargo.
#
#   curl --proto '=https' --tlsv1.2 -sSf \
#     https://raw.githubusercontent.com/bogarrichard/Comms/main/scripts/install.sh | sh
#
# Environment overrides:
#   COMMS_REPO  git URL to build from   (default: https://github.com/bogarrichard/Comms)
#   COMMS_REF   branch or tag to build  (default: the repository's default branch)
#   COMMS_YES=1 install rustup without asking if cargo is missing
set -eu

REPO="${COMMS_REPO:-https://github.com/bogarrichard/Comms}"
REF="${COMMS_REF:-}"

say() { printf 'comms-install: %s\n' "$*"; }
die() { printf 'comms-install: error: %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

# Piped into sh, stdin is the script, so questions must go to the terminal.
confirm() {
  [ "${COMMS_YES:-}" = 1 ] && return 0
  [ -r /dev/tty ] || return 1
  printf '%s [y/N] ' "$1" >/dev/tty
  read -r answer </dev/tty || return 1
  case "$answer" in y | Y | yes | YES) return 0 ;; *) return 1 ;; esac
}

[ "$(uname -s)" = Linux ] || say "warning: only Linux is a tested target (see README)"

# Build prerequisites: a C compiler and cmake (bundled SQLite, rustls/aws-lc).
have git || die "git is required"
have cc || have gcc || die "a C compiler is required (e.g. sudo apt install build-essential)"
have cmake || die "cmake is required (e.g. sudo apt install cmake)"

# Rust toolchain.
# shellcheck source=/dev/null
if ! have cargo && [ -r "$HOME/.cargo/env" ]; then . "$HOME/.cargo/env"; fi
if ! have cargo; then
  confirm "cargo not found. Install Rust via rustup (https://sh.rustup.rs)?" ||
    die "cargo is required; install Rust from https://rustup.rs and re-run"
  have curl || die "curl is required to install rustup"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  # shellcheck source=/dev/null
  . "$HOME/.cargo/env"
fi

say "building comms from $REPO${REF:+ ($REF)}; the first build takes a few minutes"
if [ -n "$REF" ]; then
  cargo install --locked --git "$REPO" --branch "$REF" comms-tui ||
    cargo install --locked --git "$REPO" --tag "$REF" comms-tui
else
  cargo install --locked --git "$REPO" comms-tui
fi

BIN="${CARGO_HOME:-$HOME/.cargo}/bin"
say "installed: $BIN/comms"
case ":$PATH:" in
  *":$BIN:"*) ;;
  *) say "add $BIN to your PATH (new shells source ~/.cargo/env if rustup set it up)" ;;
esac
say "try it: comms --demo"
say "real use: register the app (docs/setup-entra.md), then run: python3 scripts/configure.py"
