# Using Comms with Signal

:::{warning}
**Status: experimental.** Text only, and **not yet run against a live `signal-cli`**: the integration is implemented from `signal-cli`'s documentation and tested against a fake. Your first run is the first live test.
:::

Comms links to your Signal account as a **secondary device**, exactly like Signal Desktop, using [signal-cli](https://github.com/AsamK/signal-cli) (a separate program, GPL-3.0, started by Comms as a child process). Your phone stays the primary device.

**Text only for now.** Attachments appear as `[attachment: name]`; reactions, edits, deletions, typing indicators and read receipts you send are not supported.

## 1. Install signal-cli

Get a release from <https://github.com/AsamK/signal-cli/releases> (or your package manager) and make sure `signal-cli --version` works. For a QR code in the terminal, also install `qrencode` (`sudo apt install qrencode`); without it you get a link to open on the phone instead.

## 2. Link this computer

```bash
comms signal-link
```

Scan the QR code in Signal on your phone: **Settings → Linked devices → Link new device**. The device is called `comms`; you can remove it from the same screen at any time.

## 3. Configure

```toml
# ~/.config/comms/config.toml
provider = "signal"

[signal]
account  = "+36201234567"   # your number, international format
# cli_path = "/opt/signal-cli/bin/signal-cli"   # if it is not on PATH
```

(or `COMMS_PROVIDER=signal COMMS_SIGNAL_ACCOUNT=+36...`). Then run `comms`. The sidebar title and the highlights are blue for Signal (purple for Teams).

## Things to know

* **No history.** Signal does not send old messages to a newly linked device. Conversations start empty and fill with messages that arrive while Comms (or its `signal-cli`) is running. Groups you are in are listed from the start.
* **Messages arrive only while Comms runs.** Anything sent while it was closed is delivered by Signal's servers the next time it connects (they hold it for a limited time).
* **Where messages are stored.** Received messages are kept unencrypted in `~/.local/share/comms/signal-inbox.jsonl` (mode `0600`) and in the normal cache `comms.db`, protected only by file permissions. `comms logout --wipe-cache` deletes both. signal-cli keeps its own account data under `~/.local/share/signal-cli`.
* **Unofficial.** Signal offers no public API. `signal-cli` is widely used, but Signal can change its protocol and break old versions (update `signal-cli` if it stops connecting), and Signal's terms discourage unofficial clients. Use it knowing that.
* **One connection at a time.** Switch with `provider = "teams"` / `"signal"` (or `COMMS_PROVIDER`). Both keep their own conversations in the cache.
* If you see "signal-cli is not responding … Is +36… linked?", run `comms signal-link` again and check `account`.

## Not verified yet

The JSON that `signal-cli` sends and expects (`receive` notifications, `send`, `listGroups`, `listContacts`) is implemented from its documentation and tested against a fake, **not yet against a live `signal-cli`**. Your first run is the first live test; if something does not show up, the details of what `signal-cli` actually sent are what is needed to fix it.

---

Signal is a trademark of Signal Messenger, LLC. Comms is not affiliated with or endorsed by Signal Messenger, LLC, and signal-cli is a separate project. See {doc}`trademarks`.
