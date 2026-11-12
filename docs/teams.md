# Using Comms with Microsoft Teams

:::{warning}
**Status: experimental.** The Teams support works end to end against mocked Microsoft Graph responses and the built-in demo backend. It has **not yet been run against a live tenant**; your first sign-in is the first live test.
:::

Comms reads and sends your Teams **chats** (1:1, group, meeting) and **team channels** through Microsoft Graph, signed in as you with delegated permissions. Teams is purple in the interface.

## Set up

1. Register an Entra app once (about five minutes): {doc}`setup-entra`.
2. Run the setup questionnaire. It creates or updates `~/.config/comms/config.toml` and can start the browser sign-in at the end:

   ```bash
   python3 scripts/configure.py
   ```

   Or edit the file by hand: `client_id = "00000000-0000-0000-0000-000000000000"` (also settable with `COMMS_CLIENT_ID`).
3. Sign in: `comms login` (browser) or `comms login --device-code` (over SSH, without a browser).
4. Start the client: `comms`.

Tokens live in the operating system's keyring only. If no keyring is available, Comms fails closed instead of writing a token to disk.

## Features

- Chats and team channels, with unread markers.
- Opens instantly from the local SQLite cache; the network never blocks the interface.
- Send messages. Ambiguous failures are reconciled, **never blindly resent**.
- Load older messages, channel replies shown inline, a conversation filter and configurable keys.

## Limitations

- Not yet validated against a live Microsoft tenant.
- Reading **channel messages needs administrator consent** (`ChannelMessage.Read.All`). If your organisation blocks it, set `channels = "basic"` (list and post) or `"off"`, or `COMMS_CHANNELS`.
- Polling only, since there is no push without a server: the chat list every 60 s and the open conversation every 15 s (see `[poll]` in the config).
- No editing, deleting, reactions, attachment download, presence or message search yet.
- The Windows and macOS keyring paths are compile-gated but untested; Linux is the primary target.

More: {doc}`microsoft-graph` lists every Graph request and permission Comms uses.

---

Microsoft, Microsoft Teams, Microsoft Entra and Microsoft Graph are trademarks of the Microsoft group of companies. Comms is not affiliated with or endorsed by Microsoft. See {doc}`trademarks`.
