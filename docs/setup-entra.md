# Registering the Entra application

Comms is a *public client*: no secret is ever stored. You (or your organisation) register an app once; its **client ID** goes into the config.

1. Open <https://entra.microsoft.com> → **Applications → App registrations → New registration**.
   (Any Microsoft work/school account can register an app in its own tenant; if your organisation restricts this, ask IT or use a free developer tenant.)
2. Name: `Comms`. **Supported account types:** *Accounts in any organizational directory (multitenant)* — this lets you sign in with your `@edu.bme.hu` account even when the app is registered elsewhere. A single-tenant registration works too if you set `tenant = "<tenant id>"`.
3. **Redirect URI:** platform *Mobile and desktop applications*, value `http://localhost`.
4. After creating: **Authentication → Advanced settings → Allow public client flows = Yes** (required for `login --device-code`).
5. **API permissions → Add → Microsoft Graph → Delegated**:

   | Permission | Needed for | Admin consent |
   |---|---|---|
   | `User.Read`, `offline_access` | sign-in, staying signed in | no |
   | `Chat.ReadBasic`, `Chat.Read` | list and read chats | no |
   | `ChatMessage.Send` | send chat messages | no |
   | `Team.ReadBasic.All`, `Channel.ReadBasic.All` | list teams and channels | no |
   | `ChannelMessage.Send` | post to channels | no |
   | `ChannelMessage.Read.All` | **read channel messages** | **yes** |

   You do not have to pre-add these: Comms requests them at sign-in. Registering them just makes the consent screen predictable.
6. Copy the **Application (client) ID** into `~/.config/comms/config.toml` (`comms paths` shows the location):

   ```toml
   client_id = "…"
   tenant = "organizations"   # or your tenant id / domain
   channels = "read"          # "basic" = list + post only, "off" = chats only
   ```

## If sign-in fails

| Message | Meaning |
|---|---|
| "requires administrator approval" (`AADSTS65001/90094`) | The organisation blocks user consent for some permission. Use `channels = "basic"` or `"off"`, or ask IT to grant consent. Run `comms login` again after changing the setting. |
| `AADSTS7000218` | Enable *Allow public client flows* (step 4). |
| `AADSTS50194` | Registration is single-tenant: set `tenant` to its tenant id. |
| `AADSTS53003` / Conditional Access | Policy blocked this flow; try the other of browser / `--device-code`. |
| "no secure credential store" | Install/unlock a Secret Service provider (GNOME Keyring, KeePassXC). Comms never falls back to plaintext. |

## Privacy

Messages are cached unencrypted in `~/.local/share/comms/comms.db` (mode `0600`, directory `0700`), protected only by OS file permissions. `comms logout --wipe-cache` deletes the cache. Nothing is sent anywhere except Microsoft Graph; there is no telemetry. Logs (`comms.log`) never contain message text or tokens. Check your organisation's policy on storing Teams data locally.
