"""Direct immutable authoring, exact evidence, and manifest crash replay."""
import copy
import contextlib
import io
from pathlib import Path
import unittest
from unittest import mock

from scripts import history_authoring as A, history_contract as C, history_transaction as T
from scripts import provenance as P, versions as V
from tests import test_history_store as fixtures

def claim(*args, **kwargs):
    obj = fixtures.claim(*args, **kwargs)
    obj['authored']['fields'] = {'deps': 'rests_on', 'snapshot': 'seen', 'predicate': 'wrong_if'}
    obj['id'] = C.object_identity(obj)
    return C.validate_object(obj)


class Authoring(unittest.TestCase):
    def setUp(self):
        fixture = fixtures.Storage()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        self.fixture, self.entry, self.store = fixture, fixture.entry, fixture.store
        self.original = claim(body={'v': 1, 'of': '2026-09-16'})
        fixture.publish([self.original], op='bootstrap')

    def prepare(self, action, **kwargs):
        return A.prepare(self.entry, action, by='writer', **kwargs)

    def publish(self, mutation):
        return A.commit(self.entry, mutation, verify=lambda data: None)

    def test_set_creates_claim_and_explicit_accept_without_mutating_original(self):
        archive = Path(self.store.layout['replaced'])
        archive.write_bytes(b'original legacy archive bytes\n')
        original_bytes = self.store.capture().object_bytes[('p.input', self.original['id'])]
        mutation = self.prepare({'kind': 'set', 'id': 'p.input', 'value': 2, 'as_of': '2026-09-17'})
        self.assertEqual(self.store.state()['subjects']['p.input']['body']['v'], 1)
        self.publish(mutation)
        captured = self.store.capture()
        state = captured.state['subjects']['p.input']
        self.assertEqual(state['body']['v'], 2)
        self.assertEqual(captured.object_bytes[('p.input', self.original['id'])], original_bytes)
        new = [o for o in captured.objects.values() if o['op'] == mutation.to_data()['operation']]
        self.assertEqual({o['kind'] for o in new}, {'reading', 'act'})
        act = next(o for o in new if o['kind'] == 'act')
        self.assertEqual(act['body']['act'], 'accept')
        self.assertEqual(act['body']['over'], [self.original['id']])
        self.assertEqual(archive.read_bytes(), b'original legacy archive bytes\n')

    def test_equal_observations_get_new_operations_and_exact_retry_is_idempotent(self):
        action = {'kind': 'set', 'id': 'p.input', 'value': 1, 'as_of': '2026-09-17'}
        first = self.prepare(action)
        restored = T.PreparedMutation.from_bytes(first.to_bytes())
        self.assertEqual(restored.to_bytes(), first.to_bytes())
        self.publish(restored)
        self.publish(restored)
        second = self.prepare(action)
        self.assertNotEqual(first.to_data()['operation'], second.to_data()['operation'])
        self.publish(second)
        self.assertEqual(len(self.store.capture().commits), 3)

    def test_new_judgment_pins_real_dependencies_and_captures_typed_seen(self):
        mutation = self.prepare({'kind': 'add', 'id': 'p.ready', 'into': 'judgments',
            'body': {'verdict': 'ready', 'rests_on': ['p.input'], 'wrong_if': {'expr': 'p.input > 5'}}})
        with mock.patch.object(V, '_evaluate', side_effect=AssertionError('prototype evaluator called')):
            self.publish(mutation)
        captured = self.store.capture()
        obj = captured.objects[captured.state['subjects']['p.ready']['head']]
        self.assertEqual(obj['pins'], {'p.input': self.original['id']})
        self.assertEqual(obj['body']['seen']['p.input']['computed']['value'],
                         {'type': 'number', 'numerator': '1', 'denominator': '1'})
        self.assertEqual(obj['body']['seen']['p.input']['computed']['basis']['as_of'], None)
        self.assertEqual(mutation.to_data()['receipt']['before']['authoring']['version'], 7)
        self.assertEqual(mutation.to_data()['receipt']['profile'], 'core/v1')
        result = mutation.to_data()['receipt']['after']['assessment']['nodes']['p.ready']['state']['falsifier']
        self.assertEqual(result['status'], 'does_not_hold')
        original_seen = copy.deepcopy(obj['body']['seen'])
        self.publish(self.prepare({'kind': 'set', 'id': 'p.input', 'value': 2,
                                   'as_of': '2026-09-18'}))
        self.assertEqual(self.store.capture().objects[obj['id']]['body']['seen'], original_seen)
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(P.core_check([str(self.entry)]), 0)

    def test_new_judgment_snapshot_retains_formula_value_and_basis(self):
        formula = claim('p.ratio', op='formula-snapshot', body={
            'rule': {'expr': 'p.input / 2'}})
        self.fixture.publish([formula], op='formula-snapshot')
        mutation = self.prepare({'kind': 'add', 'id': 'p.formula_ready', 'into': 'judgments',
            'body': {'verdict': 'ready', 'rests_on': ['p.ratio'],
                     'wrong_if': {'expr': 'p.ratio > 1'}}})
        made = [C.decode_document(item['after']) for item in mutation.files
                if item['role'] == 'history_object']
        judgment = next(item for item in made if item['kind'] == 'judgment')
        computed = judgment['body']['seen']['p.ratio']['computed']
        self.assertEqual(computed['value'],
                         {'type': 'number', 'numerator': '1', 'denominator': '2'})
        self.assertEqual(computed['basis']['recipe'], 'merkle-inputs/v1')

    def test_retained_version_one_judgment_receipt_replays_without_new_seen(self):
        mutation = A.prepare(self.entry, {'kind': 'add', 'id': 'p.legacy_ready',
            'into': 'judgments', 'body': {'verdict': 'ready', 'rests_on': ['p.input'],
                'wrong_if': {'expr': 'p.input > 5'}}}, by='writer',
            operation='retained-v1', _receipt_version=1)
        self.assertEqual(mutation.to_data()['receipt']['before']['authoring']['version'], 1)
        made = [C.decode_document(item['after']) for item in mutation.files
                if item['role'] == 'history_object']
        self.assertNotIn('seen', next(item for item in made if item['kind'] == 'judgment')['body'])
        A.verify_prepared(self.entry, T.PreparedMutation.from_bytes(mutation.to_bytes()))

    def test_retained_version_five_proposal_replays_without_new_seen(self):
        mutation = A.prepare_proposal(self.entry, 'p.old_proposal', {
            'verdict': 'ready', 'rests_on': ['p.input'],
            'wrong_if': {'expr': 'p.input > 5'}}, 'judgments', because='retained',
            by='writer', operation='retained-v5', _receipt_version=5)
        self.assertEqual(mutation.to_data()['receipt']['before']['authoring']['version'], 5)
        made = [C.decode_document(item['after']) for item in mutation.files
                if item['role'] == 'history_object']
        self.assertNotIn('seen', next(item for item in made if item['kind'] == 'judgment')['body'])
        A.verify_prepared(self.entry, T.PreparedMutation.from_bytes(mutation.to_bytes()))

    def test_public_add_then_core_check_has_no_missing_snapshot(self):
        action = {'kind': 'add', 'id': 'p.public_ready', 'into': 'judgments',
                  'as_of': '2026-09-01', 'body': {'verdict': 'ready',
                  'rests_on': ['p.input'], 'wrong_if': {'expr': 'p.input > 5'}}}
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(P.apply([str(self.entry)], action), 0)
            self.assertEqual(P.core_check([str(self.entry)]), 0)

    def test_user_supplied_seen_is_refused_before_automatic_capture(self):
        before = self.store.capture().inventory
        with self.assertRaises((P.Refused, SystemExit)):
            self.prepare({'kind': 'add', 'id': 'p.bad_seen', 'into': 'judgments',
                'body': {'verdict': 'ready', 'rests_on': ['p.input'],
                         'wrong_if': {'expr': 'p.input > 5'}, 'seen': {'p.input': 99}}})
        self.assertEqual(self.store.capture().inventory, before)

    def test_blocked_missing_dependency_records_gap_without_fabricating_seen(self):
        mutation = self.prepare({'kind': 'add', 'id': 'p.wait', 'into': 'judgments',
            'body': {'verdict': 'pending', 'rests_on': ['p.missing'],
                     'wrong_if': {'expr': 'p.missing > 5'},
                     'blocked_on': {'missing': ['p.missing'], 'why': 'awaiting source'}}})
        made = [C.decode_document(item['after']) for item in mutation.files
                if item['role'] == 'history_object']
        judgment = next(item for item in made if item['kind'] == 'judgment')
        self.assertEqual(judgment['pins'], {})
        self.assertEqual(judgment['pin_gaps'], {'p.missing': 'unavailable'})
        self.assertEqual(judgment['body']['seen'], {})

    def test_review_records_current_pins_and_preserves_old_seen_and_formula(self):
        formula = claim('p.ratio', op='formula', body={'rule': {'expr': 'p.input / 3'}})
        judgment = claim('p.ready', kind='judgment', op='judgment',
            body={'verdict': 'ready', 'rests_on': ['p.ratio'], 'wrong_if': {'expr': 'p.ratio > 5'},
                  'seen': {'p.ratio': 'historical unknown'}}, pins={'p.ratio': formula['id']})
        self.fixture.publish([formula, judgment], op='add-judgment')
        mutation = self.prepare({'kind': 'review', 'id': 'p.ready'})
        self.publish(mutation)
        capture = self.store.capture()
        self.assertEqual(capture.objects[judgment['id']], judgment)
        self.assertEqual(capture.objects[formula['id']], formula)
        review = next(o for o in capture.objects.values() if o['op'] == mutation.to_data()['operation'])
        self.assertEqual(review['body']['read'], {'p.ratio': formula['id']})
        self.assertEqual(review['body']['act'], 'review')
        self.assertNotIn('seen', review['body'])
        self.assertEqual(capture.state['subjects']['p.ready']['head'], judgment['id'])

    def test_review_pins_current_version_after_dependency_replacement(self):
        judgment = claim('p.ready', kind='judgment', op='judgment', body={
            'verdict': 'ready', 'rests_on': ['p.input'], 'wrong_if': {'expr': 'p.input > 5'},
            'seen': {'p.input': 1}}, pins={'p.input': self.original['id']})
        self.fixture.publish([judgment], op='judgment-bootstrap')
        self.publish(self.prepare({'kind': 'set', 'id': 'p.input', 'value': 2}))
        current = self.store.state()['subjects']['p.input']['head']
        mutation = self.prepare({'kind': 'review', 'id': 'p.ready'})
        self.publish(mutation)
        captured = self.store.capture()
        review = next(o for o in captured.objects.values() if o['op'] == mutation.to_data()['operation'])
        self.assertEqual(review['body']['read'], {'p.input': current})
        self.assertNotEqual(current, self.original['id'])
        self.assertEqual(captured.objects[judgment['id']]['pins'], {'p.input': self.original['id']})
        self.assertEqual(captured.objects[judgment['id']]['body']['seen'], {'p.input': 1})

    def test_replacement_uses_current_writer_intent_and_retains_predicate(self):
        old = claim('p.ready', kind='judgment', op='old', body={
            'verdict': 'old', 'rests_on': ['p.input'], 'wrong_if': {'expr': 'p.input > 0'},
            'seen': {'p.input': 0}}, pins={'p.input': self.original['id']})
        self.fixture.publish([old], op='old-judgment')
        mutation = self.prepare({'kind': 'add', 'id': 'p.ready', 'body': {
            'verdict': 'new', 'rests_on': ['p.input'], 'wrong_if': {'expr': 'p.input > 5'}}})
        self.publish(mutation)
        capture = self.store.capture()
        self.assertEqual(capture.objects[old['id']], old)
        self.assertEqual(capture.state['subjects']['p.ready']['body']['verdict'], 'new')

    def test_exact_arithmetic_rejects_born_broken_before_publication(self):
        before = self.store.capture().inventory
        with self.assertRaisesRegex(P.Refused, 'born broken'):
            self.prepare({'kind': 'add', 'id': 'p.ready', 'body': {'verdict': 'ready',
                'rests_on': ['p.input'], 'wrong_if': {'expr': '9007199254740993 > 9007199254740992'}}})
        self.assertEqual(self.store.capture().inventory, before)

    def test_crash_after_manifest_replays_original_bytes_without_duplicate_evidence(self):
        mutation = self.prepare({'kind': 'set', 'id': 'p.input', 'value': 2})
        before = self.entry.read_bytes()
        with mock.patch.object(T, '_replace', side_effect=OSError('view crash')):
            with self.assertRaisesRegex(OSError, 'view crash'):
                self.publish(mutation)
        self.assertEqual(self.entry.read_bytes(), before)
        self.assertIn(mutation.to_data()['operation'], self.store.capture().commits)
        self.publish(T.PreparedMutation.from_bytes(mutation.to_bytes()))
        self.assertEqual(self.store.capture().state['subjects']['p.input']['body']['v'], 2)
        self.assertEqual(len(self.store.capture().commits), 2)

    def test_crash_before_manifest_keeps_previous_acceptance_and_retries(self):
        mutation = self.prepare({'kind': 'set', 'id': 'p.input', 'value': 2})
        publish = T.publish_immutable
        def fault(path, *args, **kwargs):
            if Path(path).parent == Path(self.store.layout['history_commits']):
                raise OSError('manifest crash')
            return publish(path, *args, **kwargs)
        with mock.patch.object(T, 'publish_immutable', side_effect=fault):
            with self.assertRaisesRegex(OSError, 'manifest crash'):
                self.publish(mutation)
        self.assertEqual(self.store.state()['subjects']['p.input']['body']['v'], 1)
        self.publish(T.PreparedMutation.from_bytes(mutation.to_bytes()))
        self.assertEqual(self.store.state()['subjects']['p.input']['body']['v'], 2)

    def test_changed_archive_and_external_verifier_failure_publish_nothing(self):
        mutation = self.prepare({'kind': 'set', 'id': 'p.input', 'value': 2})
        with self.assertRaisesRegex(ValueError, 'policy'):
            A.commit(self.entry, mutation, verify=lambda data: (_ for _ in ()).throw(ValueError('policy')))
        archive = Path(self.store.layout['replaced'])
        archive.write_bytes(b'concurrent archive edit')
        with self.assertRaisesRegex(C.HistoryError, 'concurrent_archive_edit'):
            self.publish(mutation)
        self.assertEqual(len(self.store.capture().commits), 1)

    def test_source_replacement_citation_and_observation_time_survive_retry(self):
        source = claim('source.report', op='source', body={'url': 'https://example.test/report'})
        self.fixture.publish([source], op='source-commit')
        mutation = self.prepare({'kind': 'set', 'id': 'p.input', 'value': 2,
            'as_of': None, 'source': 'source.report', 'at': 'page 7'}, recorded_at='2026-09-17T07:02:03+00:00')
        self.publish(T.PreparedMutation.from_bytes(mutation.to_bytes()))
        captured = self.store.capture()
        obj = captured.objects[captured.state['subjects']['p.input']['head']]
        self.assertEqual((obj['body']['from'], obj['body']['at']), ('source.report', 'page 7'))
        self.assertEqual(obj['on'], '2026-09-17T07:02:03+00:00')
        self.assertIsNotNone(mutation.to_data()['receipt']['before']['authoring']['action']['as_of'])

    def test_new_collection_is_retained_in_immutable_template(self):
        mutation = self.prepare({'kind': 'add', 'id': 'source.new',
                                 'body': {'url': 'https://example.test/new'}})
        self.publish(mutation)
        captured = self.store.capture()
        self.assertIn('source.new', captured.document['sources'])
        self.assertEqual(self.store._template(captured.commits)['sources'], {})

    def test_second_preparation_refuses_stale_baseline_without_dropping_first(self):
        first = self.prepare({'kind': 'set', 'id': 'p.input', 'value': 2})
        second = self.prepare({'kind': 'set', 'id': 'p.input', 'value': 3})
        self.publish(first)
        with self.assertRaisesRegex(C.HistoryError, 'stale_baseline'):
            self.publish(second)
        self.assertEqual(self.store.state()['subjects']['p.input']['body']['v'], 2)

    def test_frozen_archive_is_rechecked_after_caller_verifier(self):
        mutation = self.prepare({'kind': 'set', 'id': 'p.input', 'value': 2})
        with self.assertRaisesRegex(C.HistoryError, 'concurrent_archive_edit'):
            A.commit(self.entry, mutation, verify=lambda data:
                     Path(self.store.layout['replaced']).write_bytes(b'changed during verification'))
        self.assertEqual(len(self.store.capture().commits), 1)

    def test_missing_dependency_is_retained_only_as_an_explicit_gap(self):
        mutation = self.prepare({'kind': 'add', 'id': 'p.wait', 'body': {'verdict': 'waiting',
            'rests_on': ['p.missing'], 'blocked_on': 'source is unavailable'}})
        made = [C.decode_document(item['after']) for item in mutation.files
                if item['role'] == 'history_object']
        judgment = next(item for item in made if item['kind'] == 'judgment')
        self.assertEqual(judgment['pins'], {})
        self.assertEqual(judgment['pin_gaps'], {'p.missing': 'unavailable'})
        self.assertEqual(judgment['body']['seen'], {})

    def test_named_legacy_profiles_are_not_promoted(self):
        for profile in ('ordinary-reader/v1', 'checked-reader/v1'):
            with self.subTest(profile=profile):
                fixture = fixtures.Storage()
                fixture.setUp()
                self.addCleanup(fixture.doCleanups)
                document = C.decode_document(fixture.entry.read_bytes())
                document['meta'].pop('reasoning')
                fixture.entry.write_bytes(C.encode_document(document))
                reading = claim(body={'v': 1, 'of': '2026-09-16'})
                reading['authored']['profile'] = profile
                reading['id'] = C.object_identity(reading)
                judgment = claim('p.ready', kind='judgment', op='legacy-judgment', body={
                    'verdict': 'ready', 'rests_on': ['p.input'], 'wrong_if': 'p.input > 5',
                    'seen': {'p.input': 1}}, pins={'p.input': reading['id']})
                judgment['authored']['profile'] = profile
                judgment['id'] = C.object_identity(judgment)
                fixture.publish([reading, judgment], op='legacy-bootstrap')
                mutation = A.prepare(fixture.entry, {'kind': 'set', 'id': 'p.input',
                                                     'value': 2, 'as_of': '2026-09-17'})
                A.commit(fixture.entry, mutation, verify=lambda data: None)
                self.assertEqual(mutation.to_data()['receipt']['profile'], profile)
                captured = fixture.store.capture()
                self.assertNotIn('reasoning', captured.document['meta'])
                current = captured.objects[captured.state['subjects']['p.input']['head']]
                self.assertEqual(current['authored']['profile'], profile)
                self.assertEqual(captured.objects[judgment['id']]['body']['seen'], {'p.input': 1})

    def test_tampered_receipt_refuses_even_with_valid_outer_serialization(self):
        mutation = self.prepare({'kind': 'set', 'id': 'p.input', 'value': 2})
        data = mutation.to_data()
        receipt = copy.deepcopy(data['receipt'])
        receipt['after']['document']['readings']['p.input']['v'] = 999
        receipt = T.semantic_receipt(**{k: receipt[k] for k in ('profile', 'capabilities', 'before', 'after')})
        files = mutation.files
        manifest_file = next(i for i in files if i['role'] == 'history_commit')
        manifest = C.decode_document(manifest_file['after'])
        manifest['receipt'] = receipt
        manifest_file['after'] = C.encode_document(manifest)
        bad = T.PreparedMutation(operation=data['operation'], authority=data['authority'],
                                 baseline=data['baseline'], receipt=receipt, files=files)
        with self.assertRaisesRegex(C.HistoryError, 'authoring_receipt_mismatch'):
            self.publish(bad)
        self.assertEqual(len(self.store.capture().commits), 1)


if __name__ == '__main__':
    unittest.main()
