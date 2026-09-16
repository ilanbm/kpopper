"""History evidence never silently becomes authority or computed review data."""
import copy
import datetime
import unittest
from unittest import mock

from scripts import history_contract as H, history_transaction as T, provenance as P, versions as V
from scripts.pending_grounding import identity
from scripts.reasoning.snapshot import Snapshot


def claim(subject='p.input', body=None, operation='op-1', kind='reading', pins=None):
    return H.make_object(subject=subject, kind=kind, by='source.a', on='2026-09-17T00:00:00Z',
                         operation=operation, body={'v': 1} if body is None else body,
                         pins=pins, authored={'collection': 'readings', 'profile': 'core/v1',
                                               'fields': {'value': 'v', 'deps': 'rests_on',
                                                          'snapshot': 'seen', 'predicate': 'wrong_if'}})


def baseline(obj):
    return {'version': 1, 'record_id': 'record-1', 'authority_generation': 1,
            'committed_set_digest': identity([]), 'heads': {obj['subject']: [obj['id']]},
            'open_acts': {}}


def projection(obj):
    return {'projection_version': 1,
            'authority': H.authority(record_id='record-1', authority='history', generation=1),
            'baseline': baseline(obj), 'identity_schemes': [H.ID_SCHEME], 'rules': {},
            'rules_digest': identity({}), 'closure_digest': identity([obj]),
            'coverage': {'scope': 'all', 'subjects': [obj['subject']], 'complete': True},
            'subjects': {obj['subject']: {'acceptance': 'accepted', 'heads': [obj['id']], 'open_acts': []}},
            'pins': {obj['id']: {'subject': obj['subject'], 'version': obj['id'],
                                'status': 'recorded', 'object': obj}},
            'integrity': {'complete': True, 'findings': []}}


def receipt():
    return T.semantic_receipt(profile='core/v1', capabilities={'requires': ['arithmetic/v1']},
                              before={'snapshot_id': identity('before')},
                              after={'snapshot_id': identity('after')})


class TypedObjects(unittest.TestCase):
    def test_shared_python_graph_is_bounded_before_recursive_validation(self):
        shared = ['leaf']
        for _ in range(8):
            shared = [shared] * 10
        with mock.patch.object(H, '_check_finite') as finite:
            with self.assertRaisesRegex(H.HistoryError, 'history_limit'):
                H.detached({'value': shared})
            finite.assert_not_called()

    def test_identity_distinguishes_typed_values_and_absence(self):
        bodies = [{'v': datetime.date(2026, 9, 17)}, {'v': '2026-09-17'},
                  {'v': True}, {'v': 1}, {'v': 1.0}, {'v': None}, {}, {'v': [1, 2]}, {'v': [2, 1]}]
        self.assertEqual(len({claim(body=body)['id'] for body in bodies}), len(bodies))

    def test_retry_reuses_envelope_but_fresh_observation_is_distinct(self):
        original = claim()
        self.assertEqual(original, claim())
        self.assertNotEqual(original['id'], claim(operation='op-2')['id'])
        changed = copy.deepcopy(original)
        changed['on'] = '2026-09-18T00:00:00Z'
        with self.assertRaisesRegex(H.HistoryError, 'identity_mismatch'):
            H.validate_object(changed)

    def test_legacy_identity_is_preserved_without_coercive_v2_fallback(self):
        old = V.version('p.input', 'reading', 'source.a', {'v': 1}, op='old', on='2026-09-17')
        self.assertEqual(H.validate_object(old), old)
        self.assertEqual(len(old['id']), 40)
        changed = {**old, 'id_scheme': 'unknown'}
        with self.assertRaisesRegex(H.HistoryError, 'unsupported_identity'):
            H.validate_object(changed)

    def test_original_seen_and_structured_predicate_are_detached_and_preserved(self):
        source = claim()
        body = {'verdict': 'ready', 'rests_on': ['p.input'], 'seen': {'p.input': 7},
                'wrong_if': {'expr': {'op': 'gt', 'args': [{'ref': 'p.input'}, {'lit': 2}]}}}
        obj = claim('d.ready', body, kind='judgment', pins={'p.input': source['id']})
        self.assertEqual(obj['body'], body)
        body['seen']['p.input'] = 99
        self.assertEqual(obj['body']['seen']['p.input'], 7)
        self.assertEqual(H.validate_closure({v['id']: v for v in (source, obj)})[obj['id']], obj)

    def test_missing_wrong_subject_and_act_pins_refuse_complete_closure(self):
        source = claim()
        for pins in ({'p.input': 'a' * 64}, {'p.other': source['id']}):
            obj = claim('d.ready', {'verdict': 'ready'}, kind='judgment', pins=pins)
            with self.assertRaises(H.HistoryError):
                H.validate_closure({v['id']: v for v in (source, obj)})
        act = H.make_object(subject='p.input', kind='act', by='reviewer', on='2026-09-17',
                            operation='op-2', body={'act': 'review', 'of': source['id'],
                                                    'over': [], 'because': 'checked'})
        obj = claim('d.ready', {'verdict': 'ready'}, kind='judgment', pins={'p.input': act['id']})
        with self.assertRaisesRegex(H.HistoryError, 'reference_mismatch'):
            H.validate_closure({v['id']: v for v in (source, act, obj)})

    def test_subject_and_operation_cannot_escape_store(self):
        for key in ('subject', 'operation'):
            with self.subTest(key=key), self.assertRaises(H.HistoryError):
                claim(**{key: '../escape'})


class CapturedEvidence(unittest.TestCase):
    def test_new_review_context_changes_snapshot_without_changing_values(self):
        obj = claim()
        context = projection(obj)
        doc = {'readings': {'p.input': {'v': 1}}, 'meta': {'history': context['baseline']}}
        first = H.CapturedHistory(doc, context).snapshot(as_of='2026-09-17')
        other = copy.deepcopy(context)
        other['closure_digest'] = identity('new review')
        second = H.CapturedHistory(doc, other).snapshot(as_of='2026-09-17')
        self.assertNotEqual(first.snapshot_id, second.snapshot_id)
        replay = Snapshot.from_json(first.to_json())
        self.assertEqual(replay.snapshot_id, first.snapshot_id)
        self.assertEqual(replay.to_data()['context']['history'], context)

    def test_formula_pin_carries_body_without_fabricating_value_or_seen(self):
        obj = claim(body={'rule': {'expr': 'p.missing + 1'}})
        context = projection(obj)
        doc = {'readings': {'p.input': obj['body']}, 'meta': {'history': context['baseline']}}
        captured = H.CapturedHistory(doc, context)
        self.assertNotIn('seen', captured.document['readings']['p.input'])
        self.assertNotIn('computed', captured.projection['pins'][obj['id']]['object']['body'])

    def test_marker_removed_or_changed_baseline_refuses(self):
        obj = claim()
        for field in ('record_id', 'authority_generation'):
            context = projection(obj)
            context['baseline'][field] = 'other' if field == 'record_id' else 2
            with self.subTest(field=field), self.assertRaisesRegex(H.HistoryError, 'authority_mismatch'):
                H.validate_projection(context)
        with self.assertRaisesRegex(H.HistoryError, 'baseline_mismatch'):
            H.CapturedHistory({'readings': {}}, projection(obj))

    def test_missing_evidence_does_not_masquerade_as_complete(self):
        context = projection(claim())
        context['integrity']['findings'] = [{'code': 'missing', 'subject': 'p.input',
                                             'object_id': 'a' * 64, 'detail': 'absent'}]
        with self.assertRaisesRegex(H.HistoryError, 'invalid_integrity'):
            H.validate_projection(context)
        context['integrity']['complete'] = False
        context['coverage']['complete'] = False
        self.assertEqual(H.validate_projection(context)['integrity'], context['integrity'])

    def test_projection_is_bounded_and_file_inventory_is_not_allowed(self):
        context = projection(claim())
        context['files'] = []
        with self.assertRaisesRegex(H.HistoryError, 'invalid_schema'):
            H.validate_projection(context)
        context.pop('files')
        context['rules']['large'] = 'x' * H.MAX_PROJECTION_BYTES
        with self.assertRaisesRegex(ValueError, 'history_limit'):
            H.validate_projection(context)

    def test_layout_roles_cover_current_and_legacy_custom_names(self):
        for name, prefix in [('GROUNDING.yaml', '.kpopper/'), ('PROVENANCE.yaml', 'PROVENANCE.'),
                             ('notes.yaml', 'PROVENANCE.')]:
            layout = P.layout('/tmp/example/' + name)
            self.assertTrue(layout['history'].endswith(prefix + 'history'))
            self.assertTrue(layout['history_commits'].endswith(prefix + 'history-commits'))
            self.assertTrue(layout['history_authority'].endswith(prefix + 'history.yaml'))


if __name__ == '__main__':
    unittest.main()
