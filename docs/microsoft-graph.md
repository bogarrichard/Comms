# Microsoft Graph usage

Verified against the Microsoft Graph documentation source (2026-11-04); not yet exercised against a live tenant. Details and rationale: [phase-0-plan.md](phase-0-plan.md) §2.

| Purpose | Request | Delegated permission |
|---|---|---|
| Me | `GET /me` | `User.Read` |
| Chats | `GET /me/chats?$top=50&$expand=members,lastMessagePreview&$orderby=lastMessagePreview/createdDateTime desc` (falls back without `$orderby`) | `Chat.ReadBasic` |
| Chat messages | `GET /chats/{id}/messages?$top=50` | `Chat.Read` |
| Send (chat) | `POST /chats/{id}/messages` (HTML body, escaped) | `ChatMessage.Send` |
| Teams | `GET /me/joinedTeams` | `Team.ReadBasic.All` |
| Channels | `GET /teams/{id}/channels` | `Channel.ReadBasic.All` |
| Channel messages | `GET /teams/{t}/channels/{c}/messages?$top=50&$expand=replies` (falls back without `$expand`) | `ChannelMessage.Read.All` (**admin consent**) |
| Send (channel) | `POST /teams/{t}/channels/{c}/messages` | `ChannelMessage.Send` |

Not used (unavailable to delegated desktop clients): `getAllMessages`, chat-message `delta`, tenant-wide change notifications. Real-time push needs a public webhook; Comms polls.

Throttling: `429` → wait `Retry-After` (capped, bounded attempts); reads also retry `503/504`; **sends are never retried after they may have been accepted**. Teams limits worth knowing: `GET /me/chats` 1 rps per user; `GET .../messages` 1 rps per chat; ~20 rps per app per tenant shared by all users of one client ID.

Local identifiers: chats use the Graph chat id; channels use `ch/{teamId}/{channelId}` (opaque outside the adapter).
