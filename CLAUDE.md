# Comms

Rust workspace: a terminal client for Microsoft Teams (binary `comms`, package `comms-tui`). See `docs/development.md` for build and test commands and `docs/architecture.md` for the layout.

## Versioning and commits

- **Semantic versioning** (https://semver.org). The version lives in `[workspace.package]` in `Cargo.toml`. Breaking changes bump MAJOR (while on 0.x, MINOR), new features bump MINOR, fixes bump PATCH.
- **Conventional Commits** (https://www.conventionalcommits.org) for every commit message: `<type>(<optional scope>): <summary>`, with types such as `feat`, `fix`, `docs`, `refactor`, `test`, `perf`, `build`, `ci` and `chore`. Mark breaking changes with `!` after the type/scope or a `BREAKING CHANGE:` footer. The type decides the version bump: `feat` → MINOR, `fix` → PATCH, breaking → MAJOR.

## Before committing

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

If a commit touches `docs/` (Sphinx, see `docs/development.md`), the `.githooks/pre-commit` hook builds the docs with warnings as errors; enable it with `git config core.hooksPath .githooks`.
