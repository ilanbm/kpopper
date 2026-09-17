"""The public CLI describes user operations, not onboarding bookkeeping."""
import json
import hashlib
import contextlib
import io
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]


class PublicCLI(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name).resolve()
        self.workspace = self.root / "Weekly planning"
        self.workspace.mkdir()
        self.env = {**os.environ, "XDG_STATE_HOME": str(self.root / "state"),
                    "XDG_CONFIG_HOME": str(self.root / "config"), "KPOPPER_SESSION_DISABLE": "1"}

    def cli(self, *args):
        return subprocess.run([sys.executable, str(ROOT / "scripts/cli.py"), *args],
                              cwd=self.root, env=self.env, text=True, capture_output=True)

    def test_public_help_has_operations_and_no_onboarding_protocol(self):
        result = self.cli("--help")
        self.assertEqual(result.returncode, 0, result.stderr)
        for command in ("open", "map", "config", "check", "pull", "add", "set", "review"):
            self.assertIn("kpop " + command, result.stdout)
        for internal in ("kpopper start", "choose", "shown", "--request", "_agent"):
            self.assertNotIn(internal, result.stdout)

    def test_open_missing_record_has_a_useful_result_without_writes(self):
        result = self.cli("--workspace", str(self.workspace), "open")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("kpop map", result.stdout)
        self.assertNotIn("_agent", result.stdout)
        self.assertNotIn("KPOPPER_START", result.stdout)
        self.assertEqual(list(self.workspace.iterdir()), [])
        self.assertFalse((self.root / "state").exists())

    def test_open_json_is_structured_and_uses_the_global_workspace(self):
        result = self.cli("--workspace", str(self.workspace), "open", "--json")
        self.assertEqual(result.returncode, 0, result.stderr)
        result = json.loads(result.stdout)
        self.assertEqual(result["status"], "missing")
        self.assertEqual(result["workspace"], str(self.workspace))

    def test_config_reads_without_writes_then_persists_a_guidance_flag(self):
        first = self.cli("config", "--json")
        self.assertTrue(json.loads(first.stdout)["guidance"])
        self.assertFalse((self.root / "state").exists())
        changed = self.cli("config", "--guidance", "off", "--json")
        self.assertEqual(changed.returncode, 0, changed.stderr)
        self.assertFalse(json.loads(changed.stdout)["guidance"])
        self.assertFalse(json.loads(self.cli("config", "--json").stdout)["guidance"])
        self.assertEqual(list(self.workspace.iterdir()), [])

    def test_help_never_creates_state_or_attempts_an_operation(self):
        for command in ("open", "map", "config", "check", "pull", "add", "set", "review"):
            with self.subTest(command=command):
                result = self.cli(command, "--help")
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn("usage:", result.stdout.lower())
        self.assertFalse((self.root / "state").exists())

    def test_old_start_is_rejected_with_a_clear_replacement(self):
        result = self.cli("start", "choose", "map")
        self.assertEqual(result.returncode, 2)
        self.assertIn("kpop map", result.stderr)
        self.assertFalse((self.root / "state").exists())

    def test_unknown_options_and_bad_workspace_fail_without_writes(self):
        for args in (("map", "--deeper"), ("config", "--guidance", "maybe"),
                     ("--workspace", str(self.root / "missing"), "open")):
            with self.subTest(args=args):
                self.assertEqual(self.cli(*args).returncode, 2)
        self.assertFalse((self.root / "state").exists())

    def test_explicit_relative_record_is_resolved_before_workspace_discovery(self):
        (self.root / "PROVENANCE.yaml").write_text('known:\n  root.value: {v: 1, from: "parent"}\n')
        (self.workspace / "work.yaml").write_text('meta: {scope: "Weekly time"}\nknown:\n  week.hours: {v: 10, from: "calendar"}\n')
        result = self.cli("--workspace", str(self.workspace), "open", "work.yaml", "--json")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["record"], str(self.workspace / "work.yaml"))
        self.assertIn("Weekly time", json.loads(result.stdout)["view"])
        self.assertEqual(json.loads(result.stdout)["record_sha256"],
                         hashlib.sha256((self.workspace / "work.yaml").read_bytes()).hexdigest())

    def test_json_legacy_operations_preserve_their_exit_code(self):
        result = self.cli("--workspace", str(self.workspace), "check", "--json")
        self.assertEqual(result.returncode, 1)
        value = json.loads(result.stdout)
        self.assertEqual(value["exit_code"], 1)
        self.assertIn("no record", value["error"])

    def test_open_rejects_a_hash_from_a_different_record_than_its_view(self):
        from scripts import workspace_cli as C
        record = self.root / "PROVENANCE.yaml"
        record.write_text("known:\n  price: {v: 10}\n")
        location = {"workspace": str(self.root), "record": str(record), "status": "found", "key": "test"}
        def changing_view(*args):
            record.write_text("known:\n  price: {v: 100}\n")
            return subprocess.CompletedProcess([], 0, "price: 10", ""), False
        output = io.StringIO()
        with patch.object(C.W, "locate", return_value=location), \
                patch.object(C.S, "read_view", side_effect=changing_view), \
                contextlib.redirect_stdout(output):
            code = C.open_context(["--json"])
        self.assertEqual(code, 2)
        self.assertIn("changed", json.loads(output.getvalue())["error"])

    def test_json_flag_does_not_consume_a_value_after_the_separator(self):
        record = self.workspace / "PROVENANCE.yaml"
        record.write_text('known:\n  week.note: {v: original, from: "request"}\n')
        result = self.cli("--workspace", str(self.workspace), "--json", "set", "week.note", "--", "--json")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(json.loads(result.stdout)["exit_code"], 0)
        import yaml
        self.assertEqual(yaml.safe_load(record.read_text())["known"]["week.note"]["v"], "--json")

    def test_explicit_uppercase_yaml_never_falls_back_to_the_default_record(self):
        (self.root / "PROVENANCE.yaml").write_text('meta: {scope: "Default"}\nknown:\n  a.value: {v: 1, from: "source"}\n')
        (self.root / "notes.YAML").write_text('meta: {scope: "Explicit notes"}\nknown:\n  b.value: {v: 2, from: "source"}\n')
        result = self.cli("open", "notes.YAML", "--json")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Explicit notes", json.loads(result.stdout)["view"])
        (self.root / "notes.txt").write_text('known: {}')
        self.assertEqual(self.cli("open", "notes.txt").returncode, 2)

    def test_global_json_remains_an_option_before_a_separator(self):
        for name, key in (("one.yaml", "one.value"), ("two.yaml", "two.value")):
            (self.root / name).write_text('known:\n  ' + key + ': {v: 1, from: "source"}\n')
        result = self.cli("--json", "open", "one.yaml", "--", "two.yaml")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["status"], "found")

    def test_plain_terminal_map_fails_without_saving_a_preference_or_task(self):
        self.env.pop("KPOPPER_AGENT_SESSION", None)
        self.env.pop("CODEX_THREAD_ID", None)
        result = self.cli("map", "--json")
        self.assertEqual(result.returncode, 2)
        self.assertEqual(json.loads(result.stdout)["status"], "unavailable")
        self.assertFalse((self.root / "state").exists())

    def test_mapping_requires_acceptance_and_an_actual_result_before_completion(self):
        self.env["KPOPPER_AGENT_SESSION"] = "mapping-fixture"
        created = self.cli("--workspace", str(self.workspace), "map", "--deep", "--json")
        self.assertEqual(created.returncode, 0, created.stderr)
        task = json.loads(created.stdout)
        self.assertEqual((task["mode"], task["status"]), ("deep", "ready"))
        self.assertIn("commitments", task["instructions"])
        report = self.workspace / "mapping.md"
        report.write_text("Reviewed the agreed weekly commitments and documented the open questions.")
        complete = [value if value != "REPORT_PATH" else str(report) for value in task["protocol"]["complete"]]
        result = subprocess.run(complete, env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 2)
        accepted = subprocess.run(task["protocol"]["accept"], env=self.env, capture_output=True, text=True)
        self.assertEqual(json.loads(accepted.stdout)["mapping"], "running")
        result = subprocess.run(complete, env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["mapping"], "complete")
        opened = self.cli("--workspace", str(self.workspace), "open", "--json")
        self.assertEqual(json.loads(opened.stdout)["mapping"]["report"], str(report))

    def test_another_session_cannot_replace_an_accepted_mapping(self):
        self.env["KPOPPER_AGENT_SESSION"] = "owner-a"
        task = json.loads(self.cli("map", "--json").stdout)
        subprocess.run(task["protocol"]["accept"], env=self.env, capture_output=True, text=True, check=True)
        self.env["KPOPPER_AGENT_SESSION"] = "owner-b"
        rejected = self.cli("map", "--deep", "--json")
        self.assertEqual(rejected.returncode, 2)
        self.env["KPOPPER_AGENT_SESSION"] = "owner-a"
        current = json.loads(self.cli("_agent", "task").stdout)
        self.assertEqual((current["request"], current["status"]), (task["request"], "running"))

    def test_checker_timeout_records_a_controlled_retryable_failure(self):
        sys.path.insert(0, str(ROOT / "scripts"))
        import mapping as M
        import workspace as W
        record = self.workspace / "PROVENANCE.yaml"
        record.write_text('known:\n  note.value: {v: 1, from: "source"}\n')
        with patch.dict(os.environ, {**self.env, "KPOPPER_AGENT_SESSION": "check-fixture"}):
            location = W.locate(self.workspace)
            task = M.request(location)
            M.transition(location, task["request"], "accept")
            with patch.object(M.subprocess, "run", side_effect=subprocess.TimeoutExpired("check", 60)):
                with self.assertRaisesRegex(ValueError, "mapping remains running"):
                    M.transition(location, task["request"], "complete", str(record))
            saved = M.read(location)
            self.assertEqual(saved["mapping"], "running")
            self.assertIn("timed out", saved["check"]["error"])


if __name__ == "__main__":
    unittest.main()
