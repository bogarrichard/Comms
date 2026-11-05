#!/bin/sh
# Tests for scripts/install.sh. It runs the real script with a PATH that holds only
# stub tools, so nothing is built and nothing leaves the machine. `curl` is a fake
# GitHub: it serves tarballs this test builds, and logs every URL it is asked for.
#
#   sh tests/scripts/test_install.sh
# The stubs are written from single-quoted strings on purpose: their $variables must
# expand when the stub runs, not when it is created.
# shellcheck disable=SC2016
set -u

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
SCRIPT="${INSTALL_SCRIPT:-$ROOT/scripts/install.sh}"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
ENVBIN=$(command -v env)

pass=0
fail=0
ok() { pass=$((pass + 1)); printf '  ok   %s\n' "$1"; }
bad() { fail=$((fail + 1)); printf '  FAIL %s\n' "$1"; [ -n "${2:-}" ] && printf '%s\n' "$2" | sed 's/^/         /'; }

stub() { printf '#!/bin/sh\n%s\n' "$2" >"$SB/bin/$1"; chmod +x "$SB/bin/$1"; }
unstub() { rm -f "$SB/bin/$1"; }
link_tool() { # link a real tool into the sandbox PATH
  p=$(command -v "$1") && ln -s "$p" "$SB/bin/$1"
}

# Fresh sandbox:
#   $SB/bin       the only directory on PATH
#   $SB/home      HOME          $SB/tmp   TMPDIR
#   $SB/releases  what the fake GitHub serves (<tag>/<file>)
#   $SB/cargo.log arguments cargo was run with     $SB/curl.log   URLs curl was asked for
new_sandbox() {
  SB=$(mktemp -d "$TMP/sb.XXXXXX")
  mkdir -p "$SB/bin" "$SB/home" "$SB/tmp" "$SB/releases"
  : >"$SB/cargo.log"
  : >"$SB/curl.log"
  ln -s /bin/sh "$SB/bin/sh"
  for t in cat mkdir chmod mktemp tar gzip rm cp mv cut sha256sum shasum; do link_tool "$t"; done
  stub uname 'if [ "$1" = -m ]; then echo "${FAKE_ARCH:-x86_64}"; else echo "${FAKE_OS:-Linux}"; fi'
  stub git 'exit 0'
  stub cc 'exit 0'
  stub cmake 'exit 0'
  stub_cargo
  stub_curl
}

# Logs its arguments. FAKE_BRANCH_FAIL=1 makes `--branch` fail (the ref is then a tag).
stub_cargo() {
  stub cargo 'echo "$*" >>"'"$SB"'/cargo.log"
case " $* " in *" --branch "*) [ "${FAKE_BRANCH_FAIL:-}" = 1 ] && exit 101 ;; esac
exit 0'
}

# A fake GitHub: /releases/latest redirects to $FAKE_LATEST, downloads come from
# $FAKE_SB/releases/<tag>/<file>, and https://sh.rustup.rs serves $FAKE_SB/rustup.sh.
stub_curl() {
  stub curl 'out=""; wfmt=""; url=""
echo "$*" >>"$FAKE_SB/curl.args"
while [ $# -gt 0 ]; do
  case "$1" in
    -o) out=$2; shift ;;
    -w) wfmt=$2; shift ;;
    --proto | --retry | --retry-delay) shift ;;
    -*) ;;
    *) url=$1 ;;
  esac
  shift
done
echo "$url" >>"$FAKE_SB/curl.log"
case "$url" in
  */releases/latest)
    [ -n "${FAKE_LATEST:-}" ] || exit 22
    [ -n "$wfmt" ] && printf "%s" "${url%/latest}/tag/$FAKE_LATEST"
    exit 0 ;;
  */releases/download/*)
    rest=${url#*/releases/download/}
    f="$FAKE_SB/releases/$rest"
    [ -f "$f" ] || exit 22
    cp "$f" "$out" ;;
  https://sh.rustup.rs)
    cat "$FAKE_SB/rustup.sh" ;;
  *) exit 22 ;;
esac'
}

# make_release TAG TARGET [good|corrupt|norun|empty]: a real tarball plus its .sha256.
make_release() {
  tag=$1; target=$2; kind=${3:-good}
  name="comms-$tag-$target"
  stage="$SB/stage/$name"
  mkdir -p "$stage/scripts" "$SB/releases/$tag"
  echo "# configure" >"$stage/scripts/configure.py"
  echo license >"$stage/LICENSE"
  case "$kind" in
    norun) printf '#!/bin/sh\nexit 1\n' >"$stage/comms" ;;
    empty) : ;;
    *) printf '#!/bin/sh\n[ "${1:-}" = --version ] && echo "comms %s"\nexit 0\n' "$tag" >"$stage/comms" ;;
  esac
  [ -f "$stage/comms" ] && chmod 755 "$stage/comms"
  tar -czf "$SB/releases/$tag/$name.tar.gz" -C "$SB/stage" "$name"
  if command -v sha256sum >/dev/null 2>&1; then sum=$(sha256sum "$SB/releases/$tag/$name.tar.gz" | cut -d' ' -f1)
  else sum=$(shasum -a 256 "$SB/releases/$tag/$name.tar.gz" | cut -d' ' -f1); fi
  [ "$kind" = corrupt ] && sum=0000000000000000000000000000000000000000000000000000000000000000
  printf '%s  %s.tar.gz\n' "$sum" "$name" >"$SB/releases/$tag/$name.tar.gz.sha256"
}

# Run install.sh in the sandbox with extra VAR=value settings. No controlling terminal
# (setsid) so /dev/tty prompts cannot hang or read the developer's keyboard.
# Output goes to $SB/out, the exit status to $RC.
run_install() {
  set -- "$ENVBIN" -i HOME="$SB/home" PATH="$SB/bin" TMPDIR="$SB/tmp" FAKE_SB="$SB" "$@" /bin/sh "$SCRIPT"
  if command -v setsid >/dev/null 2>&1; then
    setsid -w "$@" </dev/null >"$SB/out" 2>&1
  else
    "$@" </dev/null >"$SB/out" 2>&1
  fi
  RC=$?
}
# The source-build path, which needs COMMS_FROM_SOURCE=1 on a platform with a binary.
run_source() { run_install COMMS_FROM_SOURCE=1 "$@"; }

expect_fail() { # name, pattern
  if [ "$RC" -ne 0 ] && grep -q -- "$2" "$SB/out"; then ok "$1"; else bad "$1 (rc=$RC, wanted /$2/)" "$(cat "$SB/out")"; fi
}
expect_ok() { # name
  if [ "$RC" -eq 0 ]; then ok "$1"; else bad "$1 (rc=$RC)" "$(cat "$SB/out")"; fi
}
expect_log() { # name, pattern
  if grep -q -- "$2" "$SB/cargo.log"; then ok "$1"; else bad "$1 (wanted /$2/ in cargo log)" "$(cat "$SB/cargo.log")"; fi
}
expect_out() { # name, pattern
  if grep -q -- "$2" "$SB/out"; then ok "$1"; else bad "$1 (wanted /$2/ in output)" "$(cat "$SB/out")"; fi
}
expect_no_out() { # name, pattern
  if grep -q -- "$2" "$SB/out"; then bad "$1 (found /$2/ in output)" "$(cat "$SB/out")"; else ok "$1"; fi
}
expect_curl() { # name, pattern
  if grep -q -- "$2" "$SB/curl.log"; then ok "$1"; else bad "$1 (wanted /$2/ in curl log)" "$(cat "$SB/curl.log")"; fi
}
expect_no_curl() { # name, pattern
  if grep -q -- "$2" "$SB/curl.log"; then bad "$1 (found /$2/ in curl log)" "$(cat "$SB/curl.log")"; else ok "$1"; fi
}
expect_file() { # name, path
  if [ -f "$2" ]; then ok "$1"; else bad "$1 (missing $2)"; fi
}
expect_no_file() { # name, path
  if [ -e "$2" ]; then bad "$1 (found $2)"; else ok "$1"; fi
}

TARGET=x86_64-unknown-linux-gnu

echo "prebuilt binary"
# No git, compiler or cmake: a binary install must not need them.
new_sandbox; unstub git; unstub cc; unstub cmake; unstub cargo
make_release v1.2.3 $TARGET; run_install FAKE_LATEST=v1.2.3
expect_ok "installs the latest release without any build tools"
expect_file "puts comms in ~/.local/bin" "$SB/home/.local/bin/comms"
if "$SB/home/.local/bin/comms" --version | grep -q "comms v1.2.3"; then ok "the installed binary is the release's"; else bad "the installed binary is the release's"; fi
expect_file "keeps the setup scripts for later" "$SB/home/.local/share/comms/scripts/configure.py"
expect_curl "looks up the latest tag" "/releases/latest"
expect_curl "downloads the archive for this platform" "/releases/download/v1.2.3/comms-v1.2.3-$TARGET.tar.gz$"
if grep -q -- "--retry" "$SB/curl.args" && ! grep -v -- "--retry" "$SB/curl.args" | grep -q .; then ok "every release request retries transient errors"; else bad "every release request retries transient errors" "$(cat "$SB/curl.args")"; fi
expect_curl "downloads the checksum" "/releases/download/v1.2.3/comms-v1.2.3-$TARGET.tar.gz.sha256$"
expect_out "says what was installed" "installed comms v1.2.3"
expect_out "points at configure.py" "configure.py"
expect_out "mentions Signal linking" "comms signal-link"
if [ ! -s "$SB/cargo.log" ]; then ok "cargo was never run"; else bad "cargo was never run" "$(cat "$SB/cargo.log")"; fi
if [ -z "$(ls -A "$SB/tmp")" ]; then ok "temporary files are cleaned up"; else bad "temporary files are cleaned up" "$(ls -A "$SB/tmp")"; fi

new_sandbox; make_release v0.9.0 $TARGET; make_release v2.0.0 $TARGET
run_install COMMS_VERSION=v0.9.0 FAKE_LATEST=v2.0.0
expect_ok "COMMS_VERSION installs that release"
expect_no_curl "and does not ask for the latest" "/releases/latest"
expect_curl "and downloads the pinned tag" "/download/v0.9.0/"

new_sandbox; make_release v1.0.0 $TARGET
run_install FAKE_LATEST=v1.0.0 COMMS_BIN_DIR="$SB/mybin" XDG_DATA_HOME="$SB/xdg"
expect_file "COMMS_BIN_DIR chooses where comms goes" "$SB/mybin/comms"
expect_file "XDG_DATA_HOME chooses where the scripts go" "$SB/xdg/comms/scripts/configure.py"
expect_out "warns when the bin dir is not on PATH" "add $SB/mybin to your PATH"

new_sandbox; make_release v1.0.0 $TARGET
run_install FAKE_LATEST=v1.0.0 COMMS_BIN_DIR="$SB/mybin" PATH="$SB/bin:$SB/mybin"
expect_ok "installs when the bin dir is already on PATH"
expect_no_out "no PATH warning then" "to your PATH"

new_sandbox; make_release v1.0.0 aarch64-apple-darwin
run_install FAKE_LATEST=v1.0.0 FAKE_OS=Darwin FAKE_ARCH=arm64
expect_ok "Apple silicon gets the aarch64-apple-darwin build"
expect_curl "and asks for that archive" "comms-v1.0.0-aarch64-apple-darwin.tar.gz$"

new_sandbox; make_release v1.0.0 $TARGET
mkdir -p "$SB/home/.local/bin"; printf '#!/bin/sh\necho old\n' >"$SB/home/.local/bin/comms"; chmod 755 "$SB/home/.local/bin/comms"
run_install FAKE_LATEST=v1.0.0
if "$SB/home/.local/bin/comms" --version | grep -q "comms v1.0.0"; then ok "replaces an existing install"; else bad "replaces an existing install"; fi
expect_no_file "leaves no temporary copy behind" "$SB/home/.local/bin/.comms.new"

echo "prebuilt binary: things that go wrong"
new_sandbox; make_release v1.0.0 $TARGET corrupt
run_install FAKE_LATEST=v1.0.0
expect_fail "a checksum mismatch is refused" "checksum mismatch"
expect_no_file "and nothing is installed" "$SB/home/.local/bin/comms"

new_sandbox; run_install FAKE_LATEST=v1.0.0
expect_fail "a release without a binary for this platform points at the source build" "COMMS_FROM_SOURCE=1"
expect_no_file "and installs nothing" "$SB/home/.local/bin/comms"

new_sandbox; run_install
expect_fail "failing to look up the latest release is reported" "latest release"

new_sandbox; run_install COMMS_VERSION=latest
expect_fail "a malformed COMMS_VERSION is rejected" "unexpected release tag"
expect_no_curl "before any download" "/releases/download"

new_sandbox; make_release v1.0.0 $TARGET norun
run_install FAKE_LATEST=v1.0.0
expect_fail "a binary that cannot run here is refused" "does not run"
expect_no_file "and not installed" "$SB/home/.local/bin/comms"

new_sandbox; make_release v1.0.0 $TARGET empty
run_install FAKE_LATEST=v1.0.0
expect_fail "an archive without comms is refused" "does not contain a comms binary"

new_sandbox; make_release v1.0.0 $TARGET; unstub curl
run_install FAKE_LATEST=v1.0.0
expect_fail "missing curl is reported" "curl is required"

new_sandbox; make_release v1.0.0 $TARGET; unstub tar
run_install FAKE_LATEST=v1.0.0
expect_fail "missing tar is reported" "tar is required"

new_sandbox; make_release v1.0.0 $TARGET; unstub sha256sum; unstub shasum
run_install FAKE_LATEST=v1.0.0
expect_fail "no checksum tool is reported, not skipped" "sha256sum or shasum"
expect_no_file "and nothing is installed" "$SB/home/.local/bin/comms"

new_sandbox; make_release v1.0.0 $TARGET; unstub sha256sum
run_install FAKE_LATEST=v1.0.0
expect_ok "shasum alone is enough on macOS-like systems"

echo "choosing binary or source"
new_sandbox; run_install FAKE_OS=Darwin FAKE_ARCH=x86_64
expect_out "a platform without a prebuilt binary says so" "no prebuilt binary for Darwin/x86_64"
expect_out "warns that only Linux is tested" "only Linux is a tested target"
expect_log "and builds from source" "install --locked --git https://github.com/bogarrichard/Comms comms-tui"
expect_ok "non-Linux still installs from source"

new_sandbox; make_release v1.0.0 $TARGET
run_install FAKE_LATEST=v1.0.0 COMMS_FROM_SOURCE=1
expect_log "COMMS_FROM_SOURCE=1 builds even where a binary exists" "comms-tui"
expect_no_curl "without downloading one" "/releases"

new_sandbox; make_release v1.0.0 $TARGET
run_install FAKE_LATEST=v1.0.0 COMMS_REF=main
expect_log "COMMS_REF implies a source build" "--branch main comms-tui"
expect_no_curl "without downloading a binary" "/releases"

echo "source build: prerequisites"
new_sandbox; unstub git; run_source
expect_fail "missing git is reported" "git is required"

new_sandbox; unstub cc; run_source
expect_fail "missing C compiler is reported" "C compiler is required"

new_sandbox; unstub cc; stub gcc 'exit 0'; run_source
expect_ok "gcc is accepted instead of cc"

new_sandbox; unstub cmake; run_source
expect_fail "missing cmake is reported" "cmake is required"

echo "source build: rust toolchain"
new_sandbox; unstub cargo; run_source
expect_fail "missing cargo without consent stops" "cargo is required"

new_sandbox; unstub cargo; unstub curl; run_source COMMS_YES=1
expect_fail "COMMS_YES=1 but no curl stops" "curl is required"

# The fake rustup creates ~/.cargo/{env,bin/cargo} like the real one.
new_sandbox; unstub cargo
cat >"$SB/rustup.sh" <<'EOF'
mkdir -p "$HOME/.cargo/bin"
printf '#!/bin/sh\necho "$*" >>"%s/cargo.log"\n' "$FAKE_SB" >"$HOME/.cargo/bin/cargo"
chmod +x "$HOME/.cargo/bin/cargo"
printf 'export PATH="$HOME/.cargo/bin:$PATH"\n' >"$HOME/.cargo/env"
EOF
run_source COMMS_YES=1
expect_ok "COMMS_YES=1 installs rustup then builds"
expect_log "build runs with the freshly installed cargo" "install --locked --git"

new_sandbox; mkdir -p "$SB/home/.cargo/bin"
printf '#!/bin/sh\necho "$*" >>"%s/cargo.log"\n' "$SB" >"$SB/home/.cargo/bin/cargo"
chmod +x "$SB/home/.cargo/bin/cargo"
printf 'export PATH="$HOME/.cargo/bin:$PATH"\n' >"$SB/home/.cargo/env"
unstub cargo; run_source
expect_ok "cargo found through ~/.cargo/env"
expect_log "uses that cargo" "comms-tui"

echo "source build: install"
new_sandbox; run_source
expect_ok "default install succeeds"
expect_log "builds comms-tui from the default repo, locked" "install --locked --git https://github.com/bogarrichard/Comms comms-tui"
if grep -q -- "--branch\|--tag" "$SB/cargo.log"; then bad "default install pins no ref"; else ok "default install pins no ref"; fi
expect_out "prints the installed path" "installed: .*/bin/comms"
expect_out "points at the next steps" "comms --demo"

new_sandbox; run_source COMMS_REPO=https://example.com/fork
expect_log "COMMS_REPO overrides the repository" "--git https://example.com/fork comms-tui"

new_sandbox; run_source COMMS_REF=main
expect_log "COMMS_REF is tried as a branch first" "--branch main comms-tui"

new_sandbox; run_source COMMS_REF=v0.1.0 FAKE_BRANCH_FAIL=1
expect_ok "a tag ref succeeds after the branch attempt fails"
expect_log "falls back to --tag" "--tag v0.1.0 comms-tui"

new_sandbox; stub cargo 'exit 101'; run_source
if [ "$RC" -ne 0 ]; then ok "a failing cargo build fails the install"; else bad "a failing cargo build fails the install"; fi

new_sandbox; run_source
expect_out "warns when the cargo bin dir is not on PATH" "add .*/bin to your PATH"

new_sandbox; run_source CARGO_HOME="$SB/ch" PATH="$SB/bin:$SB/ch/bin"
expect_no_out "no PATH warning when it is already there" "to your PATH"

echo "syntax"
if /bin/sh -n "$SCRIPT"; then ok "sh -n parses"; else bad "sh -n parses"; fi
if command -v shellcheck >/dev/null 2>&1; then
  if shellcheck -s sh "$SCRIPT"; then ok "shellcheck clean"; else bad "shellcheck clean"; fi
else
  echo "  skip shellcheck (not installed)"
fi

printf '\n%d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
