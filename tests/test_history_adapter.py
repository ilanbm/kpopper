"""Detached history conversion retains evidence without inventing evaluations."""
import copy
import datetime
from pathlib import Path
import unittest
from unittest import mock

from scripts import history_adapter as A, history_contract as C, versions as V
from scripts.pending_grounding import identity
from scripts.reasoning.assessment import assess
from scripts.reasoning.contract import digest
from scripts.reasoning.snapshot import Snapshot


FIELDS = {'value': 'v', 'deps': 'rests_on', 'snapshot': 'seen', 'predicate': 'wrong_if'}
HEADERS = {'meta': {'reasoning': {'version': 2, 'profile': 'core/v1', 'requires': ['arithmetic/v1']}}}


def claim(subject='p.input', body=None, *, kind='reading', pins=None, operation='claim',
          fields=None, profile='core/v1', collection=None):
    return C.make_object(subject=subject, kind=kind, by='writer', on='2026-09-17',
                         operation=operation, body={'v': 1} if body is None else body,
                         pins=pins, authored={'collection': collection or ('decisions' if kind == 'judgment' else 'readings'),
                         'fields': FIELDS if fields is None else fields, 'profile': profile})


def fixture(*claims, acceptance=None, acts=(), pins=None, complete=True):
    objects = {obj['id']: obj for obj in (*claims, *acts)}
    subjects = {}
    for obj in claims:
        subjects.setdefault(obj['subject'], {'acceptance': (acceptance or {}).get(obj['subject'], 'accepted'),
                                            'heads': [], 'open_acts': []})['heads'].append(obj['id'])
    for obj in acts:
        subjects[obj['subject']]['open_acts'].append(obj['id'])
    subjects = {name: {**state, 'heads': sorted(state['heads']), 'open_acts': sorted(state['open_acts'])}
                for name, state in sorted(subjects.items())}
    baseline = {'version': 1, 'record_id': 'record', 'authority_generation': 1,
                'committed_set_digest': identity('commits'),
                'heads': {name: state['heads'] for name, state in subjects.items()},
                'open_acts': {name: state['open_acts'] for name, state in subjects.items() if state['open_acts']}}
    projection = {'projection_version': 1,
                  'authority': C.authority(record_id='record', authority='history', generation=1),
                  'baseline': baseline, 'identity_schemes': sorted({C.ID_SCHEME if 'id_scheme' in obj else C.LEGACY_SCHEME
                                                                 for obj in objects.values()}),
                  'rules': {}, 'rules_digest': identity({}), 'closure_digest': identity(objects),
                  'coverage': {'scope': 'all', 'subjects': list(subjects), 'complete': complete},
                  'subjects': subjects, 'pins': pins or {},
                  'integrity': {'complete': complete, 'findings': []}}
    return objects, projection


def capture(*claims, **kwargs):
    return A.capture_history(*fixture(*claims, **kwargs), document=HEADERS)


class HistoryAdapterTests(unittest.TestCase):
    def test_preserves_authored_collection_fields_body_seen_and_pin_mapping(self):
        old = claim(body={'v': 3})
        roles = {**FIELDS, 'deps': 'grounds', 'snapshot': 'observed', 'predicate': 'fails_if'}
        decision = claim('d.ready', {'grounds': {'p.input': old['id']},
                                    'observed': {'p.input': 99}, 'fails_if': {'expr': 'p.input > 10'}},
                         kind='judgment', pins={'p.input': old['id']}, fields=roles, collection='choices')
        source = claim(fields=roles, operation='current')
        objects, projection = fixture(source, decision)
        objects[old['id']] = old
        result = A.capture_history(objects, projection, document=HEADERS)
        body = result.document['choices']['d.ready']
        self.assertEqual(body, {**decision['body'], 'grounds': ['p.input']})
        self.assertEqual(result.document['schema'], roles)
        self.assertEqual(result.projection['pins'][decision['id']]['object'], decision)
        evidence = A.pin_review_evidence(result.projection, old['id'])
        self.assertEqual(evidence['status'], 'recorded')
        self.assertEqual(evidence['evidence_kind'], 'version_pin')
        self.assertEqual(evidence['value']['numerator'], '3')
        self.assertEqual(body['observed']['p.input'], 99)
        self.assertEqual(decision['body']['grounds'], {'p.input': old['id']})

    def test_no_seen_is_added_for_a_resolvable_pin(self):
        source = claim()
        decision = claim('d.ready', {}, kind='judgment', pins={'p.input': source['id']})
        result = capture(source, decision)
        self.assertEqual(result.document['decisions']['d.ready'], {'rests_on': ['p.input']})
        evidence = A.pin_review_evidence(result.projection, source['id'])
        self.assertEqual(evidence['basis_status'], 'not_recorded')

    def test_review_only_context_changes_identity_and_replays_without_io(self):
        source = claim()
        act = C.make_object(subject=source['subject'], kind='act', by='reviewer', on='2026-09-17',
                            operation='review', body={'act': 'review', 'of': source['id'], 'over': [], 'because': 'read'})
        first = capture(source).snapshot(as_of='2026-09-17')
        second_capture = capture(source, acts=(act,))
        second = second_capture.snapshot(as_of='2026-09-17')
        self.assertEqual(first.to_data()['nodes'], second.to_data()['nodes'])
        self.assertNotEqual(first.snapshot_id, second.snapshot_id)
        serialized = second.to_json()
        with mock.patch.object(Path, 'read_bytes', side_effect=AssertionError('filesystem')), \
                mock.patch('builtins.open', side_effect=AssertionError('filesystem')), \
                mock.patch('subprocess.run', side_effect=AssertionError('native evaluator')), \
                mock.patch.object(V, '_evaluate', side_effect=AssertionError('legacy fallback')):
            replay = Snapshot.from_json(serialized)
            evidence = A.pin_review_evidence(replay.to_data()['context']['history'], source['id'])
            detached = A.capture_history(*fixture(source, acts=(act,)), document=HEADERS)
        self.assertEqual(replay.snapshot_id, second.snapshot_id)
        self.assertEqual(evidence['status'], 'recorded')
        self.assertEqual(detached.document, second_capture.document)

    def test_accepted_unknown_stays_two_independent_dimensions(self):
        decision = claim('d.ready', {'rests_on': [], 'wrong_if': 'Supplier changes terms'}, kind='judgment')
        result = capture(decision)
        report = assess(result.snapshot(), ['d.ready'])
        self.assertEqual(result.projection['subjects']['d.ready']['acceptance'], 'accepted')
        self.assertEqual(report['nodes']['d.ready']['state']['falsifier']['status'], 'unknown')
        self.assertEqual(report['schema_version'], 2)

    def test_contested_alternatives_remain_context_not_scalar_nodes(self):
        left, right = claim(body={'v': True}), claim(body={'v': 1}, operation='other')
        result = capture(left, right, acceptance={'p.input': 'contested'})
        self.assertNotIn('readings', result.document)
        self.assertEqual(set(result.projection['pins']), {left['id'], right['id']})
        with self.assertRaisesRegex(C.HistoryError, 'ambiguous_accepted_selection'):
            capture(left, right)

    def test_equal_accepted_meanings_do_not_depend_on_input_order(self):
        first, second = claim(), claim(operation='other')
        left, right = capture(first, second), capture(second, first)
        self.assertEqual(left.document, right.document)
        self.assertEqual(left.projection, right.projection)

    def test_formula_pin_never_acquires_value_basis_or_seen(self):
        formula = claim(body={'rule': {'expr': 'p.missing + 1'}})
        result = capture(formula)
        evidence = A.pin_review_evidence(result.projection, formula['id'])
        self.assertEqual(evidence['status'], 'recorded')
        self.assertEqual(evidence['body'], formula['body'])
        self.assertEqual(evidence['value_status'], 'unavailable')
        self.assertIsNone(evidence['value'])
        self.assertEqual(evidence['basis_status'], 'not_recorded')
        self.assertNotIn('seen', result.document['readings']['p.input'])

    def test_stored_computed_v2_is_decoded_only_under_recorded_profile(self):
        basis = {'version': 1, 'profile': 'core/v1', 'modules': ['arithmetic/v1'], 'dependencies': []}
        basis['digest'] = digest(basis)
        body = {'rule': {'expr': 'p.old + 1'}, 'computed': {'version': 2,
                    'value': {'type': 'number', 'numerator': '2', 'denominator': '1'}, 'basis': basis}}
        core = claim(body=body)
        result = capture(core)
        evidence = A.pin_review_evidence(result.projection, core['id'])
        self.assertEqual(evidence['value'], body['computed']['value'])
        self.assertEqual(evidence['basis'], basis)
        self.assertEqual(evidence['profile'], 'core/v1')
        legacy = claim(body=body, profile='ordinary-reader/v1')
        result = A.capture_history(*fixture(legacy))
        evidence = A.pin_review_evidence(result.projection, legacy['id'])
        self.assertEqual(evidence['status'], 'recorded')
        self.assertEqual(evidence['value_status'], 'unavailable')
        self.assertEqual(evidence['findings'][0]['code'], 'invalid_history')

    def test_missing_corrupt_pin_and_incomplete_coverage_survive(self):
        for status in ('unavailable', 'corrupt'):
            missing = 'a' * 64
            decision = claim('d.ready', {}, kind='judgment', pins={'p.missing': missing})
            pins = {missing: {'subject': 'p.missing', 'version': missing, 'status': status, 'object': None}}
            objects, projection = fixture(decision, pins=pins, complete=False)
            result = A.capture_history(objects, projection, document=HEADERS)
            self.assertFalse(result.projection['integrity']['complete'])
            self.assertFalse(result.projection['coverage']['complete'])
            self.assertEqual(result.projection['pins'][missing], pins[missing])
            evidence = A.pin_review_evidence(result.projection, missing)
            self.assertEqual(evidence['status'], 'unavailable')
            self.assertIn('pin_' + status, [finding['code'] for finding in evidence['findings']])
            projection['integrity']['complete'] = projection['coverage']['complete'] = True
            with self.assertRaises(C.HistoryError):
                A.capture_history(objects, projection, document=HEADERS)

    def test_unresolved_invalid_and_incompatible_mappings_refuse(self):
        legacy = V.version('p.old', 'reading', 'source', {'v': 1}, op='old', on='2026-09-17')
        cases = [(legacy,), (claim(fields={'value': 'v'}),),
                 (claim(), claim('p.other', fields={**FIELDS, 'snapshot': 'observed'})),
                 (claim(), claim('p.other', profile='checked-reader/v1')),
                 (claim(collection='meta'),)]
        for objects in cases:
            with self.subTest(objects=objects), self.assertRaises(C.HistoryError):
                capture(*objects)

    def test_pin_dependency_disagreement_refuses(self):
        source = claim()
        bad = claim('d.ready', {'rests_on': ['p.other']}, kind='judgment', pins={'p.input': source['id']})
        with self.assertRaisesRegex(C.HistoryError, 'pin_dependency_mismatch'):
            capture(source, bad)

    def test_typed_bodies_are_preserved_and_text_is_not_a_number(self):
        bodies = [True, 1, '1', None, datetime.date(2026, 9, 17), '2026-09-17']
        snapshots = []
        for value in bodies:
            obj = claim(body={'v': value})
            result = capture(obj)
            self.assertEqual(type(result.document['readings']['p.input']['v']), type(value))
            snapshots.append(result.snapshot().snapshot_id)
            evidence = A.pin_review_evidence(result.projection, obj['id'])
            if isinstance(value, str):
                self.assertEqual(evidence['value']['type'], 'text')
            if type(value) is bool:
                self.assertEqual(evidence['value']['type'], 'boolean')
        self.assertEqual(len(set(snapshots)), len(bodies))

    def test_pin_subject_and_object_binding_refuse_mismatch(self):
        obj, other = claim(), claim(body={'v': 2}, operation='other')
        result = capture(obj)
        with self.assertRaisesRegex(C.HistoryError, 'reference_mismatch'):
            A.pin_review_evidence(result.projection, obj['id'], subject='p.other')
        objects, projection = fixture(obj)
        projection['pins'][other['id']] = {'subject': other['subject'], 'version': other['id'],
                                         'status': 'recorded', 'object': other}
        with self.assertRaisesRegex(C.HistoryError, 'pin_object_mismatch'):
            A.capture_history(objects, projection, document=HEADERS)

    def test_projection_is_bounded_and_never_contains_file_inventory(self):
        obj = claim()
        objects, projection = fixture(obj)
        projection['files'] = []
        with self.assertRaisesRegex(C.HistoryError, 'invalid_schema'):
            A.capture_history(objects, projection)
        projection.pop('files')
        projection['rules']['big'] = 'x' * C.MAX_PROJECTION_BYTES
        with self.assertRaisesRegex(C.HistoryError, 'history_limit'):
            A.capture_history(objects, projection)
        self.assertNotIn('files', capture(obj).projection)

    def test_core_requires_declared_capabilities_and_templates_cannot_supply_nodes(self):
        source = claim()
        with self.assertRaisesRegex(C.HistoryError, 'missing_reasoning_declaration'):
            A.capture_history(*fixture(source))
        with self.assertRaisesRegex(C.HistoryError, 'invalid_document'):
            A.capture_history(*fixture(source), document={**HEADERS, 'readings': {'p.fake': {'v': 4}}})
        result = A.capture_history(*fixture(source), document={**HEADERS, 'questions': {}, 'title': 'Original heading'})
        self.assertEqual(result.document['questions'], {})
        self.assertEqual(result.document['title'], 'Original heading')

    def test_checked_profile_is_retained_without_claiming_core_conversion(self):
        obj = claim(profile='checked-reader/v1')
        result = A.capture_history(*fixture(obj))
        self.assertNotIn('reasoning', result.document['meta'])
        self.assertEqual(A.pin_review_evidence(result.projection, obj['id'])['profile'], 'checked-reader/v1')

    def test_stale_recorded_basis_is_unavailable_without_erasing_the_pin(self):
        basis = {'version': 1, 'profile': 'core/v1', 'dependencies': []}
        basis['digest'] = digest(basis)
        basis['dependencies'] = ['changed']
        obj = claim(body={'computed': {'version': 2, 'value': {'type': 'boolean', 'value': True}, 'basis': basis}})
        result = capture(obj)
        evidence = A.pin_review_evidence(result.projection, obj['id'])
        self.assertEqual(evidence['status'], 'recorded')
        self.assertEqual(evidence['value_status'], 'unavailable')
        self.assertEqual(evidence['basis_status'], 'unavailable')
        self.assertEqual(evidence['body'], obj['body'])

    def test_declared_partial_scope_remains_partial(self):
        obj = claim()
        objects, projection = fixture(obj, complete=False)
        projection['coverage']['scope'] = 'selected'
        result = A.capture_history(objects, projection, document=HEADERS)
        self.assertEqual(result.projection['coverage'], projection['coverage'])
        self.assertFalse(result.projection['integrity']['complete'])

    def test_actual_committed_object_mapping_is_consumed_without_store_access(self):
        obj = claim()
        _, projection = fixture(obj)
        raw = C.encode_document(obj)
        manifest = C.make_commit(marker=projection['authority'], operation='commit', parents={},
                                 baseline=projection['baseline'], objects=[(obj, raw)], receipt={'validated': True},
                                 view=b'captured-view', view_template=C.document_template(HEADERS))
        committed = C.committed_objects(projection['authority'], {'commit': C.encode_document(manifest)},
                                         {(obj['subject'], obj['id']): raw})
        result = A.capture_history(committed, projection, document=manifest['view_template'])
        self.assertEqual(result.document['readings']['p.input'], obj['body'])

    def test_caller_and_result_mutations_do_not_change_captured_evidence(self):
        obj = claim()
        objects, projection = fixture(obj)
        result = A.capture_history(objects, projection, document=HEADERS)
        objects[obj['id']]['body']['v'] = 99
        output = result.document
        output['readings']['p.input']['v'] = 42
        self.assertEqual(result.document['readings']['p.input']['v'], 1)
        self.assertEqual(result.projection['pins'][obj['id']]['object']['body']['v'], 1)


if __name__ == '__main__':
    unittest.main()
