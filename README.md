<p align="center">
  <img src="docs/_static/logo.png" alt="Comms logo" width="128">
</p>

# Comms

A fast, keyboard-driven, native terminal client for **Microsoft Teams** (chats and team channels), **Signal** (text) and **Slack**, built in Rust on a provider-independent core. Each connection has its own colour: Teams is purple, Signal is blue, Slack is green.

> **Status: early.** The TUI works end to end against mocked Microsoft Graph responses and a built-in demo backend. It has **not yet been run against a live tenant** — your first sign-in is the first live test. See [Known limitations](#known-limitations).

Documentation: <https://bogarrichard.github.io/Comms/> (sources in [`docs/`](docs/), built with Sphinx).

## Install

```bash
curl --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/bogarrichard/Comms/main/scripts/install.sh | sh
```

This downloads the prebuilt binary of the latest [release](https://github.com/bogarrichard/Comms/releases) (Linux x86_64, macOS Apple silicon), verifies its SHA-256 and puts `comms` in `~/.local/bin`. Windows: download the `.zip` from the release page. Other platforms, or `COMMS_FROM_SOURCE=1`, build from source with cargo.

Useful variables: `COMMS_VERSION=v0.1.0` (pin a release), `COMMS_BIN_DIR=/usr/local/bin`, `COMMS_FROM_SOURCE=1`. To read the script before running it: `curl -sSf <url> -o install.sh && less install.sh && sh install.sh`.

## Try it (no account needed)

```bash
comms --demo        # or: cargo run -p comms-tui -- --demo
```

## Use it with Teams

1. Register an Entra app (5 minutes): [docs/setup-entra.md](docs/setup-entra.md)
2. Run the setup questionnaire (creates or updates `~/.config/comms/config.toml`; it can start the browser sign-in at the end):
   ```bash
   python3 scripts/configure.py
   ```
   Or edit the file by hand: `client_id = "00000000-0000-0000-0000-000000000000"` (also settable via `COMMS_CLIENT_ID`).
3. `cargo run -p comms-tui -- login` (browser) or `... login --device-code` (SSH)
4. `cargo run -p comms-tui`

## Use it with Signal

Comms links to your Signal account as a secondary device (like Signal Desktop) through [signal-cli](https://github.com/AsamK/signal-cli). Text only for now. Set up in [docs/signal.md](docs/signal.md):

```bash
comms signal-link          # shows a QR code / link; scan it in Signal → Settings → Linked devices
```
```toml
# ~/.config/comms/config.toml
provider = "signal"
[signal]
account = "+36201234567"
```

Press `?` in the client for key bindings (vim-style: `j`/`k`, `Enter`, `/`, `i`, `Esc`, `q`).

## Features

- Chats (1:1, group, meeting) and team channels, with unread markers
- Opens from a local SQLite cache instantly; the network never blocks the UI
- Send messages; ambiguous failures are reconciled, **never blindly resent**
- Load older messages, channel replies shown inline, conversation filter, configurable keys
- Tokens live in the OS keyring only (fails closed if none is available)
- One connection at a time (`provider = "teams"` or `"signal"`), each with its own accent colour

## Known limitations

- Not yet validated against a live Microsoft tenant.
- Reading **channel messages needs administrator consent** (`ChannelMessage.Read.All`). If your organisation blocks it, set `channels = "basic"` (list + post) or `"off"`.
- Polling only (no push without a server): chat list every 60 s, open conversation every 15 s.
- No editing, deleting, reactions, attachments download, presence or message search yet.
- Signal: text only (attachments show as `[attachment: …]`), no history before linking (Signal gives linked devices none), no reactions/edits/deletes, and it relies on the unofficial `signal-cli`. See [docs/signal.md](docs/signal.md).
- Windows and macOS keyring paths compile-gated but untested; Linux is the primary target.

## Layout

See [docs/architecture.md](docs/architecture.md). Research and decisions: [docs/phase-0-plan.md](docs/phase-0-plan.md). Development: [docs/development.md](docs/development.md).

License: Apache-2.0 (see `LICENSE`).
