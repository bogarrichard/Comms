# Development

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace          # no Microsoft credentials needed
cargo run -p comms-tui -- --demo
```

* Rust stable ≥ 1.94 (set by `sqlx`). SQLite is bundled (FTS5 available); first build compiles it and needs a C compiler and cmake (rustls/aws-lc).
* Tests use `wiremock` for HTTP, `Store::open_in_memory`, and `comms-testkit::FakeProvider`. TUI tests render to ratatui's `TestBackend`.
* `COMMS_CONFIG_DIR` / `COMMS_DATA_DIR` redirect config and cache; `COMMS_LOG=debug` raises the log level (log file, never the terminal).
* Dependency policy: permissive licenses only (`deny.toml`). GPL/Slint licences are excluded until the GUI licensing decision.
* Live integration testing against a real tenant is manual for now: `comms login`, then use the client. An automated `--features live-graph` suite is future work.
