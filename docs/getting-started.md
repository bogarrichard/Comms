# Getting started

## Install

```bash
curl --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/bogarrichard/Comms/main/scripts/install.sh | sh
```

The installer downloads the prebuilt binary of the latest [release](https://github.com/bogarrichard/Comms/releases) (Linux x86_64 and macOS Apple silicon), verifies its SHA-256, checks that it runs, and installs it to `~/.local/bin` together with the setup scripts in `~/.local/share/comms/scripts`. Make sure `~/.local/bin` is on your `PATH`.

| Variable | Effect |
|---|---|
| `COMMS_VERSION=v0.2.0` | install that release instead of the latest |
| `COMMS_BIN_DIR=/usr/local/bin` | install the binary there |
| `COMMS_FROM_SOURCE=1` | build with cargo instead of downloading a binary |
| `COMMS_REF=main` | build that branch or tag from source (implies `COMMS_FROM_SOURCE=1`) |
| `COMMS_REPO=<git url>` | build from another repository (source builds) |
| `COMMS_YES=1` | install rustup without asking if cargo is missing (source builds) |

To read the script before running it:

```bash
curl -sSf https://raw.githubusercontent.com/bogarrichard/Comms/main/scripts/install.sh -o install.sh
less install.sh && sh install.sh
```

Other platforms: download the archive from the release page (Windows has a `.zip`), or build from source, which needs a C compiler and cmake. If GitHub answers with a temporary 504, wait a minute and run the installer again.

## Try it without an account

```bash
comms --demo
```

The demo runs on fake data. Press `?` for the key bindings (vim-style: `j`/`k`, `Enter`, `/`, `i`, `Esc`, `q`).

## Connect a real service

Comms talks to one service at a time, chosen with `provider` in the config file (`comms paths` shows where it is).

| Service | Colour | Setup |
|---|---|---|
| Microsoft Teams | purple | Register an Entra application once, then sign in with your browser: {doc}`setup-entra` |
| Signal | blue | Link Comms as a secondary device through `signal-cli`: {doc}`signal` |

The setup questionnaire writes the Teams part of the config for you:

```bash
python3 ~/.local/share/comms/scripts/configure.py
```

## Update and uninstall

Run the installer again to update. To remove Comms:

```bash
comms logout --wipe-cache       # forget the sign-in, delete cached messages
rm ~/.local/bin/comms
rm -r ~/.local/share/comms
```
