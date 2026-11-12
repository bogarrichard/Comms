# Development

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace          # no Microsoft credentials needed
cargo run -p comms-tui -- --demo
```

* Rust stable ≥ 1.94 (set by `sqlx`). SQLite is bundled (FTS5 available); first build compiles it and needs a C compiler and cmake (rustls/aws-lc).
* The Signal provider is tested against an in-memory fake `signal-cli` (`crates/comms-provider-signal/src/tests.rs`); no Signal account or `signal-cli` is needed.
* The Slack provider is tested with `wiremock` standing in for `slack.com/api`; no Slack workspace or token is needed.
* Backends are Cargo features of `comms-tui` / `comms-bootstrap`. Build a subset with `cargo build -p comms-tui --no-default-features --features slack`; CI-style check of the combinations: `cargo check -p comms-tui --no-default-features --features teams` (and `signal`, `slack`, or none).
* Tests use `wiremock` for HTTP, `Store::open_in_memory`, and `comms-testkit::FakeProvider`. TUI tests render to ratatui's `TestBackend`.
* `COMMS_CONFIG_DIR` / `COMMS_DATA_DIR` redirect config and cache; `COMMS_LOG=debug` raises the log level (log file, never the terminal).
* Dependency policy: permissive licenses only (`deny.toml`). GPL/Slint licences are excluded until the GUI licensing decision.
* Live integration testing against a real tenant is manual for now: `comms login`, then use the client. An automated `--features live-graph` suite is future work.

## Script tests

`scripts/install.sh` and `scripts/configure.py` have their own tests. No network is used and nothing is built or installed: the shell test runs the real script with a PATH of stubs, where `curl` is a fake GitHub serving tarballs the test builds (so checksum, version lookup and failure paths are exercised for real). `INSTALL_SCRIPT=<file>` runs the tests against another copy, handy for checking that a deliberately broken installer is caught.

```bash
python3 -m unittest discover -s tests/scripts -v
sh tests/scripts/test_install.sh
shellcheck -s sh scripts/install.sh      # if shellcheck is installed
```

## Documentation

The docs are [Sphinx](https://www.sphinx-doc.org/) with the Markdown parser MyST and the Furo theme; the sources are the `.md` files in `docs/`, and the version shown comes from `Cargo.toml`. Build and open them locally:

```bash
python3 -m venv .venv && .venv/bin/pip install -r docs/requirements.txt
.venv/bin/sphinx-build -W --keep-going -b html docs docs/_build/html
xdg-open docs/_build/html/index.html
```

`-W` makes warnings errors, so a broken cross-reference or heading fails the build. Add a new page by creating `docs/<name>.md` and listing it in a `toctree` in `docs/index.md`.

**Pre-commit hook.** `.githooks/pre-commit` runs that build whenever a commit touches `docs/` or `Cargo.toml`, and blocks the commit if it fails. Enable it once per clone (`git config core.hooksPath .githooks`); skip it for one commit with `git commit --no-verify`. It uses `.venv/bin/sphinx-build` or any `sphinx-build` on your `PATH`. `sh tests/scripts/test_docs_hook.sh` tests the hook itself.

**Publishing.** The `Docs` workflow builds the site on every docs change and, on `main`, deploys it to GitHub Pages: <https://bogarrichard.github.io/Comms/>. It needs Settings → Pages → Source set to **GitHub Actions**; no branch or folder is chosen. Pull requests only build, as a check.

## Releasing

1. Make sure `main` is green (CI runs the Rust checks on Linux/Windows/macOS and the script tests on Linux/macOS).
2. Set `version` in `[workspace.package]` of `Cargo.toml` (SemVer; `0.x` minors may break), run `cargo build` so `Cargo.lock` follows, and commit it as `chore(release): vX.Y.Z`.
3. Tag and push: `git tag vX.Y.Z && git push origin vX.Y.Z`.

The `Release` workflow then checks the tag equals the Cargo version, re-runs the tests, builds `comms` for Linux x86_64, macOS arm64 and Windows x86_64, smoke-tests `comms --version`, and publishes a GitHub release with the archives and `.sha256` files. A tag with a suffix (`v0.2.0-rc.1`) is published as a pre-release. Users can also install a tag from source: `COMMS_REF=v0.1.0 sh scripts/install.sh`.
