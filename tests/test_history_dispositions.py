"""Explicit root disposition, causal proposal/retirement and compatibility boundaries."""
import copy
import itertools
import unittest
from unittest import mock

from scripts import history_authoring as A, history_adapter as Adapter
from scripts import history_contract as C, history_store as H, history_transaction as T, versions as V
from scripts.reasoning.snapshot import Snapshot
from tests import test_history_store as fixtures
from tests.test_history_authoring import claim


def word(target, verb, operation, *, over=(), saw=()):
    return C.make_object(subject=target['subject'], kind='act', by='reviewer', on='2026-09-17',
        operation=operation, saw=saw, body={'act': verb, 'of': target['id'], 'over': sorted(over),
                                        'because': 'explicit ' + verb})


class Dispositions(unittest.TestCase):
    def state(self, *objects):
        return H.reduce({obj['id']: obj for obj in objects})['subjects']['p.input']

    def test_all_open_word_combinations_have_deterministic_precedence(self):
        root = claim(op='root')
        successor = claim(op='successor', value=2, saw=[root['id']])
        words = {'stands': word(root, 'accept', 'accept'), 'out': word(root, 'refute', 'refute'),
                 'proposed': word(root, 'propose', 'propose'), 'retired': word(root, 'retire', 'retire'),
                 'corrected': word(successor, 'correct', 'correct', over=[root['id']]),
                 'replaced': word(successor, 'accept', 'replace', over=[root['id']])}
        names = list(words)
        for size in range(1, len(names) + 1):
            for selected in itertools.combinations(names, size):
                with self.subTest(words=selected):
                    held = [root, successor, *(words[name] for name in selected)]
                    state = self.state(*held)
                    self.assertEqual(state, self.state(*reversed(held)))
                    if selected == ('proposed',):
                        self.assertIn(root['id'], state['proposals'])
                        self.assertNotIn(root['id'], state['heads'])
                    elif 'stands' in selected:
                        self.assertIn(root['id'], state['heads'])
                        self.assertEqual(root['id'] in state['disputed_acts'], len(selected) > 1)
                    else:
                        expected = next(kind for kind in ('corrected', 'out', 'retired', 'replaced', 'proposed') if kind in selected)
                        self.assertEqual(state['marks'][root['id']], 'refuted' if expected == 'out' else expected)

    def test_answered_proposal_and_retirement_require_explicit_return(self):
        root = claim()
        propose = word(root, 'propose', 'proposal')
        self.assertEqual(self.state(root, propose)['acceptance'], 'proposed')
        concurrent = word(root, 'accept', 'concurrent')
        self.assertEqual(self.state(root, propose, concurrent)['acceptance'], 'contested')
        accept = word(root, 'accept', 'accept', saw=[propose['id']])
        self.assertEqual(self.state(root, propose, accept)['acceptance'], 'accepted')
        retire = word(root, 'retire', 'retire', saw=[accept['id'], propose['id']])
        self.assertEqual(self.state(root, propose, accept, retire)['acceptance'], 'retired')
        returned = word(root, 'accept', 'return', saw=[retire['id'], accept['id'], propose['id']])
        self.assertEqual(self.state(root, propose, accept, retire, returned)['acceptance'], 'accepted')

    def test_proposed_or_retired_reading_is_never_promoted_by_source_clock(self):
        root = claim(at={'day': '2020-01-01'})
        newer = claim(op='newer', value=2, at={'day': '2030-01-01'})
        for verb in ('propose', 'retire'):
            state = self.state(root, newer, word(newer, verb, verb))
            self.assertEqual(state['heads'], [root['id']])
            self.assertEqual(state['implied'], [])

    def test_group_metadata_changes_object_identity_not_claim_meaning(self):
        first = claim()
        second = copy.deepcopy(first)
        first['authored']['hypothesis'] = {'version': 1, 'name': 'one', 'head': {'folds': 'not yet'}}
        second['authored']['hypothesis'] = {'version': 1, 'name': 'two', 'head': {'folds': 'after reading'}}
        first['id'], second['id'] = C.object_identity(first), C.object_identity(second)
        self.assertNotEqual(first['id'], second['id'])
        self.assertEqual(C.claim_meaning(C.validate_object(first)), C.claim_meaning(C.validate_object(second)))
        second['authored']['hypothesis']['name'] = '../escape'
        second['id'] = C.object_identity(second)
        with self.assertRaisesRegex(C.HistoryError, 'invalid_history_hypothesis'):
            C.validate_object(second)

    def test_old_prototype_identity_cannot_smuggle_new_act_verbs(self):
        root = claim()
        obj = word(root, 'propose', 'proposal')
        obj.pop('schema_version')
        obj.pop('id_scheme')
        obj['id'] = V.ident(obj)
        with self.assertRaisesRegex(C.HistoryError, 'invalid_act'):
            C.validate_object(obj)


class Writer(unittest.TestCase):
    def setUp(self):
        fixture = fixtures.Storage()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        self.fixture, self.entry, self.store = fixture, fixture.entry, fixture.store
        self.root = claim()
        fixture.publish([self.root], op='compatible-bootstrap')

    def publish(self, mutation):
        A.commit(self.entry, mutation, verify=lambda data: None)

    def test_explicit_initial_and_existing_proposals_are_nonstanding_and_replay(self):
        before = Snapshot.capture([str(self.entry)], read_mode='frozen')
        for subject in ('p.new', 'p.input'):
            mutation = A.prepare_proposal(self.entry, subject, {'v': 5}, 'readings', because='record as proposal')
            self.publish(mutation)
            self.publish(T.PreparedMutation.from_bytes(mutation.to_bytes()))
        captured = self.store.capture()
        self.assertEqual(captured.state['subjects']['p.new']['acceptance'], 'proposed')
        self.assertEqual(captured.state['subjects']['p.new']['heads'], [])
        self.assertEqual(captured.state['subjects']['p.input']['head'], self.root['id'])
        self.assertEqual(len(captured.state['subjects']['p.input']['proposals']), 1)
        after = Snapshot.capture([str(self.entry)], read_mode='frozen')
        self.assertEqual(before.to_data()['nodes'], after.to_data()['nodes'])
        self.assertNotEqual(before.snapshot_id, after.snapshot_id)
        with mock.patch.object(H.Store, 'capture', side_effect=AssertionError('live read')):
            replay = Snapshot.from_json(after.to_json())
        self.assertEqual(replay.snapshot_id, after.snapshot_id)
        self.assertEqual(replay.to_data()['context']['history']['requires'],
                         [C.EXPLICIT_ROOT_DISPOSITION, 'subject-paths/v2'])
        self.assertEqual(self.store.rebuild(), self.entry.read_bytes())

    def test_proposal_can_be_explicitly_accepted_then_retired(self):
        self.publish(A.prepare_proposal(self.entry, 'p.new', {'v': 5}, 'readings', because='proposal'))
        target = self.store.state()['subjects']['p.new']['proposals'][0]
        for kind, expected in [('accept', 'accepted'), ('retire', 'retired')]:
            mutation = A.prepare_act(self.entry, {'kind': kind, 'id': 'p.new', 'of': target,
                                                 'over': [], 'because': kind})
            self.publish(mutation)
            self.assertEqual(self.store.state()['subjects']['p.new']['acceptance'], expected)
        snap = Snapshot.capture([str(self.entry)], read_mode='frozen')
        self.assertNotIn('p.new', snap.to_data()['nodes'])
        self.assertEqual(snap.to_data()['context']['history']['dispositions']['p.new']['marks'][target], 'retired')

    def test_strict_add_has_explicit_accept_and_removing_disposition_refuses(self):
        mutation = A.prepare(self.entry, {'kind': 'add', 'id': 'p.new', 'body': {'v': 7}})
        objects = [C.decode_document(item['after']) for item in mutation.files if item['role'] == 'history_object']
        self.assertEqual({obj['kind'] for obj in objects}, {'reading', 'act'})
        manifest = C.decode_document(next(item['after'] for item in mutation.files if item['role'] == 'history_commit'))
        self.assertEqual(manifest['requires'], [C.EXPLICIT_ROOT_DISPOSITION, 'subject-paths/v2'])
        root = next(obj for obj in objects if obj['kind'] != 'act')
        manifest['objects'] = [item for item in manifest['objects'] if item['id'] == root['id']]
        capture = self.store.capture()
        commits = {**capture.commits, manifest['operation']: C.encode_document(manifest)}
        raw = {**capture.object_bytes, (root['subject'], root['id']): C.encode_document(root)}
        with self.assertRaisesRegex(C.HistoryError, 'missing_root_disposition'):
            C.committed_objects(capture.marker, commits, raw)

    def test_store_refuses_strict_root_without_act_before_any_publication(self):
        root = claim('p.silent', value=9, op='silent-root')
        mutation = self.fixture.mutation([root], op='silent-root')
        files = mutation.files
        commit_file = next(item for item in files if item['role'] == 'history_commit')
        record_file = next(item for item in files if item['role'] == 'record')
        manifest = C.decode_document(commit_file['after'])
        manifest['requires'] = [C.EXPLICIT_ROOT_DISPOSITION]
        capture = self.store.capture()
        rendered = self.store.render(capture, objects={**capture.objects, root['id']: root},
            commits={**capture.commits, 'silent-root': C.encode_document(manifest)})
        manifest['view_sha256'] = C.sha256(rendered)
        commit_file['after'], record_file['after'] = C.encode_document(manifest), rendered
        data = mutation.to_data()
        strict = T.PreparedMutation(operation=data['operation'], authority=data['authority'],
            baseline=data['baseline'], files=files, receipt=data['receipt'])
        with self.assertRaisesRegex(C.HistoryError, 'missing_root_disposition'):
            self.store.commit(strict, verify=lambda data: None)
        self.assertEqual(capture.inventory, self.store.capture().inventory)

    def test_lowlevel_compatibility_root_stays_valid_and_unknown_requires_refuses(self):
        captured = self.store.capture()
        self.assertEqual(C.committed_objects(captured.marker, captured.commits, captured.object_bytes), captured.objects)
        manifest = C.decode_document(next(iter(captured.commits.values())))
        manifest['requires'] = ['unknown/v99']
        with self.assertRaisesRegex(C.HistoryError, 'unsupported_history_capability'):
            C.validate_commit(manifest)

    def test_old_non_strict_authoring_replay_remains_byte_exact(self):
        old = A.prepare(self.entry, {'kind': 'add', 'id': 'p.old', 'body': {'v': 3}}, _strict=False)
        raw = old.to_bytes()
        self.publish(old)
        self.publish(T.PreparedMutation.from_bytes(raw))
        self.assertEqual(old.to_bytes(), raw)
        self.assertEqual(C.decode_document(next(item['after'] for item in old.files if item['role'] == 'history_commit'))['requires'],
                         ['subject-paths/v2'])

    def test_proposal_review_retains_actual_pin_without_promoting_judgment(self):
        self.publish(A.prepare_proposal(self.entry, 'd.proposed', {'verdict': 'not accepted',
            'rests_on': ['p.input'], 'wrong_if': {'expr': 'p.input > 5'}}, 'judgments', because='hypothesis'))
        captured = self.store.capture()
        version = captured.state['subjects']['d.proposed']['proposals'][0]
        review = C.make_object(subject='d.proposed', kind='act', by='reviewer', on='2026-09-17', operation='review',
            body={'act': 'review', 'of': version, 'over': [], 'because': 'read current input',
                  'read': {'p.input': self.root['id']}})
        self.fixture.publish([review], op='review-commit')
        projection = Adapter.from_store_capture(self.store.capture()).projection
        self.assertEqual(projection['subjects']['d.proposed']['acceptance'], 'proposed')
        self.assertEqual(projection['dispositions']['d.proposed']['reviews'], [review])
        self.assertIn(self.root['id'], projection['pins'])


if __name__ == '__main__':
    unittest.main()
