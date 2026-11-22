"""What the Hermes container's entrypoint does to Hermes's configuration (python/hermes/configure.py):
the model key is taken out of every copy of it. Hermes's YAML module is replaced by JSON, which is
valid YAML: a file that is not JSON stands for one Hermes cannot read."""

import json
import stat
import sys
import tempfile
import types
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "hermes"))
import configure  # noqa: E402

KEY = "not-a-real-key-0123456789"
yaml = types.SimpleNamespace(safe_load=json.loads, safe_dump=lambda c: json.dumps(c, indent=2) + "\n")


class Scrub(unittest.TestCase):
    def setUp(self):
        self.home = Path(tempfile.mkdtemp(prefix="hermes-home-"))
        self.addCleanup(lambda: __import__("shutil").rmtree(self.home, True))
        self.config = self.home / "config.yaml"

    def test_a_readable_configuration_names_the_variable(self):
        self.config.write_text(json.dumps({"model": {"default": "m", "api_key": KEY}, "other": 1}))
        configure.scrub(self.home, KEY, yaml)
        config = json.loads(self.config.read_text())
        self.assertEqual(config, {"model": {"default": "m", "key_env": "MARMOT_MODEL_KEY"}, "other": 1})

    def test_an_unreadable_configuration_loses_its_key_lines(self):
        self.config.write_text(
            "model:\n"
            "  default: some/model\n"
            f"  api_key: {KEY}\n"
            "  base_url: https://models.example/v1\n"
            "  extra: [unclosed\n"
            "other:\n"
            "  api_key: |\n"
            f"    {KEY}\n"
            "    more of it\n"
            "  kept: yes\n"
            f"note: {KEY}\n"
        )
        configure.scrub(self.home, KEY, yaml)
        text = self.config.read_text()
        self.assertNotIn(KEY, text)
        self.assertNotIn("api_key", text)
        self.assertNotIn("more of it", text)
        for kept in ("default: some/model", "base_url: https://models.example/v1", "kept: yes", "extra: [unclosed"):
            self.assertIn(kept, text)
        self.assertEqual(stat.S_IMODE(self.config.stat().st_mode), 0o600)
        self.assertEqual(sorted(p.name for p in self.home.iterdir()), ["config.yaml"], "no copy is left beside it")

    def test_an_unreadable_backup_is_removed(self):
        backups = self.home / "backups/config"
        backups.mkdir(parents=True)
        (backups / "one").write_text(f"model: {{api_key: {KEY}")
        (self.home / "config.yaml.bak").write_text(f"model:\n  api_key: {KEY}\n  [")
        self.config.write_text(json.dumps({"model": {"default": "m"}}))
        configure.scrub(self.home, KEY, yaml)
        self.assertFalse((backups / "one").exists())
        self.assertFalse((self.home / "config.yaml.bak").exists())
        self.assertEqual(json.loads(self.config.read_text()), {"model": {"default": "m"}})

    def test_a_configuration_without_the_key_is_left_alone(self):
        text = "model:\n  default: some/model\n  [not json either\n"
        self.config.write_text(text)
        configure.scrub(self.home, KEY, yaml)
        self.assertEqual(self.config.read_text(), text)


if __name__ == "__main__":
    unittest.main()


class Work(unittest.TestCase):
    """Hermes's commands run in the work folder the container mounts, at the path the core knows it by
    (MARMOT_WORK), set at every start: a file the agent names is then one the core can find (D-090)."""

    def setUp(self):
        self.home = Path(tempfile.mkdtemp(prefix="hermes-home-"))
        self.addCleanup(lambda: __import__("shutil").rmtree(self.home, True))
        self.config = self.home / "config.yaml"

    def test_the_work_folder_is_where_hermess_commands_run_and_the_rest_is_kept(self):
        self.config.write_text(json.dumps({"terminal": {"cwd": "/work", "timeout": 60}, "other": 1}))
        configure.work(self.home, "/srv/agents/work", yaml)
        self.assertEqual(json.loads(self.config.read_text()),
                         {"terminal": {"cwd": "/srv/agents/work", "timeout": 60}, "other": 1})
        self.config.write_text(json.dumps({"other": 1}))
        configure.work(self.home, "/srv/agents/work", yaml)
        self.assertEqual(json.loads(self.config.read_text()), {"other": 1, "terminal": {"cwd": "/srv/agents/work"}})
        self.assertEqual(stat.S_IMODE(self.config.stat().st_mode), 0o600)

    def test_with_no_work_folder_named_nothing_changes(self):
        self.config.write_text("not: [json")
        configure.work(self.home, "", yaml)
        configure.work(self.home, "/srv/agents/work", yaml)
        self.assertEqual(self.config.read_text(), "not: [json")
