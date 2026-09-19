"""Named history proposals never become base facts without an explicit fold."""
import copy
from pathlib import Path
import unittest
from unittest import mock

from scripts import history_hypotheses as HH, history_contract as C, history_transaction as T
from scripts import history_adapter as D, history_store as H
from scripts.reasoning.snapshot import Snapshot, SnapshotError
from tests import test_history_snapshot_capture as fixture


class NamedHypotheses(unittest.TestCase):
    def setUp(self):
        original = fixture.HistorySnapshotCapture()
        original.setUp()
        self.addCleanup(original.doCleanups)
        self.fixture, self.entry, self.store = original, original.entry, original.store

    def prepare(self, kind='set', subject='p.input', name='alternative', **kwargs):
        action = {'kind': kind, 'id': subject, **kwargs.pop('action', {'value': 2})}
        return HH.prepare(self.entry, name, action, by='writer', **kwargs)

    def commit(self, mutation):
        return HH.commit(self.entry, mutation, verify=lambda data: None)

    def groups(self):
        adapted = D.from_store_capture(self.store.capture())
        return HH.layers(adapted.projection, adapted.document)

    def test_proposal_keeps_base_and_source_free_snapshot_has_named_layer(self):
        before = self.store.capture()
        mutation = self.prepare(head={'claim': 'larger input', 'wrong_if': {'expr': 'p.input > 10'}, 'born': '2026-09-17'})
        self.assertEqual(self.store.capture().inventory, before.inventory)
        self.commit(mutation)
        current = self.store.capture()
        self.assertEqual(current.state['subjects']['p.input']['body']['v'], 1)
        self.assertEqual(len(current.state['subjects']['p.input']['proposals']), 1)
        self.assertEqual(current.objects[self.fixture.source['id']], self.fixture.source)
        snapshot = Snapshot.capture(self.entry, read_mode='frozen')
        data = snapshot.to_data()
        self.assertEqual(data['document']['readings']['p.input']['v'], 1)
        self.assertEqual(data['hypotheses']['alternative']['document']['readings']['p.input']['v'], 2)
        self.assertEqual(data['hypotheses']['alternative']['head']['claim'], 'larger input')
        self.assertIn('alternative', data['context']['history_hypotheses']['groups'])
        with mock.patch.object(Path, 'read_bytes', side_effect=AssertionError('source access')):
            replay = Snapshot.from_json(snapshot.to_json())
        self.assertEqual(replay.snapshot_id, snapshot.snapshot_id)

    def test_new_root_proposal_and_edit_never_auto_accept(self):
        self.commit(self.prepare('add', 'p.new', action={'body': {'v': 4}, 'into': 'readings'}))
        original = self.groups()[1]['groups']['alternative']['p.new'][0]
        self.commit(self.prepare('set', 'p.new', action={'value': 5}))
        captured = self.store.capture()
        self.assertEqual(captured.state['subjects']['p.new']['acceptance'], 'proposed')
        self.assertNotIn('p.new', captured.document['readings'])
        self.assertEqual(captured.objects[original]['body'], {'v': 4})
        self.assertEqual(captured.state['subjects']['p.new']['marks'][original], 'retired')
        self.assertEqual(self.groups()[0]['alternative']['doc']['readings']['p.new']['v'], 5)

    def test_retained_version_one_judgment_proposal_replays_without_new_seen(self):
        mutation = HH.prepare(self.entry, 'legacy', {'kind': 'add', 'id': 'd.legacy',
            'into': 'decisions', 'body': {'verdict': 'ready', 'rests_on': ['p.input'],
                                          'wrong_if': {'expr': 'p.input > 5'}}},
            by='writer', operation='legacy-hypothesis', _receipt_version=1)
        self.assertEqual(mutation.to_data()['receipt']['before']['hypothesis_authoring']['version'], 1)
        made = [C.decode_document(item['after']) for item in mutation.files
                if item['role'] == 'history_object']
        self.assertNotIn('seen', next(item for item in made if item['kind'] == 'judgment')['body'])
        HH.verify_prepared(self.entry, T.PreparedMutation.from_bytes(mutation.to_bytes()))

    def test_fold_all_selected_subjects_is_one_explicit_commit(self):
        self.commit(self.prepare())
        self.commit(self.prepare('add', 'p.new', action={'body': {'v': 4}, 'into': 'readings'}))
        before_count = len(self.store.capture().commits)
        mutation = HH.prepare_fold(self.entry, ['alternative'], because='Verified alternative', by='reviewer')
        self.commit(mutation)
        self.assertEqual(len(self.store.capture().commits), before_count + 1)
        self.assertEqual(self.store.state()['subjects']['p.input']['body']['v'], 2)
        self.assertEqual(self.store.state()['subjects']['p.new']['body']['v'], 4)
        self.assertEqual(self.groups()[0], {})
        self.assertNotIn('alternative', Snapshot.capture(self.entry, read_mode='frozen').to_data()['hypotheses'])

    def test_refutation_closes_group_proposals_only(self):
        self.commit(self.prepare())
        self.commit(self.prepare(name='other', action={'value': 3}))
        original = self.fixture.source
        self.commit(HH.prepare_refute(self.entry, ['alternative'], because='Rejected this scenario'))
        captured = self.store.capture()
        self.assertEqual(captured.state['subjects']['p.input']['head'], original['id'])
        self.assertEqual(captured.objects[original['id']], original)
        self.assertEqual(set(self.groups()[0]), {'other'})

    def test_fold_checks_existing_judgments_in_prepared_history(self):
        decision = fixture.claim('d.ready', kind='judgment', operation='decision',
            body={'verdict': 'ready', 'rests_on': ['p.input'],
                  'wrong_if': {'expr': 'p.input > 5'}},
            pins={'p.input': self.fixture.source['id']})
        self.fixture.publish([decision], 'decision')
        self.commit(self.prepare(action={'value': 9}))
        before = self.store.capture()
        with self.assertRaisesRegex(C.HistoryError, 'hypothesis_candidate_not_clean.*d.ready'):
            HH.prepare_fold(self.entry, ['alternative'], because='candidate must be checked')
        self.assertEqual(before.inventory, self.store.capture().inventory)
        self.assertIn('alternative', self.groups()[0])
        # A previously prepared receipt keeps its original admission semantics
        # during recovery; new operations always opt in to the stronger check.
        old = HH.prepare_fold(self.entry, ['alternative'], because='retained old operation',
                              _assessment_version=0)
        self.assertNotIn('assessment_version', old.to_data()['receipt']['before']['hypothesis_authoring'])
        HH.verify_prepared(self.entry, old)

    def test_prepared_assessment_retains_history_and_matches_publication(self):
        from scripts import history_prospective
        self.commit(self.prepare())
        captured = self.store.capture()
        mutation = HH.prepare_fold(self.entry, ['alternative'], because='verified candidate',
                                   recorded_at='2026-09-18T00:00:00+00:00')
        self.assertEqual(mutation.to_data()['receipt']['before']['hypothesis_authoring']['assessment_version'], 1)
        checked = history_prospective.assess(captured, mutation, as_of='2026-09-18')
        self.assertEqual(checked['introduced']['holes'], [])
        self.assertEqual(checked['introduced']['falsified'], [])
        candidate = checked['after'].snapshot.to_data()
        self.assertEqual(candidate['context']['operation_scope'], 'committed_history')
        self.assertNotIn('alternative', candidate['hypotheses'])
        self.commit(mutation)
        actual = D.from_store_capture(self.store.capture()).snapshot(as_of='2026-09-18').to_data()
        self.assertEqual(candidate['document'], actual['document'])
        self.assertEqual(candidate['context']['history'], actual['context']['history'])

    def test_review_keeps_original_seen_and_pins_group_dependency(self):
        self.commit(self.prepare())
        group_input = self.groups()[1]['groups']['alternative']['p.input'][0]
        self.commit(self.prepare('add', 'd.ready', action={'body': {
            'verdict': 'ready', 'rests_on': ['p.input'], 'wrong_if': {'expr': 'p.input > 5'}}, 'into': 'decisions'}))
        version = self.groups()[1]['groups']['alternative']['d.ready'][0]
        original = copy.deepcopy(self.store.capture().objects[version])
        self.assertEqual(original['body']['seen']['p.input']['computed']['value'],
                         {'type': 'number', 'numerator': '2', 'denominator': '1'})
        self.assertNotIn('d.ready', self.store.capture().document.get('decisions', {}))
        self.commit(self.prepare('set', action={'value': 3}))
        new_input = self.groups()[1]['groups']['alternative']['p.input'][0]
        mutation = self.prepare('review', 'd.ready', action={})
        self.commit(mutation)
        capture = self.store.capture()
        self.assertEqual(capture.objects[version], original)
        self.assertEqual(original['pins']['p.input'], group_input)
        self.assertEqual(original['body']['seen']['p.input']['computed']['value'],
                         {'type': 'number', 'numerator': '2', 'denominator': '1'})
        review = next(obj for obj in capture.objects.values() if obj['op'] == mutation.to_data()['operation'])
        self.assertEqual(review['body']['read'], {'p.input': new_input})
        snapshot = Snapshot.capture(self.entry, read_mode='frozen').to_data()
        self.assertEqual(snapshot['context']['history']['dispositions']['d.ready']['reviews'][0]['body']['read'], {'p.input': new_input})

    def test_physical_name_collision_refuses_without_deleting_file(self):
        directory = Path(self.store.layout['hypotheses'])
        directory.mkdir(parents=True)
        physical = directory / 'alternative.yaml'
        raw = b'hypothesis: {claim: original}\nreadings: {p.input: {v: 8}}\n'
        physical.write_bytes(raw)
        with self.assertRaisesRegex(C.HistoryError, 'hypothesis_authority_collision'):
            self.prepare()
        self.assertEqual(physical.read_bytes(), raw)
        physical.unlink()
        self.commit(self.prepare())
        physical.write_bytes(raw)
        with self.assertRaisesRegex((C.HistoryError, SnapshotError), 'hypothesis_authority_collision'):
            Snapshot.capture(self.entry, read_mode='frozen')
        self.assertEqual(physical.read_bytes(), raw)

    def test_never_fold_and_explicit_resolution_for_standing_judgment(self):
        self.commit(self.prepare(head={'folds': 'never'}))
        with self.assertRaisesRegex(C.HistoryError, 'hypothesis_never_folds'):
            HH.prepare_fold(self.entry, ['alternative'], because='fold')
        judgment = fixture.claim('d.base', kind='judgment', operation='base-judgment', body={
            'verdict': 'old', 'rests_on': ['p.input'], 'wrong_if': {'expr': 'p.input > 10'}},
            pins={'p.input': self.fixture.source['id']})
        self.fixture.publish([judgment], 'base-judgment')
        self.commit(self.prepare('add', 'd.base', name='decision', action={'body': {
            'verdict': 'new', 'rests_on': ['p.input'], 'wrong_if': {'expr': 'p.input > 20'}}}))
        with self.assertRaisesRegex(C.HistoryError, 'hypothesis_fold_requires_resolution'):
            HH.prepare_fold(self.entry, ['decision'], because='fold')
        self.commit(HH.prepare_fold(self.entry, ['decision'], because='Explicit named decision', take=['d.base']))
        self.assertEqual(self.store.state()['subjects']['d.base']['body']['verdict'], 'new')

    def test_unknown_missing_pin_is_preserved_not_fabricated(self):
        self.commit(self.prepare('add', 'd.wait', action={'body': {'verdict': 'waiting', 'rests_on': ['p.missing'],
            'wrong_if': {'expr': 'p.missing > 2'}, 'blocked_on': 'source unavailable'}, 'into': 'decisions'}))
        captured = self.store.capture()
        version = self.groups()[1]['groups']['alternative']['d.wait'][0]
        self.assertEqual(captured.objects[version]['pin_gaps'], {'p.missing': 'unavailable'})
        self.assertEqual(captured.objects[version]['pins'], {})
        snapshot = Snapshot.capture(self.entry, read_mode='frozen').to_data()
        self.assertEqual(snapshot['hypotheses']['alternative']['document']['decisions']['d.wait']['rests_on'], ['p.missing'])
        self.assertFalse(snapshot['context']['history']['coverage']['complete'])
        with self.assertRaisesRegex(C.HistoryError, 'unavailable_review_pin'):
            self.prepare('review', 'd.wait', action={})

    def test_snapshot_replay_rejects_forged_or_removed_named_layer(self):
        self.commit(self.prepare())
        snapshot = Snapshot.capture(self.entry, read_mode='frozen').to_data()
        for mode in ('remove', 'edit'):
            changed = copy.deepcopy(snapshot)
            if mode == 'remove':
                changed['hypotheses'].pop('alternative')
            else:
                changed['hypotheses']['alternative']['document']['readings']['p.input']['v'] = 999
            with self.assertRaisesRegex(SnapshotError, 'history_hypothesis_layer_mismatch'):
                Snapshot.from_snapshot(changed)

    def test_stale_preparation_refuses_and_after_manifest_retry_is_exact(self):
        first, stale = self.prepare(), self.prepare(name='other', action={'value': 3})
        with mock.patch.object(T, '_replace', side_effect=OSError('view interruption')), self.assertRaises(OSError):
            self.commit(first)
        before_count = len(self.store.capture().commits)
        self.commit(T.PreparedMutation.from_bytes(first.to_bytes()))
        self.assertEqual(len(self.store.capture().commits), before_count)
        with self.assertRaisesRegex(C.HistoryError, 'stale_baseline'):
            self.commit(stale)

    def test_parallel_edits_of_one_group_are_retained_and_cannot_choose_scalar(self):
        first = self.prepare(operation='left', head={'claim': 'same named question'})
        second = self.prepare(operation='right', head={'claim': 'same named question'}, action={'value': 3})
        self.commit(first)
        for item in second.files:
            if item['role'] != 'record':
                T.publish_immutable(self.entry.parent / item['path'], item['after'], root=self.entry.parent)
        self.store.rebuild(write=True)
        groups, index = self.groups()
        self.assertIn('contested hypothesis subject', groups['alternative']['error'])
        self.assertEqual(len(index['groups']['alternative']['p.input']), 2)
        self.assertNotIn('p.input', groups['alternative']['doc'].get('readings', {}))
        self.assertEqual(self.store.state()['subjects']['p.input']['body']['v'], 1)
        snapshot = Snapshot.capture(self.entry, read_mode='frozen').to_data()
        self.assertIn('contested', snapshot['hypotheses']['alternative']['error'])
        with self.assertRaisesRegex(C.HistoryError, 'unresolved_history_hypothesis'):
            HH.prepare_fold(self.entry, ['alternative'], because='cannot pick one')
        self.commit(HH.prepare_refute(self.entry, ['alternative'], because='Reject both proposed alternatives'))
        self.assertEqual(self.groups()[0], {})
        self.assertEqual(self.store.state()['subjects']['p.input']['body']['v'], 1)

    def test_physical_collision_created_by_verifier_blocks_publication(self):
        mutation = self.prepare()
        before = self.entry.read_bytes()
        directory = Path(self.store.layout['hypotheses'])
        def collide(data):
            directory.mkdir(parents=True)
            (directory / 'alternative.yaml').write_text('readings: {p.input: {v: 99}}\n')
        with self.assertRaisesRegex(C.HistoryError, 'concurrent_hypothesis_edit'):
            HH.commit(self.entry, mutation, verify=collide)
        self.assertEqual(self.entry.read_bytes(), before)
        self.assertEqual(len(self.store.capture().commits), 1)
        self.assertTrue((directory / 'alternative.yaml').exists())

    def test_recorded_body_seen_and_group_head_survive_edit(self):
        source = self.fixture.source
        group = {'version': 1, 'name': 'retained', 'head': {'claim': 'original', 'born': 'unknown', 'folds': 'never'}}
        judgment = fixture.claim('d.retained', kind='judgment', operation='imported-proposal', body={
            'verdict': 'original', 'rests_on': ['p.input'], 'wrong_if': {'expr': 'p.input > 10'},
            'seen': {'p.input': 'original historical evidence'}}, pins={'p.input': source['id']})
        judgment['authored']['hypothesis'] = group
        judgment['id'] = C.object_identity(judgment)
        propose = fixture.act(judgment, 'propose', operation='imported-disposition')
        self.fixture.publish([judgment, propose], 'imported')
        original = copy.deepcopy(self.store.capture().objects[judgment['id']])
        self.commit(self.prepare('review', 'd.retained', name='retained', action={}))
        self.assertEqual(self.store.capture().objects[judgment['id']], original)
        snapshot = Snapshot.capture(self.entry, read_mode='frozen').to_data()
        self.assertEqual(snapshot['hypotheses']['retained']['head'], group['head'])
        self.assertEqual(snapshot['hypotheses']['retained']['document']['decisions']['d.retained']['seen'],
                         {'p.input': 'original historical evidence'})

    def test_profile_and_review_scope_cannot_silently_change(self):
        with self.assertRaisesRegex(C.HistoryError, 'history_profile_migration_required'):
            self.prepare(action={'value': 2, 'profile': 'ordinary-reader/v1'})
        self.commit(self.prepare('add', 'd.scoped', action={'body': {
            'verdict': 'ready', 'rests_on': ['p.input'], 'wrong_if': {'expr': 'p.input > 5'},
            'scope': {'kind': 'project', 'environment': 'one'}}, 'into': 'decisions'}))
        before = self.entry.read_bytes()
        with self.assertRaisesRegex(C.HistoryError, 'history_review_scope_change_requires_claim'):
            self.prepare('review', 'd.scoped', action={'_record_scope': {'kind': 'project', 'environment': 'two'}})
        self.assertEqual(self.entry.read_bytes(), before)

    def test_changing_proposed_judgment_grounds_creates_new_version_and_retires_old(self):
        old_body = {'verdict': 'ready', 'rests_on': ['p.input'], 'wrong_if': {'expr': 'p.input > 5'}}
        self.commit(self.prepare('add', 'd.change', action={'body': old_body, 'into': 'decisions'}))
        old = self.groups()[1]['groups']['alternative']['d.change'][0]
        new_body = {**old_body, 'wrong_if': {'expr': 'p.input > 10'}}
        self.commit(self.prepare('add', 'd.change', action={'body': new_body}))
        current = self.store.capture()
        self.assertEqual({key: value for key, value in current.objects[old]['body'].items()
                          if key != 'seen'}, old_body)
        self.assertEqual(current.objects[old]['body']['seen']['p.input']['computed']['value'],
                         {'type': 'number', 'numerator': '1', 'denominator': '1'})
        self.assertEqual(current.state['subjects']['d.change']['marks'][old], 'retired')
        current_body = self.groups()[0]['alternative']['doc']['decisions']['d.change']
        self.assertEqual({key: value for key, value in current_body.items() if key != 'seen'},
                         new_body)
        self.assertNotIn('d.change', current.document['decisions'])


if __name__ == '__main__':
    unittest.main()
