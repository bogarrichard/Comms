# Comms – Pre-implementation review and Phase 0 plan

Status: **for review** – nothing below has been implemented yet.
Date of research: 2026-11-04.

This document covers:

1. Current repository state vs. SPEC.md
2. Verified Microsoft Graph capabilities for Phase 1
3. Architectural risks and open questions
4. Proposed Cargo workspace structure
5. How the TUI (Ratatui) and GUI (Slint) share the application core
6. Dependencies, versions and licenses
7. Phase 0 implementation plan
8. Phase 1 outline (full plan to follow after Phase 0)
9. Decisions requested from the project owner

---

## 1. Repository state

| Item | State |
|---|---|
| Commits | 1 (`Initial commit`) |
| Files | `README.md` (title only), `LICENSE` (**Apache-2.0**) |
| Rust code / workspace | None |
| CI | None |
| Toolchain in dev container | rustc / cargo 1.97.0 (stable) |

Everything in the spec is still to be built. One discrepancy: the spec says the
project license is "to be decided separately", but the repository already
contains an Apache-2.0 `LICENSE`. See decision D1.

---

## 2. Microsoft Graph capabilities relevant to Phase 1 (verified)

Source: the `microsoftgraph/microsoft-graph-docs-contrib` repository (the source
of learn.microsoft.com/graph), `main` branch, read on 2026-11-04. Permission
tables come from the generated `includes/permissions/*.md` files and
`concepts/permissions-reference.md`. Throttling numbers come from
`includes/throttling-teams.md`.

**All Teams chat APIs require a work or school account. Personal Microsoft
accounts are "Not supported" for every chat endpoint below.**

### 2.1 Endpoints (v1.0, delegated)

| Purpose | Request | Least-privileged delegated permission | Notes |
|---|---|---|---|
| Current user | `GET /me` | `User.Read` | |
| List chats | `GET /me/chats` | `Chat.ReadBasic` (`Chat.Read`, `Chat.ReadWrite` higher) | `$top` max 50. `$expand=members,lastMessagePreview`. `$orderby=lastMessagePreview/createdDateTime desc` (descending only). Follow `@odata.nextLink`. `$expand=members` returns at most **25** members per chat. |
| Get chat | `GET /chats/{chat-id}` | `Chat.ReadBasic` | No OData query parameters. |
| Unread state | `chat.viewpoint.lastMessageReadDateTime` | (same as list chats) | Only populated in delegated context. Compare it with `lastMessagePreview.createdDateTime` to get an unread indicator. |
| Chat members | `GET /chats/{chat-id}/members` | `Chat.ReadBasic` | No OData query parameters. |
| List messages | `GET /chats/{chat-id}/messages` | `Chat.Read` | `$top` max 50. `$orderby` `lastModifiedDateTime` (default) or `createdDateTime`, **descending only**. `$filter`: `lastModifiedDateTime gt/lt`, `createdDateTime lt`. The filter works only together with `$orderby` on the same property. |
| Send message | `POST /chats/{chat-id}/messages` | `ChatMessage.Send` (`Chat.ReadWrite` higher) | Returns `201` with the created message. **No documented idempotency key.** |
| Edit message | `PATCH /chats/{chat-id}/messages/{message-id}` | `Chat.ReadWrite` | Delegated: any property except `policyViolation` can be changed; returns `204`. Fails if a participant uses Skype. No federation. |
| Delete message | `POST /users/{user-id}/chats/{chat-id}/messages/{message-id}/softDelete` | `Chat.ReadWrite` | `undoSoftDelete` also exists. Application permissions not supported. |
| Mark read (Phase 2) | `POST /chats/{chat-id}/markChatReadForUser` | `Chat.ReadWrite` | |

`AdminConsentRequired = No` for `User.Read`, `offline_access`, `Chat.ReadBasic`,
`Chat.Read`, `ChatMessage.Send` and `Chat.ReadWrite`. Tenants can still
restrict user consent, though (see R2).

### 2.2 APIs that are NOT available to a delegated desktop client

| API | Why it matters | Status |
|---|---|---|
| `GET /users/{id}/chats/getAllMessages` | Cross-chat message export | **Application permissions only** (and a metered/licensed API) |
| `GET /users/{id}/chats/getAllMessages/delta` | Delta sync across chats | **Application permissions only**; only covers the last 8 months |
| Tenant-wide chat/channel change notifications | Real-time push | Application permissions only |

**Consequence for sync.** Incremental sync has to run per chat:

1. `GET /me/chats?$expand=lastMessagePreview&$orderby=lastMessagePreview/createdDateTime desc`
   shows which chats have new activity.
2. For each changed chat (and the open chat):
   `GET /chats/{id}/messages?$orderby=lastModifiedDateTime desc&$filter=lastModifiedDateTime gt {watermark}`
   returns new **and** edited/deleted messages since the stored per-chat watermark.

Still to verify against a live tenant: whether reactions and soft-deletes
always bump `lastModifiedDateTime`. The docs suggest it but don't state it.
This is the only "incremental synchronization where supported" mechanism
available, so the spec requirement is met per chat, not globally.

### 2.3 Change notifications (real-time)

Delegated subscriptions exist for:

- `/chats/{chat-id}/messages` with `Chat.Read`
- `/users/{id}/chats/getAllMessages` (user-level) with `Chat.Read` / `Chat.ReadWrite`

Both require a **publicly reachable HTTPS `notificationUrl`** (a webhook).
Subscriptions longer than 1 hour also need a `lifecycleNotificationUrl`, and
resource data needs an encryption certificate. A desktop client can't host
that endpoint, so real-time push needs a separately deployed relay service.
**Phase 1–2 use polling.** Notifications are a later, separate decision (see R4).

### 2.4 Throttling (Teams-specific limits)

| Request | Per app per tenant | Per resource | Per user |
|---|---|---|---|
| `GET /me/chats` | 20 rps | – | **1 rps** |
| `GET /chats/{id}` | 200 rps | 1 rps per chat | 5 rps |
| `GET /chats/{id}/messages` | 20 rps | **1 rps per chat** | – |
| `POST /chats/{id}/messages` | 20 rps | 1 rps per chat | 1 rps |
| `PATCH` / `softDelete` message | 30 rps | 1 rps per chat | – |

Throttled responses are `429` with `Retry-After` (seconds). If `Retry-After`
is missing, Microsoft recommends exponential backoff. The **per-app-per-tenant**
budget is shared by every user of the same client ID in the same tenant (see R3).

### 2.5 Authentication (Microsoft identity platform v2)

- Public client registration. No secret.
- **Authorization code + PKCE** using a loopback redirect `http://localhost`.
  For localhost redirect URIs Entra ignores the port, so an ephemeral port is
  fine. Register exactly one localhost redirect URI.
- **Device code flow**: the user has 15 minutes (`expires_in`). Poll at
  `interval`, handle `authorization_pending` and `expired_token`. Requires
  "Allow public client flows" on the app registration.
- Refresh tokens have no fixed lifetime, can be revoked, and must be replaced
  by the new refresh token on every refresh. `offline_access` is needed to get one.

### 2.6 Message content model (relevant to `ContentBlock`)

`chatMessage` fields include: `id`, `etag`, `messageType` (message vs.
`systemEventMessage`), `createdDateTime`, `lastModifiedDateTime`,
`lastEditedDateTime`, `deletedDateTime`, `from`, `body { contentType: text|html, content }`,
`attachments`, `mentions`, `reactions`, `replyToId`, `subject`, `importance`,
`eventDetail`, `messageHistory`, `webUrl`.

The body is HTML in most real messages (mentions are `<at id="n">`, inline
images reference `hostedContents`, emoji, code blocks, quotes). The Graph
adapter needs an HTML → `ContentBlock` converter and a `ContentBlock` → HTML
serializer for sending.

---

## 3. Architectural risks and open questions

Each item has a recommendation. Items that need approval are repeated in §9.

| # | Risk / question | Impact | Recommendation |
|---|---|---|---|
| R1 | **Slint licensing.** Slint is `GPL-3.0-only OR Slint Royalty-free 2.0 OR Slint commercial`. None of these is a permissive license. | The GUI binary would be GPL-3.0, or use the royalty-free license, which has attribution conditions for desktop apps. This conflicts with "prefer permissive dependencies". | Don't add Slint in Phase 0. `comms-gui` is a stub until D2 is decided. The core crates stay permissive, so the TUI is unaffected. |
| R2 | **Entra app registration and consent.** Someone has to own a multi-tenant public client ID. Many enterprise tenants disable user consent, or block consent to apps from unverified publishers. | Users may be unable to sign in without admin approval, even though the permissions don't need admin consent. | Ship with a configurable `client_id` and `tenant`, so a developer or an organisation can use its own registration. Document how to register one. Publisher verification is a later decision (D3). |
| R3 | **Polling vs. throttling.** Delegated polling is the only sync path. `GET /me/chats` is limited to 1 rps per user. The 20 rps per-app-per-tenant budget is shared across all users of the client ID. | Aggressive polling across many users in one tenant will cause 429s. | A central rate limiter in the Graph adapter (token bucket per endpoint class). Adaptive polling: the open chat often (about 10–15 s), the chat list less often (about 30–60 s), background chats only when the chat list shows activity. Back off on 429. All intervals configurable. |
| R4 | **No real-time push without a server.** | Latency equals the poll interval. Typing indicators and presence push aren't feasible. | Accept for Phase 1–2. Expose `Capability::RealtimeEvents = false`. A relay service (possibly the future EU backend) is a separate, approval-gated project. |
| R5 | **Send idempotency.** Graph has no client-supplied message ID or idempotency key for `POST /chats/{id}/messages`. | A timeout after the server accepted the message could cause a duplicate if retried blindly. | Outbox state machine: `Queued → InFlight → Sent / Failed / Uncertain`. Retry automatically **only** when the request provably never reached the server (connection refused, DNS or TLS failure before sending) or got a 429/503 with `Retry-After`. On a timeout or reset after sending, mark it `Uncertain` and reconcile: fetch the latest messages and match on sender = me, normalized content hash and a time window. If there's no match, surface it to the user ("may not have been sent – retry?"). Never resend automatically. |
| R6 | **HTML content fidelity.** Teams HTML is rich (cards, hosted images, mentions, quotes, code). | Converter scope could grow without limit. | A small structural block set (`Paragraph`, `Text{style}`, `Mention`, `Link`, `Code`, `CodeBlock`, `Quote`, `List`, `Emoji`, `Image(ref)`, `Unsupported{kind, fallback_text}`). Unknown markup degrades to text. Keep the original HTML in `provider_metadata` only so it can be re-rendered later. Fixture-driven tests. |
| R7 | **Token storage portability.** Linux Secret Service needs a running keyring daemon (often missing in headless/SSH/WSL sessions). Windows Credential Manager limits the blob size (about 2.5 KB), and Entra refresh tokens can approach that. | Login may fail, or tokens may not persist, on some setups. | Use `keyring-core` with the Secret Service store as the Linux default. If no secure store is available, **fail closed** with a clear message (no plaintext fallback, per the spec). An optional `keyutils` session-only store is a candidate (D5). Verify the Windows size limit in Phase 2 and chunk if needed. |
| R8 | **SQLx specifics.** Compile-time-checked macros need `DATABASE_URL` or a committed `.sqlx` cache. The `sqlite` feature also enables `load-extension`. | CI friction; extension loading is unnecessary attack surface. | Use `sqlite-bundled` only (bundled SQLite has FTS5 compiled in, verified in `libsqlite3-sys` `build.rs`). Use runtime `query`/`query_as` with integration tests at first, and adopt `.sqlx` offline macros once the schema settles. Storage is behind `comms-storage` APIs, so swapping to rusqlite later would stay contained. |
| R9 | **TLS backend.** `reqwest` 0.13 defaults to rustls with `aws-lc-rs` (a C/CMake build; ISC/Apache/MIT/BSD-3 mix) and `rustls-platform-verifier`, which uses the OS trust store. | The OS trust store matters for corporate TLS-inspecting proxies, which is good. aws-lc adds a build dependency (cmake). | Keep the defaults (platform verifier). Let `cargo-deny` confirm the license mix. Fall back to the `ring` provider if aws-lc causes build problems on Windows/macOS. |
| R10 | **Async trait object safety.** Native `async fn` in traits isn't `dyn`-compatible, but providers are chosen at runtime (`Arc<dyn MessagingProvider>`). | | Use `async-trait` for the provider trait. The boxing cost is negligible next to network I/O. |
| R11 | **Composition root.** Something has to construct the Graph provider without frontends depending on it. | Risk of Graph types leaking into frontends, or duplicated wiring. | A small `comms-bootstrap` crate builds config, storage, auth, the provider registry and the `AppHandle`. Both apps call it. It is the only crate besides the adapter that names `comms-provider-msgraph`. |
| R12 | **Incremental-sync assumptions** (§2.2). | Edits/deletes/reactions could be missed if `lastModifiedDateTime` isn't bumped. | Add an optional live integration test early in Phase 1. Also do a periodic "recent window" re-fetch of the open chat (last 50 messages) as a safety net. |
| R13 | **Channels need admin consent.** Reading channel messages delegated needs `ChannelMessage.Read.All` (**admin consent required**). | Phase 4 channel support may be unavailable in many tenants. | Document it. Keep channels behind `Capability::Channels`, enabled only when consent was granted. |
| R14 | **Identifier strategy.** | | Internal IDs: UUIDv7 newtypes (`ConversationId(Uuid)` etc.) are time-ordered, generated locally (so an outbox message has an ID before the server knows it), and stable across re-sync. Provider IDs: `RemoteId(String)` and `ProviderKind` stored alongside, with `UNIQUE(account_id, remote_id)` for dedupe. |
| R15 | **Migrations location.** The spec puts `migrations/` at the repository root. | `sqlx::migrate!` embeds files relative to the crate; a root folder breaks `cargo package`. | Deviation: `crates/comms-storage/migrations/` (D6). |

---

## 4. Proposed Cargo workspace

```
comms/
├── Cargo.toml                  # [workspace], shared [workspace.dependencies], lints
├── rust-toolchain.toml         # channel = "stable", components rustfmt + clippy
├── rustfmt.toml
├── clippy.toml
├── deny.toml                   # cargo-deny: license allow-list, advisories, bans
├── .github/workflows/ci.yml
├── crates/
│   ├── comms-domain/           # pure types: ids, entities, ContentBlock, errors. deps: serde, chrono, uuid, thiserror
│   ├── comms-provider-api/     # MessagingProvider trait, Capabilities, Page<T>/PageCursor, ProviderError
│   ├── comms-provider-msgraph/ # Graph DTOs (private), mapping, HTML<->ContentBlock, rate limiter, retry
│   ├── comms-auth/             # generic OAuth2/OIDC PKCE + device code, TokenStore trait, keyring store
│   ├── comms-storage/          # SQLx/SQLite, migrations/, repositories, FTS5 search, outbox tables
│   │   └── migrations/
│   ├── comms-sync/             # sync engine: schedulers, watermarks, outbox processor, reconciliation
│   ├── comms-app/              # Command/Event API, AppHandle, query facade, view-models, config types
│   ├── comms-bootstrap/        # composition root (config → storage → auth → providers → AppHandle)
│   └── comms-testkit/          # FakeProvider, fixtures, temp DB helpers (dev-dependency only)
├── apps/
│   ├── comms-tui/              # Ratatui + Crossterm binary
│   └── comms-gui/              # Slint binary (stub until licensing decision D2)
└── docs/
    ├── architecture.md
    ├── provider-api.md
    ├── microsoft-graph.md
    ├── development.md
    └── phase-0-plan.md         # this file
```

Additions to the spec: `comms-bootstrap` (R11) and `comms-testkit` (shared test
doubles). Moved: `migrations/` (R15).

Dependency rules, enforced in CI with a small `cargo metadata` check script:

```
comms-domain            → (no internal deps)
comms-provider-api      → domain
comms-provider-msgraph  → provider-api, domain, auth
comms-auth              → (no internal deps; provider supplies endpoints/scopes)
comms-storage           → domain
comms-sync              → provider-api, storage, domain
comms-app               → sync, storage, provider-api, domain
comms-bootstrap         → app, storage, auth, provider-msgraph
comms-tui / comms-gui   → bootstrap, app, domain   (never provider-msgraph, never storage directly)
```

Workspace settings: `edition = "2024"`, `rust-version = "1.94"` (the strictest
dependency is sqlx 0.9; Slint needs 1.92; ratatui 0.30 needs 1.88),
`resolver = "3"`. Workspace lints: `unsafe_code = "forbid"`,
`clippy::all = warn` (CI denies warnings), `unwrap_used`/`expect_used` warn
outside tests.

---

## 5. How Ratatui and Slint share the application core

The core is a **runtime-agnostic-to-UI service** running on Tokio. Frontends
talk to it through three things only:

```rust
// comms-app
pub struct AppHandle {
    commands: mpsc::Sender<Command>,         // fire-and-forget intents
    events:   broadcast::Sender<AppEvent>,   // subscribe() → receiver per frontend
    queries:  Queries,                       // async read-only access to the local DB read model
}

pub enum Command { LoadConversations, OpenConversation(ConversationId), LoadOlderMessages{..},
                   SendMessage{..}, EditMessage{..}, DeleteMessage{..}, SearchMessages{..},
                   RefreshConversations, Synchronize, ... }

pub enum AppEvent { ConversationsUpdated, MessagesUpdated{conversation, range},
                    MessageSendStarted{local_id}, MessageSendSucceeded{..}, MessageSendFailed{..},
                    SyncProgress{..}, SyncCompleted{..}, ConnectionStatusChanged(..), Error(..) }
```

Principles:

- **Local-first read path.** Events are mostly invalidations ("conversation X
  changed"). The frontend then calls `queries.messages(conv, window)`, which
  reads SQLite. That's indexed, local and around a millisecond. SQLite stays
  the single source of truth for the UI, events stay small, and both frontends
  render identical data.
- **View-models live in `comms-app`.** Types like `ConversationSummary` (title,
  unread, last-activity, preview text) and `MessageView` (sender display name,
  rendered `ContentBlock`s, edit/delete flags, send state) are computed once in
  the core. Frontends only lay them out.
- **UI state lives in the frontend.** Selection, scroll offset, focus, input
  buffer and modal stack belong to each frontend, and nothing in `comms-app`
  depends on `ratatui` or `slint`.
- **Capabilities flow through.** `queries.capabilities(account)` lets both UIs
  hide edit/delete/reactions when a provider doesn't support them. No
  provider-name checks anywhere.

**TUI integration.** Everything runs on one Tokio runtime. The main loop is a
`tokio::select!` over the `crossterm::event::EventStream`, the `AppEvent`
receiver, and a render tick. Key events map through a configurable keymap to
TUI actions, which either change local UI state or send a `Command`. Rendering
is synchronous `terminal.draw(...)` from in-memory view state. It never awaits
the network, and only awaits local queries, which are kicked off and stored
when their results arrive.

**Slint integration.** Slint's event loop must own the main thread, so Tokio
runs on a background multi-thread runtime. Slint callbacks such as
`on_send_clicked` only do `handle.send(Command::SendMessage{..})`, with no
logic (the spec requirement). A background task subscribes to `AppEvent`s,
runs the query, and pushes results into the UI with
`slint::Weak::upgrade_in_event_loop` (or `invoke_from_event_loop`). The message
list is a custom `slint::Model` backed by a windowed cache that pages through
`queries.messages(...)`. Slint's `ListView` only instantiates visible rows,
which gives virtualization.

Because both frontends see only `AppHandle`, Phase 3 adds a GUI without touching
domain, providers, storage or sync. The `comms-app` tests drive the same
`AppHandle` against `comms-testkit::FakeProvider`.

---

## 6. Dependencies, versions and licenses

Queried from crates.io on 2026-11-04 (newest stable release; versions are
pinned in `Cargo.lock` at implementation time, and none released within the
last ~2 weeks are chosen without reason).

| Purpose | Crate | Version | MSRV | License | Phase |
|---|---|---|---|---|---|
| Async runtime | tokio | 1.54 | 1.85 | MIT | 0 |
| | tokio-util | 0.7 | 1.85 | MIT | 1 |
| Errors | thiserror | 2.0 | 1.77 | MIT OR Apache-2.0 | 0 |
| | anyhow (binaries only) | 1.0 | 1.68 | MIT OR Apache-2.0 | 0 |
| Serialization | serde / serde_json | 1.0 | – | MIT OR Apache-2.0 | 0 |
| Time | chrono | 0.4.45 | 1.62 | MIT OR Apache-2.0 | 0 |
| IDs | uuid (v7, serde) | 1.29 | 1.89 | Apache-2.0 OR MIT | 0 |
| Async trait | async-trait | 0.1 | 1.71 | MIT OR Apache-2.0 | 0 |
| Logging | tracing / tracing-subscriber | 0.1.44 / 0.3.23 | 1.65 | MIT | 0 |
| Config | toml | 1.x (≥2 weeks old) | 1.85 | MIT OR Apache-2.0 | 1 |
| Paths | directories | 6.0 | – | MIT OR Apache-2.0 | 1 |
| HTTP | reqwest (rustls, json) | 0.13 | 1.85 | MIT OR Apache-2.0 | 1 |
| TLS (transitive) | rustls-platform-verifier / aws-lc-rs | 0.7 / 1.19 | | MIT/Apache; ISC AND (Apache-2.0 OR ISC) | 1 |
| URL | url | 2.5 | 1.63 | MIT OR Apache-2.0 | 1 |
| OAuth | oauth2 | 5.0 | 1.65 | MIT OR Apache-2.0 | 1 – note: its built-in reqwest integration targets reqwest 0.12. We'd implement its `AsyncHttpClient` trait over reqwest 0.13, about 30 lines. |
| Browser launch | open | 5.4 | 1.62 | MIT | 1 |
| Secret storage | keyring-core + zbus-secret-service-keyring-store (Linux), windows-native-, apple-native-keyring-store | 1.x | | MIT OR Apache-2.0 | 1 – `keyring` v4 is now a facade/CLI over these. |
| Secrets in memory | secrecy | 0.10 | 1.60 | Apache-2.0 OR MIT | 1 |
| Database | sqlx (`sqlite-bundled`, `runtime-tokio`, `chrono`, `uuid`, `migrate`) | 0.9.0 | **1.94** | MIT OR Apache-2.0 | 1 |
| SQLite (transitive) | libsqlite3-sys (bundled; FTS5 on) | 0.37 | | MIT (SQLite itself: public domain) | 1 |
| HTML parsing | html5ever (or scraper, ISC) | 0.40 | 1.85 | MIT OR Apache-2.0 | 1 |
| TUI | ratatui (+ ratatui-crossterm) | 0.30 | 1.88 | MIT | 1 |
| Terminal | crossterm (`event-stream`) | 0.29 | 1.63 | MIT | 1 |
| Text input | tui-textarea | 0.7 | | MIT | 1 (optional; may hand-roll – last release 2024, check compatibility with ratatui 0.30) |
| Rate limiting | governor | 0.10 | | MIT | 1 (or a small in-house token bucket – decide in Phase 1) |
| GUI | slint / slint-build | 1.18 | 1.92 | **GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0** | 3 – **needs decision D2** |
| Test: HTTP mocks | wiremock | 0.6 | | MIT OR Apache-2.0 | 1 |
| Test: snapshots | insta | 1.49 | 1.66 | Apache-2.0 | 1 (TUI rendering via ratatui `TestBackend`) |
| Test: temp files | tempfile | 3.27 | 1.63 | MIT OR Apache-2.0 | 1 |
| CI tool | cargo-deny | 0.20 | | MIT OR Apache-2.0 | 0 |

Phase 0 only pulls in tokio (for test macros / trait signatures), serde,
chrono, uuid, thiserror, async-trait and tracing. Everything else comes in
when it's first used.

**`deny.toml` license allow-list (proposed):** MIT, Apache-2.0,
Apache-2.0 WITH LLVM-exception, BSD-2-Clause, BSD-3-Clause, ISC, Zlib,
Unicode-3.0, MIT-0, CC0-1.0, OpenSSL (only if aws-lc requires it). GPL and the
Slint licenses are **not** allowed workspace-wide. If D2 approves Slint, add a
targeted exception for `comms-gui` only.

---

## 7. Phase 0 implementation plan

Goal: a buildable, tested, documented skeleton with the domain model and
provider API in place, and CI green. Each step is one small commit.

**0.1 Workspace scaffolding**

- Root `Cargo.toml`: `[workspace]` with all members listed in §4,
  `[workspace.package]` (edition 2024, rust-version 1.94, license per D1),
  `[workspace.dependencies]`, `[workspace.lints]`.
- `rust-toolchain.toml`, `rustfmt.toml`, `clippy.toml`, `.gitignore`, `.editorconfig`.
- Empty `lib.rs` for each crate. `main.rs` for each app prints the version and exits.
- Check: `cargo build --workspace` passes.

**0.2 `comms-domain`**

- Strongly typed IDs: `AccountId`, `ConversationId`, `MessageId`, `ParticipantId`,
  `AttachmentId` (UUIDv7 newtypes, `Display`/`FromStr`/serde); `RemoteId(String)`;
  `ProviderKind(&'static str-like newtype)`; `ProviderRef { kind, remote_id }`.
- Entities: `Account`, `Conversation` (`ConversationKind::{Direct, Group, Channel{..}, Meeting, Other}`),
  `Participant`, `Message` (adds `remote: Option<ProviderRef>`, `deleted_at`,
  `send_state`, `reply_to`), `Attachment`, `Reaction`, `Presence`, `SyncState`,
  `PageCursor` (opaque, provider-owned string).
- `ContentBlock` / `Inline` structural rich text (R6) plus `to_plain_text()`
  for search indexing and previews.
- `DomainError` (thiserror).
- Tests: ID round-trips, serde round-trips, `to_plain_text` cases, invariants
  (for example, a deleted message has no editable content).

**0.3 `comms-provider-api`**

- `Capabilities` as a `bitflags`-like struct of bools (no extra dependency):
  direct, group, channels, edit, delete, reactions, attachments, presence,
  typing, search, threads, realtime_events.
- `#[async_trait] trait MessagingProvider: Send + Sync` with `capabilities()`,
  `list_conversations(PageRequest)`, `get_conversation`, `get_messages(conv, MessageQuery{cursor, since, limit})`,
  `send_message(conv, OutgoingMessage)`, `edit_message`, `delete_message`,
  `search_messages`, `list_participants`, `current_account`.
- `Page<T> { items, next: Option<PageCursor> }`.
- `ProviderError` (thiserror): `Unsupported(Capability)`, `Unauthorized`,
  `Forbidden{missing_permission: Option<String>}`, `NotFound`,
  `RateLimited{retry_after}`, `Network{phase: BeforeSend|AfterSend}` (R5),
  `InvalidResponse`, `Server{status}`, `Other`. Plus `ProviderError::is_retry_safe()`.
- Default trait methods return `Unsupported` for optional capabilities.
- Tests: default methods return the right `Unsupported` variant; retry-safety classification.

**0.4 `comms-testkit`**

- An in-memory `FakeProvider` implementing the trait, with scripted failures
  (rate-limit, network after send) and pagination over fixtures. Used by later
  sync and app tests. Tests: pagination yields all items exactly once.

**0.5 Stub crates**

- `comms-auth`, `comms-storage`, `comms-sync`, `comms-app`, `comms-bootstrap`,
  `comms-provider-msgraph` get module docs that state their responsibility and
  allowed dependencies, plus one trivial test each so "each crate has unit
  tests" holds from day one.
- The `comms-provider-msgraph` stub holds no Graph code yet, just
  `capabilities()` reflecting §2 (edit/delete true, realtime false, channels
  false, search false since local search is used).

**0.6 Tooling and CI**

- `deny.toml` (licenses, advisories, bans on duplicate major versions as warnings).
- `scripts/check-deps.sh` (or a small `xtask`) to enforce the dependency rules in §4.
- `.github/workflows/ci.yml` on `ubuntu-latest`: `cargo fmt --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`, `cargo deny check`, dependency-rule check.
  A second job builds and tests `windows-latest` and `macos-latest` for
  non-GUI crates, to catch Linux-only assumptions early.
- `Swatinem/rust-cache` for build caching.

**0.7 Documentation**

- `README.md`: purpose, status, build instructions.
- `docs/architecture.md`: layers, dependency rules, command/event model (§5), local-first read path.
- `docs/provider-api.md`: trait contract, pagination, error semantics, capability rules, retry safety.
- `docs/microsoft-graph.md`: §2 of this document, the permissions table, unsupported features, sync strategy, throttling.
- `docs/development.md`: toolchain, commands, CI, how to run optional integration tests (Phase 1).
- A deviations log section (R11, R15, and anything else that comes up).

**Phase 0 acceptance:** `cargo fmt --check`, `cargo clippy -D warnings`,
`cargo test --workspace` and `cargo deny check` all pass locally and in CI, and
the docs above exist. The end-of-phase report lists implemented functionality,
files, test results, limitations, remaining work and the recommended next step,
as required by the spec.

---

## 8. Phase 1 outline (a detailed plan follows Phase 0)

1. **Storage:** initial migration (accounts, conversations, participants,
   conversation_participants, messages, attachments, reactions, sync_state,
   pending_operations, `messages_fts` FTS5 external-content table with
   triggers). Upserts keyed on `(account_id, remote_id)`. DB file mode `0600`
   on Unix, in an XDG data dir.
2. **Auth:** PKCE loopback flow (default) and device code flow (`--device-code`
   flag for SSH/headless). Token store via keyring-core. Refresh with rotation.
   Logout deletes tokens and, optionally, the local cache.
3. **Graph adapter:** HTTP client with rate limiter, `Retry-After`, typed
   errors and response validation. Endpoints from §2.1. HTML→ContentBlock.
   Wiremock tests for pagination, 429/503 retry and mapping.
4. **Sync:** chat-list poller, per-chat watermark sync, initial 100-message
   cache (2 × `$top=50`), load-older via `createdDateTime lt`, outbox processor
   (R5).
5. **App core + TUI:** commands and events, three-panel layout, keymap config,
   status line (connection/sync), error toasts, unread markers.
6. **Optional integration tests** (`--features live-graph`, env-provided client
   ID and tenant).

Phase 1 acceptance criterion from the spec: authenticate, open an existing
conversation, read cached messages, send a message.

---

## 9. Decisions requested

| ID | Decision | Recommendation |
|---|---|---|
| D1 | Project license: keep the existing Apache-2.0, or dual MIT OR Apache-2.0, or something else? | Dual **MIT OR Apache-2.0** (Rust ecosystem norm), or keep Apache-2.0 if preferred. |
| D2 | Slint licensing for the GUI (R1): accept GPL-3.0 for the GUI binary, accept the Slint Royalty-free license (attribution terms), or evaluate a permissive alternative (e.g. iced, egui) later? | Defer until Phase 3. No Slint dependency before then. |
| D3 | Who owns the Entra app registration (a shared multi-tenant client ID vs. a bring-your-own client ID)? Publisher verification? | A configurable client ID from day one. A shared registration only after a decision on ownership and verification. |
| D4 | Phase 1 delegated scopes: (a) `openid profile offline_access User.Read Chat.ReadBasic Chat.Read ChatMessage.Send` and request `Chat.ReadWrite` incrementally when the user first edits or deletes; or (b) `Chat.ReadWrite` up front. | **(a)**: least privilege, matching spec §7. |
| D5 | Token storage when no Secret Service is available: fail closed, or allow a kernel-keyutils session-only store (tokens lost on logout or reboot)? | Fail closed by default; keyutils as an explicit opt-in. |
| D6 | Move `migrations/` into `crates/comms-storage/migrations/` (R15)? | Yes. |
| D7 | Add the `comms-bootstrap` and `comms-testkit` crates (R11)? | Yes. |
| D8 | Real-time notifications via a relay service (R4): out of scope until after Phase 2? | Yes, out of scope. Polling only. |
