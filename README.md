<p align="center">
  <img src="docs/_static/logo.png" alt="Comms logo" width="128">
</p>

# Comms

A fast, keyboard-driven terminal client for **Microsoft Teams**, **Signal** and **Slack**, built in Rust on a provider-independent core. Each connection has its own colour: Teams is purple, Signal is blue, Slack is green.

> [!WARNING]
> **Early software.** Teams and Signal have only been tested against mocks and the demo backend, not live services. Slack has been tried on a real workspace. See the status of each app below.

Documentation: <https://bogarrichard.github.io/Comms/> (sources in [`docs/`](docs/), built with Sphinx).

## Install

```bash
curl --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/bogarrichard/Comms/main/scripts/install.sh | sh
```

Prebuilt binaries for Linux x86_64 and macOS Apple silicon, SHA-256 verified, installed to `~/.local/bin`. Windows, pinned versions and source builds: [docs/getting-started.md](docs/getting-started.md).

## Try it without an account

```bash
comms --demo          # fake chats and channels
comms --demo-slack    # the Slack-style layout with threads
```

Press `?` in the client for the key bindings (vim-style: `j`/`k`, `Enter`, `/`, `i`, `Esc`, `q`).

## Apps

| App | Status | Quick start | Docs |
|---|---|---|---|
| **Slack** (green) | Early, tried on a real workspace | `comms slack-login`, then `comms` | [docs/slack.md](docs/slack.md) |
| **Microsoft Teams** (purple) | Experimental, not yet run against a live tenant | register an Entra app, `comms login`, `comms` | [docs/teams.md](docs/teams.md) |
| **Signal** (blue) | Experimental, text only, not yet run against a live `signal-cli` | `comms signal-link`, then `comms` | [docs/signal.md](docs/signal.md) |

One app at a time: choose it with `provider = "teams" | "signal" | "slack"` in `~/.config/comms/config.toml` (`comms paths` shows where) or with `COMMS_PROVIDER`.

## Features

- Opens instantly from a local SQLite cache; the network never blocks the interface.
- Send messages. Ambiguous failures are reconciled and **never blindly resent**.
- Chats, channels and threads, unread markers, a conversation filter, configurable keys.
- Polling, with no server of ours in between: your credentials go only to the service you chose.

## More

[Architecture](docs/architecture.md), [development](docs/development.md) and the [research and decisions](docs/phase-0-plan.md).

License: Apache-2.0 (see `LICENSE`).
