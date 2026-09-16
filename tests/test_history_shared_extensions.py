"""Shared split-record, durable callback, and non-circular render boundaries."""
import copy
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from scripts import history_contract as C, history_transaction as T
from scripts.pending_grounding import identity
from tests.test_history_contract import baseline, claim, receipt


class SplitRecordMembers(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.entry = self.root / 'project/GROUNDING.yaml'
        self.shard = self.root / 'shared/readings.yaml'
        for path, raw in ((self.entry, b'entry-before'), (self.shard, b'shard-before')):
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(raw)
        self.files = [
            {'path': 'project/GROUNDING.yaml', 'role': 'record', 'before': b'entry-before', 'after': b'entry-after'},
            {'path': 'shared/readings.yaml', 'role': 'record_member', 'before': b'shard-before', 'after': b'shard-after'}]
        self.baseline = {'record_members': {item['path']: C.sha256(item['before']) for item in self.files}}

    def prepare(self, **changes):
        args = {'entry': 'project/GROUNDING.yaml', 'operation': 'split-op',
                'authority': C.authority(record_id='record-1', authority='legacy', generation=0),
                'baseline': self.baseline, 'files': self.files, 'receipt': receipt()}
        args.update(changes)
        return T.PreparedMutation(**args)

    def test_split_members_publish_with_root_relative_private_journal(self):
        mutation = self.prepare()
        self.assertEqual(T.PreparedMutation.from_bytes(mutation.to_bytes()).to_bytes(), mutation.to_bytes())
        journal = T.journal_for(self.entry, root=self.root)
        self.assertEqual(journal, 'project/' + T.journal_for(self.entry))
        verified = []
        def verify(envelope):
            # Integration owns membership resolution; this fixture knows its reader's files.
            actual = {str(path.relative_to(self.root)): C.sha256(path.read_bytes())
                      for path in (self.entry, self.shard)}
            self.assertEqual(envelope['baseline']['record_members'], actual)
            verified.append(envelope)
        T.publish_legacy(self.root, journal, mutation, verify=verify)
        self.assertEqual(verified, [mutation.to_data()])
        self.assertEqual(self.entry.read_bytes(), b'entry-after')
        self.assertEqual(self.shard.read_bytes(), b'shard-after')
        self.assertEqual((self.root / journal).parent.joinpath('.gitignore').read_bytes(), b'*\n')
        self.assertFalse((self.root / journal).exists())
        self.assertFalse((self.root / '.kpopper').exists())

    def test_member_missing_or_hash_mismatch_refuses_without_io(self):
        for defect in ('missing-map', 'missing-member', 'missing-entry', 'wrong-hash', 'wrong-entry-hash'):
            captured = copy.deepcopy(self.baseline)
            if defect == 'missing-map':
                captured = {}
            elif defect == 'missing-member':
                captured['record_members'].pop('shared/readings.yaml')
            elif defect == 'missing-entry':
                captured['record_members'].pop('project/GROUNDING.yaml')
            else:
                path = 'project/GROUNDING.yaml' if defect == 'wrong-entry-hash' else 'shared/readings.yaml'
                captured['record_members'][path] = C.sha256(b'unrelated')
            with self.subTest(defect=defect), self.assertRaises(C.HistoryError):
                self.prepare(baseline=captured)
        self.assertEqual(self.entry.read_bytes(), b'entry-before')
        self.assertEqual(self.shard.read_bytes(), b'shard-before')
        self.assertFalse((self.entry.parent / '.kpopper').exists())

    def test_member_cannot_impersonate_entry_or_layout_role(self):
        for index, role in ((0, 'record_member'), (1, 'record'), (1, 'replaced'), (1, 'unknown')):
            files = copy.deepcopy(self.files)
            files[index]['role'] = role
            with self.subTest(index=index, role=role), self.assertRaises(C.HistoryError):
                self.prepare(files=files)

    def test_member_before_image_must_exist_and_match_capture(self):
        for before in (None, b'different', 'shard-before'):
            files = copy.deepcopy(self.files)
            files[1]['before'] = before
            with self.subTest(before=before), self.assertRaises(C.HistoryError):
                self.prepare(files=files)

    def test_membership_paths_are_normalized_and_digests_are_sha256(self):
        for members in (None, [], 'not-members'):
            with self.subTest(members=members), self.assertRaises(C.HistoryError):
                self.prepare(baseline={'record_members': members})
        for path in ('../escape', '/absolute', 'shared/../readings.yaml', 'shared//readings.yaml'):
            captured = copy.deepcopy(self.baseline)
            captured['record_members'][path] = C.sha256(b'captured')
            with self.subTest(path=path), self.assertRaises(C.HistoryError):
                self.prepare(baseline=captured)
        captured = copy.deepcopy(self.baseline)
        captured['record_members']['shared/readings.yaml'] = 'not-a-digest'
        with self.assertRaises(C.HistoryError):
            self.prepare(baseline=captured)

    def test_record_member_is_legacy_only(self):
        with self.assertRaisesRegex(C.HistoryError, 'authority_transition_required'):
            self.prepare(authority=C.authority(record_id='record-1', authority='history', generation=1))

    def test_membership_claim_does_not_bypass_mandatory_reader_verification(self):
        def refuse(_):
            raise C.HistoryError('reader_membership_changed')
        with self.assertRaisesRegex(C.HistoryError, 'reader_membership_changed'):
            T.publish_legacy(self.root, T.journal_for(self.entry, root=self.root), self.prepare(), verify=refuse)
        self.assertEqual(self.entry.read_bytes(), b'entry-before')
        self.assertEqual(self.shard.read_bytes(), b'shard-before')
        self.assertFalse((self.entry.parent / '.kpopper').exists())

    def test_journal_root_must_contain_entry(self):
        with self.assertRaisesRegex(C.HistoryError, 'invalid_path'):
            T.journal_for(self.entry, root=self.root / 'elsewhere')


class DurableCompletionCallbacks(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.entry = self.root / 'GROUNDING.yaml'
        self.entry.write_bytes(b'before')
        self.journal = T.journal_for(self.entry)
        self.mutation = T.PreparedMutation(operation='callback-op',
            authority=C.authority(record_id='record-1', authority='legacy', generation=0),
            baseline={}, receipt=receipt(),
            files=[{'path': 'GROUNDING.yaml', 'role': 'record', 'before': b'before', 'after': b'after'}])

    def test_failed_completion_after_receipt_preserves_exact_journal_for_idempotent_recovery(self):
        durable_receipt = self.root / '.kpopper/completion.json'
        calls = []
        def complete(envelope):
            calls.append(envelope)
            self.assertEqual(self.entry.read_bytes(), b'after')
            self.assertEqual((self.root / self.journal).read_bytes(), self.mutation.to_bytes())
            T.publish_immutable(durable_receipt, C.encode_document(envelope['receipt']), root=self.root)
            if len(calls) == 1:
                raise OSError('lost completion acknowledgment')
        with self.assertRaisesRegex(OSError, 'lost completion acknowledgment'):
            T.publish_legacy(self.root, self.journal, self.mutation,
                             verify=lambda _: None, on_committed=complete)
        retained = (self.root / self.journal).read_bytes()
        completed = durable_receipt.read_bytes()
        with self.assertRaisesRegex(C.HistoryError, 'recovery_required'):
            with T.reader_guard(self.root, self.journal):
                self.fail('pending completion must block ordinary readers')
        restored = T.recover_legacy(self.root, self.journal, verify=lambda _: None, on_committed=complete)
        self.assertEqual(calls, [self.mutation.to_data(), self.mutation.to_data()])
        self.assertEqual(restored.to_bytes(), retained)
        self.assertEqual(durable_receipt.read_bytes(), completed)
        self.assertFalse((self.root / self.journal).exists())

    def test_rollback_does_not_invoke_completion_callback(self):
        def fail(_):
            raise OSError('receipt unavailable')
        with self.assertRaisesRegex(OSError, 'receipt unavailable'):
            T.publish_legacy(self.root, self.journal, self.mutation,
                             verify=lambda _: None, on_committed=fail)
        callback = mock.Mock()
        T.recover_legacy(self.root, self.journal, verify=lambda _: None,
                         direction='before', on_committed=callback)
        callback.assert_not_called()
        self.assertEqual(self.entry.read_bytes(), b'before')
        self.assertFalse((self.root / self.journal).exists())

    def test_recovery_callback_failure_keeps_journal_unchanged(self):
        path = self.root / self.journal
        path.parent.mkdir(parents=True)
        raw = self.mutation.to_bytes()
        path.write_bytes(raw)
        def fail(_):
            raise OSError('completion failure')
        with self.assertRaisesRegex(OSError, 'completion failure'):
            T.recover_legacy(self.root, self.journal, verify=lambda _: None, on_committed=fail)
        self.assertEqual(path.read_bytes(), raw)
        self.assertEqual(self.entry.read_bytes(), b'after')


class NonCircularRenderBindings(unittest.TestCase):
    def setUp(self):
        obj = claim()
        self.commit = C.make_commit(
            marker=C.authority(record_id='record-1', authority='history', generation=1),
            operation='op-1', parents={}, baseline=baseline(obj),
            objects=[(obj, C.encode_document(obj))], receipt=receipt(), view=b'draft',
            view_template={'title': 'Record', 'readings': {}, 'meta': {'profile': 'checked'}})

    def test_digest_is_stable_after_view_and_receipt_finalization(self):
        draft_digest = C.committed_set_digest({'op-1': self.commit})
        finalized = copy.deepcopy(self.commit)
        view = C.encode_document({'meta': {'committed_set_digest': draft_digest}})
        finalized['view_sha256'] = C.sha256(view)
        finalized['receipt'] = T.semantic_receipt(profile='core/v1', capabilities={},
            before={}, after={'view_sha256': C.sha256(view), 'committed_set_digest': draft_digest})
        self.assertEqual(C.committed_set_digest({'op-1': C.encode_document(finalized)}), draft_digest)
        self.assertNotEqual(C.sha256(C.encode_document(finalized)), C.sha256(C.encode_document(self.commit)))

    def test_digest_changes_when_object_or_context_effects_change(self):
        original = C.committed_set_digest({'op-1': self.commit})
        for changed in ('object', 'template', 'baseline', 'parents', 'authority'):
            commit = copy.deepcopy(self.commit)
            if changed == 'object':
                commit['objects'][0]['sha256'] = C.sha256(b'changed object')
            elif changed == 'template':
                commit['view_template']['title'] = 'Changed title'
            elif changed == 'baseline':
                commit['baseline_digest'] = identity('changed baseline')
            elif changed == 'parents':
                commit['parents'] = {'earlier': C.sha256(b'earlier manifest')}
            else:
                commit['authority_generation'] = 2
            with self.subTest(changed=changed):
                self.assertNotEqual(C.committed_set_digest({'op-1': commit}), original)

    def test_template_retains_scalar_headers_and_removes_generated_state_and_bodies(self):
        document = {'title': 'Research', 'revision': 2, 'published': False,
                    'schema': {'value': 'v'}, 'record': {'id': 'r'}, 'also': ['parts.yaml'],
                    'meta': {'history': baseline(claim()), 'owner': 'team',
                             'reasoning': {'profile': 'core/v1', 'version': 2}},
                    'readings': {'p.input': {'v': 1, 'seen': {'p.old': 9}}},
                    'judgments': {'d.ready': {'verdict': 'yes'}}, 'empty': {}}
        saved = copy.deepcopy(document)
        result = C.document_template(document)
        expected = copy.deepcopy(document)
        expected['meta'].pop('history')
        expected['readings'] = {}
        expected['judgments'] = {}
        self.assertEqual(result, expected)
        self.assertEqual(document, saved)
        result['meta']['reasoning']['version'] = 999
        self.assertEqual(document, saved)

    def test_commit_rejects_template_with_generated_baseline_or_claim_bodies(self):
        for template in ({'meta': {'history': baseline(claim())}}, {'readings': {'p.input': {'v': 1}}}):
            altered = {**self.commit, 'view_template': template}
            with self.subTest(template=template), self.assertRaisesRegex(C.HistoryError, 'invalid_view_template'):
                C.validate_commit(altered)


if __name__ == '__main__':
    unittest.main()
