"""Immutable contribution meaning is independent of the receiving record."""
import copy
import unittest
from unittest.mock import patch

from scripts import pending_grounding as G
from scripts.reasoning.contract import CapabilityError
from tests.test_pending_grounding import Repository


SCOPE = {'kind': 'project', 'environment': 'all'}
CORE = {'version': 2, 'profile': 'core/v1', 'requires': ['arithmetic/v1']}
LEGACY = {'version': 1, 'profile': 'ordinary-reader/v1', 'requires': []}


def document(core=True):
    doc = {'known': {'a.input': {'v': 3}, 'a.result': {
        'rule': {'expr': 'a.input + 1'}, 'scope': SCOPE}}}
    if core:
        doc['meta'] = {'reasoning': copy.deepcopy(CORE), 'name': 'Unrelated metadata'}
    return doc


def prepare(doc=None):
    return G.prepare(document() if doc is None else doc, ['a.result'],
                     scope=SCOPE, shareability='project')


def reidentify(bundle):
    bundle['revision'] = G.identity(bundle['manifest'])
    return bundle


class CapabilityTests(unittest.TestCase):
    def test_v2_binds_declaration_and_preserves_only_applicable_metadata(self):
        bundle = prepare()
        self.assertEqual(bundle['manifest']['version'], 2)
        self.assertEqual(bundle['manifest']['reasoning'], CORE)
        self.assertEqual(bundle['manifest']['document']['meta'], {'reasoning': CORE})
        self.assertEqual(G.validate_bundle(bundle), CORE)

    def test_legacy_identity_is_explicit_only_in_manifest(self):
        bundle = prepare(document(False))
        self.assertEqual(bundle['manifest']['reasoning'], LEGACY)
        self.assertNotIn('meta', bundle['manifest']['document'])
        self.assertFalse(G.equivalent(bundle, document(), {}))

    def test_supported_record_format_versions_have_same_meaning(self):
        bundle = prepare()
        target = document()
        target['meta']['reasoning']['version'] = 1
        self.assertEqual(G.document_capabilities(target)['version'], 1)
        self.assertTrue(G.equivalent(bundle, target, {}))
        self.assertEqual(G.meaning_capabilities(target),
                         {'profile': 'core/v1', 'requires': ['arithmetic/v1']})

    def test_actual_operator_cannot_hide_behind_supported_declaration(self):
        doc = document()
        doc['known']['a.result']['rule'] = {'op': 'future_map', 'args': []}
        with self.assertRaises(CapabilityError) as error:
            prepare(doc)
        self.assertEqual(error.exception.code, 'unsupported_capability')

    def test_actual_history_cannot_hide_behind_supported_declaration(self):
        for version in (2, 3):
            doc = document()
            doc['meta']['reasoning']['version'] = 1
            doc['judgments'] = {'j.test': {'rests_on': ['a.input'],
                'seen': {'a.input': {'computed': {'version': version, 'value': None}}},
                'wrong_if': {'expr': 'a.input < 0'}}}
            with self.subTest(version=version), self.assertRaises(CapabilityError):
                G.document_capabilities(doc)

    def test_historical_basis_cannot_hide_future_modules(self):
        from scripts.reasoning.contract import digest
        doc = document()
        basis = {'version': 1, 'profile': 'core/v1', 'modules': ['future/v1']}
        basis['digest'] = digest(basis)
        doc['judgments'] = {'j.test': {'rests_on': ['a.input'],
            'seen': {'a.input': {'computed': {'version': 2,
                'value': {'type': 'null'}, 'basis': basis}}},
            'wrong_if': {'expr': 'a.input < 0'}}}
        with self.assertRaises(CapabilityError) as error:
            G.document_capabilities(doc)
        self.assertEqual(error.exception.code, 'unsupported_capability')

    def test_scope_evidence_must_be_complete_and_explicit(self):
        doc = document()
        doc['known']['a.result'] = {'rests_on': ['scope.items'], 'scope': SCOPE}
        doc['scopes'] = {'scope.items': {'collection_scope': {'collection': 'items', 'fields': ['v']}}}
        doc['items'] = {'item.one': {'v': 1, 'file': 'evidence/member.txt'}}
        with self.assertRaisesRegex(ValueError, 'evidence allowlist'):
            prepare(doc)
        bundle = G.prepare(doc, ['a.result'], scope=SCOPE, shareability='project',
                           evidence={'evidence/member.txt': b'proof'})
        self.assertEqual(bundle['files'], {'evidence/member.txt': b'proof'})

    def test_writer_scope_history_is_supported_and_definition_bound(self):
        from scripts.reasoning.authoring import World
        from scripts import provenance as P
        from tests.test_reasoning_history import document as history_document
        doc = history_document()
        history = World(P, doc).history('scope.items', {}, None)
        doc['judgments']['j']['reviewed'] = {'scope.items': history}
        self.assertEqual(G.document_capabilities(doc), CORE)
        history['computed']['rule']['collection'] = 'other'
        with self.assertRaises(CapabilityError):
            G.document_capabilities(doc)

    def test_versionless_core_history_still_validates_historical_operators(self):
        doc = document()
        doc['judgments'] = {'j.test': {'rests_on': ['a.input'],
            'seen': {'a.input': {'computed': {'value': 3,
                'rule': {'op': 'future/v1', 'args': []}}}},
            'wrong_if': {'expr': 'a.input < 0'}}}
        with self.assertRaises(CapabilityError) as error:
            G.document_capabilities(doc)
        self.assertEqual(error.exception.code, 'unsupported_capability')

    def test_legacy_scope_metadata_does_not_acquire_core_membership_semantics(self):
        doc = document(False)
        doc['schema'] = {'deps': 'rests_on', 'snapshot': 'seen', 'predicate': 'wrong_if'}
        doc['known']['a.result'] = {'scope': SCOPE,
            'collection_scope': {'collection': 'items', 'fields': ['v']}}
        doc['items'] = {'item.one': {'v': 1}}
        bundle = prepare(doc)
        self.assertTrue(G.equivalent(bundle, doc, {}))
        bundle['manifest']['version'] = 1
        del bundle['manifest']['reasoning']
        reidentify(bundle)
        self.assertTrue(G.equivalent(bundle, doc, {}))

    def test_supported_v2_requires_complete_evidence_and_dependency_closure(self):
        for damage in ('evidence', 'dependency', 'scope'):
            bundle = prepare()
            stored = bundle['manifest']['document']
            if damage == 'evidence':
                stored['known']['a.result']['file'] = 'missing.txt'
            elif damage == 'dependency':
                del stored['known']['a.input']
            else:
                stored['known']['a.result']['scope']['environment'] = 'elsewhere'
            reidentify(bundle)
            # Raw evidence stays available, without claiming interpretability.
            G.validate_bundle(bundle, supported=False)
            with self.subTest(damage=damage), self.assertRaises(ValueError):
                G.validate_bundle(bundle)

    def test_checks_do_not_execute(self):
        with patch('scripts.reasoning.evaluate.Evaluator.evaluate', side_effect=AssertionError('execution')):
            self.assertEqual(G.document_capabilities(document()), CORE)
            G.validate_bundle(prepare())

    def test_unknown_capability_raw_validation_requires_manifest_agreement(self):
        bundle = prepare()
        future = {'version': 9, 'profile': 'future/v1', 'requires': ['future/v1']}
        bundle['manifest']['reasoning'] = copy.deepcopy(future)
        bundle['manifest']['document']['meta']['reasoning'] = copy.deepcopy(future)
        reidentify(bundle)
        self.assertEqual(G.validate_bundle(bundle, supported=False), future)
        with self.assertRaises(CapabilityError):
            G.validate_bundle(bundle)
        bundle['manifest']['reasoning']['requires'] = ['other/v1']
        reidentify(bundle)
        with self.assertRaisesRegex(ValueError, 'agreement|disagree'):
            G.validate_bundle(bundle, supported=False)

    def test_scope_captures_complete_membership_and_projected_rule_inputs(self):
        doc = document()
        doc['known']['a.result'] = {'rests_on': ['scope.items'], 'scope': SCOPE}
        doc['scopes'] = {'scope.items': {'collection_scope': {'collection': 'items', 'fields': ['v']}}}
        doc['items'] = {'item.one': {'rule': {'expr': 'a.input + 1'}}, 'item.two': {'v': 8}}
        bundled = prepare(doc)['manifest']['document']
        self.assertEqual(bundled['items'], doc['items'])
        self.assertIn('a.input', bundled['known'])
        doc['items']['item.two']['private'] = True
        with self.assertRaisesRegex(ValueError, 'private'):
            prepare(doc)

    def test_empty_scope_membership_is_retained_and_affects_equivalence(self):
        doc = document()
        doc['known']['a.result'] = {'rests_on': ['scope.items'], 'scope': SCOPE}
        doc['scopes'] = {'scope.items': {'collection_scope': {'collection': 'items', 'fields': []}}}
        doc['items'] = {}
        bundle = prepare(doc)
        self.assertEqual(bundle['manifest']['document']['items'], {})
        doc['items']['item.new'] = {'v': 1}
        self.assertFalse(G.equivalent(bundle, doc, {}))


class StoreCapabilityTests(Repository):
    def archive(self, bundle):
        prefix = 'contributions/' + bundle['revision'] + '/'
        tree = {prefix + 'manifest.json': self.store._write_blob(G.json_bytes(G._encode(bundle['manifest'])))}
        tree.update({prefix + 'evidence/' + name: self.store._write_blob(data)
                     for name, data in bundle['files'].items()})
        old = self.store.head()
        new = self.store._commit(tree, old)
        self.assertTrue(self.store._cas(old, new))

    def test_unknown_capability_can_be_archived_but_not_newly_captured(self):
        bundle = prepare()
        bundle['manifest']['reasoning']['requires'].append('future/v1')
        bundle['manifest']['document']['meta']['reasoning']['requires'].append('future/v1')
        reidentify(bundle)
        self.archive(bundle)
        self.assertEqual(self.store.read_bundle(bundle['revision']), bundle)
        with self.assertRaises(CapabilityError):
            self.capture(bundle)

    def test_raw_read_refuses_manifest_document_disagreement(self):
        bundle = prepare()
        bundle['manifest']['reasoning']['requires'].append('future/v1')
        reidentify(bundle)
        self.archive(bundle)
        with self.assertRaisesRegex(ValueError, 'agreement|disagree'):
            self.store.read_bundle(bundle['revision'])

    def test_v1_read_never_reextracts_and_compatible_replay_preserves_identity(self):
        bundle = prepare(document(False))
        bundle['manifest']['version'] = 1
        bundle['manifest'].pop('reasoning', None)
        reidentify(bundle)
        receipt = self.capture(bundle)
        with patch.object(G, 'closure', side_effect=AssertionError('new extraction')):
            self.assertEqual(self.store.read_bundle(bundle['revision']), bundle)
            replay = self.capture(bundle)
        self.assertTrue(replay['replay'])
        self.assertEqual(replay['commit'], receipt['commit'])
