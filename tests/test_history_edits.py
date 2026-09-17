"""Explicit edited-view recording retains raw evidence and leaves acceptance alone."""
import copy
from pathlib import Path
import unittest
from unittest import mock

from scripts import history_edits as E, history_authoring as A, history_contract as C
from scripts import history_transaction as T, history_adapter
from tests import test_history_authoring as fixtures


class EditedProposals(unittest.TestCase):
    setUp = fixtures.Authoring.setUp
    prepare = fixtures.Authoring.prepare
    publish = fixtures.Authoring.publish
    def edit(self, change, comment='# editor comment retained verbatim\n'):
        document = C.decode_document(self.entry.read_bytes())
        change(document)
        raw = comment.encode() + C.encode_document(document)
        self.entry.write_bytes(raw)
        return raw

    def prepare_edit(self, **kwargs):
        return E.prepare_proposals(self.entry, because='Explicit proposal from editor',
            by='editor', operation='edited-proposals', recorded_at='2026-09-17', **kwargs)

    def commit_edit(self, mutation):
        return E.commit(self.entry, mutation, verify=lambda data: None)

    def test_new_collection_refusal_names_the_unhandled_collection(self):
        raw = self.edit(lambda document: document.update(questions={'q.new': 'New question'}))
        with self.assertRaisesRegex(C.HistoryError, 'template_disposition_required.*questions'):
            self.prepare_edit()
        self.assertEqual(self.entry.read_bytes(), raw)

    def test_existing_body_is_only_proposed_and_raw_comments_are_retained(self):
        original = self.store.capture()
        raw = self.edit(lambda d: d['readings']['p.input'].update(v=2))
        mutation = self.prepare_edit()
        self.assertEqual(self.entry.read_bytes(), raw)
        self.assertEqual(self.store.capture().commits, original.commits)
        E.verify_prepared(self.entry, mutation)
        self.commit_edit(mutation)
        captured = self.store.capture()
        subject = captured.state['subjects']['p.input']
        self.assertEqual(subject['head'], self.original['id'])
        self.assertEqual(subject['body']['v'], 1)
        self.assertEqual(len(subject['proposals']), 1)
        proposed = captured.objects[subject['proposals'][0]]
        self.assertEqual(proposed['body']['v'], 2)
        self.assertNotIn('seen', proposed['body'])
        evidence = next(i for i in mutation.files if i['role'] == 'history_evidence')
        self.assertEqual((self.entry.parent / evidence['path']).read_bytes(), raw)
        self.assertEqual(mutation.to_data()['receipt']['before']['authoring']['evidence'],
                         {evidence['path']: C.sha256(raw)})
        self.assertEqual(self.store.rebuild(write=False), self.entry.read_bytes())
        self.assertEqual(captured.objects[self.original['id']], self.original)

    def test_new_subject_and_multiple_bodies_have_one_manifest_and_no_new_head(self):
        def changes(document):
            document['readings']['p.input']['v'] = 3
            document['readings']['p.new'] = {'v': 9}
        self.edit(changes)
        mutation = self.prepare_edit(subjects=['p.new', 'p.input'])
        self.assertEqual(sum(i['role'] == 'history_commit' for i in mutation.files), 1)
        self.commit_edit(mutation)
        captured = self.store.capture()
        self.assertEqual(len(captured.commits), 2)
        self.assertEqual(captured.state['subjects']['p.new']['acceptance'], 'proposed')
        self.assertEqual(captured.state['subjects']['p.new']['heads'], [])
        self.assertNotIn('p.new', captured.document['readings'])
        self.assertEqual(captured.document['readings']['p.input']['v'], 1)

    def test_original_seen_stays_exact_and_no_review_act_is_created(self):
        original = fixtures.claim('p.ready', kind='judgment', op='judgment',
            body={'verdict': 'old', 'rests_on': ['p.input'], 'wrong_if': {'expr': 'p.input > 5'},
                  'seen': {'p.input': {'unavailable': 'original'}}}, pins={'p.input': self.original['id']})
        self.fixture.publish([original], op='judgment')
        self.edit(lambda d: d['judgments']['p.ready'].update(verdict='candidate'))
        self.commit_edit(self.prepare_edit())
        captured = self.store.capture()
        state = captured.state['subjects']['p.ready']
        proposal = captured.objects[state['proposals'][0]]
        self.assertEqual(captured.objects[original['id']], original)
        self.assertEqual(proposal['body']['seen'], original['body']['seen'])
        self.assertFalse([o for o in captured.objects.values() if o['kind'] == 'act' and o['body']['act'] == 'review'])

    def test_refusals_preserve_exact_raw_bytes_and_no_evidence(self):
        cases = [
            ('header', lambda d: d['meta'].update(schema='changed'), {}, 'template_disposition_required'),
            ('delete', lambda d: d['readings'].pop('p.input'), {}, 'deletion_disposition_required'),
            ('partial', lambda d: d['readings'].update({'p.new': {'v': 2}, 'p.input': {'v': 3}}),
             {'subjects': ['p.new']}, 'unhandled_view_edits'),
            ('comment-only', lambda d: None, {}, 'no_body_proposals'),
            ('forged', lambda d: d['meta']['history']['heads'].update({'p.input': []}), {}, 'baseline_mismatch'),
            ('missing', lambda d: d['meta'].pop('history'), {}, 'missing_history_baseline'),
        ]
        canonical = self.entry.read_bytes()
        for name, change, kwargs, error in cases:
            with self.subTest(name=name):
                self.entry.write_bytes(canonical)
                raw = self.edit(change)
                before = {p: p.read_bytes() for p in self.entry.parent.rglob('*') if p.is_file()}
                with self.assertRaises(C.HistoryError):
                    self.prepare_edit(**kwargs)
                self.assertEqual(self.entry.read_bytes(), raw)
                self.assertEqual(before, {p: p.read_bytes() for p in self.entry.parent.rglob('*') if p.is_file()})

    def test_stale_baseline_and_unknown_baseline_are_not_recorded(self):
        old = self.entry.read_bytes()
        self.publish(self.prepare({'kind': 'set', 'id': 'p.input', 'value': 3}))
        self.entry.write_bytes(old)
        raw = self.edit(lambda d: d['readings']['p.input'].update(v=2))
        with self.assertRaisesRegex(C.HistoryError, 'stale_edit_baseline'):
            self.prepare_edit()
        self.assertEqual(self.entry.read_bytes(), raw)
        raw = self.edit(lambda d: d['meta']['history'].update(committed_set_digest='f' * 64))
        with self.assertRaises(C.HistoryError):
            self.prepare_edit()
        self.assertEqual(self.entry.read_bytes(), raw)

    def test_missing_refs_refuse_no_implicit_accept(self):
        raw = self.edit(lambda d: d['judgments'].update({'p.new': {
            'verdict': 'maybe', 'rests_on': ['p.missing'], 'wrong_if': {'expr': 'p.missing > 0'}}}))
        with self.assertRaisesRegex(C.HistoryError, 'unresolved_history_subject'):
            self.prepare_edit()
        self.assertEqual(self.entry.read_bytes(), raw)
        self.assertEqual(len(self.store.capture().commits), 1)

    def test_manifest_crashes_recover_exactly_and_run_external_verifier(self):
        for phase in ('before', 'after'):
            with self.subTest(phase=phase):
                raw = self.edit(lambda d: d['readings']['p.input'].update(v=8))
                mutation = E.prepare_proposals(self.entry, because='candidate', operation='crash-' + phase,
                                               recorded_at='2026-09-17')
                publish = T.publish_immutable
                def crash(path, *args, **kwargs):
                    if phase == 'before' and Path(path).parent == Path(self.store.layout['history_commits']):
                        raise OSError('manifest crash')
                    return publish(path, *args, **kwargs)
                with mock.patch.object(T, 'publish_immutable', side_effect=crash):
                    if phase == 'after':
                        with mock.patch.object(T, '_replace', side_effect=OSError('view crash')):
                            with self.assertRaises(OSError):
                                self.commit_edit(mutation)
                    else:
                        with self.assertRaises(OSError):
                            self.commit_edit(mutation)
                self.assertEqual(self.entry.read_bytes(), raw)
                self.assertEqual(self.store.capture().state['subjects']['p.input']['head'], self.original['id'])
                retained = T.PreparedMutation.from_bytes(mutation.to_bytes())
                verifier = mock.Mock()
                E.commit(self.entry, retained, verify=verifier)
                E.commit(self.entry, retained, verify=verifier)
                self.assertEqual(verifier.call_count, 2)
                self.assertEqual(self.entry.read_bytes(), self.store.render(self.store.capture()))

    def test_concurrent_edit_and_missing_verifier_refuse(self):
        self.edit(lambda d: d['readings']['p.input'].update(v=4))
        mutation = self.prepare_edit()
        raw = self.edit(lambda d: d['readings']['p.input'].update(v=5))
        with self.assertRaisesRegex(C.HistoryError, 'missing_verifier'):
            E.commit(self.entry, mutation, verify=None)
        with self.assertRaisesRegex(C.HistoryError, 'concurrent_edit'):
            self.commit_edit(mutation)
        self.assertEqual(self.entry.read_bytes(), raw)

    def test_snapshot_retains_proposal_without_source_lookup(self):
        self.edit(lambda d: d['readings'].update({'p.new': {'v': 7}}))
        self.commit_edit(self.prepare_edit())
        captured = self.store.capture()
        adapted = history_adapter.from_store_capture(captured)
        snapshot = adapted.snapshot()
        from scripts.reasoning.snapshot import Snapshot
        serialized = snapshot.to_json()
        with mock.patch.object(Path, 'read_bytes', side_effect=AssertionError('source lookup')):
            frozen = Snapshot.from_json(serialized)
        self.assertEqual(frozen.to_data()['context']['history']['subjects']['p.new']['acceptance'], 'proposed')

    def test_store_builtin_rejects_forged_receipt_even_with_noop_external_verifier(self):
        raw = self.edit(lambda d: d['readings']['p.input'].update(v=7))
        mutation = self.prepare_edit()
        data = mutation.to_data()
        before = copy.deepcopy(data['receipt']['before'])
        before['authoring']['original_view_sha256'] = 'f' * 64
        receipt = T.semantic_receipt(profile=data['receipt']['profile'],
            capabilities=data['receipt']['capabilities'], before=before,
            after=data['receipt']['after'])
        files = copy.deepcopy(mutation.files)
        manifest_item = next(i for i in files if i['role'] == 'history_commit')
        manifest = C.decode_document(manifest_item['after'])
        manifest['receipt'] = receipt
        manifest_item['after'] = C.encode_document(manifest)
        forged = T.PreparedMutation(operation=data['operation'], authority=data['authority'],
            baseline=data['baseline'], files=files, receipt=receipt, entry=self.entry.name)
        with self.assertRaisesRegex(C.HistoryError, 'edit_receipt_mismatch'):
            self.store.commit(forged, verify=lambda data: None)
        self.assertEqual(self.entry.read_bytes(), raw)
        self.assertEqual(len(self.store.capture().commits), 1)

    def test_full_artifact_keeps_exact_raw_edit_when_optional_evidence_file_is_gone(self):
        from scripts import history_bundle as B
        scope = {'kind': 'project', 'environment': 'test'}
        from tests.test_history_store import Storage
        fixture = Storage()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        self.entry, self.store = fixture.entry, fixture.store
        fixture.publish([fixtures.claim(body={'v': 1, 'scope': scope})], op='bootstrap')
        raw = self.edit(lambda d: d['readings']['p.input'].update(v=9),
                        comment='# original comment: שלום\n')
        mutation = self.prepare_edit()
        self.commit_edit(mutation)
        evidence = next(i for i in mutation.files if i['role'] == 'history_evidence')
        (self.entry.parent / evidence['path']).unlink()
        artifact = B.export(self.store.capture(), roots=['p.input'], scope=scope, shareability='project')
        with mock.patch.object(Path, 'open', side_effect=AssertionError('source lookup')):
            replay = B.validate(artifact)
            self.assertEqual(E.raw_edit_evidence(C.decode_document(
                replay.commits['edited-proposals'])['receipt']), raw)
        manifest = C.decode_document(replay.commits['edited-proposals'])
        retained = manifest['receipt']['before']['authoring']
        self.assertEqual(retained['edited_view_utf8'].encode('utf-8'), raw)
        self.assertEqual(retained['edited_view_sha256'], C.sha256(raw))
        self.assertNotIn('edited_view_utf8', manifest['receipt']['after']['authoring'])
        self.assertEqual(replay.state['subjects']['p.input']['body']['v'], 1)
