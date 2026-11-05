#!/bin/sh
# Tests for scripts/install.sh. It runs the real script with a PATH that holds only
# stub tools (git, cc, cmake, cargo, curl, uname), so nothing is built or downloaded.
#
#   sh tests/scripts/test_install.sh
set -u

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
SCRIPT="$ROOT/scripts/install.sh"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

pass=0
fail=0
ok() { pass=$((pass + 1)); printf '  ok   %s\n' "$1"; }
bad() { fail=$((fail + 1)); printf '  FAIL %s\n' "$1"; [ -n "${2:-}" ] && printf '%s\n' "$2" | sed 's/^/         /'; }

# Fresh sandbox: $SB/bin (stubs), $SB/home (HOME), $SB/cargo.log (what cargo was asked).
new_sandbox() {
  SB=$(mktemp -d "$TMP/sb.XXXXXX")
  mkdir -p "$SB/bin" "$SB/home"
  : >"$SB/cargo.log"
  ln -s /bin/sh "$SB/bin/sh"
  for t in cat mkdir chmod; do ln -s "$(command -v $t)" "$SB/bin/$t"; done
  stub uname 'echo Linux'
  stub git 'exit 0'
  stub cc 'exit 0'
  stub cmake 'exit 0'
  stub_cargo
}
stub() { printf '#!/bin/sh\n%s\n' "$2" >"$SB/bin/$1"; chmod +x "$SB/bin/$1"; }
unstub() { rm -f "$SB/bin/$1"; }
# Logs its arguments. FAKE_BRANCH_FAIL=1 makes `--branch` fail (the ref is then a tag).
stub_cargo() {
  stub cargo 'echo "$*" >>"'"$SB"'/cargo.log"
case " $* " in *" --branch "*) [ "${FAKE_BRANCH_FAIL:-}" = 1 ] && exit 101 ;; esac
exit 0'
}

# Run install.sh in the sandbox. No controlling terminal (setsid) so /dev/tty prompts
# cannot hang or read the developer's keyboard. Output goes to $SB/out, status to $RC.
ENVBIN=$(command -v env)
run_install() {
  set -- "$ENVBIN" -i HOME="$SB/home" PATH="$SB/bin" "$@" /bin/sh "$SCRIPT"
  if command -v setsid >/dev/null 2>&1; then
    setsid -w "$@" </dev/null >"$SB/out" 2>&1
  else
    "$@" </dev/null >"$SB/out" 2>&1
  fi
  RC=$?
}

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

echo "prerequisites"
new_sandbox; unstub git; run_install
expect_fail "missing git is reported" "git is required"

new_sandbox; unstub cc; run_install
expect_fail "missing C compiler is reported" "C compiler is required"

new_sandbox; unstub cc; stub gcc 'exit 0'; run_install
expect_ok "gcc is accepted instead of cc"

new_sandbox; unstub cmake; run_install
expect_fail "missing cmake is reported" "cmake is required"

echo "rust toolchain"
new_sandbox; unstub cargo; run_install
expect_fail "missing cargo without consent stops" "cargo is required"

new_sandbox; unstub cargo; run_install COMMS_YES=1
expect_fail "COMMS_YES=1 but no curl stops" "curl is required"

# curl "downloads" a rustup script that, like the real one, creates ~/.cargo/{env,bin/cargo}.
new_sandbox; unstub cargo
stub curl 'cat <<EOF
mkdir -p "\$HOME/.cargo/bin"
printf "#!/bin/sh\necho \"\\\$*\" >>\"'"$SB"'/cargo.log\"\n" >"\$HOME/.cargo/bin/cargo"
chmod +x "\$HOME/.cargo/bin/cargo"
printf "export PATH=\"\$HOME/.cargo/bin:\\\$PATH\"\n" >"\$HOME/.cargo/env"
EOF'
run_install COMMS_YES=1
expect_ok "COMMS_YES=1 installs rustup then builds"
expect_log "build runs with the freshly installed cargo" "install --locked --git"

new_sandbox; mkdir -p "$SB/home/.cargo/bin"
printf '#!/bin/sh\necho "$*" >>"%s/cargo.log"\n' "$SB" >"$SB/home/.cargo/bin/cargo"
chmod +x "$SB/home/.cargo/bin/cargo"
printf 'export PATH="$HOME/.cargo/bin:$PATH"\n' >"$SB/home/.cargo/env"
unstub cargo; run_install
expect_ok "cargo found through ~/.cargo/env"
expect_log "uses that cargo" "comms-tui"

echo "install"
new_sandbox; run_install
expect_ok "default install succeeds"
expect_log "builds comms-tui from the default repo, locked" "install --locked --git https://github.com/bogarrichard/Comms comms-tui"
if grep -q -- "--branch\|--tag" "$SB/cargo.log"; then bad "default install pins no ref"; else ok "default install pins no ref"; fi
expect_out "prints the installed path" "installed: .*/bin/comms"
expect_out "points at the demo and configure steps" "scripts/configure.py"

new_sandbox; run_install COMMS_REPO=https://example.com/fork
expect_log "COMMS_REPO overrides the repository" "--git https://example.com/fork comms-tui"

new_sandbox; run_install COMMS_REF=main
expect_log "COMMS_REF is tried as a branch first" "--branch main comms-tui"

new_sandbox; run_install COMMS_REF=v0.1.0 FAKE_BRANCH_FAIL=1
expect_ok "a tag ref succeeds after the branch attempt fails"
expect_log "falls back to --tag" "--tag v0.1.0 comms-tui"

new_sandbox; stub cargo 'exit 101'; run_install
if [ "$RC" -ne 0 ]; then ok "a failing cargo build fails the install"; else bad "a failing cargo build fails the install"; fi

echo "PATH hint"
new_sandbox; run_install
expect_out "warns when the cargo bin dir is not on PATH" "add .*/bin to your PATH"

new_sandbox; run_install CARGO_HOME="$SB/ch" PATH="$SB/bin:$SB/ch/bin"
if grep -q "to your PATH" "$SB/out"; then bad "no PATH warning when it is already there"; else ok "no PATH warning when it is already there"; fi

echo "platform"
new_sandbox; stub uname 'echo Darwin'; run_install
expect_out "non-Linux gets a warning, not a failure" "only Linux is a tested target"
expect_ok "non-Linux still installs"

echo "syntax"
if /bin/sh -n "$SCRIPT"; then ok "sh -n parses"; else bad "sh -n parses"; fi
if command -v shellcheck >/dev/null 2>&1; then
  if shellcheck -s sh "$SCRIPT"; then ok "shellcheck clean"; else bad "shellcheck clean"; fi
else
  echo "  skip shellcheck (not installed)"
fi

printf '\n%d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
