"""Capability meaning survives contribution routing, overlay and publication."""
import contextlib
import copy
import io
import json
from pathlib import Path
from unittest.mock import patch

from scripts import knowledge_views as V, pending_grounding as G, pending_publication as C
from scripts import provenance as P, project_modes as M
from scripts.reasoning import authoring as A
from scripts.reasoning.snapshot import Snapshot
from tests.test_pending_grounding import Repository, fixture_bundle
from tests import test_pending_publication as PF


CORE = {'version': 2, 'profile': 'core/v1', 'requires': ['arithmetic/v1']}
SCOPE = {'kind': 'external', 'environment': 'API v2'}


def bundle_for(document, root='p.result'):
    document = copy.deepcopy(document)
    G.entries(document)[root][1]['scope'] = copy.deepcopy(SCOPE)
    return G.prepare(document, [root], scope=SCOPE, shareability='project')


def retain_raw(store, bundle):
    """Import an already immutable archive without interpreting it."""
    revision = bundle['revision']
    head = store.head()
    tree = store.tree(head)
    prefix = 'contributions/' + revision + '/'
    tree[prefix + 'manifest.json'] = store._write_blob(G.json_bytes(G._encode(bundle['manifest'])))
    for path, data in bundle['files'].items():
        tree[prefix + 'evidence/' + path] = store._write_blob(data)
    tree['events/archive.json'] = store._write_blob(G.json_bytes(dict(
        event_id='archive', contribution_id='archive', revision=revision, sequence=1)))
    new = store._commit(tree, head)
    assert store._cas(head, new)


class ContributionRoutes(Repository):
    def setUp(self):
        super().setUp()
        trigger = patch.object(C, 'trigger_after_capture', return_value={'started': False})
        trigger.start()
        self.addCleanup(trigger.stop)
        self.path = self.root / 'GROUNDING.yaml'
        self.doc = {'meta': {'reasoning': copy.deepcopy(CORE)},
                    'known': {'p.input': {'v': 10, 'from': 'measured'}}}
        self.save()

    def save(self):
        self.path.write_text(P.yaml.safe_dump(self.doc, sort_keys=False))

    def apply(self, action):
        with contextlib.redirect_stdout(io.StringIO()) as output:
            P.apply([str(self.path)], action)
        return output.getvalue()

    def routed(self, kind='add', nid='p.result', **kwargs):
        return dict(kind=kind, id=nid, shareability='project', scope='external',
                    environment='API v2', event_id='event-' + nid, **kwargs)

    def test_core_route_normalizes_and_materializes_without_legacy_evaluation(self):
        before = self.path.read_bytes()
        with patch.object(P.E, 'compute', side_effect=AssertionError('legacy computation')):
            receipt = json.loads(self.apply(self.routed(body={'rule': 'p.input / 3'})))
        bundle = self.store.read_bundle(receipt['revision'])
        self.assertEqual(bundle['manifest']['version'], 2)
        self.assertEqual(bundle['manifest']['reasoning'], CORE)
        self.assertEqual(bundle['manifest']['document']['known']['p.result']['rule'], {'expr': 'p.input / 3'})
        self.assertEqual(self.path.read_bytes(), before)
        result = V.materialize(self.root, receipt['revision'], self.base / 'export')
        exported = P.yaml.safe_load(Path(result['record']).read_text())
        self.assertEqual(exported['meta']['reasoning'], CORE)
        from scripts.reasoning.evaluate import Evaluator
        value = Evaluator(Snapshot.from_data(exported)).evaluate({'ref': 'p.result'}, declared=['p.result'])
        self.assertEqual(value['value'], {'type': 'number', 'numerator': '10', 'denominator': '3'})

    def test_routed_judgment_keeps_typed_basis_and_set_uses_authored_value(self):
        receipt = json.loads(self.apply(self.routed(nid='d.stable', body={
            'rests_on': ['p.input'], 'verdict': 'Stable', 'wrong_if': 'p.input < 0'})))
        body = G.entries(self.store.read_bundle(receipt['revision'])['manifest']['document'])['d.stable'][1]
        self.assertEqual(body['wrong_if'], {'expr': 'p.input < 0'})
        self.assertEqual(body['seen']['p.input']['computed']['version'], 2)
        self.assertIn('basis', body['seen']['p.input']['computed'])
        receipt = json.loads(self.apply(self.routed(kind='set', nid='p.input', value=2.5)))
        body = G.entries(self.store.read_bundle(receipt['revision'])['manifest']['document'])['p.input'][1]
        self.assertEqual(body['v'], 2.5)

    def test_supported_metadata_versions_have_same_overlay_meaning(self):
        self.doc['known']['p.input']['scope'] = copy.deepcopy(SCOPE)
        self.save()
        old = copy.deepcopy(self.doc)
        old['meta']['reasoning']['version'] = 1
        self.capture(bundle_for(old, 'p.input'))
        live = A.load(P, [str(self.path)], read_mode='live')
        self.assertFalse(live.knowledge_conflicts)

    def test_retired_bundle_does_not_block_promotion_and_cannot_resume(self):
        self.doc.pop('meta')
        self.save()
        receipt = self.capture()
        with self.assertRaisesRegex(P.Refused, 'pending_profile_reconciliation_required'):
            self.apply(dict(kind='add', id='p.result', profile='core/v1', body={'rule': 'p.input / 3'}))
        publisher = C.Publisher(self.root)
        publisher.action('withdraw', revisions=[receipt['revision']], reason='Explicitly retire the legacy overlay')
        ledger = self.store.head()
        self.apply(dict(kind='add', id='p.result', profile='core/v1', body={'rule': 'p.input / 3'}))
        state = publisher.path.read_bytes()
        with self.assertRaisesRegex(ValueError, 'pending_profile_reconciliation_required'):
            publisher.action('resume', revisions=[receipt['revision']])
        self.assertEqual(state, publisher.path.read_bytes())
        self.assertEqual(ledger, self.store.head())
        self.assertEqual(publisher.verify_obligations()['terminal'][receipt['revision']], 'withdrawn')

    def test_unknown_bundle_is_raw_evidence_but_not_materializable(self):
        unknown = {'version': 2, 'roots': ['p.result'], 'scope': SCOPE, 'evidence': {},
                   'reasoning': {**CORE, 'requires': ['arithmetic/v1', 'future/v1']},
                   'document': {'meta': {'reasoning': {**CORE, 'requires': ['arithmetic/v1', 'future/v1']}},
                                'known': {'p.result': {'v': 1, 'scope': SCOPE}}}}
        bundle = dict(manifest=unknown, revision=G.identity(unknown), files={})
        retain_raw(self.store, bundle)
        self.assertEqual(self.store.read_bundle(bundle['revision']), bundle)
        destination = self.base / 'unsupported'
        with self.assertRaisesRegex(ValueError, 'unsupported'):
            V.materialize(self.root, bundle['revision'], destination)
        self.assertFalse(destination.exists())
        with self.assertRaisesRegex(P.Refused, 'unsupported_capability'):
            A.load(P, [str(self.path)], read_mode='live')


class ContributionPublication(PF.PublicationTests):
    # Reuse the real local Git/provider fixture without duplicating its test suite.
    def core(self):
        return bundle_for({'meta': {'reasoning': copy.deepcopy(CORE)},
                           'known': {'p.result': {'rule': {'expr': '10 / 3'}}}})

    def capture_bundle(self, bundle):
        self.store.capture(bundle, event_id='core', contribution_id='core', shareability='project')
        return bundle['revision']

    def target_document(self, document):
        (self.root / 'GROUNDING.yaml').write_text(P.yaml.safe_dump(document, sort_keys=False))
        M.git(self.root, 'add', 'GROUNDING.yaml')
        M.git(self.root, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Record declaration')
        M.git(self.root, 'push', 'team', 'trunk')

    def test_core_publication_retains_declaration_and_acceptance(self):
        revision = self.capture_bundle(self.core())
        self.run_ok()
        doc = P.yaml.safe_load(M.git(self.remote, 'show', 'pending_grounding:GROUNDING.yaml').stdout)
        self.assertEqual(doc['meta']['reasoning'], CORE)
        self.merge()
        self.assertEqual(self.publisher.verify_obligations()['terminal'][revision], 'accepted')

    def test_legacy_executable_target_cannot_be_implicitly_promoted(self):
        self.target_document({'known': {'p.old': {'rule': {'expr': '1 + 2'}}}})
        self.capture_bundle(self.core())
        before = self.remote_head('trunk')
        result = self.publisher.run(force_retry=True)
        self.assertEqual(result['outcome'], 'attention', result)
        self.assertIn('migration', result.get('detail', ''))
        self.assertEqual(self.remote_head('trunk'), before)
        self.assertIsNone(self.remote_head())

    def test_unknown_target_refuses_publication_even_with_legacy_bundle(self):
        self.target_document({'meta': {'reasoning': {**CORE, 'requires': ['arithmetic/v1', 'future/v1']}}})
        self.capture()
        result = self.publisher.run(force_retry=True)
        self.assertEqual(result['outcome'], 'attention', result)
        self.assertIsNone(self.remote_head())

    def test_empty_named_scope_survives_publication(self):
        doc = {'meta': {'reasoning': copy.deepcopy(CORE)}, 'items': {},
               'known': {'scope.items': {'collection_scope': {'collection': 'items', 'fields': []}}}}
        revision = self.capture_bundle(bundle_for(doc, 'scope.items'))
        self.run_ok()
        published = P.yaml.safe_load(M.git(self.remote, 'show', 'pending_grounding:GROUNDING.yaml').stdout)
        self.assertEqual(published['items'], {})
        self.merge()
        self.assertEqual(self.publisher.verify_obligations()['terminal'][revision], 'accepted')

    def test_unknown_retired_evidence_does_not_block_independent_publication(self):
        bundle = self.core()
        unknown = copy.deepcopy(bundle)
        future = {**CORE, 'requires': ['arithmetic/v1', 'future/v1']}
        unknown['manifest']['reasoning'] = future
        unknown['manifest']['document']['meta']['reasoning'] = copy.deepcopy(future)
        unknown['revision'] = G.identity(unknown['manifest'])
        retain_raw(self.store, unknown)
        self.publisher.action('withdraw', revisions=[unknown['revision']], reason='Retain unsupported evidence only')
        revision = self.capture_bundle(bundle)
        self.run_ok()
        self.merge()
        proof = self.publisher.verify_obligations()
        self.assertEqual(proof['terminal'][unknown['revision']], 'withdrawn')
        self.assertEqual(proof['terminal'][revision], 'accepted')
        self.assertEqual(self.store.read_bundle(unknown['revision']), unknown)

    def test_target_hypothesis_declaration_and_promotion_are_validated(self):
        hypothesis = self.root / '.kpopper/hypotheses/alternative.yaml'
        hypothesis.parent.mkdir(parents=True)
        for document, detail in [
                ({'known': {'p.old': {'rule': {'expr': '1 + 2'}}}}, 'migration'),
                ({'meta': {'reasoning': {**CORE, 'version': 99}}}, 'unsupported')]:
            with self.subTest(document=document):
                hypothesis.write_text(P.yaml.safe_dump(document))
                M.git(self.root, 'add', '.')
                M.git(self.root, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Target hypothesis')
                M.git(self.root, 'push', 'team', 'trunk')
                self.capture_bundle(self.core())
                result = self.publisher.run(force_retry=True)
                self.assertEqual(result['outcome'], 'attention', result)
                self.assertIn(detail, result.get('detail', ''))
                self.assertIsNone(self.remote_head())

    def test_accepted_legacy_withdrawal_allows_promotion_and_offline_proof(self):
        revision = self.capture()
        original = self.store.read_bundle(revision)
        self.run_ok()
        self.merge()
        self.assertEqual(self.publisher.verify_obligations()['terminal'][revision], 'accepted')
        target = P.yaml.safe_load(M.git(self.remote, 'show', 'trunk:GROUNDING.yaml').stdout)
        (self.root / 'GROUNDING.yaml').write_text(P.yaml.safe_dump(target))
        paths = [str(self.root / 'GROUNDING.yaml')]
        with self.assertRaisesRegex(P.Refused, 'pending_profile_reconciliation_required'):
            A.prepare(P, paths, dict(kind='add', id='p.new', profile='core/v1'))
        receipts = copy.deepcopy(self.publisher._load()['receipts'])
        self.publisher.action('withdraw', revisions=[revision], reason='Retire legacy overlay after acceptance')
        with contextlib.redirect_stdout(io.StringIO()):
            P.apply(paths, dict(kind='add', id='p.new', profile='core/v1', body={'v': 1, 'from': 'measured'}))
        promoted = P.yaml.safe_load((self.root / 'GROUNDING.yaml').read_text())
        self.assertFalse(G.equivalent(original, promoted, original['files']))
        self.provider.offline = True
        proof = self.publisher.verify_obligations()
        self.assertEqual(proof['terminal'][revision], 'withdrawn')
        self.assertEqual(original, self.store.read_bundle(revision))
        self.assertEqual(receipts, self.publisher._load()['receipts'][:len(receipts)])


# Inherited fixture methods are useful, inherited tests belong to their own module.
for _name in tuple(name for name in dir(PF.PublicationTests) if name.startswith('test_')):
    setattr(ContributionPublication, _name, None)
