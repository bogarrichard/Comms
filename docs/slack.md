# Using Comms with Slack

:::{note}
**Status: early, tested by the author.** Listing conversations, reading and sending messages were verified on a real workspace. Not yet verified, or not implemented: the workspace icon in a real session, edit and delete on real Slack, thread replies (not loaded), unread markers and files (shown as `[file: name]`).
:::

Comms talks to the Slack Web API with a **user token** (`xoxp-...`), so it reads and posts as you. Slack has no "log in with a password" for third-party clients: every client needs a Slack *app*, and the app is what Slack issues the token for. **Each person creates their own app** from the manifest below (about two minutes) and installs it to their own account. That is the intended setup: the app is private to the workspace, it only ever holds your own token, and nobody else needs to approve or share anything but the install itself.

:::{note}
Slack throttles apps that are distributed commercially without Marketplace approval to **1 request per minute** for message history. An app that is created in, and only used by, your own workspace is an internal app and is not affected. Do not list the app in the Slack Marketplace or make it publicly distributable.
:::

**Early version.** Polling only (no live push yet), text with links, mentions, code and quotes; files show as `[file: name]`; thread replies are not loaded; conversations have no last-activity time or unread marker; bold/italic markers stay in the text. Edit and delete work on your own messages.

## 1. Create the app (once per workspace)

1. Open <https://api.slack.com/apps> and choose **Create New App → From scratch**. Pick a name (for example `Comms`) and your workspace.
   Faster: choose **From a manifest** instead (or **From scratch → Paste a manifest**), pick your workspace and paste this, which sets the name and every scope at once:

   ```json
   {
     "display_information": { "name": "Comms" },
     "oauth_config": {
       "redirect_urls": ["http://localhost:8765/callback"],
       "pkce_enabled": true,
       "scopes": {
         "user": [
           "channels:read", "groups:read", "im:read", "mpim:read",
           "channels:history", "groups:history", "im:history", "mpim:history",
           "users:read", "chat:write", "team:read"
         ]
       }
     },
     "settings": {
       "org_deploy_enabled": false,
       "socket_mode_enabled": false,
       "is_hosted": false,
       "token_rotation_enabled": false
     }
   }
   ```

   Then skip to step 3. Otherwise, continue by hand:
2. Under **OAuth & Permissions → User Token Scopes**, add:
   `channels:read`, `groups:read`, `im:read`, `mpim:read`, `channels:history`, `groups:history`, `im:history`, `mpim:history`, `users:read`, `chat:write`, `team:read` (the last one is only for the workspace icon; Comms works without it).
3. Click **Install to Workspace** (or **Request to install**, if your workspace needs an admin to approve it) and allow it.

## 2. Get your token

On **OAuth & Permissions**, copy the **User OAuth Token** (starts with `xoxp-`). It is a password for your Slack account, so do not share or commit it.

**Why not one shared app?** Comms has no sign-in page of its own (no OAuth redirect), so it needs the token that Slack shows on the app's settings page, and that page belongs to the people who manage the app. A colleague who is not a collaborator on it cannot get a token from it. Sharing one app also tempts people to make it publicly distributable, which makes Slack throttle it (see the note at the top). A separate app per person avoids both. If your workspace restricts creating apps, ask an admin to allow it or to approve the install.

## 3. Configure Comms

**The easy way:** run `comms slack-login`. With the app's Client ID configured (see *Sign in through the browser* below) it opens your browser; otherwise paste the token (it is not shown on screen) and press Enter. Comms checks it with Slack, then writes `provider = "slack"` and the token to `~/.config/comms/config.toml`, readable only by you, and leaves the rest of the file untouched. If `provider = "slack"` is already set and there is no token, plain `comms` offers the same prompt on start.

By hand instead:

Put the token in the environment, which keeps it out of files:

```bash
export COMMS_PROVIDER=slack
export COMMS_SLACK_TOKEN=xoxp-...
comms
```

or in the config file (make sure only you can read it, `chmod 600`):

```toml
# ~/.config/comms/config.toml
provider = "slack"

[slack]
token = "xoxp-..."
```

The sidebar title and the highlights are yellow for Slack.

## The Slack layout

With Slack the window looks like the Slack app: a **rail** on the far left (the workspace icon, or a tile with its initial, then **Home**, **Activity**, **DMs** and **Channels**, each with its unread count), the conversation list, the chat, and an optional **thread pane** on the right. Colours are green (focus, your own name), blue (selection, the active rail item, thread markers) and grey (times, hints).

| Keys | What it does |
|---|---|
| `h` / `Esc` in the list | move to the rail (`j`/`k` choose Home, Activity, DMs or Channels; `l` / `Enter` returns to the list) |
| Home / Activity / DMs / Channels | everything / conversations with unread messages / direct and group messages / channels (the number is how many conversations there have unread messages) |
| `t` | open or close the thread pane (starts with the newest thread) |
| `]` / `[` | next / previous thread |

Replies are folded into an "N replies" line under their thread's first message and shown in the pane. The rail needs 80 columns; under that it is dropped, and on terminals too narrow for chat and pane side by side the pane replaces the chat. Teams and Signal keep the plain two-panel layout.

The workspace tile shows your workspace's own icon when the terminal can display images (sixel, kitty or iTerm2 graphics; other terminals get a coarse half-block rendering) and the token has `team:read`. Otherwise it shows the workspace's initial.

Try it without a Slack account: `comms --demo-slack`.

Not yet: replying inside a thread (the pane is read-only), and the real Slack provider does not load thread replies yet (`conversations.replies`), so threads are only populated in the demo for now. Without `team:read` (or a custom icon) the tile shows the first letter of the workspace name.

## Sign in through the browser

Instead of copying a token, Comms can run Slack's own sign-in, the same way the Teams login works. Put the app's **Client ID** (Basic Information, not secret) in `[slack] client_id` or `COMMS_SLACK_CLIENT_ID`, make sure the manifest above has the redirect URL and `pkce_enabled`, then run `comms slack-login`. Your browser opens on Slack's "Allow Comms to access Slack" page; after you click **Allow**, Comms receives a one-time code on `http://localhost:8765/callback` and swaps it for **your** token. `comms slack-login --token` pastes a token instead.

How it is kept safe:

* **No secret in Comms.** It uses PKCE, so there is no client secret to leak; the Client ID is public. A stolen code is useless without the one-time verifier that never leaves your computer.
* **Only this computer can answer.** The listener binds to `127.0.0.1`, accepts one valid answer, then closes, and gives up after five minutes. The `state` value must match, so a forged or replayed redirect is refused and nothing is exchanged.
* **The token is stored carefully.** It is written to your config file with owner-only permissions, never printed or logged, and checked with Slack before it is saved.
* Close the browser tab afterwards: the address bar briefly held the one-time code, which is already spent.

If another program is using port 8765 the login says so; stop it and retry. One shared app for a team works with this flow only if Slack lets your colleagues authorise it (they need to see the same *Allow* page you saw); otherwise each person keeps their own app and Client ID.

## Settings

| Setting | Effect |
|---|---|
| `[slack] channels = "member"` (default) | list only the channels you have joined (`users.conversations`) |
| `[slack] channels = "all"` | also list every public channel in the workspace (`COMMS_SLACK_CHANNELS=member\|all`) |

Chats are listed with the newest activity first. Slack's list calls carry no activity time, so Comms reads each chat's latest message, four at a time, and caches the answer for five minutes.

## Troubleshooting

* `missing_scope (needs ...)`: add the named scope in step 1 and reinstall the app, then copy the new token.
* `not signed in or session expired`: the token was revoked or the app removed; repeat step 2.
* `rate limited`: Slack asks you to slow down; check that the app is internal to your workspace (see the note above).

---

Slack is a trademark of Slack Technologies, LLC, a Salesforce company. Comms is not affiliated with or endorsed by Slack. See {doc}`trademarks`.
