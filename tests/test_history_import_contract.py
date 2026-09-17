"""Lossless typed import bodies and explicit unavailable historical pin evidence."""
import copy
import datetime
from pathlib import Path
import unittest
from unittest import mock

from scripts import history_adapter as A, history_contract as C, history_store as H, versions as V
from scripts.pending_grounding import identity
from scripts.reasoning.snapshot import Snapshot
from tests import test_history_adapter as fixtures
from tests import test_history_store as storage


def imported(body, *, subject='p.input', kind='reading', pins=None, gaps=None,
             operation='import', profile='ordinary-reader/v1'):
    return C.make_object(subject=subject, kind=kind, by=None, on='2026-09-17T08:00:00+00:00',
                         operation=operation, body=body, pins=pins, pin_gaps=gaps,
                         authored={'collection': 'readings' if kind == 'reading' else 'decisions',
                                   'fields': fixtures.FIELDS, 'profile': profile,
                                   'locator': {'path': 'original.yaml', 'writer': None,
                                               'operation': None, 'recorded_at': None}})


class ImportContract(unittest.TestCase):
    def test_scalar_null_date_and_type_identity_survive_object_and_view(self):
        values = [None, False, 0, 1, 1.0, '1', '', datetime.date(2026, 9, 16),
                  '2026-09-16', datetime.datetime(2026, 9, 16, tzinfo=datetime.timezone.utc)]
        identities = []
        for value in values:
            with self.subTest(value=value):
                obj = imported(value)
                identities.append(obj['id'])
                self.assertEqual(C.decode_document(C.encode_document(obj)), obj)
                self.assertEqual(C.validate_closure({obj['id']: obj}), {obj['id']: obj})
                captured = A.capture_history(*fixtures.fixture(obj))
                actual = captured.document['readings']['p.input']
                self.assertEqual(identity(actual), identity(value))
                self.assertEqual(captured.projection['pins'][obj['id']]['object']['body'], value)
                snapshot = captured.snapshot(as_of='2026-09-17')
                replay = Snapshot.from_json(snapshot.to_json())
                self.assertEqual(replay.snapshot_id, snapshot.snapshot_id)
        self.assertEqual(len(set(identities)), len(values))

    def test_scalar_agreement_and_contestation_reduce_without_mapping_assumptions(self):
        a, b = imported(False), imported(0, operation='second')
        state = H.reduce({a['id']: a, b['id']: b})['subjects']['p.input']
        self.assertEqual(state['acceptance'], 'contested')
        same = imported(False, operation='same')
        agreed = H.reduce({a['id']: a, same['id']: same})['subjects']['p.input']
        self.assertEqual(agreed['acceptance'], 'accepted')
        self.assertEqual(agreed['body'], False)
        self.assertEqual(agreed['agreed'], 2)

    def test_scalar_store_render_preserves_null_and_date(self):
        fixture = storage.Storage()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        doc = C.decode_document(fixture.entry.read_bytes())
        doc['meta'].pop('reasoning')
        fixture.entry.write_bytes(C.encode_document(doc))
        fixture.publish([imported(None), imported(datetime.date(2026, 9, 16), subject='p.day')])
        rebuilt = C.decode_document(fixture.store.rebuild(write=True))
        self.assertIsNone(rebuilt['readings']['p.input'])
        self.assertIs(type(rebuilt['readings']['p.day']), datetime.date)
        self.assertEqual(A.from_store_capture(fixture.store.capture()).document['readings'], rebuilt['readings'])

    def test_prototype_and_nonreading_bodies_are_not_reinterpreted(self):
        for kind in ('judgment', 'act'):
            with self.assertRaisesRegex(C.HistoryError, 'invalid_body'):
                imported(None, kind=kind)
        for value in ([], ['original'], {'v': float('inf')}):
            with self.assertRaises(C.HistoryError):
                imported(value)
        legacy = V.version('p.input', 'reading', None, {'v': 1}, on='old', op='legacy')
        self.assertEqual(C.validate_object(legacy), legacy)
        changed = {**legacy, 'body': 1}
        changed['id'] = V.ident(changed)
        with self.assertRaisesRegex(C.HistoryError, 'invalid_body'):
            C.validate_object(changed)

    def test_omitted_gap_field_preserves_existing_envelope_and_meaning(self):
        original = imported({'v': 1})
        self.assertNotIn('pin_gaps', original)
        self.assertEqual(original['id'], identity({k: v for k, v in original.items() if k != 'id'}))
        self.assertNotIn('pin_gaps', C.claim_meaning(original))
        self.assertEqual(C.claim_meaning(original), C.claim_meaning(imported({'v': 1}, gaps={})))

    def test_legacy_list_and_map_gaps_preserve_original_seen_and_no_fake_pin(self):
        for deps in (['p.old', 'p.missing'], {'p.old': None, 'p.missing': 'unavailable'}):
            body = {'verdict': 'ready', 'rests_on': deps, 'seen': {'p.old': 42}, 'wrong_if': 'unknown'}
            obj = imported(body, kind='judgment', subject='d.ready',
                           gaps={'p.old': 'not_recorded', 'p.missing': 'unavailable'})
            self.assertEqual(C.references(obj), [])
            self.assertEqual(C.validate_closure({obj['id']: obj})[obj['id']]['body'], body)
            result = A.capture_history(*fixtures.fixture(obj))
            self.assertEqual(result.document['decisions']['d.ready']['seen'], {'p.old': 42})
            self.assertEqual(set(result.document['decisions']['d.ready']['rests_on']), {'p.old', 'p.missing'})
            projection = result.projection
            self.assertEqual(projection['subjects']['d.ready']['acceptance'], 'accepted')
            self.assertFalse(projection['coverage']['complete'])
            self.assertFalse(projection['integrity']['complete'])
            self.assertEqual({f['code'] for f in projection['integrity']['findings']},
                             {'dependency_pin_not_recorded', 'dependency_pin_unavailable'})
            self.assertEqual(set(projection['pins']), {obj['id']})
            self.assertEqual(projection['pins'][obj['id']]['object']['body'], body)
            self.assertEqual(obj['on'], '2026-09-17T08:00:00+00:00')
            self.assertIsNone(obj['authored']['locator']['recorded_at'])

    def test_gap_reason_is_part_of_identity_and_claim_meaning(self):
        a = imported({'rests_on': ['p.dep']}, kind='judgment', gaps={'p.dep': 'not_recorded'})
        b = imported({'rests_on': ['p.dep']}, kind='judgment', gaps={'p.dep': 'unavailable'})
        self.assertNotEqual(a['id'], b['id'])
        self.assertNotEqual(identity(C.claim_meaning(a)), identity(C.claim_meaning(b)))
        self.assertEqual(H.reduce({a['id']: a, b['id']: b})['subjects']['p.input']['acceptance'], 'contested')

    def test_committed_gap_capture_keeps_original_body_and_reports_support_coverage(self):
        fixture = storage.Storage()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        doc = C.decode_document(fixture.entry.read_bytes())
        doc['meta'].pop('reasoning')
        doc['decisions'] = {}
        fixture.entry.write_bytes(C.encode_document(doc))
        body = {'verdict': 'retained', 'rests_on': ['p.old'], 'seen': {'p.old': 8}}
        decision = imported(body, subject='d.ready', kind='judgment', gaps={'p.old': 'not_recorded'})
        fixture.publish([decision])
        captured = fixture.store.capture()
        self.assertEqual(captured.document['decisions']['d.ready'], body)
        adapted = A.from_store_capture(captured)
        self.assertFalse(adapted.projection['coverage']['complete'])
        self.assertEqual(adapted.projection['subjects']['d.ready']['acceptance'], 'accepted')
        self.assertEqual(adapted.projection['integrity']['findings'], C.pin_gap_findings(decision))

    def test_mixed_resolvable_pins_and_gaps_do_not_weaken_positive_closure(self):
        dep = imported(3)
        obj = imported({'rests_on': {'p.input': dep['id'], 'p.old': None}}, kind='judgment',
                       subject='d.ready', pins={'p.input': dep['id']}, gaps={'p.old': 'not_recorded'})
        with self.assertRaisesRegex(C.HistoryError, 'incomplete_closure'):
            C.validate_closure({obj['id']: obj})
        C.validate_closure({obj['id']: obj, dep['id']: dep})
        result = A.capture_history(*fixtures.fixture(dep, obj))
        evidence = A.pin_review_evidence(result.projection, dep['id'])
        self.assertEqual(evidence['value_status'], 'recorded')
        self.assertEqual(evidence['value']['numerator'], '3')
        self.assertEqual(evidence['basis_status'], 'not_recorded')

    def test_invalid_gap_mappings_refuse(self):
        cases = [({'rests_on': ['p.dep']}, {}, {'p.dep': 'invented'}),
                 ({'rests_on': ['p.dep']}, {'p.dep': 'a' * 64}, {'p.dep': 'unavailable'}),
                 ({'rests_on': ['p.dep']}, {}, {'p.other': 'not_recorded'}),
                 ({'rests_on': {'p.dep': 'a' * 64}}, {}, {'p.dep': 'unavailable'}),
                 ({}, {}, {'p.dep': 'not_recorded'})]
        for body, pins, gaps in cases:
            with self.subTest(body=body, gaps=gaps), self.assertRaises(C.HistoryError):
                imported(body, kind='judgment', pins=pins, gaps=gaps)

    def test_source_free_replay_retains_gaps_and_cannot_claim_complete_support(self):
        obj = imported({'rests_on': ['p.old'], 'seen': {'p.old': 17}}, kind='judgment',
                       subject='d.ready', gaps={'p.old': 'not_recorded'})
        result = A.capture_history(*fixtures.fixture(obj))
        serialized = result.snapshot(as_of='2026-09-17').to_json()
        with mock.patch.object(Path, 'read_bytes', side_effect=AssertionError('live read')), \
             mock.patch('builtins.open', side_effect=AssertionError('live read')):
            replay = Snapshot.from_json(serialized)
        projection = replay.to_data()['context']['history']
        self.assertEqual(projection['pins'][obj['id']]['object']['pin_gaps'], {'p.old': 'not_recorded'})
        self.assertFalse(projection['integrity']['complete'])
        bad = copy.deepcopy(projection)
        bad['integrity'] = {'complete': True, 'findings': []}
        bad['coverage']['complete'] = True
        with self.assertRaisesRegex(C.HistoryError, 'unreported_pin_gap'):
            C.validate_projection(bad)


if __name__ == '__main__':
    unittest.main()
