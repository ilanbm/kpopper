"""Pure physical-layer import; copied migration activation is a separate integration gate."""
import copy
import datetime
from pathlib import Path
import unittest
from unittest import mock

from scripts import history_hypothesis_import as I, history_contract as C, history_store as H
from scripts import provenance as P
from scripts.pending_grounding import identity
from tests.test_history_authoring import claim


class HypothesisImport(unittest.TestCase):
    def setUp(self):
        self.base = claim('p.input', value=1, op='base')
        self.objects = {self.base['id']: self.base}
        self.document = {'meta': {'reasoning': {'version': 2, 'profile': 'core/v1', 'requires': ['arithmetic/v1']}},
                         'readings': {'p.input': {'v': 1}}, 'judgments': {}}

    def source(self, name='alternative', document=None, head=None, *, entry='GROUNDING.yaml', **extra):
        document = document if document is not None else {'readings': {'p.input': {'v': 2}}}
        head = head if head is not None else {'claim': 'different input', 'folds': 'never',
                                            'born': datetime.date(2026, 9, 12)}
        raw = b'# exact physical hypothesis\r\n' + C.encode_document({'hypothesis': head, **document}).replace(b'\n', b'\r\n')
        directory = Path(P.layout('/' + entry)['hypotheses']).as_posix().lstrip('/')
        return {'name': name, 'document': document, 'head': head,
                'path': directory + '/' + name + '.yaml', 'bytes': raw, **extra}

    def prepare(self, sources, **kwargs):
        return I.prepare(self.objects, sources, base_document=self.document,
            operation='captured-import', recorded_at='2026-09-17T08:01:02Z', **kwargs)

    def test_original_bodies_dates_seen_head_and_unknown_provenance_survive_without_io(self):
        body = {'verdict': 'possible', 'rests_on': ['p.input'], 'wrong_if': {'expr': 'p.input > 5'},
                'seen': {'p.input': {'recorded_old': datetime.date(2020, 1, 1)}}}
        source = self.source(document={'judgments': {'d.possible': body}})
        original = copy.deepcopy(source)
        with mock.patch.object(Path, 'read_bytes', side_effect=AssertionError('source access')), \
             mock.patch.object(P, 'load', side_effect=AssertionError('source access')):
            result = self.prepare([source])
        self.assertEqual(source, original)
        version = result['groups']['alternative']['versions']['d.possible']
        obj = result['objects'][version]
        self.assertEqual(obj['body'], body)
        self.assertEqual(obj['authored']['hypothesis']['head'], source['head'])
        self.assertEqual(obj['authored']['hypothesis']['head']['folds'], 'never')
        self.assertIsNone(obj['by'])
        self.assertEqual(obj['on'], '2026-09-17T08:01:02Z')
        self.assertTrue(all(value is None for value in obj['authored']['locator']['original'].values()))
        self.assertEqual(obj['pins'], {})
        self.assertEqual(obj['pin_gaps'], {'p.input': 'not_recorded'})
        read = obj['authored']['locator']['import']['observations']['p.input']
        self.assertEqual(read['kind'], 'base_version')
        self.assertEqual(read['version'], self.base['id'])
        self.assertFalse(result['historical_support_complete'])
        self.assertEqual(result['physical']['physical'][0], {'name': source['name'], 'path': source['path'],
                                                           'sha256': C.sha256(source['bytes'])})

    def test_new_and_existing_subjects_get_uniform_propose_and_never_change_base(self):
        source = self.source(document={'readings': {'p.input': {'v': 2}, 'p.new': {'v': True},
                                                     'p.scalar': datetime.date(2026, 9, 16)}})
        result = self.prepare({'alternative': {key: value for key, value in source.items() if key != 'name'}})
        self.assertEqual(len(result['objects']), 6)
        self.assertEqual({obj['body']['act'] for obj in result['objects'].values() if obj['kind'] == 'act'}, {'propose'})
        state = H.reduce({**self.objects, **result['objects']})['subjects']
        self.assertEqual(state['p.input']['head'], self.base['id'])
        self.assertEqual(state['p.new']['acceptance'], 'proposed')
        self.assertEqual(state['p.scalar']['acceptance'], 'proposed')
        self.assertEqual(state['p.new']['heads'], [])
        self.assertEqual(result['groups']['alternative']['document'], source['document'])

    def test_local_and_inherited_observations_never_borrow_another_hypothesis(self):
        one = self.source('one', document={'readings': {'p.local': {'v': 3}}, 'judgments': {
            'd.local': {'rests_on': ['p.local', 'p.input', 'p.only_other', 'p.absent'],
                        'verdict': 'possible', 'blocked_on': 'missing records'}}})
        two = self.source('two', document={'readings': {'p.only_other': {'v': 7}, 'p.local': {'v': 9}}})
        result = self.prepare([two, one])
        obj = result['objects'][result['groups']['one']['versions']['d.local']]
        observed = obj['authored']['locator']['import']['observations']
        self.assertEqual(observed['p.local']['kind'], 'hypothesis_entry')
        self.assertEqual(observed['p.local']['name'], 'one')
        self.assertEqual(observed['p.local']['body_digest'], identity({'v': 3}))
        self.assertEqual(observed['p.input']['kind'], 'base_version')
        self.assertEqual(obj['pin_gaps'], {'p.local': 'not_recorded', 'p.input': 'not_recorded',
                                         'p.only_other': 'unavailable', 'p.absent': 'unavailable'})
        self.assertEqual(observed['p.only_other']['kind'], 'unavailable')
        self.assertEqual(result, self.prepare([one, two]))

    def test_mixed_declared_profiles_and_custom_roles_are_preserved_without_promotion(self):
        legacy = self.source('legacy', document={'known': {'p.legacy': {'v': 'p.input + 1'}}})
        core = self.source('core', document={'meta': copy.deepcopy(self.document['meta']),
            'schema': {'deps': 'needs', 'snapshot': 'observed', 'predicate': 'fails_when'},
            'judgments': {'d.core': {'needs': ['p.input'], 'observed': {'p.input': 0},
                                    'fails_when': {'expr': 'p.input > 4'}, 'verdict': 'possible'}}})
        checked = self.source('checked', document={'known': {'p.checked': {'v': 2}}}, profile='checked-reader/v1')
        result = self.prepare([legacy, core, checked])
        self.assertEqual({name: group['profile'] for name, group in result['groups'].items()},
                         {'checked': 'checked-reader/v1', 'core': 'core/v1', 'legacy': 'ordinary-reader/v1'})
        obj = result['objects'][result['groups']['core']['versions']['d.core']]
        self.assertEqual(obj['authored']['fields'], {'deps': 'needs', 'snapshot': 'observed', 'predicate': 'fails_when'})
        self.assertEqual(obj['body']['observed'], {'p.input': 0})
        self.assertEqual(obj['authored']['locator']['document_headers']['meta'], core['document']['meta'])
        self.assertNotIn('meta', result['groups']['legacy']['document'])
        with self.assertRaisesRegex(C.HistoryError, 'hypothesis_profile_promotion_refused'):
            self.prepare([{**legacy, 'profile': 'core/v1'}])

    def test_exact_original_file_headers_and_sparse_collections_are_retained(self):
        document = {'meta': {'updated': datetime.date(2026, 9, 1), 'private_note': 'retained'},
                    'schema': {'deps': 'rests_on', 'snapshot': 'seen', 'predicate': 'wrong_if'},
                    'known': {'p.new': 'literal'}, 'open': {}}
        source = self.source(document=document)
        result = self.prepare([source])
        obj = result['objects'][result['groups']['alternative']['versions']['p.new']]
        self.assertEqual(result['groups']['alternative']['document'], document)
        self.assertEqual(obj['authored']['locator']['document_headers']['meta'], document['meta'])
        self.assertEqual(obj['authored']['locator']['document_headers']['schema'], document['schema'])
        self.assertEqual(obj['body'], 'literal')

    def test_original_source_mismatch_and_unrepresentable_shapes_refuse(self):
        source = self.source()
        cases = [({**source, 'document': {'readings': {'p.input': {'v': 999}}}}, 'hypothesis_source_mismatch'),
                 ({**source, 'head': {'folds': 'yes'}}, 'hypothesis_source_mismatch'),
                 ({**source, 'path': '.kpopper/hypotheses/other.yaml'}, 'invalid_hypothesis_import_path'),
                 ({**source, 'path': '../external.yaml'}, 'invalid_path'),
                 (self.source(document={}), 'empty_hypothesis_requires_group_anchor'),
                 (self.source(document={'judgments': {'p.input': {'rests_on': ['p.input'], 'verdict': 'kind changed'}}}),
                  'hypothesis_kind_conflict'),
                 (self.source(document={'judgments': {'d.map': {'rests_on': {'p.input': self.base['id']}}}}),
                  'unsupported_legacy_hypothesis_dependencies')]
        for item, code in cases:
            with self.subTest(code=code):
                with self.assertRaisesRegex(C.HistoryError, code):
                    self.prepare([item])
        with self.assertRaisesRegex(C.HistoryError, 'duplicate_hypothesis_import'):
            self.prepare([source, source])
        broken = copy.deepcopy(self.document)
        broken['readings']['p.input']['v'] = 999
        with self.assertRaisesRegex(C.HistoryError, 'hypothesis_import_base_mismatch'):
            I.prepare(self.objects, [source], base_document=broken, operation='op', recorded_at='date')

    def test_layout_mapping_and_optional_fields_are_strict(self):
        for entry in ('GROUNDING.yaml', 'PROVENANCE.yaml', 'custom.yml'):
            source = self.source(entry=entry)
            result = self.prepare([source], entry=entry)
            self.assertEqual(I.validate_mapping(result['physical'], entry=entry), result['physical'])
        with self.assertRaisesRegex(C.HistoryError, 'invalid_hypothesis_field_roles'):
            self.prepare([self.source(fields={'deps': 'seen', 'snapshot': 'seen', 'predicate': 'wrong_if'})])
        value = {'version': 1, 'physical': [{'name': 'a', 'path': '.kpopper/hypotheses/a.yaml', 'sha256': '0' * 64,
                                            'ignored': 'must refuse'}]}
        with self.assertRaisesRegex(C.HistoryError, 'invalid_schema'):
            I.validate_mapping(value)


if __name__ == '__main__':
    unittest.main()
