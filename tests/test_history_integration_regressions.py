"""Concrete store/reader/recovery regressions at real callable boundaries."""
import contextlib
import copy
import io
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

from scripts import history_contract as C, history_store as H, history_transaction as T
from scripts import history_adapter as A, provenance as P
from scripts.pending_grounding import identity
from tests import test_history_store as fixtures
from tests import test_history_contract as adapter_fixtures


class IntegrationRegressions(unittest.TestCase):
    def setUp(self):
        self.fixture = fixtures.Storage()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)

    def test_glob_metacharacters_in_record_directory_never_hide_history(self):
        f = self.fixture
        root = f.entry.parent / 'notes [2026]'
        root.mkdir()
        shutil.move(str(f.entry.parent / '.kpopper'), root / '.kpopper')
        shutil.move(str(f.entry), root / 'GROUNDING.yaml')
        f.entry = root / 'GROUNDING.yaml'
        f.store = H.Store(f.entry)
        claim = fixtures.claim()
        f.publish([claim])
        captured = f.store.capture()
        self.assertEqual(set(captured.commits), {'operation'})
        self.assertEqual(captured.state['subjects']['p.input']['heads'], [claim['id']])
        self.assertEqual(C.decode_document(f.store.rebuild())['meta']['history'], captured.baseline)

    def test_sequential_commits_reference_only_frontier(self):
        f = self.fixture
        for index in range(8):
            f.publish([fixtures.claim('p.input' + str(index), op='claim-' + str(index))], 'op-' + str(index))
        captured = f.store.capture()
        manifests = [C.decode_document(raw) for raw in captured.commits.values()]
        self.assertEqual(sum(len(item['parents']) for item in manifests), 7)
        self.assertEqual(set(C.commit_frontier(captured.commits)), {'op-7'})
        self.assertEqual(len(captured.objects), 8)

    def test_noncanonical_after_view_refuses_before_any_objects_are_published(self):
        f = self.fixture
        mutation = f.mutation([fixtures.claim()])
        data, files = mutation.to_data(), mutation.files
        record = next(item for item in files if item['role'] == 'record')
        record['after'] += b'\n# alternate generated serialization\n'
        commit = next(item for item in files if item['role'] == 'history_commit')
        manifest = C.decode_document(commit['after'])
        manifest['view_sha256'] = C.sha256(record['after'])
        commit['after'] = C.encode_document(manifest)
        altered = T.PreparedMutation(operation=data['operation'], authority=data['authority'],
            baseline=data['baseline'], files=files, receipt=data['receipt'])
        with self.assertRaisesRegex(C.HistoryError, 'view_projection_mismatch'):
            f.store.commit(altered, verify=lambda _: None)
        self.assertEqual(f.store.capture().commits, {})

    def test_rebuilt_known_view_can_recover_after_next_failed_refresh(self):
        f = self.fixture
        first = fixtures.claim()
        f.publish([first])
        f.store.rebuild(write=True)
        review = fixtures.act(first, 'review', op='review')
        mutation = f.mutation([review], 'second')
        with mock.patch.object(T, '_replace', side_effect=OSError('crash')):
            with self.assertRaisesRegex(OSError, 'crash'):
                f.store.commit(mutation, verify=lambda _: None)
        f.store.rebuild(write=True)
        self.assertEqual(f.entry.read_bytes(), f.store.render(f.store.capture()))

    def test_removed_manifest_cannot_return_empty_state_from_nonempty_view(self):
        f = self.fixture
        f.publish([fixtures.claim()])
        (Path(f.store.layout['history_commits']) / 'operation.yaml').unlink()
        with self.assertRaisesRegex(C.HistoryError, 'incomplete_view_baseline'):
            f.store.capture()

    def test_rebuilt_union_remains_recoverable_after_later_interrupted_commit(self):
        f = self.fixture
        f.publish([fixtures.claim()], 'root')
        first = f.mutation([fixtures.claim('p.a', op='a')], 'branch-a')
        second = f.mutation([fixtures.claim('p.b', op='b')], 'branch-b')
        f.store.commit(first, verify=lambda _: None)
        # A Git-style union introduces the second complete branch independently.
        for item in second.files:
            if item['role'].startswith('history_'):
                T.publish_immutable(f.entry.parent / item['path'], item['after'], root=f.entry.parent)
        f.store.rebuild(write=True)
        next_write = f.mutation([fixtures.claim('p.c', op='c')], 'next')
        with mock.patch.object(T, '_replace', side_effect=OSError('crash')):
            with self.assertRaises(OSError):
                f.store.commit(next_write, verify=lambda _: None)
        f.store.rebuild(write=True)
        self.assertIn('p.c', C.decode_document(f.entry.read_bytes())['readings'])

    def test_authored_locator_does_not_turn_agreement_into_contention(self):
        first = adapter_fixtures.claim()
        second = copy.deepcopy(first)
        second['op'] = 'different-observation'
        second['authored']['locator'] = {'file': 'second-source.yaml', 'line': 7}
        second['id'] = C.object_identity(second)
        state = H.reduce({obj['id']: obj for obj in (first, second)})
        self.assertEqual(state['subjects']['p.input']['acceptance'], 'accepted')
        projection = adapter_fixtures.projection(first)
        heads = sorted([first['id'], second['id']])
        projection['subjects']['p.input']['heads'] = heads
        projection['baseline']['heads']['p.input'] = heads
        capture = A.capture_history({obj['id']: obj for obj in (first, second)}, projection,
            document={'meta': {'reasoning': {'version': 2, 'profile': 'core/v1', 'requires': ['arithmetic/v1']}}})
        self.assertEqual(capture.document['readings']['p.input']['v'], 1)

    def test_adapter_template_collection_collision_is_explicit(self):
        obj = adapter_fixtures.claim()
        with self.assertRaisesRegex(C.HistoryError, 'unresolved_mapping'):
            A.capture_history({obj['id']: obj}, adapter_fixtures.projection(obj),
                              document={'readings': 'header collision'})

    def test_structured_predicate_matches_legacy_text_only_during_comparison(self):
        predicate = {'op': 'gt', 'args': [{'ref': 'p.value'}, {'num': '1'}]}
        structured = {'verdict': 'yes', 'wrong_if': predicate}
        old = {'verdict': 'yes', 'wrong_if': P.predicate_text(predicate), 'day': '2026-09-17'}
        self.assertEqual(P._version_core(structured), P._version_core(old))
        self.assertEqual(structured['wrong_if'], predicate)


class DirectRecoveryCommand(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name).resolve()
        (self.root / 'records').mkdir()
        (self.root / '.kpopper').mkdir()
        self.entry = self.root / 'records' / 'GROUNDING.yaml'
        self.entry.write_text('known:\n  p.value: {v: 2}\n')
        self.caller = self.root / 'GROUNDING.yaml'
        self.caller.write_text('known:\n  p.other: {v: 1}\n')
        (self.root / '.kpopper/project.json').write_text(json.dumps({
            'version': 1, 'mode': 'simple', 'record': str(self.entry), 'publication': None, 'generation': 0}))

    def interrupt(self):
        original = T._replace
        def failing(path, data):
            if path == self.entry:
                raise OSError('interrupted')
            return original(path, data)
        with mock.patch.object(T, '_replace', side_effect=failing), contextlib.redirect_stdout(io.StringIO()):
            with self.assertRaisesRegex(OSError, 'interrupted'):
                P.apply([str(self.caller)], {'kind': 'set', 'id': 'p.value', 'value': 5})

    def test_same_caller_path_routes_recovery_to_configured_record(self):
        self.interrupt()
        P.recover_direct([str(self.caller)])
        self.assertEqual(P.load([str(self.caller)])['known']['p.value']['v'], 5)

    def test_actual_cli_recovers_and_reports_no_pending_recovery(self):
        self.interrupt()
        cli = Path(__file__).resolve().parents[1] / 'scripts/cli.py'
        command = [sys.executable, str(cli), 'recover', '--record', str(self.caller), '--json']
        result = subprocess.run(command, cwd=self.root, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(json.loads(result.stdout)['state'], 'recovered')
        again = subprocess.run(command, cwd=self.root, capture_output=True, text=True)
        self.assertEqual(again.returncode, 1)
        self.assertEqual(json.loads(again.stdout)['code'], 'no_recovery_pending')

    def test_invalid_sibling_journal_reports_its_actual_path(self):
        journal = self.entry.parent / '.kpopper/.history-local/stray.json'
        journal.parent.mkdir(parents=True)
        journal.write_text('not a journal')
        with self.assertRaises(P.Refused) as caught:
            P.load([str(self.entry)], read_mode='frozen')
        self.assertIn('invalid_pending_journal', str(caught.exception))
        self.assertIn(str(journal), str(caught.exception))


if __name__ == '__main__':
    unittest.main()
