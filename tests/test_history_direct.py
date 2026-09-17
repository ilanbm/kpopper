"""Direct authoring publishes one recoverable, fully validated legacy generation."""
import contextlib
import copy
import io
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import yaml
from scripts import provenance as P, history_transaction as T


class DirectTransactions(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.entry = Path(self.temp.name).resolve() / 'GROUNDING.yaml'
        self.doc = {'known': {'p.value': {'v': 2}}, 'judgments': {'c.answer': {
            'verdict': 'old', 'rests_on': ['p.value'], 'wrong_if': 'p.value > 1',
            'seen': {'p.value': 1}}}}
        self.entry.write_text(yaml.safe_dump(self.doc, sort_keys=False))
        self.action = {'kind': 'add', 'id': 'c.answer', 'body': {
            'verdict': 'new', 'rests_on': ['p.value'], 'wrong_if': 'p.value > 3'}}

    def apply(self, action=None):
        with contextlib.redirect_stdout(io.StringIO()):
            return P.apply([str(self.entry)], copy.deepcopy(action or self.action))

    def interrupt(self):
        original = T._replace
        def injected(path, data):
            if path == self.entry:
                raise OSError('injected record publication failure')
            return original(path, data)
        with patch.object(T, '_replace', side_effect=injected):
            with self.assertRaisesRegex(OSError, 'injected record'):
                self.apply()
        return self.entry.parent / T.journal_for(self.entry)

    def test_publication_failure_blocks_reader_and_other_direct_writer(self):
        before = self.entry.read_bytes()
        journal = self.interrupt()
        self.assertEqual(self.entry.read_bytes(), before)
        self.assertTrue(Path(P.replaced_path([str(self.entry)])).is_file())
        self.assertTrue(journal.is_file())
        with self.assertRaisesRegex((ValueError, P.Refused), 'recovery_required'):
            P.load([str(self.entry)])
        with self.assertRaisesRegex((ValueError, P.Refused), 'recovery_required'):
            self.apply({'kind': 'set', 'id': 'p.value', 'value': 5})
        self.assertEqual(self.entry.read_bytes(), before)

    def test_recovery_publishes_exact_prepared_operation(self):
        journal = self.interrupt()
        prepared = T.PreparedMutation.from_bytes(journal.read_bytes())
        P.recover_direct([str(self.entry)])
        self.assertFalse(journal.exists())
        for item in prepared.files:
            self.assertEqual((self.entry.parent / item['path']).read_bytes(), item['after'])
        self.assertEqual(P.bodies(P.load([str(self.entry)]))['c.answer']['verdict'], 'new')

    def test_recovery_refuses_unrelated_edit(self):
        journal = self.interrupt()
        changed = self.entry.read_bytes() + b'\n# unrelated edit\n'
        self.entry.write_bytes(changed)
        with self.assertRaisesRegex(ValueError, 'concurrent_edit'):
            P.recover_direct([str(self.entry)])
        self.assertEqual(self.entry.read_bytes(), changed)
        self.assertTrue(journal.exists())

    def test_archive_preserves_structured_predicate_and_seen(self):
        self.doc['meta'] = {'reasoning': {'version': 2, 'profile': 'core/v1', 'requires': ['arithmetic/v1']}}
        old = self.doc['judgments']['c.answer']
        old['wrong_if'] = {'expr': 'p.value > 1'}
        old['seen'] = {'p.value': 1}
        self.entry.write_text(yaml.safe_dump(self.doc, sort_keys=False))
        self.apply()
        archived = P.read_replaced([str(self.entry)])['c.answer'][0]
        self.assertEqual(archived['wrong_if'], old['wrong_if'])
        self.assertEqual(archived['seen'], old['seen'])

    def test_candidate_failure_never_publishes_archive_or_record(self):
        before = self.entry.read_bytes()
        with patch.object(P, '_report', side_effect=ValueError('candidate assessment failed')):
            with self.assertRaisesRegex(ValueError, 'candidate assessment failed'):
                self.apply()
        self.assertEqual(self.entry.read_bytes(), before)
        self.assertFalse(Path(P.replaced_path([str(self.entry)])).exists())
        self.assertFalse((self.entry.parent / T.journal_for(self.entry)).exists())

    def test_history_authority_refuses_yaml_only_direct_write(self):
        before = self.entry.read_bytes()
        marker = Path(P.layout(self.entry)['history_authority'])
        marker.parent.mkdir(parents=True)
        marker.write_text('authority: history\n')
        with self.assertRaisesRegex(P.Refused, 'invalid_schema|history_direct_writer_unsupported'):
            self.apply({'kind': 'set', 'id': 'p.value', 'value': 5})
        self.assertEqual(self.entry.read_bytes(), before)

    def test_inactive_authority_allows_legacy_write_without_reading_history(self):
        from scripts import history_contract as C
        marker = Path(P.layout(self.entry)['history_authority'])
        marker.parent.mkdir(parents=True)
        raw = C.encode_document(C.authority(record_id='retained-history', authority='legacy', generation=3))
        marker.write_bytes(raw)
        history = Path(P.layout(self.entry)['history'])
        history.mkdir()
        (history / 'inactive-evidence').write_bytes(b'retained; not reactivated')
        self.apply({'kind': 'set', 'id': 'p.value', 'value': 5})
        self.assertEqual(P.load([str(self.entry)])['known']['p.value']['v'], 5)
        self.assertEqual(marker.read_bytes(), raw)
        self.assertEqual((history / 'inactive-evidence').read_bytes(), b'retained; not reactivated')

    def test_core_archive_retains_typed_original_seen(self):
        self.doc.pop('judgments')
        self.entry.write_text(yaml.safe_dump(self.doc, sort_keys=False))
        self.apply({'kind': 'add', 'id': 'c.typed', 'profile': 'core/v1', 'body': {
            'verdict': 'old', 'rests_on': ['p.value'], 'wrong_if': 'p.value > 3'}})
        old = copy.deepcopy(yaml.safe_load(self.entry.read_text())['judgments']['c.typed'])
        self.apply({'kind': 'set', 'id': 'p.value', 'value': 4})
        self.apply({'kind': 'add', 'id': 'c.typed', 'body': {
            'verdict': 'new', 'rests_on': ['p.value'], 'wrong_if': 'p.value > 5'}})
        archived = P.read_replaced([str(self.entry)])['c.typed'][0]
        self.assertIn('computed', old['seen']['p.value'])
        self.assertEqual(archived['seen'], old['seen'])
        self.assertEqual(archived['wrong_if'], old['wrong_if'])

    def test_pointer_member_and_archive_publish_together(self):
        shard = self.entry.parent / 'judgments.yaml'
        shard.write_text(yaml.safe_dump({'judgments': self.doc.pop('judgments')}, sort_keys=False))
        self.doc['record'] = shard.name
        self.entry.write_text(yaml.safe_dump(self.doc, sort_keys=False))
        original = T._replace
        def injected(path, data):
            if path == shard:
                raise OSError('injected shard publication failure')
            return original(path, data)
        with patch.object(T, '_replace', side_effect=injected):
            with self.assertRaisesRegex(OSError, 'injected shard'):
                self.apply()
        journal = self.entry.parent / T.journal_for(self.entry)
        mutation = T.PreparedMutation.from_bytes(journal.read_bytes())
        self.assertIn('record_member', {item['role'] for item in mutation.files})
        with self.assertRaisesRegex((ValueError, P.Refused), 'recovery_required'):
            P.load([str(shard)])
        with self.assertRaisesRegex((ValueError, P.Refused), 'recovery_required'):
            P.apply([str(shard)], {'kind': 'review', 'id': 'c.answer'})
        P.recover_direct([str(self.entry)])
        self.assertEqual(P.bodies(P.load([str(self.entry)]))['c.answer']['verdict'], 'new')

    def test_changed_pointer_membership_refuses_recovery(self):
        self.interrupt()
        other = self.entry.parent / 'other.yaml'
        other.write_text('known: {p.other: {v: 3}}\n')
        self.entry.write_text(self.entry.read_text() + '\nrecord: other.yaml\n')
        with self.assertRaisesRegex(ValueError, 'concurrent_edit'):
            P.recover_direct([str(self.entry)])

    def test_nested_member_publishes_with_its_archive(self):
        outside = self.entry.parent / 'nested'
        outside.mkdir()
        shard = outside / 'judgments.yaml'
        shard.write_text(yaml.safe_dump({'judgments': self.doc.pop('judgments')}, sort_keys=False))
        self.doc['record'] = 'nested/judgments.yaml'
        self.entry.write_text(yaml.safe_dump(self.doc, sort_keys=False))
        self.apply()
        self.assertTrue(Path(P.replaced_path([str(self.entry)])).exists())
        self.assertEqual(P.bodies(P.load([str(self.entry)]))['c.answer']['verdict'], 'new')

    def test_section_review_prepares_view_bytes(self):
        view = Path(P.layout(self.entry)['view'])
        view.parent.mkdir()
        view.write_text('sections:\n  - title: Summary\n    text: "Value {{p.value}}"\n    seen: {p.value: 1}\n')
        calls = []
        publish = T.publish_legacy
        def observed(root, journal, mutation, **kwargs):
            calls.append(mutation)
            return publish(root, journal, mutation, **kwargs)
        with patch.object(T, 'publish_legacy', side_effect=observed):
            self.apply({'kind': 'review', 'id': 'Summary', 'as_of': '2026-09-17'})
        self.assertEqual([item['role'] for item in calls[0].files], ['view'])
        self.assertEqual(yaml.safe_load(view.read_text())['sections'][0]['seen'], {'p.value': 2})

    def test_first_add_keeps_journal_before_image_on_publication_failure(self):
        self.entry.unlink()
        def location():
            return {'status': 'found' if self.entry.exists() else 'missing',
                    'record': str(self.entry), 'workspace': str(self.entry.parent)}
        original = T._replace
        def injected(path, data):
            if path == self.entry:
                raise OSError('injected first-add failure')
            return original(path, data)
        with patch.object(P, '_workspace_location', side_effect=location), \
                patch.object(T, '_replace', side_effect=injected):
            with self.assertRaisesRegex(OSError, 'injected first-add'):
                P._apply_first_add({'kind': 'add', 'id': 'p.value', 'body': {'v': 2}})
        journal = self.entry.parent / T.journal_for(self.entry)
        mutation = T.PreparedMutation.from_bytes(journal.read_bytes())
        self.assertEqual(self.entry.read_bytes(), next(i['before'] for i in mutation.files if i['role'] == 'record'))
        P.recover_direct([str(self.entry)])
        self.assertEqual(P.bodies(P.load([str(self.entry)]))['p.value']['v'], 2)
