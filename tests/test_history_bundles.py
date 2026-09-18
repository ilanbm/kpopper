"""Portable committed evidence never silently becomes YAML-only knowledge."""
import copy
import datetime
from pathlib import Path
import tempfile
import unittest
from unittest import mock

from scripts import history_bundle as B, history_contract as C, history_store as H
from scripts import pending_grounding as G, project_modes as M, history_transaction as T
from tests.test_history_snapshot_capture import claim, act, TEMPLATE
from tests import test_history_snapshot_capture as fixture

SCOPE = {'kind': 'project', 'environment': 'fixture'}


def reading(subject='p.input', value=1, *, operation='reading', **kw):
    return claim(subject, operation=operation, body={'v': value, 'scope': copy.deepcopy(SCOPE), **kw})


class HistoryBundles(unittest.TestCase):
    publish = fixture.HistorySnapshotCapture.publish

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.entry = self.root / 'GROUNDING.yaml'
        self.store = H.Store(self.entry)
        self.marker = C.authority(record_id='portable-fixture', authority='history', generation=1)
        self.marker_path = Path(self.store.layout['history_authority'])
        self.marker_path.parent.mkdir(parents=True)
        # Deliberately retain a noncanonical comment in the authority bytes.
        self.marker_path.write_bytes(b'# original marker\n' + C.encode_document(self.marker))
        doc = copy.deepcopy(TEMPLATE)
        doc['meta']['history'] = H.baseline(self.marker, {}, H.reduce({}))
        self.entry.write_bytes(C.encode_document(doc))
        self.first = reading(value=datetime.date(2026, 9, 17))
        self.publish([self.first], 'initial')

    def artifact(self, roots=None):
        capture = self.store.capture()
        return B.export(capture, roots=roots or ['p.input'], scope=SCOPE,
                        shareability='project')

    def bundle(self, artifact=None, evidence=None):
        artifact = artifact or self.artifact()
        return G.prepare(B.adapt(artifact).document, artifact['manifest']['roots'], scope=SCOPE,
                         shareability='project', history=artifact, evidence=evidence)

    def rehash(self, artifact):
        artifact['manifest']['files'] = {p: C.sha256(raw) for p, raw in artifact['files'].items()}
        artifact['revision'] = G.identity(artifact['manifest'])
        return artifact

    def test_source_free_replay_retains_exact_bytes_types_and_profile(self):
        artifact = self.artifact()
        expected = B.adapt(artifact).snapshot(as_of='2026-09-17').snapshot_id
        self.temp.cleanup()
        with mock.patch.object(Path, 'read_bytes', side_effect=AssertionError('live lookup')):
            captured = B.validate(artifact)
            replay = B.adapt(artifact)
            self.assertEqual(replay.snapshot(as_of='2026-09-17').snapshot_id, expected)
        self.assertEqual(captured.objects[self.first['id']], self.first)
        self.assertIsInstance(replay.document['readings']['p.input']['v'], datetime.date)
        self.assertEqual(replay.document['meta']['reasoning'], TEMPLATE['meta']['reasoning'])
        self.assertTrue(artifact['files']['authority.yaml'].startswith(b'# original marker'))

    def test_missing_changed_extra_and_rehashed_missing_objects_refuse(self):
        original = self.artifact()
        path = next(p for p in original['files'] if p.startswith('objects/'))
        for mutation in ('missing', 'changed', 'extra', 'rehashed-missing'):
            artifact = copy.deepcopy(original)
            if mutation in ('missing', 'rehashed-missing'):
                del artifact['files'][path]
            elif mutation == 'changed':
                artifact['files'][path] += b'\n'
            else:
                artifact['files']['unrelated.yaml'] = b'private: true\n'
            if mutation == 'rehashed-missing':
                self.rehash(artifact)
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                B.validate(artifact)

    def test_manifest_parent_closure_and_operation_membership_refuse(self):
        second = reading(operation='second', value=2)
        self.publish([second], 'second')
        artifact = self.artifact()
        del artifact['files']['commits/initial.yaml']
        self.rehash(artifact)
        with self.assertRaisesRegex(ValueError, 'incomplete_commit'):
            B.validate(artifact)

    def test_no_unrelated_subject_or_private_history_leak(self):
        other = reading('p.other', operation='other')
        self.publish([other], 'other')
        with self.assertRaisesRegex(ValueError, 'full_authorization'):
            self.artifact()
        self.assertEqual(set(B.validate(self.artifact(['p.input', 'p.other'])).state['subjects']),
                         {'p.input', 'p.other'})
        private = reading('p.secret', operation='private', private=True)
        self.publish([private], 'private')
        with self.assertRaisesRegex(ValueError, 'private'):
            self.artifact(['p.input', 'p.other', 'p.secret'])

    def test_orphan_staging_is_not_exported(self):
        orphan = reading('private.orphan', operation='orphan', private=True)
        path = Path(self.store.layout['history']) / orphan['subject'] / (orphan['id'] + '.yaml')
        path.parent.mkdir(parents=True)
        path.write_bytes(C.encode_document(orphan))
        artifact = self.artifact()
        self.assertFalse(any('private.orphan' in p for p in artifact['files']))

    def test_historical_scope_is_preserved_and_not_relabelled(self):
        old = reading('p.other', operation='old')
        old['body']['scope'] = {'kind': 'code', 'environment': 'previous', 'commit': 'a' * 40}
        old['id'] = C.object_identity(old)
        replacement = reading('p.other', operation='replacement')
        correction = act(replacement, 'correct', operation='correct', over=[old['id']])
        self.publish([old, replacement, correction], 'replace')
        captured = B.validate(self.artifact(['p.input', 'p.other']))
        self.assertEqual(captured.objects[old['id']]['body']['scope']['commit'], 'a' * 40)

    def test_old_formats_refuse_history_and_v3_binds_full_closure(self):
        artifact = self.artifact()
        doc = B.adapt(artifact).document
        with self.assertRaisesRegex(ValueError, 'history-aware contribution'):
            G.prepare(doc, ['p.input'], scope=SCOPE, shareability='project')
        bundle = self.bundle(artifact)
        self.assertEqual(bundle['manifest']['version'], 3)
        self.assertEqual(G.validate_bundle(bundle)['profile'], 'core/v1')
        changed = copy.deepcopy(bundle)
        changed['manifest']['history']['revision'] = '0' * 64
        changed['revision'] = G.identity(changed['manifest'])
        with self.assertRaises(ValueError):
            G.validate_bundle(changed, supported=False)
        self.assertFalse(G.equivalent(bundle, doc, {}))
        self.assertTrue(G.equivalent(bundle, doc, {}, history=artifact))

    def test_newer_retained_history_is_equivalent_without_erasing_new_claims(self):
        artifact = self.artifact()
        bundle = self.bundle(artifact)
        newer = reading(value=7, operation='newer')
        correction = act(newer, 'correct', operation='correct', over=[self.first['id']])
        self.publish([newer, correction], 'newer')
        target = self.artifact()
        self.assertTrue(G.equivalent(bundle, B.adapt(target).document, {}, history=target))
        self.assertEqual(B.adapt(target).document['readings']['p.input']['v'], 7)
        self.assertFalse(G.equivalent(bundle, {'readings': {}}, {}, history=target))

    def test_historical_referenced_files_are_required(self):
        newer = reading(value=2, operation='newer', file='evidence/original.txt')
        self.publish([newer], 'newer')
        artifact = self.artifact()
        with self.assertRaisesRegex(ValueError, 'all historical referenced files'):
            self.bundle(artifact)
        self.assertEqual(G.validate_bundle(self.bundle(artifact, {'evidence/original.txt': b'exact evidence'}))['profile'],
                         'core/v1')

    def test_known_stale_entry_is_retained_but_never_overrides_new_history(self):
        original_entry = self.entry.read_bytes()
        newer = reading(value=9, operation='newer')
        correction = act(newer, 'correct', operation='correct', over=[self.first['id']])
        self.publish([newer, correction], 'newer')
        self.entry.write_bytes(original_entry)
        artifact = self.artifact()
        self.assertEqual(artifact['files']['entry.yaml'], original_entry)
        self.assertEqual(B.adapt(artifact).document['readings']['p.input']['v'], 9)
        self.assertEqual(self.bundle(artifact)['manifest']['document']['readings']['p.input']['v'], 9)
        edited = C.decode_document(original_entry)
        edited['readings']['p.input']['v'] = 'unrecorded edit'
        self.entry.write_bytes(C.encode_document(edited))
        with self.assertRaisesRegex(ValueError, 'unresolved_view_edit'):
            self.artifact()

    def test_retired_private_body_still_refuses_export(self):
        secret = reading('p.secret', operation='secret', private=True)
        public = reading('p.secret', operation='public')
        correction = act(public, 'correct', operation='correct', over=[secret['id']])
        self.publish([secret, public, correction], 'private-replacement')
        self.assertNotIn('private', self.store.capture().document['readings']['p.secret'])
        with self.assertRaisesRegex(ValueError, 'private'):
            self.artifact(['p.input', 'p.secret'])

    def test_prototype_and_typed_identifiers_survive_without_rewriting(self):
        from scripts import versions as V
        old = act(self.first, 'review', operation='prototype-review', read={})
        del old['schema_version']
        del old['id_scheme']
        old['id'] = V.ident(old)
        self.publish([old], 'import')
        artifact = self.artifact()
        restored = B.validate(artifact)
        self.assertEqual(len(old['id']), 40)
        self.assertEqual(restored.objects[old['id']], old)
        self.assertEqual(restored.objects[self.first['id']], self.first)
        self.assertEqual(B.adapt(artifact).projection['identity_schemes'],
                         ['prototype/v1', 'typed-history/v2'])

    def test_unknown_transport_capability_and_size_limit_refuse(self):
        artifact = self.artifact()
        artifact['manifest']['requires'] = ['future-history/v2']
        artifact['revision'] = G.identity(artifact['manifest'])
        with self.assertRaisesRegex(ValueError, 'unsupported_history_bundle'):
            B.validate(artifact)
        artifact = self.artifact()
        with mock.patch.object(B, 'MAX_BYTES', 1), self.assertRaisesRegex(ValueError, 'history_limit'):
            B.validate(artifact)

    def test_rehashed_orphan_object_and_changed_bound_template_refuse(self):
        artifact = self.artifact()
        orphan = reading('orphan.subject', operation='orphan')
        artifact['files']['objects/orphan.subject/' + orphan['id'] + '.yaml'] = C.encode_document(orphan)
        self.rehash(artifact)
        with self.assertRaisesRegex(ValueError, 'history_bundle_membership'):
            B.validate(artifact)
        artifact = self.artifact()
        commit = C.decode_document(artifact['files']['commits/initial.yaml'])
        commit['view_template']['meta']['reasoning']['requires'] = []
        artifact['files']['commits/initial.yaml'] = C.encode_document(commit)
        self.rehash(artifact)
        with self.assertRaises(ValueError):
            B.validate(artifact)

    def test_pending_ledger_retains_replayable_bytes(self):
        M.git(self.root, 'init', '-b', 'trunk')
        M.git(self.root, 'config', 'user.name', 'Test')
        M.git(self.root, 'config', 'user.email', 'test@example.test')
        bundle = self.bundle()
        store = G.Store(self.root)
        receipt = store._capture(bundle, event_id='history-event', contribution_id='history', shareability='project')
        restored = store.read_bundle(bundle['revision'])
        self.assertEqual(restored, bundle)
        self.assertEqual(B.adapt(B.from_contribution(restored)).document, bundle['manifest']['document'])
        self.assertEqual(receipt['revision'], bundle['revision'])

    def test_v1_v2_identity_and_semantics_are_unchanged(self):
        document = {'known': {'api.limit': {'v': 3, 'scope': SCOPE}}}
        for version in (1, 2):
            bundle = G._prepare(document, ['api.limit'], scope=SCOPE, shareability='project', version=version)
            manifest = {'version': version, 'roots': ['api.limit'], 'document': document,
                        'scope': SCOPE, 'evidence': {}}
            if version == 2:
                manifest['reasoning'] = {'version': 1, 'profile': 'ordinary-reader/v1', 'requires': []}
            self.assertEqual(bundle['revision'], G.identity(manifest))
            self.assertTrue(G.equivalent(bundle, document, {}))
            G.validate_bundle(bundle)
