# Using Comms with Slack

Comms talks to the Slack Web API with a **user token** (`xoxp-...`), so it reads and posts as you. Slack has no "log in with a password" for third-party clients: every client needs a Slack *app*, and the app is what Slack issues the token for. For a lab or team, one person creates the app once; everyone else installs it to get their own token.

:::{note}
Slack throttles apps that are distributed commercially without Marketplace approval to **1 request per minute** for message history. An app that is created in, and only used by, your own workspace is an internal app and is not affected. Do not list the app in the Slack Marketplace or make it publicly distributable.
:::

**Early version.** Polling only (no live push yet), text with links, mentions, code and quotes; files show as `[file: name]`; thread replies are not loaded; conversations have no last-activity time or unread marker; bold/italic markers stay in the text. Edit and delete work on your own messages.

## 1. Create the app (once per workspace)

1. Open <https://api.slack.com/apps> and choose **Create New App → From scratch**. Pick a name (for example `Comms`) and your workspace.
2. Under **OAuth & Permissions → User Token Scopes**, add:
   `channels:read`, `groups:read`, `im:read`, `mpim:read`, `channels:history`, `groups:history`, `im:history`, `mpim:history`, `users:read`, `chat:write`.
3. Click **Install to Workspace** (or **Request to install**, if your workspace needs an admin to approve it) and allow it.

## 2. Get your token

Everyone who uses the app repeats this: on **OAuth & Permissions**, copy the **User OAuth Token** (starts with `xoxp-`). It is a password for your Slack account, so do not share or commit it.

Anyone else in the workspace opens the same app's install link to authorise it and gets their own token.

## 3. Configure Comms

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

With Slack the window looks like the Slack app: a narrow **rail** on the far left (the workspace tile with its initial, then **Home**, **DMs** and **Activity**), the conversation list, the chat, and an optional **thread pane** on the right. Colours are green (focus, your own name), blue (selection, the active rail item, thread markers) and grey (times, hints).

| Keys | What it does |
|---|---|
| `h` / `Esc` in the list | move to the rail (`j`/`k` choose Home, DMs or Activity; `l` / `Enter` returns to the list) |
| Home / DMs / Activity | everything / direct and group messages / conversations with unread messages (the number is how many) |
| `t` | open or close the thread pane (starts with the newest thread) |
| `]` / `[` | next / previous thread |

Replies are folded into an "N replies" line under their thread's first message and shown in the pane. The rail needs 80 columns; under that it is dropped, and on terminals too narrow for chat and pane side by side the pane replaces the chat. Teams and Signal keep the plain two-panel layout.

Try it without a Slack account: `comms --demo-slack`.

Not yet: replying inside a thread (the pane is read-only), and the real Slack provider does not load thread replies yet (`conversations.replies`), so threads are only populated in the demo for now. The workspace tile shows the first letter of the workspace name once the provider reports one; until then it shows `S`.

## Troubleshooting

* `missing_scope (needs ...)`: add the named scope in step 1 and reinstall the app, then copy the new token.
* `not signed in or session expired`: the token was revoked or the app removed; repeat step 2.
* `rate limited`: Slack asks you to slow down; check that the app is internal to your workspace (see the note above).
