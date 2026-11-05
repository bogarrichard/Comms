"""Tests for scripts/configure.py (standard library only).

    python3 -m unittest discover -s tests/scripts -v
"""
import importlib.util
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "configure.py"
GOOD_ID = "12345678-1234-1234-1234-123456789abc"

spec = importlib.util.spec_from_file_location("configure", SCRIPT)
configure = importlib.util.module_from_spec(spec)
spec.loader.exec_module(configure)


def run(*args, stdin="", env_extra=None):
    """Run the script as a subprocess with no tty and an isolated environment."""
    env = {k: v for k, v in os.environ.items() if k not in ("COMMS_CONFIG_DIR", "XDG_CONFIG_HOME")}
    env.update(env_extra or {})
    return subprocess.run(
        [sys.executable, "-I", str(SCRIPT), *args],
        input=stdin, capture_output=True, text=True, env=env, timeout=30,
    )


class Validators(unittest.TestCase):
    def test_client_id(self):
        self.assertIsNone(configure.v_client_id(GOOD_ID))
        self.assertIsNone(configure.v_client_id(GOOD_ID.upper()))
        for bad in ("", "nope", GOOD_ID + "0", GOOD_ID[:-1], "g" * 8 + GOOD_ID[8:]):
            self.assertIsNotNone(configure.v_client_id(bad), bad)

    def test_tenant(self):
        for ok in ("organizations", "common", "consumers", GOOD_ID, "contoso.com", "a-b.example.org"):
            self.assertIsNone(configure.v_tenant(ok), ok)
        for bad in ("", "contoso", "-x.com", "has space.com", "foo.c"):
            self.assertIsNotNone(configure.v_tenant(bad), bad)

    def test_channels(self):
        for ok in ("read", "basic", "off"):
            self.assertIsNone(configure.v_channels(ok))
        self.assertIsNotNone(configure.v_channels("all"))

    def test_int_range(self):
        check = configure.v_int(5, 3600)
        self.assertIsNone(check("5"))
        self.assertIsNone(check("3600"))
        for bad in ("4", "3601", "-1", "x", "", "1.5"):
            self.assertIsNotNone(check(bad), bad)


class TomlEditing(unittest.TestCase):
    def test_read_values(self):
        text = (
            '# comment = "ignored"\n'
            'client_id = "abc"  # trailing\n'
            "initial_messages = 50\n"
            "\n[poll]\nopen_secs = 20\n\n[keys]\nquit = \"q\"\n"
        )
        v = configure.read_values(text)
        self.assertEqual(v["client_id"], "abc")
        self.assertEqual(v["initial_messages"], 50)
        self.assertEqual(v["poll.open_secs"], 20)
        self.assertEqual(v["keys.quit"], "q")
        self.assertNotIn("comment", v)

    def test_set_into_empty(self):
        self.assertEqual(configure.set_value("", "client_id", "x"), 'client_id = "x"\n')

    def test_replace_keeps_comment(self):
        out = configure.set_value('tenant = "common"  # why\n', "tenant", "organizations")
        self.assertEqual(out, 'tenant = "organizations"  # why\n')

    def test_top_level_key_goes_before_first_table(self):
        out = configure.set_value('client_id = "a"\n\n[keys]\nquit = "q"\n', "tenant", "common")
        lines = out.splitlines()
        self.assertLess(lines.index('tenant = "common"'), lines.index("[keys]"))
        v = configure.read_values(out)
        self.assertEqual(v["tenant"], "common")
        self.assertEqual(v["keys.quit"], "q")

    def test_top_level_key_with_only_tables(self):
        out = configure.set_value('[keys]\nquit = "q"\n', "client_id", "a")
        self.assertEqual(configure.read_values(out), {"client_id": "a", "keys.quit": "q"})
        self.assertLess(out.index("client_id"), out.index("[keys]"))

    def test_new_table_appended(self):
        out = configure.set_value('client_id = "a"\n', "poll.open_secs", 30)
        self.assertTrue(out.endswith("[poll]\nopen_secs = 30\n"))
        self.assertEqual(configure.read_values(out)["poll.open_secs"], 30)

    def test_key_added_to_existing_table(self):
        out = configure.set_value("[poll]\nopen_secs = 1\n\n[keys]\nquit = \"q\"\n", "poll.chats_secs", 90)
        v = configure.read_values(out)
        self.assertEqual(v["poll.chats_secs"], 90)
        self.assertEqual(v["poll.open_secs"], 1)
        self.assertLess(out.index("chats_secs"), out.index("[keys]"))

    def test_same_key_name_in_other_table_untouched(self):
        text = '[keys]\ntenant = "mine"\n'
        out = configure.set_value(text, "tenant", "common")
        v = configure.read_values(out)
        self.assertEqual(v["keys.tenant"], "mine")
        self.assertEqual(v["tenant"], "common")

    def test_string_escaping_round_trips(self):
        out = configure.set_value("", "tenant", 'a"b\\c')
        self.assertEqual(out, 'tenant = "a\\"b\\\\c"\n')

    def test_output_is_valid_toml(self):
        try:
            import tomllib
        except ImportError:  # Python < 3.11
            self.skipTest("tomllib unavailable")
        text = ""
        for key, value in (("client_id", GOOD_ID), ("tenant", "organizations"), ("channels", "basic"),
                           ("initial_messages", 80), ("poll.chats_secs", 60), ("poll.open_secs", 15)):
            text = configure.set_value(text, key, value)
        data = tomllib.loads(text)
        self.assertEqual(data["client_id"], GOOD_ID)
        self.assertEqual(data["initial_messages"], 80)
        self.assertEqual(data["poll"], {"chats_secs": 60, "open_secs": 15})


class ConfigPath(unittest.TestCase):
    def test_env_override_wins(self):
        old = os.environ.get("COMMS_CONFIG_DIR")
        os.environ["COMMS_CONFIG_DIR"] = "/somewhere"
        try:
            self.assertEqual(configure.config_path(), Path("/somewhere/config.toml"))
        finally:
            if old is None:
                del os.environ["COMMS_CONFIG_DIR"]
            else:
                os.environ["COMMS_CONFIG_DIR"] = old


class Cli(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.dir = Path(self._tmp.name)
        self.path = self.dir / "sub" / "config.toml"

    def test_flags_create_config(self):
        r = run("--path", str(self.path), "--client-id", GOOD_ID, "--channels", "basic")
        self.assertEqual(r.returncode, 0, r.stderr)
        v = configure.read_values(self.path.read_text())
        self.assertEqual(v["client_id"], GOOD_ID)
        self.assertEqual(v["channels"], "basic")
        self.assertIn("comms login", r.stdout)
        self.assertFalse(self.path.with_name("config.toml.bak").exists())

    def test_dry_run_writes_nothing(self):
        r = run("--path", str(self.path), "--client-id", GOOD_ID, "--dry-run")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("Dry run", r.stdout)
        self.assertFalse(self.path.exists())

    def test_update_preserves_content_and_makes_backup(self):
        self.path.parent.mkdir(parents=True)
        original = f'# my notes\nclient_id = "{GOOD_ID}"\n\n[keys]\nquit = "q"\n'
        self.path.write_text(original)
        r = run("--path", str(self.path), "--tenant", "contoso.com")
        self.assertEqual(r.returncode, 0, r.stderr)
        new = self.path.read_text()
        self.assertIn("# my notes", new)
        v = configure.read_values(new)
        self.assertEqual(v["tenant"], "contoso.com")
        self.assertEqual(v["keys.quit"], "q")
        self.assertEqual(self.path.with_name("config.toml.bak").read_text(), original)

    def test_no_change_is_reported(self):
        self.path.parent.mkdir(parents=True)
        self.path.write_text(f'client_id = "{GOOD_ID}"\n')
        r = run("--path", str(self.path), "--client-id", GOOD_ID)
        self.assertEqual(r.returncode, 0)
        self.assertIn("Nothing to change", r.stdout)
        self.assertFalse(self.path.with_name("config.toml.bak").exists())

    def test_invalid_flags_rejected(self):
        for flags in (["--client-id", "nope"], ["--client-id", GOOD_ID, "--channels", "all"],
                      ["--client-id", GOOD_ID, "--tenant", "x"],
                      ["--client-id", GOOD_ID, "--initial-messages", "0"],
                      ["--client-id", GOOD_ID, "--initial-messages", "501"]):
            r = run("--path", str(self.path), *flags)
            self.assertNotEqual(r.returncode, 0, flags)
            self.assertFalse(self.path.exists(), flags)

    def test_client_id_required_for_new_config(self):
        r = run("--path", str(self.path), "--tenant", "common")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("--client-id is required", r.stderr)

    def test_interactive_defaults(self):
        # client id, tenant default, channels default, no advanced, save
        r = run("--path", str(self.path), stdin=f"{GOOD_ID}\n\n\nn\ny\n")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        v = configure.read_values(self.path.read_text())
        self.assertEqual(v["client_id"], GOOD_ID)
        self.assertEqual(v["tenant"], "organizations")
        self.assertEqual(v["channels"], "read")

    def test_interactive_reprompts_on_bad_input(self):
        r = run("--path", str(self.path), stdin=f"bad\n{GOOD_ID}\n\nnope\noff\nn\ny\n")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertEqual(r.stdout.count("!"), 2, r.stdout)
        self.assertEqual(configure.read_values(self.path.read_text())["channels"], "off")

    def test_interactive_advanced(self):
        r = run("--path", str(self.path), stdin=f"{GOOD_ID}\n\n\ny\n40\n30\n600\n20\ny\n")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        v = configure.read_values(self.path.read_text())
        self.assertEqual(v["initial_messages"], 40)
        self.assertEqual(v["poll.chats_secs"], 30)
        self.assertEqual(v["poll.channels_secs"], 600)
        self.assertEqual(v["poll.open_secs"], 20)

    def test_interactive_decline_save(self):
        r = run("--path", str(self.path), stdin=f"{GOOD_ID}\n\n\nn\nn\n")
        self.assertEqual(r.returncode, 1)
        self.assertFalse(self.path.exists())

    def test_eof_in_questionnaire_fails_cleanly(self):
        r = run("--path", str(self.path), stdin="")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("No input available", r.stderr)
        self.assertNotIn("Traceback", r.stderr)

    def test_default_path_honours_config_dir(self):
        r = run("--client-id", GOOD_ID, env_extra={"COMMS_CONFIG_DIR": str(self.dir)})
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertTrue((self.dir / "config.toml").exists())


if __name__ == "__main__":
    unittest.main()
