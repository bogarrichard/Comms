# Comms

A fast, keyboard-driven, native terminal client for Microsoft Teams **chats and team channels**, built in Rust on a provider-independent core (so other messaging backends can be added later).

> **Status: early.** The TUI works end to end against mocked Microsoft Graph responses and a built-in demo backend. It has **not yet been run against a live tenant** — your first sign-in is the first live test. See [Known limitations](#known-limitations).

## Try it (no account needed)

```bash
cargo run -p comms-tui -- --demo
```

## Use it with Teams

1. Register an Entra app (5 minutes): [docs/setup-entra.md](docs/setup-entra.md)
2. Put the client ID in `~/.config/comms/config.toml` (or `COMMS_CLIENT_ID`):
   ```toml
   client_id = "00000000-0000-0000-0000-000000000000"
   ```
3. `cargo run -p comms-tui -- login` (browser) or `... login --device-code` (SSH)
4. `cargo run -p comms-tui`

Press `?` in the client for key bindings (vim-style: `j`/`k`, `Enter`, `/`, `i`, `Esc`, `q`).

## Features

- Chats (1:1, group, meeting) and team channels, with unread markers
- Opens from a local SQLite cache instantly; the network never blocks the UI
- Send messages; ambiguous failures are reconciled, **never blindly resent**
- Load older messages, channel replies shown inline, conversation filter, configurable keys
- Tokens live in the OS keyring only (fails closed if none is available)

## Known limitations

- Not yet validated against a live Microsoft tenant.
- Reading **channel messages needs administrator consent** (`ChannelMessage.Read.All`). If your organisation blocks it, set `channels = "basic"` (list + post) or `"off"`.
- Polling only (no push without a server): chat list every 60 s, open conversation every 15 s.
- No editing, deleting, reactions, attachments download, presence or message search yet.
- Windows and macOS keyring paths compile-gated but untested; Linux is the primary target.

## Layout

See [docs/architecture.md](docs/architecture.md). Research and decisions: [docs/phase-0-plan.md](docs/phase-0-plan.md). Development: [docs/development.md](docs/development.md).

License: Apache-2.0 (see `LICENSE`).
