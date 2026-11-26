"""Tests for check_socket_compat.py (D-165, #222)."""

from __future__ import annotations

import copy
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

import check_socket_compat as compat

REPO_ROOT = Path(__file__).resolve().parents[2]


def make_minimal_repo(root: Path, *, protocol: str = "marmot.agent-control.v3", version: str = "0.1.0") -> None:
    (root / "crates/marmot-agent/src").mkdir(parents=True)
    (root / "packages/claude-code/src").mkdir(parents=True)
    (root / "packages/openclaw/src").mkdir(parents=True)
    (root / "python/marmot_hermes").mkdir(parents=True)
    (root / "fixtures/socket").mkdir(parents=True)

    (root / "Cargo.toml").write_text(
        f'[workspace]\nmembers = ["crates/*"]\n\n[workspace.package]\nversion = "{version}"\n',
        encoding="utf-8",
    )
    (root / "crates/marmot-agent/src/socket.rs").write_text(
        f'pub const PROTOCOL: &str = "{protocol}";\n',
        encoding="utf-8",
    )

    for pkg_dir, pkg_name in (
        ("packages/claude-code", "marmot-agent-claude-code"),
        ("packages/openclaw", "marmot-agent-openclaw"),
    ):
        (root / pkg_dir / "package.json").write_text(
            json.dumps(
                {
                    "name": pkg_name,
                    "version": version,
                    "marmot": {"socketProtocol": protocol},
                }
            ),
            encoding="utf-8",
        )
        (root / pkg_dir / "package-lock.json").write_text(
            json.dumps(
                {
                    "name": pkg_name,
                    "version": version,
                    "packages": {"": {"name": pkg_name, "version": version}},
                }
            ),
            encoding="utf-8",
        )
        (root / pkg_dir / "src/socket.ts").write_text(
            f'export const PROTOCOL = "{protocol}";\n',
            encoding="utf-8",
        )

    (root / "python/pyproject.toml").write_text(
        f'[project]\nname = "marmot-hermes"\nversion = "{version}"\n\n'
        f'[tool.marmot]\nsocket_protocol = "{protocol}"\n',
        encoding="utf-8",
    )
    (root / "python/marmot_hermes/plugin.yaml").write_text(
        f"manifest_version: 1\nname: marmot\nversion: {version}\n",
        encoding="utf-8",
    )
    (root / "python/marmot_hermes/__init__.py").write_text(
        f'__version__ = "{version}"\n',
        encoding="utf-8",
    )
    (root / "python/marmot_hermes/core.py").write_text(
        f'PROTOCOL = "{protocol}"\n',
        encoding="utf-8",
    )

    schema = {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": protocol,
        "anyOf": [{"$ref": "#/$defs/FromCore"}, {"$ref": "#/$defs/FromConnector"}],
        "$defs": {
            "FromCore": {"anyOf": [{"$ref": "#/$defs/Prompt"}]},
            "FromConnector": {"anyOf": [{"$ref": "#/$defs/Hello"}]},
            "Prompt": {
                "type": "object",
                "properties": {"text": {"type": "string"}},
                "required": ["text"],
                "unevaluatedProperties": False,
            },
            "Hello": {
                "type": "object",
                "properties": {"params": {"$ref": "#/$defs/HelloParams"}},
                "required": ["params"],
                "unevaluatedProperties": False,
            },
            "HelloParams": {
                "type": "object",
                "properties": {
                    "protocol": {"type": "string", "const": protocol},
                    "kind": {"type": "string", "enum": ["driver", "channel"]},
                },
                "required": ["protocol", "kind"],
                "unevaluatedProperties": False,
            },
        },
    }
    (root / "fixtures/socket/schema.json").write_text(json.dumps(schema, indent=2), encoding="utf-8")


class TrackChecks(unittest.TestCase):
    def test_live_repository_passes(self) -> None:
        tracks, protocol, errors = compat.check_tracks(REPO_ROOT)
        self.assertEqual(errors, [])
        self.assertEqual(protocol, "marmot.agent-control.v3")
        self.assertEqual(set(tracks), {"marmot-agent", "claude-code", "openclaw", "hermes"})
        for name, info in tracks.items():
            self.assertTrue(compat.is_valid_semver(info["version"]), name)
            self.assertEqual(info["socket_protocol"], protocol, name)

    def test_zero_placeholder_version_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            make_minimal_repo(root, version="0.0.0")
            _, _, errors = compat.check_tracks(root)
            self.assertTrue(any("0.0.0" in e for e in errors), errors)

    def test_semver_grammar_accepts_build_metadata_and_rejects_leading_zero_prerelease(self) -> None:
        valid = ("0.1.0", "1.2.3", "1.2.3-rc.1", "1.2.3-0", "1.2.3-alpha.01a", "1.2.3+build.7", "1.2.3-rc.1+sha.abc")
        invalid = ("0.0.0", "0.0.0-rc.1", "01.2.3", "1.2.3-01", "1.2.3-rc.01", "1.2.3+", "1.2", "v1.2.3")
        for ver in valid:
            with self.subTest(valid=ver):
                self.assertTrue(compat.is_valid_semver(ver))
        for ver in invalid:
            with self.subTest(invalid=ver):
                self.assertFalse(compat.is_valid_semver(ver))

    def test_lockfile_version_drift_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            make_minimal_repo(root)
            lock = root / "packages/openclaw/package-lock.json"
            data = json.loads(lock.read_text(encoding="utf-8"))
            data["version"] = "0.2.0"
            lock.write_text(json.dumps(data), encoding="utf-8")
            _, _, errors = compat.check_tracks(root)
            self.assertTrue(any("packages/openclaw/package-lock.json" in e for e in errors), errors)

    def test_hermes_plugin_yaml_drift_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            make_minimal_repo(root)
            (root / "python/marmot_hermes/plugin.yaml").write_text(
                "manifest_version: 1\nname: marmot\nversion: 0.9.9\n", encoding="utf-8"
            )
            _, _, errors = compat.check_tracks(root)
            self.assertTrue(any("plugin.yaml" in e for e in errors), errors)

    def test_protocol_mismatch_in_connector_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            make_minimal_repo(root)
            pkg = root / "packages/claude-code/package.json"
            data = json.loads(pkg.read_text(encoding="utf-8"))
            data["marmot"]["socketProtocol"] = "marmot.agent-control.v2"
            pkg.write_text(json.dumps(data), encoding="utf-8")
            _, _, errors = compat.check_tracks(root)
            self.assertTrue(any("packages/claude-code/package.json" in e for e in errors), errors)


class SchemaCompat(unittest.TestCase):
    def setUp(self) -> None:
        self.base_schema = json.loads((REPO_ROOT / compat.SCHEMA_REL).read_text(encoding="utf-8"))

    def test_identical_schema_is_none(self) -> None:
        kind, violations = compat.compare_schemas(self.base_schema, copy.deepcopy(self.base_schema))
        self.assertEqual((kind, violations), ("none", []))

    def test_additive_optional_core_property_and_connector_variant_pass(self) -> None:
        new = copy.deepcopy(self.base_schema)
        # Core adding a new optional property is ignored by older connectors.
        new["$defs"]["PromptParams"]["properties"]["priority"] = {"type": "string"}
        new["$defs"]["PromptParams"]["description"] = "Updated doc"
        # Connector gaining a new message variant (that new core accepts) is additive.
        new["$defs"]["FromConnector"]["anyOf"].append({"$ref": "#/$defs/HeartbeatMessage"})
        new["$defs"]["HeartbeatMessage"] = {
            "type": "object",
            "properties": {"method": {"type": "string", "const": "heartbeat"}},
            "required": ["method"],
            "unevaluatedProperties": False,
        }
        kind, violations = compat.compare_schemas(self.base_schema, new)
        self.assertEqual((kind, violations), ("additive", []))

    def test_removing_property_is_breaking(self) -> None:
        new = copy.deepcopy(self.base_schema)
        del new["$defs"]["PromptParams"]["properties"]["text"]
        kind, violations = compat.compare_schemas(self.base_schema, new)
        self.assertEqual(kind, "breaking")
        self.assertTrue(any("PromptParams.properties.text" in v for v in violations), violations)

    def test_adding_required_field_to_existing_object_is_breaking(self) -> None:
        new = copy.deepcopy(self.base_schema)
        new["$defs"]["PromptParams"]["properties"]["priority"] = {"type": "string"}
        new["$defs"]["PromptParams"]["required"].append("priority")
        kind, violations = compat.compare_schemas(self.base_schema, new)
        self.assertEqual(kind, "breaking")
        self.assertTrue(any("added required field" in v for v in violations), violations)

    def test_making_core_sent_field_optional_is_breaking(self) -> None:
        new = copy.deepcopy(self.base_schema)
        new["$defs"]["PromptParams"]["required"].remove("text")
        kind, violations = compat.compare_schemas(self.base_schema, new)
        self.assertEqual(kind, "breaking")
        self.assertTrue(any("core-sent object removed required field" in v for v in violations), violations)

    def test_widening_core_sent_field_to_nullable_is_breaking(self) -> None:
        new = copy.deepcopy(self.base_schema)
        new["$defs"]["PromptParams"]["properties"]["text"] = {"type": ["string", "null"]}
        kind, violations = compat.compare_schemas(self.base_schema, new)
        self.assertEqual(kind, "breaking")
        self.assertTrue(any("core-sent schema added allowed type" in v for v in violations), violations)

    def test_adding_core_sent_union_branch_is_breaking(self) -> None:
        new = copy.deepcopy(self.base_schema)
        new["$defs"]["FromCore"]["anyOf"].append({"$ref": "#/$defs/PingMessage"})
        new["$defs"]["PingMessage"] = {
            "type": "object",
            "properties": {"method": {"type": "string", "const": "ping"}},
            "required": ["method"],
            "unevaluatedProperties": False,
        }
        kind, violations = compat.compare_schemas(self.base_schema, new)
        self.assertEqual(kind, "breaking")
        self.assertTrue(any("core-sent union added new branch" in v for v in violations), violations)

    def test_widening_connector_only_field_is_additive_and_narrowing_is_breaking(self) -> None:
        widened = copy.deepcopy(self.base_schema)
        widened["$defs"]["HelloParams"]["properties"]["token"] = {"type": ["string", "null"]}
        self.assertEqual(compat.compare_schemas(self.base_schema, widened), ("additive", []))
        kind, violations = compat.compare_schemas(widened, self.base_schema)
        self.assertEqual(kind, "breaking")
        self.assertTrue(any("removed allowed type" in v for v in violations), violations)

    def test_replacing_field_with_restricted_ref_is_breaking(self) -> None:
        new = copy.deepcopy(self.base_schema)
        new["$defs"]["RestrictedText"] = {"type": "string", "enum": ["hello"]}
        new["$defs"]["PromptParams"]["properties"]["text"] = {"$ref": "#/$defs/RestrictedText"}
        kind, violations = compat.compare_schemas(self.base_schema, new)
        self.assertEqual(kind, "breaking")
        self.assertTrue(any("tightened with new enum" in v for v in violations), violations)

    def test_introducing_all_of_or_unhandled_keyword_fails_closed(self) -> None:
        new = copy.deepcopy(self.base_schema)
        new["$defs"]["PromptParams"]["allOf"] = [{"required": ["session"]}]
        kind, violations = compat.compare_schemas(self.base_schema, new)
        self.assertEqual(kind, "breaking")
        self.assertTrue(any("unhandled schema keyword 'allOf'" in v for v in violations), violations)

    def test_schema_valued_additional_properties_and_boolean_items_are_checked(self) -> None:
        # Changing a map's additionalProperties value schema from string to integer is breaking.
        old_map = copy.deepcopy(self.base_schema)
        old_map["$defs"]["HelloParams"]["properties"]["labels"] = {
            "type": "object",
            "additionalProperties": {"type": "string"},
        }
        new_map = copy.deepcopy(old_map)
        new_map["$defs"]["HelloParams"]["properties"]["labels"]["additionalProperties"] = {
            "type": "integer",
        }
        kind, violations = compat.compare_schemas(old_map, new_map)
        self.assertEqual(kind, "breaking")
        self.assertTrue(any("additionalProperties" in v for v in violations), violations)

        # Replacing array items with boolean false is breaking.
        bad_items = copy.deepcopy(self.base_schema)
        bad_items["$defs"]["PromptParams"]["properties"]["attachments"]["items"] = False
        kind, violations = compat.compare_schemas(self.base_schema, bad_items)
        self.assertEqual(kind, "breaking")
        self.assertTrue(any("tightened items to false" in v for v in violations), violations)

    def test_default_annotation_change_is_additive(self) -> None:
        new = copy.deepcopy(self.base_schema)
        new["$defs"]["HelloParams"]["properties"]["token"]["default"] = ""
        self.assertEqual(compat.compare_schemas(self.base_schema, new), ("additive", []))

    def test_changing_property_type_is_breaking(self) -> None:
        new = copy.deepcopy(self.base_schema)
        new["$defs"]["PromptParams"]["properties"]["text"] = {"type": "integer"}
        kind, violations = compat.compare_schemas(self.base_schema, new)
        self.assertEqual(kind, "breaking")
        self.assertTrue(any("removed allowed type" in v for v in violations), violations)

    def test_removing_union_branch_is_breaking(self) -> None:
        new = copy.deepcopy(self.base_schema)
        new["$defs"]["CoreRequestMessage"]["oneOf"].pop()
        kind, violations = compat.compare_schemas(self.base_schema, new)
        self.assertEqual(kind, "breaking")
        self.assertTrue(any("union branch removed" in v for v in violations), violations)

    def test_incrementing_protocol_version_allows_breaking_changes(self) -> None:
        new = copy.deepcopy(self.base_schema)
        new["title"] = "marmot.agent-control.v4"
        new["$defs"]["HelloParams"]["properties"]["protocol"]["const"] = "marmot.agent-control.v4"
        del new["$defs"]["PromptParams"]["properties"]["text"]
        kind, violations = compat.compare_schemas(self.base_schema, new)
        self.assertEqual((kind, violations), ("protocol-bump", []))

    def test_decrementing_protocol_version_is_breaking(self) -> None:
        new = copy.deepcopy(self.base_schema)
        new["title"] = "marmot.agent-control.v2"
        kind, violations = compat.compare_schemas(self.base_schema, new)
        self.assertEqual(kind, "breaking")
        self.assertTrue(any("without incrementing" in v for v in violations), violations)


class CliWithGitBase(unittest.TestCase):
    def test_cli_detects_breaking_diff_and_missing_base_ref(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            make_minimal_repo(root)
            env = {
                **os.environ,
                "GIT_AUTHOR_NAME": "Test",
                "GIT_AUTHOR_EMAIL": "test@example.com",
                "GIT_COMMITTER_NAME": "Test",
                "GIT_COMMITTER_EMAIL": "test@example.com",
            }
            subprocess.run(["git", "init", "-q"], cwd=root, check=True, env=env)
            subprocess.run(["git", "add", "."], cwd=root, check=True, env=env)
            subprocess.run(["git", "commit", "-q", "-m", "base", "--no-gpg-sign"], cwd=root, check=True, env=env)

            # Missing base ref fails closed
            self.assertEqual(compat.main(["--root", str(root), "--base", "does-not-exist"]), 1)

            # Additive optional property passes
            schema_path = root / compat.SCHEMA_REL
            schema = json.loads(schema_path.read_text(encoding="utf-8"))
            schema["$defs"]["Prompt"]["properties"]["extra"] = {"type": "string"}
            schema_path.write_text(json.dumps(schema), encoding="utf-8")
            self.assertEqual(compat.main(["--root", str(root), "--base", "HEAD"]), 0)

            # Making a core-sent property required fails
            schema["$defs"]["Prompt"]["required"].append("extra")
            schema_path.write_text(json.dumps(schema), encoding="utf-8")
            self.assertEqual(compat.main(["--root", str(root), "--base", "HEAD"]), 1)


if __name__ == "__main__":
    unittest.main()
