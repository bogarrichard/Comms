# Architecture

```
apps/comms-tui ──► comms-bootstrap ──► comms-app ──► comms-storage (SQLite)
   (Ratatui)         (wiring)            │  ▲             │
                         │               │  └ events      ▼
                         │               ▼           comms-domain
                         ├──► comms-provider-api ◄── comms-testkit (fake provider / demo)
                         ├──► comms-provider-msgraph  (the only crate that knows Graph)
                         ├──► comms-provider-signal   (the only crate that knows signal-cli)
                         └──► comms-auth              (OAuth2 PKCE / device code, keyring)
```

* **comms-domain** – typed ids (UUIDv7 internally, opaque `RemoteId` from providers), `Conversation`, `Message`, structural `ContentBlock`s. No provider types.
* **comms-provider-api** – `MessagingProvider` trait, `Capabilities`, paging, `ProviderError` with `request_not_applied()` (is a retry provably safe?).
* **comms-provider-msgraph** – documented v1.0 delegated endpoints, HTML→`ContentBlock`, throttling (`Retry-After`, backoff), pagination links restricted to the Graph host.
* **comms-provider-signal** – talks JSON-RPC to a `signal-cli jsonRpc` child process (no Signal protocol code or AGPL dependency is linked in). Signal pushes messages and keeps no history for linked devices, while the core polls, so the crate buffers what it receives (`inbox.rs`, also appended to a private file) and serves it through `get_messages`. A `send` that gets an error answer is `failed` (safe to retry); no answer is `uncertain`.
* **comms-storage** – schema/migrations, duplicate prevention by `(conversation, remote_id)`, edit/delete updates, adoption of locally-sent messages whose outcome was uncertain.
* **comms-app** – the engine. Frontends send `Command`s, subscribe to `AppEvent`s, and read from the local cache via `AppHandle` queries. Polling, pagination/backfill and the send state machine live here.
* **comms-bootstrap** – settings, paths, builds Graph or demo backends.
* **comms-tui** – rendering and keys only. `theme.rs` gives each backend an accent colour (Signal blue, Teams purple) chosen from `AppHandle::provider_kind()`.

## Sharing the core between Ratatui and Slint

Frontends only see `AppHandle`: `send(Command)`, `subscribe() -> Receiver<AppEvent>`, async cache queries. The TUI runs on one Tokio runtime and `select!`s over terminal events, core events and a tick. A future Slint GUI keeps Slint's loop on the main thread, runs the core on a background Tokio runtime, and posts query results back with `invoke_from_event_loop`; callbacks only `send(Command)`. Events are invalidations; data is re-read from SQLite, so both UIs render identical state.

## Send safety

`pending → sent | failed | uncertain`. `failed` means the provider provably did not apply it (4xx, throttled, connect error). Anything ambiguous (timeout, 5xx, garbled success body) is `uncertain`: the engine re-syncs the conversation and *adopts* a matching server message instead of resending. Resending is a manual action (`R`).

## Deviations from SPEC.md

| Spec | Done | Why |
|---|---|---|
| `comms-sync` crate | sync lives in `comms-app::engine` | too small to justify a crate yet |
| `migrations/` at repo root | `crates/comms-storage/migrations/` | `sqlx::migrate!` embeds relative to the crate |
| extra crates | `comms-bootstrap`, `comms-testkit` | composition root; shared test doubles |
| `apps/comms-gui` | not created | Slint licensing undecided (GPL / royalty-free / commercial) |
| FTS5 message search | not yet | out of the minimal scope; schema allows adding it |
| `pending_operations` table | `messages.send_state` | one outbox kind so far |
| Ctrl+Enter to send | Enter sends, Alt+Enter/Ctrl+J newline | terminals rarely report Ctrl+Enter |
| PKCE browser flow *and* device code | both | device code can be blocked by Conditional Access |
