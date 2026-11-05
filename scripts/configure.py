#!/usr/bin/env python3
"""Create or update the Comms config.toml by answering a few questions.

    python3 scripts/configure.py              # interactive questionnaire
    python3 scripts/configure.py --dry-run    # show the result, write nothing
    python3 scripts/configure.py --client-id <GUID> --channels basic   # no questions

Only the Python standard library is needed (3.8+). An existing config is updated in
place: comments, your [keys] table and anything this script does not ask about are
kept, and the previous file is saved next to it as config.toml.bak.
"""
import argparse
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

GUID = re.compile(r"^[0-9a-fA-F]{8}-([0-9a-fA-F]{4}-){3}[0-9a-fA-F]{12}$")
CHANNEL_CHOICES = {
    "read": "read channel messages too (usually needs an administrator to consent)",
    "basic": "list teams/channels and post, but not read channel messages",
    "off": "chats only",
}
DEFAULTS = {
    "tenant": "organizations",
    "channels": "read",
    "initial_messages": 100,
    "poll.chats_secs": 60,
    "poll.channels_secs": 300,
    "poll.open_secs": 15,
}


# --------------------------------------------------------------------------- paths
def config_path() -> Path:
    """Same lookup as `comms paths` (COMMS_CONFIG_DIR wins)."""
    override = os.environ.get("COMMS_CONFIG_DIR")
    if override:
        return Path(override) / "config.toml"
    home = Path.home()
    if sys.platform == "darwin":
        base = home / "Library" / "Application Support" / "comms"
    elif os.name == "nt":
        base = Path(os.environ.get("APPDATA", home / "AppData" / "Roaming")) / "comms" / "config"
    else:
        base = Path(os.environ.get("XDG_CONFIG_HOME", home / ".config")) / "comms"
    return base / "config.toml"


# --------------------------------------------------------- minimal in-place TOML edit
SECTION = re.compile(r"^\s*\[([^\[\]]+)\]\s*(#.*)?$")
KEYLINE = re.compile(r"^(\s*)([A-Za-z0-9_-]+)\s*=\s*(.*?)\s*$")


def _unquote(raw: str):
    raw = re.sub(r"\s+#.*$", "", raw).strip()
    if len(raw) >= 2 and raw[0] == raw[-1] == '"':
        return raw[1:-1]
    if re.fullmatch(r"\d+", raw):
        return int(raw)
    return raw


def read_values(text: str) -> dict:
    """Current values as {'client_id': ..., 'poll.open_secs': ...} (top level + [poll])."""
    values, section = {}, ""
    for line in text.splitlines():
        m = SECTION.match(line)
        if m:
            section = m.group(1).strip()
            continue
        m = KEYLINE.match(line)
        if m and not line.lstrip().startswith("#"):
            key = m.group(2) if not section else f"{section}.{m.group(2)}"
            values[key] = _unquote(m.group(3))
    return values


def _render(value) -> str:
    return str(value) if isinstance(value, int) else '"' + str(value).replace("\\", "\\\\").replace('"', '\\"') + '"'


def set_value(text: str, dotted: str, value) -> str:
    """Set `key` (top level) or `table.key` in place; add the key/table if absent."""
    table, _, key = dotted.rpartition(".")
    lines = text.splitlines()
    section, last_in_section, header_at = "", None, None
    for i, line in enumerate(lines):
        m = SECTION.match(line)
        if m:
            section = m.group(1).strip()
            if section == table:
                header_at = i
            continue
        if section == table:
            km = KEYLINE.match(line)
            if km and not line.lstrip().startswith("#"):
                last_in_section = i
                if km.group(2) == key:
                    comment = re.search(r"\s+#.*$", km.group(3))
                    lines[i] = f"{km.group(1)}{key} = {_render(value)}{comment.group(0) if comment else ''}"
                    return "\n".join(lines) + "\n"
    new = f"{key} = {_render(value)}"
    if table == "":
        # top-level keys must come before the first [table]
        first_table = next((i for i, l in enumerate(lines) if SECTION.match(l)), len(lines))
        at = (last_in_section + 1) if last_in_section is not None else first_table
        lines.insert(at, new)
        if at == first_table and first_table < len(lines) - 1:
            lines.insert(at + 1, "")
    elif header_at is not None:
        lines.insert((last_in_section if last_in_section is not None else header_at) + 1, new)
    else:
        if lines and lines[-1].strip():
            lines.append("")
        lines += [f"[{table}]", new]
    return "\n".join(lines) + "\n"


# ------------------------------------------------------------------------ questions
def ask(prompt, default=None, validate=None, hint=None):
    while True:
        if hint:
            print(hint)
        suffix = f" [{default}]" if default not in (None, "") else ""
        try:
            answer = input(f"{prompt}{suffix}: ").strip()
        except EOFError:
            print()
            sys.exit("No input available; use the --client-id/--tenant/... flags instead.")
        if not answer and default not in (None, ""):
            answer = str(default)
        error = validate(answer) if validate else None
        if not error:
            return answer
        print(f"  ! {error}\n")


def v_client_id(s):
    return None if GUID.match(s) else "That is not a GUID like 00000000-0000-0000-0000-000000000000."


def v_tenant(s):
    if s in ("organizations", "common", "consumers") or GUID.match(s):
        return None
    if re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9.-]*\.[A-Za-z]{2,}", s):
        return None
    return "Use organizations, common, a tenant GUID, or a domain such as contoso.com."


def v_channels(s):
    return None if s in CHANNEL_CHOICES else "Type one of: " + ", ".join(CHANNEL_CHOICES) + "."


def v_int(low, high):
    def check(s):
        return None if s.isdigit() and low <= int(s) <= high else f"Enter a whole number from {low} to {high}."
    return check


def yes(prompt, default=False):
    d = "Y/n" if default else "y/N"
    try:
        a = input(f"{prompt} [{d}]: ").strip().lower()
    except EOFError:
        return default
    return default if not a else a in ("y", "yes")


def questionnaire(cur: dict) -> dict:
    print("Comms setup. Press Enter to keep the value shown in [brackets].\n")
    out = {}
    out["client_id"] = ask(
        "Application (client) ID",
        cur.get("client_id"),
        v_client_id,
        "Your Entra app registration's Application (client) ID.\n"
        "Don't have one yet? Follow docs/setup-entra.md (about 5 minutes), then paste it here.",
    )
    print()
    out["tenant"] = ask(
        "Tenant",
        cur.get("tenant", DEFAULTS["tenant"]),
        v_tenant,
        "Which accounts may sign in? 'organizations' = any work/school account (right for most people).\n"
        "Use a tenant ID or domain only if your app registration is single-tenant.",
    )
    print()
    print("How much of Teams channels should Comms use?")
    for name, text in CHANNEL_CHOICES.items():
        print(f"  {name:<6} {text}")
    out["channels"] = ask("Channels", cur.get("channels", DEFAULTS["channels"]), v_channels)
    print()
    if yes("Change advanced settings (history size, refresh intervals)?"):
        out["initial_messages"] = int(ask(
            "Messages to fetch when a conversation is first opened",
            cur.get("initial_messages", DEFAULTS["initial_messages"]), v_int(1, 500)))
        for key, label in (("chats_secs", "Refresh chat list every (seconds)"),
                           ("channels_secs", "Refresh channel list every (seconds)"),
                           ("open_secs", "Refresh the open conversation every (seconds)")):
            out[f"poll.{key}"] = int(ask(label, cur.get(f"poll.{key}", DEFAULTS[f"poll.{key}"]), v_int(5, 3600)))
    return out


def from_flags(args, cur) -> dict:
    out = {}
    checks = {"client_id": v_client_id, "tenant": v_tenant, "channels": v_channels}
    for key in checks:
        val = getattr(args, key)
        if val is not None:
            err = checks[key](val)
            if err:
                sys.exit(f"--{key.replace('_', '-')}: {err}")
            out[key] = val
    if args.initial_messages is not None:
        err = v_int(1, 500)(str(args.initial_messages))
        if err:
            sys.exit(f"--initial-messages: {err}")
        out["initial_messages"] = args.initial_messages
    if "client_id" not in out and "client_id" not in cur:
        sys.exit("--client-id is required when there is no existing config.")
    return out


# ------------------------------------------------------------------------------ main
def main() -> int:
    p = argparse.ArgumentParser(description="Create or update the Comms config.toml.")
    p.add_argument("--path", type=Path, help="config file to edit (default: where Comms looks)")
    p.add_argument("--dry-run", action="store_true", help="print the result, write nothing")
    p.add_argument("--client-id")
    p.add_argument("--tenant")
    p.add_argument("--channels")
    p.add_argument("--initial-messages", type=int)
    args = p.parse_args()

    path = args.path or config_path()
    text = path.read_text(encoding="utf-8") if path.exists() else ""
    cur = read_values(text)
    print(f"Config file: {path}" + ("" if path.exists() else "  (will be created)") + "\n")

    scripted = any(getattr(args, k) is not None for k in ("client_id", "tenant", "channels", "initial_messages"))
    answers = from_flags(args, cur) if scripted else questionnaire(cur)

    new = text
    for key, value in answers.items():
        if cur.get(key) != value:
            new = set_value(new, key, value)

    if new == text:
        print("\nNothing to change.")
        return 0
    print("\n--- resulting config ---\n" + new.rstrip() + "\n------------------------")
    if args.dry_run:
        print("Dry run: nothing written.")
        return 0
    if not scripted and not yes("Save this?", default=True):
        print("Not saved.")
        return 1
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        path.with_name(path.name + ".bak").write_text(text, encoding="utf-8")
    path.write_text(new, encoding="utf-8")
    print(f"Saved {path}.")
    return offer_login(scripted)


def offer_login(scripted: bool) -> int:
    """Finish with the browser sign-in (`comms login`); it validates the client ID for real."""
    comms = shutil.which("comms")
    if scripted or not sys.stdin.isatty():
        print("Next: comms login")
        return 0
    if not comms:
        print("Next: run `comms login` (opens your browser) once Comms is on your PATH.")
        return 0
    if yes("Sign in now? This opens your browser."):
        return subprocess.call([comms, "login"])
    print("Later: comms login   (on a machine without a browser: comms login --device-code)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
