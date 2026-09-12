"""Watch configuration and snapshots survive the supported record entry rename."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import yaml

from scripts import watch as W
from scripts import watch_shared as S


@unittest.skipIf(os.name == "nt", "Watch background state requires POSIX locking")
class WatchLayoutTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name).resolve()
        self.repo = self.root / "repo"
        self.repo.mkdir()
        self.env = patch.dict(os.environ, {"XDG_STATE_HOME": str(self.root / "state")})
        self.env.start()
        self.addCleanup(self.env.stop)
        self.git(self.repo, "init", "-b", "main")
        self.git(self.repo, "config", "user.email", "fixture@example.test")
        self.git(self.repo, "config", "user.name", "Fixture")

    def git(self, cwd, *args):
        result = subprocess.run(["git", *args], cwd=cwd, text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        return result.stdout.strip()

    def commit(self, cwd, message="Fixture"):
        self.git(cwd, "add", ".")
        self.git(cwd, "commit", "-m", message)

    @staticmethod
    def record(path):
        path.write_text(yaml.safe_dump({"known": {"service.limit": {"v": 1}}}, sort_keys=False))

    @staticmethod
    def hypothesis(path, value):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("hypothesis:\n  claim: Try another limit\n  folds: never\nknown:\n  trial.limit: {v: %d}\n" % value)

    def test_rename_retains_configuration_shared_queue_and_worktree_state(self):
        legacy = self.repo / "PROVENANCE.yaml"
        self.record(legacy)
        self.commit(self.repo)
        peer = self.root / "peer"
        self.git(self.repo, "worktree", "add", "-b", "peer", str(peer))

        watched = W.Watch(self.repo)
        watched.setup(base_ref="main", shared_private=True)
        old_config = watched.config_path
        old_project_state = watched.project_state
        old_shared = watched.config()["shared_record"]
        W.I._save(watched.state / "delivery" / "kept.json", {"episode": "before-rename"})
        report = {
            "id": "external.limit", "name": "External limit", "value": 4,
            "date": "2026-09-12", "scope": {"kind": "external", "environment": "fixture"},
            "source": {"url": "https://example.test/report", "at": "limit"},
            "source_quote": "The external limit is four.", "event_id": "before-rename",
        }
        with patch.object(W, "launch"):
            S.capture(watched, report)

        legacy.rename(self.repo / "GROUNDING.yaml")
        renamed = W.Watch(self.repo)
        peer_watch = W.Watch(peer)

        self.assertEqual(renamed.config_path, old_config)
        self.assertEqual(renamed.project_state, old_project_state)
        self.assertEqual(renamed.state, watched.state)
        self.assertEqual(renamed.config()["entry"], "GROUNDING.yaml")
        self.assertTrue(renamed.config()["enabled"])
        self.assertEqual(renamed.config()["shared_record"], old_shared)
        self.assertEqual(W.I._load(renamed.state / "delivery" / "kept.json")["episode"], "before-rename")
        self.assertEqual([r["id"] for r in S.receipts(renamed)], ["before-rename"])
        self.assertEqual(peer_watch.config_path, old_config)
        self.assertEqual(peer_watch.project_state, old_project_state)
        self.assertEqual(peer_watch.config()["entry"], "PROVENANCE.yaml")
        self.assertEqual([r["id"] for r in S.receipts(peer_watch)], ["before-rename"])
        self.assertEqual(renamed.status()["state"], "pending")
        with patch.object(W, "launch"):
            peer_watch.request()
            worktrees = renamed.request_all()["worktrees"]
        self.assertEqual({item["workspace"] for item in worktrees}, {str(self.repo), str(peer)})

    def test_dual_alias_configs_refuse_to_orphan_either_state(self):
        legacy = self.repo / "PROVENANCE.yaml"
        self.record(legacy)
        self.commit(self.repo)
        watched = W.Watch(self.repo)
        watched.setup(base_ref="main", shared_private=True)
        delivery = watched.state / "delivery" / "kept.json"
        W.I._save(delivery, {"episode": "legacy"})
        report = {
            "id": "external.limit", "name": "External limit", "value": 4,
            "date": "2026-09-12", "scope": {"kind": "external", "environment": "fixture"},
            "source": {"url": "https://example.test/report", "at": "limit"},
            "source_quote": "The external limit is four.", "event_id": "legacy-queue",
        }
        with patch.object(W, "launch"):
            receipt = Path(S.capture(watched, report)["receipt"])
        legacy.rename(self.repo / "GROUNDING.yaml")

        modern_entry = "GROUNDING.yaml"
        modern_config = watched.common / "kpopper-watch" / (W.digest(modern_entry) + ".json")
        W.I._save(modern_config, {"base_ref": "refs/heads/main", "enabled": True,
                                 "entry": modern_entry, "schema": 1, "shared_record": None})
        before = {path: path.read_bytes() for path in (watched.config_path, modern_config, receipt, delivery)}

        with self.assertRaisesRegex(ValueError, "multiple watch configurations.*reconcile"):
            W.Watch(self.repo)
        self.assertEqual({path: path.read_bytes() for path in before}, before)

    def test_records_in_separate_directories_keep_independent_configs(self):
        self.record(self.repo / "GROUNDING.yaml")
        nested = self.repo / "nested"
        nested.mkdir()
        self.record(nested / "PROVENANCE.yaml")
        self.commit(self.repo)

        outer = W.Watch(self.repo)
        inner = W.Watch(nested)
        outer.setup(base_ref="main")
        inner.setup(base_ref="main")

        self.assertNotEqual(outer.config_path, inner.config_path)
        self.assertEqual(outer.config()["entry"], "GROUNDING.yaml")
        self.assertEqual(inner.config()["entry"], "nested/PROVENANCE.yaml")

    def test_each_ref_resolves_legacy_and_modern_entry_and_hypotheses(self):
        cases = [
            ("PROVENANCE.yaml", "PROVENANCE.d", "GROUNDING.yaml", ".kpopper/hypotheses"),
            ("GROUNDING.yaml", ".kpopper/hypotheses", "PROVENANCE.yaml", "PROVENANCE.d"),
        ]
        for index, (base_entry, base_hypotheses, working_entry, working_hypotheses) in enumerate(cases):
            with self.subTest(base=base_entry, working=working_entry):
                case = self.root / ("case-%d" % index)
                case.mkdir()
                self.git(case, "init", "-b", "main")
                self.git(case, "config", "user.email", "fixture@example.test")
                self.git(case, "config", "user.name", "Fixture")
                self.record(case / base_entry)
                self.hypothesis(case / base_hypotheses / "trial.yaml", 1)
                self.commit(case)
                work = self.root / ("work-%d" % index)
                self.git(case, "worktree", "add", "-b", "change-%d" % index, str(work))
                before = W.Watch(work)
                before.setup(base_ref="main")

                (work / base_entry).rename(work / working_entry)
                target = work / working_hypotheses
                target.parent.mkdir(parents=True, exist_ok=True)
                (work / base_hypotheses).rename(target)
                self.hypothesis(target / "trial.yaml", 2)

                after = W.Watch(work)
                after.setup(base_ref="main")
                snapshot = after.snapshot()
                self.assertEqual(snapshot["working"]["hypotheses"][0]["doc"]["known"]["trial.limit"]["v"], 2)
                self.assertEqual(snapshot["ancestor"]["hypotheses"][0]["doc"]["known"]["trial.limit"]["v"], 1)
                self.assertEqual(snapshot["main"]["hypotheses"][0]["doc"]["known"]["trial.limit"]["v"], 1)
                self.assertIn(after.process()["state"], {"clear", "attention"})


if __name__ == "__main__":
    unittest.main()
