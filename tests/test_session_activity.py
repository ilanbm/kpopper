"""Session attribution follows published writes; hook delivery is independent of validation."""
import contextlib
import concurrent.futures
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import yaml
from scripts import provenance as P, history_transaction as T

ROOT = Path(__file__).resolve().parents[1]


@unittest.skipUnless(os.name == 'posix', 'shell hooks and record locking')
class SessionActivity(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name).resolve()
        self.work = self.root / 'work'
        self.work.mkdir()
        self.record = self.work / 'GROUNDING.yaml'
        self.doc = {'sources': {'s.file': {'file': 'source.txt'}},
                    'known': {'p.old': {'v': 1, 'from': 's.file'}}}
        self.save()
        self.sid = 'session-a'
        self.env = {**os.environ, 'TMPDIR': str(self.root),
                    'KPOPPER_AGENT_SESSION': self.sid, 'CODEX_THREAD_ID': '',
                    'KPOPPER_READ_MODE': 'frozen', 'KPOPPER_SESSION_DISABLE': '1'}
        self.addCleanup(patch.stopall)
        patch.dict(os.environ, self.env).start()
        self.mark = self.root / ('kpopper-base-' + self.sid)
        P.mark(str(self.mark), [str(self.record)])

    def save(self):
        self.record.write_text(yaml.safe_dump(self.doc, sort_keys=False))

    def add(self, name='p.new', **extra):
        action = {'kind': 'add', 'id': name, 'body': {'v': 2, 'from': 's.file'}, **extra}
        with contextlib.redirect_stdout(io.StringIO()):
            return P.apply([str(self.record)], action)

    def diagnostics(self):
        return subprocess.run([sys.executable, str(ROOT / 'scripts/provenance.py'), 'gate', str(self.mark), str(self.record), '--session', self.sid, '--host', 'codex'],
            input=json.dumps({'session_id': self.sid, 'cwd': str(self.work)}),
            cwd=self.root, env=self.env, text=True, capture_output=True, timeout=30)

    def gate(self):
        with contextlib.redirect_stdout(io.StringIO()) as output:
            code = P.gate(str(self.mark), [str(self.record)])
        return code, output.getvalue()

    def git(self, *args):
        return subprocess.run(['git', *args], cwd=self.work, text=True, capture_output=True, check=True)

    def test_imported_commit_is_not_session_authorship(self):
        self.git('init', '-q')
        self.git('add', '.')
        self.git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.test', 'commit', '-qm', 'base')
        self.doc['sources']['s.imported'] = {'file': 'upstream.txt'}
        self.doc['known']['p.imported'] = {'v': 3, 'from': 's.imported'}
        self.save()
        self.git('add', '.')
        self.git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.test', 'commit', '-qm', 'upstream')
        incoming = self.git('rev-parse', 'HEAD').stdout.strip()
        self.git('checkout', '-q', 'HEAD~1')
        P.mark(str(self.mark), [str(self.record)])
        self.git('checkout', '-q', incoming)
        stopped = self.diagnostics()
        self.assertEqual(stopped.returncode, 0, stopped.stdout)
        self.assertNotIn('this session wrote', stopped.stdout)

    def test_owned_write_still_warns_after_commit_and_delivers_once(self):
        self.git('init', '-q')
        self.add()
        self.git('add', '.')
        self.git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.test', 'commit', '-qm', 'local')
        first, second = self.diagnostics(), self.diagnostics()
        self.assertEqual(first.returncode, 2, first.stdout)
        self.assertIn('p.new', first.stdout)
        self.assertEqual(second.returncode, 0, second.stdout)
        self.assertEqual(second.stdout, '')
        # Delivery history never turns canonical validation into success.
        self.assertEqual(self.gate()[0], 2)
        self.assertEqual(self.gate()[0], 2)

    def test_new_problem_warns_without_repeating_old_problem(self):
        self.add('p.first')
        self.assertEqual(self.diagnostics().returncode, 2)
        self.add('p.second')
        again = self.diagnostics()
        self.assertEqual(again.returncode, 2, again.stdout)
        self.assertIn('p.second', again.stdout)
        self.assertNotIn('p.first', again.stdout)

    def test_direct_edit_other_session_and_replaced_body_are_not_owned(self):
        self.doc['known']['p.manual'] = {'v': 5}
        self.save()
        with patch.dict(os.environ, {'KPOPPER_AGENT_SESSION': 'session-b'}):
            self.add('p.other')
        self.add('p.mine')
        doc = yaml.safe_load(self.record.read_text())
        doc['known']['p.mine']['v'] = 99
        self.record.write_text(yaml.safe_dump(doc))
        self.assertEqual(self.diagnostics().returncode, 0)

    def test_failed_write_has_no_authorship_receipt(self):
        with patch.object(T, 'publish_legacy', side_effect=OSError('publication failed')):
            with self.assertRaisesRegex(OSError, 'publication failed'):
                self.add()
        # An unrelated later edit must not inherit the failed write's ownership.
        self.doc['known']['p.new'] = {'v': 2, 'from': 's.file'}
        self.save()
        self.assertEqual(self.diagnostics().returncode, 0)

    def test_validation_failure_is_not_repeated_or_lost(self):
        self.doc['schema'] = {'deps': 'rests_on', 'snapshot': 'seen', 'predicate': 'wrong_if'}
        self.doc['judgments'] = {'d.broken': {'verdict': 'broken', 'rests_on': ['p.missing'], 'seen': {}}}
        self.save()
        self.assertEqual(self.diagnostics().returncode, 2)
        self.assertEqual(self.diagnostics().returncode, 0)
        self.assertEqual(self.gate()[0], 2)
        self.assertTrue(P.check_lines([str(self.record)])[0])

    def test_resume_preserves_delivery_state(self):
        self.add()
        self.assertEqual(self.diagnostics().returncode, 2)
        resumed = subprocess.run([sys.executable, str(ROOT / 'scripts/session_start.py'), '--host', 'codex'],
            input=json.dumps({'session_id': self.sid, 'cwd': str(self.work), 'source': 'resume'}),
            cwd=self.root, env=self.env, text=True, capture_output=True, timeout=30)
        self.assertEqual(resumed.returncode, 0, resumed.stdout)
        self.assertEqual(self.diagnostics().returncode, 0)

    def test_untouched_legacy_and_core_records_never_continue_the_turn(self):
        for core in (False, True):
            with self.subTest(core=core):
                self.sid = 'untouched-core' if core else 'untouched-legacy'
                self.mark = self.root / ('kpopper-base-' + self.sid)
                if core:
                    self.doc['meta'] = {'reasoning': {'version': 1, 'profile': 'core/v1',
                                                     'requires': ['arithmetic/v1']}}
                    self.save()
                P.mark(str(self.mark), [str(self.record)])
                with contextlib.redirect_stdout(io.StringIO()) as output:
                    code = P.gate(str(self.mark), [str(self.record)], turns=9, nudged_at=8)
                self.assertEqual((code, output.getvalue()), (0, ''))
                (self.root / ('kpopper-ground-' + self.sid + '.json')).write_text(
                    json.dumps({'turns': 9, 'nudged_turn': 8}))
                result = self.diagnostics()
                self.assertEqual((result.returncode, result.stdout, result.stdout), (0, '', ''))

    def test_untouched_history_record_never_continues_the_turn(self):
        from tests.test_history_snapshot_capture import HistorySnapshotCapture
        fixture = HistorySnapshotCapture()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        self.record, self.work = fixture.entry, fixture.entry.parent
        P.mark(str(self.mark), [str(self.record)])
        with contextlib.redirect_stdout(io.StringIO()) as output:
            code = P.gate(str(self.mark), [str(self.record)], turns=9, nudged_at=8)
        self.assertEqual((code, output.getvalue()), (0, ''))
        (self.root / ('kpopper-ground-' + self.sid + '.json')).write_text(
            json.dumps({'turns': 9, 'nudged_turn': 8}))
        result = self.diagnostics()
        self.assertEqual((result.returncode, result.stdout, result.stdout), (0, '', ''))

    def test_named_hypothesis_write_is_owned(self):
        self.add('p.hypothesis', hypothesis='proposal')
        first = self.diagnostics()
        self.assertEqual(first.returncode, 2, first.stdout)
        self.assertIn('p.hypothesis', first.stdout)
        self.assertEqual(self.diagnostics().returncode, 0)

    def test_pointer_and_alias_keep_the_actual_written_file(self):
        member = self.work / 'member.yaml'
        self.record.rename(member)
        self.record.write_text('record: member.yaml\n')
        P.mark(str(self.mark), [str(self.record)])
        self.add()
        self.assertEqual(self.diagnostics().returncode, 2)
        self.assertEqual(self.diagnostics().returncode, 0)

    def test_copied_body_cannot_borrow_another_records_write_receipt(self):
        other = self.work / 'other.yaml'
        other.write_bytes(self.record.read_bytes())
        with contextlib.redirect_stdout(io.StringIO()):
            P.apply([str(other)], {'kind': 'add', 'id': 'p.new', 'body': {'v': 2, 'from': 's.file'}})
        self.record.write_bytes(other.read_bytes())
        self.assertEqual(self.diagnostics().returncode, 0)

    def test_concurrent_diagnostics_deliver_each_problem_once(self):
        self.add()
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
            results = list(pool.map(lambda _: self.diagnostics(), range(2)))
        self.assertEqual(sorted(p.returncode for p in results), [0, 2], [p.stdout for p in results])

    def test_cli_session_binding_and_delivery_receipt(self):
        result = subprocess.run([sys.executable, str(ROOT / 'scripts/cli.py'), 'add', 'p.new',
            'v=2', 'from=s.file'], cwd=self.work, env=self.env, text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertEqual(self.diagnostics().returncode, 2)
        self.assertEqual(self.diagnostics().returncode, 0)

    def test_codex_thread_identity_binds_a_write_without_an_explicit_override(self):
        with patch.dict(os.environ, {'KPOPPER_AGENT_SESSION': '', 'CODEX_THREAD_ID': self.sid}):
            self.add()
        self.assertEqual(self.diagnostics().returncode, 2)

    def test_new_session_has_independent_delivery_history(self):
        self.doc['schema'] = {'deps': 'rests_on', 'snapshot': 'seen', 'predicate': 'wrong_if'}
        self.doc['judgments'] = {'d.broken': {'verdict': 'broken', 'rests_on': ['p.missing'], 'seen': {}}}
        self.save()
        self.assertEqual(self.diagnostics().returncode, 2)
        self.sid = 'session-b'
        (self.root / ('kpopper-base-' + self.sid)).write_bytes(self.mark.read_bytes())
        self.assertEqual(self.diagnostics().returncode, 2)

    def test_active_history_direct_write_is_owned(self):
        from scripts import session_activity as activity, history_adapter
        from tests.test_history_snapshot_capture import HistorySnapshotCapture, claim
        fixture = HistorySnapshotCapture()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        self.record, self.work = fixture.entry, fixture.entry.parent
        P.mark(str(self.mark), [str(self.record)])
        stale = self.record.read_bytes()
        fixture.publish([claim('p.imported', operation='imported')], 'imported')
        current = self.record.read_bytes()
        self.record.write_bytes(stale)
        with self.assertRaisesRegex(ValueError, 'stale_view'), contextlib.redirect_stdout(io.StringIO()):
            P.apply([str(self.record)], {'kind': 'add', 'id': 'p.new', 'body': {'v': 2}})
        self.record.write_bytes(current)
        with contextlib.redirect_stdout(io.StringIO()):
            P.apply([str(self.record)], {'kind': 'add', 'id': 'p.new', 'body': {'v': 2}})
        # Verify against the authoritative projection as well as the public Stop hook.
        doc = P.Record(history_adapter.from_store_capture(fixture.store.capture()).document)
        doc.origins = {section: {nid: str(self.record) for nid in members}
                       for section, members in P.collections_of(doc).items()}
        self.assertIn('p.imported', P.bodies(doc))
        self.assertEqual(activity.owned(P, doc, self.sid), {'p.new'})
        first = self.diagnostics()
        self.assertEqual(first.returncode, 2, first.stdout)
        self.assertIn('p.new', first.stdout)
        self.assertNotIn('p.imported', first.stdout)
        self.assertEqual(self.diagnostics().returncode, 0)

    def test_unavailable_receipt_storage_does_not_fail_a_committed_write(self):
        from scripts import session_activity as activity
        with patch.object(activity, '_update', side_effect=OSError('unavailable')):
            self.assertEqual(self.add(), 0)
        self.assertIn('p.new', P.bodies(P.load([str(self.record)])))
        self.assertEqual(self.diagnostics().returncode, 0)

    def test_private_draft_is_not_session_authorship(self):
        with patch.dict(os.environ, {'KPOPPER_PRIVATE_HOME': str(self.root / 'private')}):
            self.assertEqual(self.add(shareability='private'), 0)
        self.doc['known']['p.new'] = {'v': 2, 'from': 's.file'}
        self.save()
        self.assertEqual(self.diagnostics().returncode, 0)

    def test_delivery_storage_failure_does_not_change_admission(self):
        from scripts import session_activity as activity
        self.add()
        with patch.object(activity, '_update', side_effect=OSError('unavailable')):
            self.assertEqual(activity.stop(P, str(self.mark), [str(self.record)], self.sid), 0)
            self.assertEqual(self.gate()[0], 2)

    def test_missing_or_corrupt_ownership_state_leaves_authorship_unknown(self):
        from scripts import session_activity as activity
        self.add()
        path = activity._home(self.sid) / ('writes-' + activity._key(self.record) + '.json')
        for value in ('{broken', '[]', '{"p.new": {"unexpected": true}}'):
            path.write_text(value)
            self.assertEqual(self.diagnostics().returncode, 0)
        path.unlink()
        self.assertEqual(self.diagnostics().returncode, 0)

    def core_record(self):
        self.doc['meta'] = {'reasoning': {'version': 2, 'profile': 'core/v1',
                                         'requires': ['arithmetic/v1']}}
        self.save()
        P.mark(str(self.mark), [str(self.record)])

    def test_core_imports_are_not_owned_but_direct_writes_are(self):
        self.core_record()
        self.doc['known']['p.imported'] = {'v': 3}
        self.save()
        self.assertEqual(self.diagnostics().returncode, 0)
        self.add()
        first = self.diagnostics()
        self.assertEqual(first.returncode, 2, first.stdout)
        self.assertIn('p.new', first.stdout)
        self.assertNotIn('p.imported', first.stdout)
        self.assertEqual(self.diagnostics().returncode, 0)
        self.assertEqual(self.gate()[0], 2)

    def test_core_capture_failure_is_delivered_once_and_remains_invalid(self):
        self.core_record()
        marker = Path(P.layout(self.record)['history_authority'])
        marker.parent.mkdir(exist_ok=True)
        marker.write_text('invalid: marker\n')
        first = self.diagnostics()
        self.assertEqual(first.returncode, 2, first.stdout)
        self.assertIn('capture failure', first.stdout)
        self.assertEqual(self.diagnostics().returncode, 0)
        self.assertEqual(self.gate()[0], 2)

    def test_first_default_history_write_has_a_session_receipt(self):
        from scripts.session_start import EMPTY_MARK
        self.record.unlink()
        self.mark.write_text(json.dumps(EMPTY_MARK))
        result = subprocess.run([sys.executable, str(ROOT / 'scripts/cli.py'), 'add', 'p.new', 'v=2'],
            cwd=self.work, env=self.env, text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stdout)
        self.assertEqual(yaml.safe_load(self.record.read_text())['meta']['reasoning']['profile'], 'core/v1')
        first = self.diagnostics()
        self.assertEqual(first.returncode, 2, first.stdout)
        self.assertIn('p.new', first.stdout)
        self.assertEqual(self.diagnostics().returncode, 0)


if __name__ == '__main__':
    unittest.main()
