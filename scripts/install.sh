#!/bin/sh
# Install Comms (the `comms` binary).
#
#   curl --proto '=https' --tlsv1.2 -sSf \
#     https://raw.githubusercontent.com/bogarrichard/Comms/main/scripts/install.sh | sh
#
# Downloads the prebuilt binary from the latest GitHub release and checks its
# SHA-256. Where no binary exists for your platform (or with COMMS_FROM_SOURCE=1)
# it builds from source with cargo instead.
#
# Environment overrides:
#   COMMS_VERSION      release tag to install, e.g. v0.1.0   (default: the latest release)
#   COMMS_BIN_DIR      where to put `comms`                  (default: ~/.local/bin)
#   COMMS_FROM_SOURCE=1  build with cargo instead of downloading a binary
#   COMMS_REPO         git URL of the repository             (default: https://github.com/bogarrichard/Comms)
#   COMMS_REF          branch or tag to build from source    (implies COMMS_FROM_SOURCE=1)
#   COMMS_YES=1        install rustup without asking if cargo is missing (source builds)
set -eu

REPO="${COMMS_REPO:-https://github.com/bogarrichard/Comms}"
REF="${COMMS_REF:-}"
BIN_DIR="${COMMS_BIN_DIR:-$HOME/.local/bin}"
DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/comms"

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

# The release target for this machine, or nothing when there is no prebuilt binary.
detect_target() {
  case "$(uname -s)/$(uname -m)" in
    Linux/x86_64 | Linux/amd64) echo x86_64-unknown-linux-gnu ;;
    Darwin/arm64 | Darwin/aarch64) echo aarch64-apple-darwin ;;
    *) echo "" ;;
  esac
}

path_hint() {
  case ":$PATH:" in
    *":$1:"*) ;;
    *) say "add $1 to your PATH, e.g. put  export PATH=\"$1:\$PATH\"  in your shell profile" ;;
  esac
}

next_steps() {
  say "try it:      comms --demo"
  say "Teams:       register the app (docs/setup-entra.md), then configure and sign in"
  say "Signal:      comms signal-link   (needs signal-cli; see docs/signal.md)"
  say "Slack:       comms slack-login   (paste your user token; see docs/slack.md)"
}

# --------------------------------------------------------------- prebuilt binary
sha256_of() {
  if have sha256sum; then sha256sum "$1" | cut -d' ' -f1
  elif have shasum; then shasum -a 256 "$1" | cut -d' ' -f1
  else die "need sha256sum or shasum to verify the download"; fi
}

install_binary() {
  target=$1
  have curl || die "curl is required"
  have tar || die "tar is required"
  have mktemp || die "mktemp is required"

  # GitHub occasionally answers 5xx; the curl calls below retry those before giving up.
  tag="${COMMS_VERSION:-}"
  if [ -z "$tag" ]; then
    # /releases/latest redirects to /releases/tag/<tag>; no API token or jq needed.
    final=$(curl --proto '=https' --tlsv1.2 --retry 4 --retry-delay 2 -fsSL -o /dev/null -w '%{url_effective}' "$REPO/releases/latest") ||
      die "could not look up the latest release of $REPO"
    tag=${final##*/}
  fi
  case "$tag" in
    v[0-9]*.[0-9]*.[0-9]*) ;;
    *) die "unexpected release tag '$tag' (set COMMS_VERSION=vX.Y.Z, or COMMS_FROM_SOURCE=1)" ;;
  esac

  name="comms-$tag-$target"
  base="$REPO/releases/download/$tag"
  work=$(mktemp -d) || die "could not create a temporary directory"
  trap 'rm -rf "$work"' EXIT INT TERM

  say "downloading $name"
  curl --proto '=https' --tlsv1.2 --retry 4 --retry-delay 2 -fsSL -o "$work/$name.tar.gz" "$base/$name.tar.gz" ||
    die "download failed: $base/$name.tar.gz (does release $tag exist? try COMMS_FROM_SOURCE=1)"
  curl --proto '=https' --tlsv1.2 --retry 4 --retry-delay 2 -fsSL -o "$work/$name.tar.gz.sha256" "$base/$name.tar.gz.sha256" ||
    die "download failed: $base/$name.tar.gz.sha256"

  want=$(cut -d' ' -f1 <"$work/$name.tar.gz.sha256")
  got=$(sha256_of "$work/$name.tar.gz")
  [ -n "$want" ] && [ "$want" = "$got" ] || die "checksum mismatch for $name.tar.gz; refusing to install"

  tar -xzf "$work/$name.tar.gz" -C "$work" || die "could not unpack the archive"
  src="$work/$name"
  [ -f "$src/comms" ] || die "the archive does not contain a comms binary"
  chmod 755 "$src/comms"
  "$src/comms" --version >/dev/null 2>&1 || die "the downloaded binary does not run on this system"

  mkdir -p "$BIN_DIR" || die "cannot create $BIN_DIR"
  # Copy under a temporary name, then rename: safe even while an old comms is running.
  if ! cp "$src/comms" "$BIN_DIR/.comms.new" || ! mv -f "$BIN_DIR/.comms.new" "$BIN_DIR/comms"; then
    rm -f "$BIN_DIR/.comms.new"
    die "cannot write to $BIN_DIR"
  fi
  if [ -d "$src/scripts" ]; then
    mkdir -p "$DATA_DIR"
    rm -rf "$DATA_DIR/scripts"
    cp -R "$src/scripts" "$DATA_DIR/scripts"
  fi

  say "installed comms $tag: $BIN_DIR/comms"
  [ -d "$DATA_DIR/scripts" ] && say "setup script: python3 $DATA_DIR/scripts/configure.py"
  path_hint "$BIN_DIR"
  next_steps
}

# ------------------------------------------------------------------- from source
install_source() {
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

  bin="${CARGO_HOME:-$HOME/.cargo}/bin"
  say "installed: $bin/comms"
  path_hint "$bin"
  next_steps
}

# -------------------------------------------------------------------------- main
target=$(detect_target)
if [ "${COMMS_FROM_SOURCE:-}" = 1 ] || [ -n "$REF" ]; then
  install_source
elif [ -n "$target" ]; then
  install_binary "$target"
else
  say "no prebuilt binary for $(uname -s)/$(uname -m); building from source"
  [ "$(uname -s)" = Linux ] || say "warning: only Linux is a tested target (see README)"
  install_source
fi
