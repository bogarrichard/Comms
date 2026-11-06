#!/bin/sh
# Tests for .githooks/pre-commit: run the real hook in a scratch git repository with a
# fake sphinx-build, so no Sphinx installation is needed.
#
#   sh tests/scripts/test_docs_hook.sh
set -u

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
HOOK="${PRE_COMMIT_HOOK:-$ROOT/.githooks/pre-commit}"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

pass=0
fail=0
ok() { pass=$((pass + 1)); printf '  ok   %s\n' "$1"; }
bad() { fail=$((fail + 1)); printf '  FAIL %s\n' "$1"; [ -n "${2:-}" ] && printf '%s\n' "$2" | sed 's/^/         /'; }

# A repo with docs/ and a Rust file; $REPO/bin/sphinx-build records its arguments and
# exits with $SPHINX_RC (default 0).
new_repo() {
  REPO=$(mktemp -d "$TMP/repo.XXXXXX")
  mkdir -p "$REPO/docs" "$REPO/src" "$REPO/bin"
  git -C "$REPO" init -q
  git -C "$REPO" config user.email t@example.com
  git -C "$REPO" config user.name t
  printf '# x\n' >"$REPO/docs/index.md"
  printf 'fn main() {}\n' >"$REPO/src/main.rs"
  printf '[workspace.package]\nversion = "0.1.0"\n' >"$REPO/Cargo.toml"
  printf '#!/bin/sh\necho "$*" >>"%s/sphinx.log"\nexit "${SPHINX_RC:-0}"\n' "$REPO" >"$REPO/bin/sphinx-build"
  chmod +x "$REPO/bin/sphinx-build"
  git -C "$REPO" add -A
}

# run_hook [VAR=value ...]: PATH is the real one plus the fake sphinx-build. With
# NOSPHINX=1 it is a directory holding only the tools the hook itself needs, so a
# sphinx-build installed on this machine cannot leak in. TMPDIR is private to the repo.
run_hook() {
  mkdir -p "$REPO/tmp"
  path="$REPO/bin:$PATH"
  for arg in "$@"; do
    if [ "$arg" = NOSPHINX=1 ]; then
      mkdir -p "$REPO/minimal"
      for t in git grep mktemp rm cat sh; do
        p=$(command -v "$t") && ln -sf "$p" "$REPO/minimal/$t"
      done
      path="$REPO/minimal"
    fi
  done
  (cd "$REPO" && env "$@" TMPDIR="$REPO/tmp" PATH="$path" sh "$HOOK") >"$REPO/out" 2>&1
  RC=$?
}

sphinx_ran() { [ -s "$REPO/sphinx.log" ]; }

new_repo
run_hook
if [ "$RC" -eq 0 ] && sphinx_ran; then ok "builds the docs when docs files are staged"; else bad "builds the docs when docs files are staged" "$(cat "$REPO/out")"; fi
if grep -q -- "-W" "$REPO/sphinx.log" && grep -q -- "--keep-going" "$REPO/sphinx.log"; then ok "warnings are errors, and all of them are reported"; else bad "warnings are errors" "$(cat "$REPO/sphinx.log")"; fi

new_repo
git -C "$REPO" reset -q
git -C "$REPO" add src/main.rs
run_hook
if [ "$RC" -eq 0 ] && ! sphinx_ran; then ok "skips the build for commits that do not touch the docs"; else bad "skips the build for non-docs commits" "$(cat "$REPO/out")"; fi

new_repo
git -C "$REPO" reset -q
git -C "$REPO" add Cargo.toml
run_hook
if [ "$RC" -eq 0 ] && sphinx_ran; then ok "builds when Cargo.toml changes (it supplies the docs version)"; else bad "builds when Cargo.toml changes" "$(cat "$REPO/out")"; fi

new_repo
run_hook SPHINX_RC=1
if [ "$RC" -ne 0 ] && grep -q "docs build failed" "$REPO/out"; then ok "a failing docs build blocks the commit"; else bad "a failing docs build blocks the commit (rc=$RC)" "$(cat "$REPO/out")"; fi

new_repo
run_hook NOSPHINX=1
if [ "$RC" -ne 0 ] && grep -q "pip install -r docs/requirements.txt" "$REPO/out"; then ok "missing sphinx-build blocks the commit with install instructions"; else bad "missing sphinx-build blocks the commit (rc=$RC)" "$(cat "$REPO/out")"; fi

new_repo
mkdir -p "$REPO/.venv/bin"
printf '#!/bin/sh\necho venv >>"%s/sphinx.log"\n' "$REPO" >"$REPO/.venv/bin/sphinx-build"
chmod +x "$REPO/.venv/bin/sphinx-build"
run_hook
if grep -q '^venv$' "$REPO/sphinx.log"; then ok "prefers the repository's .venv"; else bad "prefers the repository's .venv" "$(cat "$REPO/sphinx.log")"; fi

new_repo
run_hook
if sphinx_ran && [ -z "$(ls -A "$REPO/tmp")" ]; then ok "the temporary build directory is removed afterwards"; else bad "the temporary build directory is removed afterwards" "$(ls -A "$REPO/tmp")"; fi
new_repo
run_hook SPHINX_RC=1
if [ -z "$(ls -A "$REPO/tmp")" ]; then ok "and also when the build fails"; else bad "and also when the build fails" "$(ls -A "$REPO/tmp")"; fi

echo "syntax"
if sh -n "$HOOK"; then ok "sh -n parses"; else bad "sh -n parses"; fi
if command -v shellcheck >/dev/null 2>&1; then
  if shellcheck -s sh "$HOOK" "$0"; then ok "shellcheck clean"; else bad "shellcheck clean"; fi
else
  echo "  skip shellcheck (not installed)"
fi

printf '\n%d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
