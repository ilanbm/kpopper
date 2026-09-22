"""First use is contextual, opt-in, and does not depend on a Git repository."""
import concurrent.futures
import contextlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
import onboarding as O
import workspace as W
import session_start as S
import mapping as M


class WorkspaceFixture:
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name).resolve()
        self.work = self.root / "Buying a home"
        self.work.mkdir()
        self.private = self.root / "private"
        self.env = {**os.environ, "XDG_STATE_HOME": str(self.private),
                    "XDG_CONFIG_HOME": str(self.root / "config"),
                    "KPOPPER_SESSION_DISABLE": "1", "TMPDIR": str(self.root),
                    "KPOPPER_AGENT_SESSION": "fixture-session"}
        self.addCleanup(patch.stopall)
        patch.dict(os.environ, self.env).start()

    def cli(self, *args, cwd=None):
        return subprocess.run([sys.executable, str(ROOT / "scripts/cli.py"), "_agent", *args],
                              cwd=cwd or self.work, capture_output=True, text=True, env=self.env)

    def map(self, deep=False, cwd=None):
        return subprocess.run([sys.executable, str(ROOT / "scripts/cli.py"), "map", "--json"]
                              + (["--deep"] if deep else []), cwd=cwd or self.work,
                              capture_output=True, text=True, env=self.env)

    def finish(self, request):
        accepted = self.cli("accept", "--request", request)
        if accepted.returncode:
            return accepted
        report = self.work / "mapping-report.md"
        report.write_text("Reviewed the supplied deadline and its source. No additional sources were in scope.")
        return self.cli("complete", "--request", request, "--report", str(report))

    def hook(self, payload=None, name="session_open.sh", cwd=None):
        return subprocess.run(["sh", str(ROOT / "scripts" / name)], cwd=cwd or self.work,
                              input=json.dumps(payload or {"session_id": "first-use"}),
                              capture_output=True, text=True, env=self.env)

    def write_record(self, root=None, broken=False):
        record = (root or self.work) / "PROVENANCE.yaml"
        body = '''meta:
  name: Home purchase
  scope: Home purchase deadlines
sources:
  s.request: {asked: "Check the agreed deadline", name: "Current request"}
known:
  deadline.days: {v: 7, from: s.request, name: "Days remaining"}
'''
        if broken:
            body += '''schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}
judgments:
  d.ready:
    verdict: "There is time to submit"
    rests_on: [deadline.days]
    seen: {}
    wrong_if: "deadline.days < 1"
'''
        record.write_text(body, encoding="utf-8")
        return record


class FirstUse(WorkspaceFixture, unittest.TestCase):
    def test_missing_record_context_is_read_only_and_does_not_scan(self):
        result = self.cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("KPOPPER_START", result.stdout)
        self.assertIn("kpop add", result.stdout)
        self.assertIn("kpop map", result.stdout)
        self.assertIn("one-off needs nothing", result.stdout)
        self.assertIn("never offered", result.stdout)
        self.assertFalse(self.private.exists())
        self.assertEqual(list(self.work.iterdir()), [])
        # a host with skills is told the skill for each move, in its own syntax
        self.assertIn("/kpopper:record", O.context(W.locate(self.work), "claude"))
        self.assertIn("$map", O.context(W.locate(self.work), "codex"))

    def test_choice_survives_sessions_and_does_not_create_a_record(self):
        self.assertEqual(self.map().returncode, 0)
        status = json.loads(self.cli("status").stdout)
        self.assertEqual((status["mode"], status["mapping"]), ("map", "ready"))
        self.assertEqual(list(self.work.iterdir()), [])
        self.assertIn("scope", self.cli().stdout.lower())
        self.assertEqual(self.finish(status["request"]).returncode, 0)
        self.assertEqual(json.loads(self.cli("status").stdout)["mapping"], "complete")
        self.assertNotIn("Resume the requested", self.cli().stdout)

    def test_shown_is_explicit_and_a_new_project_does_not_repeat_the_tutorial(self):
        self.cli()  # Merely supplying context has not displayed a welcome.
        self.assertFalse(json.loads(self.cli("status").stdout)["introduced"])
        self.assertEqual(self.cli("shown", "welcome").returncode, 0)
        self.assertTrue(json.loads(self.cli("status").stdout)["offered"])
        self.assertNotIn("Offer the three", self.cli().stdout)
        other = self.root / "Weekly planning"
        other.mkdir()
        status = json.loads(self.cli("status", cwd=other).stdout)
        self.assertTrue(status["introduced"])
        self.assertFalse(status["offered"])
        # the offer's timing lives in the map skill; the hook says only whether it was made
        self.assertIn("never offered", self.cli(cwd=other).stdout)
        self.assertIn("already offered", self.cli().stdout)

    def test_guidance_off_persists_but_does_not_authorize_mapping(self):
        self.assertEqual(subprocess.run([sys.executable, str(ROOT / "scripts/cli.py"), "config", "--guidance", "off"], cwd=self.work, env=self.env, capture_output=True, text=True).returncode, 0)
        status = json.loads(self.cli("status").stdout)
        self.assertFalse(status["guidance"])
        self.assertIsNone(status["mode"])
        self.assertNotIn("Offer the three", self.cli().stdout)
        self.assertFalse((self.work / "PROVENANCE.yaml").exists())

    def test_independent_tip_acknowledgements_do_not_lose_each_other(self):
        with concurrent.futures.ThreadPoolExecutor(max_workers=6) as pool:
            results = list(pool.map(lambda event: self.cli("shown", event), O.EVENTS))
        self.assertTrue(all(result.returncode == 0 for result in results))
        self.assertEqual(json.loads(self.cli("status").stdout)["pending_tips"], [])

    def test_completing_an_old_mapping_cannot_erase_a_new_choice(self):
        first = json.loads(self.map().stdout)
        second = json.loads(self.map(True).stdout)
        result = self.finish(first["request"])
        self.assertEqual(result.returncode, 2)
        current = json.loads(self.cli("status").stdout)
        self.assertEqual((current["mode"], current["mapping"], current["request"]),
                         ("deep", "ready", second["request"]))

    def test_concurrent_completion_and_choice_preserve_the_new_request(self):
        first = json.loads(self.map().stdout)
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
            finished = pool.submit(self.finish, first["request"])
            selected = pool.submit(self.map, True)
            finished.result()
            self.assertEqual(selected.result().returncode, 0)
        current = json.loads(self.cli("status").stdout)
        self.assertEqual((current["mode"], current["mapping"]), ("deep", "ready"))

    def test_choose_returns_its_own_request_even_if_another_choice_follows_immediately(self):
        original_lock = O._choice_lock
        crossed = False

        @contextlib.contextmanager
        def interleave(location):
            nonlocal crossed
            with original_lock(location):
                yield
            if not crossed:
                crossed = True
                with contextlib.redirect_stdout(io.StringIO()):
                    M.request(W.locate(self.work), True)

        output = io.StringIO()
        with patch.object(O, "_choice_lock", interleave), contextlib.redirect_stdout(output):
            print(json.dumps(M.request(W.locate(self.work))))
        first = json.loads(output.getvalue())
        latest = json.loads(self.cli("status").stdout)
        self.assertEqual(first["mode"], "map")
        self.assertEqual(latest["mode"], "deep")
        self.assertNotEqual(first["request"], latest["request"])

    def test_bad_state_is_reported_and_never_replaced_by_defaults(self):
        self.map(True)
        state = O.project_dir(W.locate(self.work)) / "mapping.json"
        state.write_text("{not json", encoding="utf-8")
        self.assertNotEqual(self.map().returncode, 0)
        self.assertEqual(state.read_text(), "{not json")

    def test_guide_covers_non_code_work_and_history_limits(self):
        result = self.cli("guide")
        self.assertEqual(result.returncode, 0, result.stderr)
        for concept in ("calendar", "commitments", "scope", "historical", "source", "one-off"):
            self.assertIn(concept, result.stdout.lower())

    def test_first_finding_can_be_checked_and_grow_into_a_grounded_judgment(self):
        self.write_record()
        command = [sys.executable, str(ROOT / "scripts/cli.py")]
        result = subprocess.run(command + ["check"], cwd=self.work, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        result = subprocess.run(command + ["add", "d.ready", "rests_on=[deadline.days]",
                                "verdict=There is time to submit", "wrong_if=deadline.days < 1"],
                                cwd=self.work, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        import yaml
        record = yaml.safe_load((self.work / "PROVENANCE.yaml").read_text())
        self.assertEqual(record["judgments"]["d.ready"]["seen"], {"deadline.days": 7})

    def test_sparse_record_exception_never_hides_a_broken_judgment(self):
        import provenance as P
        for doc in (
            {"claims": {"d.ready": {"verdict": "Ready"}}},
            {"known": {"d.ready": {"rests_on": ["missing.fact"], "verdict": "Ready"}}},
            {"known": {"fact.x": {"v": 1}}, "schema": {"deps": "misspelled"}},
            {"known": {"d.ready": {"depends": ["missing.fact"]}}},
        ):
            with self.subTest(doc=doc), self.assertRaises(SystemExit):
                P.infer(doc)

    def test_enabled_checked_opening_is_not_replaced_by_legacy_output(self):
        self.write_record()
        with patch.object(S, "_run", return_value=SimpleNamespace(
                returncode=0, stdout="revision=checked-fixture\n", stderr="")) as run:
            text = S.opening({"cwd": str(self.work)})
        self.assertEqual(text, "revision=checked-fixture")
        self.assertEqual([call.args[0] for call in run.call_args_list], ["session_hook.py"])

    def test_enabled_checked_failure_never_falls_back_to_an_unchecked_view(self):
        self.write_record()
        with patch.object(S, "_run", return_value=SimpleNamespace(
                returncode=2, stdout="Checked session view unavailable", stderr="")) as run:
            text = S.opening({"cwd": str(self.work)})
        self.assertIn("Checked session view unavailable", text)
        self.assertEqual(len(run.call_args_list), 1)

    def test_named_main_agent_still_receives_its_record(self):
        self.write_record()
        with patch.object(S, "_run", return_value=SimpleNamespace(
                returncode=0, stdout="revision=main-agent\n", stderr="")):
            text = S.opening({"cwd": str(self.work), "agent_type": "planner"})
        self.assertEqual(text, "revision=main-agent")

    @unittest.skipIf(os.name == "nt", "shell hook")
    def test_malformed_hook_directory_returns_controlled_output(self):
        for bad in (12, [], {}, ""):
            with self.subTest(cwd=bad):
                result = self.hook({"session_id": "bad", "cwd": bad})
                self.assertEqual(result.returncode, 0)
                self.assertNotIn("Traceback", result.stderr)
                self.assertIn("workspace must be", result.stderr)
        result = subprocess.run([sys.executable, str(ROOT / "scripts/session_start.py"), "--cursor"],
                                input=json.dumps({"cwd": 12}), cwd=self.work, env=self.env,
                                capture_output=True, text=True)
        self.assertEqual(json.loads(result.stdout), {"additional_context": ""})

    @unittest.skipIf(os.name == "nt", "shell adapter")
    def test_cursor_first_use_json_and_first_record_stop_use_the_same_baseline(self):
        def cursor(script, payload):
            return subprocess.run(["sh", str(ROOT / "adapters/cursor/scripts" / script)],
                                  input=json.dumps(payload), cwd=self.root, env=self.env,
                                  capture_output=True, text=True)
        payload = {"conversation_id": "cursor-fixture", "cwd": str(self.work)}
        opened = cursor("gate-open.sh", payload)
        self.assertEqual(opened.returncode, 0, opened.stderr)
        self.assertIn("KPOPPER_START", json.loads(opened.stdout)["additional_context"])
        self.assertFalse((self.work / "PROVENANCE.yaml").exists())
        self.write_record(broken=True)
        stopped = cursor("gate-stop.sh", {**payload, "loop_count": 0})
        self.assertEqual((stopped.returncode, stopped.stdout, stopped.stderr), (0, "", ""))
        repeated = cursor("gate-stop.sh", {**payload, "loop_count": 1})
        self.assertEqual(repeated.stdout, "")

    @unittest.skipIf(os.name == "nt", "shell adapter")
    def test_gemini_advisory_finds_a_non_git_record_using_payload_cwd(self):
        self.write_record(broken=True)
        result = subprocess.run(["sh", str(ROOT / "adapters/gemini/scripts/checknote.sh")],
                                input=json.dumps({"cwd": str(self.work)}), cwd=self.root,
                                env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("no snapshot", result.stdout)

    @unittest.skipIf(os.name == "nt", "shell hook")
    def test_hook_uses_payload_cwd_and_leaves_only_a_temporary_baseline(self):
        result = self.hook({"session_id": "first-use", "cwd": str(self.work)}, cwd=self.root)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("KPOPPER_START", result.stdout)
        self.assertEqual(list(self.work.iterdir()), [])
        self.assertFalse(self.private.exists())
        self.assertEqual(json.loads((self.root / "kpopper-base-first-use").read_text())["ids"], [])

    @unittest.skipIf(os.name == "nt", "shell hook")
    def test_new_record_is_checked_at_stop_without_requiring_an_empty_record_at_start(self):
        self.hook()
        self.write_record(broken=True)
        result = self.hook(name="session_gate.sh")
        self.assertEqual((result.returncode, result.stdout, result.stderr), (0, "", ""))
        again = self.hook({"session_id": "first-use", "stop_hook_active": True}, name="session_gate.sh")
        self.assertEqual(again.returncode, 0)

    @unittest.skipIf(os.name == "nt", "shell hook")
    def test_existing_record_opens_without_an_unsolicited_first_use_tutorial(self):
        self.write_record()
        result = self.hook()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Home purchase", result.stdout)
        self.assertNotIn("KPOPPER_START", result.stdout)

    @unittest.skipIf(os.name == "nt", "shell hook")
    def test_subagent_does_not_offer_onboarding_or_overwrite_parent_mark(self):
        result = self.hook({"session_id": "first-use", "agent_id": "child"})
        self.assertEqual(result.stdout, "")
        self.assertFalse((self.root / "kpopper-base-first-use").exists())

    @unittest.skipIf(os.name == "nt", "shell hook")
    def test_untrusted_session_id_cannot_escape_temporary_baseline_directory(self):
        result = self.hook({"session_id": "../../outside"})
        self.assertEqual(result.returncode, 0)
        self.assertFalse((self.root.parent / "outside").exists())

    @unittest.skipIf(os.name == "nt", "shell hook")
    def test_compaction_does_not_replace_a_sessions_original_baseline(self):
        self.hook()
        before = (self.root / "kpopper-base-first-use").read_bytes()
        self.write_record()
        self.hook({"session_id": "first-use", "source": "compact"})
        self.assertEqual((self.root / "kpopper-base-first-use").read_bytes(), before)


@unittest.skipUnless(shutil.which("git"), "Git discovery")
class Locations(WorkspaceFixture, unittest.TestCase):
    def git(self, *args, cwd=None):
        result = subprocess.run(["git", *args], cwd=cwd or self.work, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        return result.stdout.strip()

    def test_nested_non_git_directory_uses_existing_record(self):
        record = self.write_record()
        nested = self.work / "contracts" / "drafts"
        nested.mkdir(parents=True)
        self.assertEqual(W.locate(nested)["record"], str(record))
        result = subprocess.run([sys.executable, str(ROOT / "scripts/provenance.py"), "where"],
                                cwd=nested, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(str(record), result.stdout)

    def test_reader_loaded_by_file_path_can_locate_a_record_without_sys_path_changes(self):
        record = self.write_record()
        nested = self.work / "notes"
        nested.mkdir()
        code = ("import importlib.util,sys; "
                "s=importlib.util.spec_from_file_location('custom_reader',sys.argv[1]); "
                "p=importlib.util.module_from_spec(s); s.loader.exec_module(p); "
                "print(p.default_paths()[0])")
        result = subprocess.run([sys.executable, "-c", code, str(ROOT / "scripts/provenance.py")],
                                cwd=nested, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), str(record))

    def test_repository_boundary_prevents_adopting_unrelated_parent_record(self):
        self.write_record(self.root)
        self.git("init", "-q")
        location = W.locate(self.work)
        self.assertEqual(location["status"], "missing")
        self.assertEqual(location["workspace"], str(self.work))

    def test_dangling_registered_record_is_unavailable_not_a_new_project(self):
        self.git("init", "-q")
        (self.work / ".git" / "kpopper-record").write_text(str(self.root / "gone.yaml"))
        location = W.locate(self.work)
        self.assertEqual(location["status"], "unavailable")
        result = self.cli()
        self.assertIn("unavailable", result.stdout)
        self.assertNotIn("Offer the three", result.stdout)
        self.assertFalse(self.private.exists())

    def test_worktrees_share_choices_and_registered_records(self):
        self.git("init", "-q")
        self.git("-c", "user.email=fixture@example.invalid", "-c", "user.name=Fixture",
                 "commit", "--allow-empty", "-m", "fixture", "-q")
        linked = self.root / "linked"
        self.git("worktree", "add", "--detach", str(linked))
        self.map()
        self.assertEqual(json.loads(self.cli("status", cwd=linked).stdout)["mode"], "map")
        record = self.write_record(self.root)
        (self.work / ".git" / "kpopper-record").write_text(str(record))
        self.assertEqual(W.locate(linked)["record"], str(record))


if __name__ == "__main__":
    unittest.main()
