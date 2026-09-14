"""A private proposal cannot be laundered through folding or a negative finding."""
import contextlib
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from scripts import consolidate as C, pending_grounding as G, project_modes as M
P = C.P
SECRET = 'SECRET-PRIVATE-PROPOSAL'


class ConsolidationPrivacy(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name).resolve()
        self.root = self.base / 'repo'
        self.root.mkdir()
        M.git(self.root, 'init', '-b', 'trunk')
        M.git(self.root, 'config', 'user.name', 'Test')
        M.git(self.root, 'config', 'user.email', 'test@example.test')
        self.record = self.root / 'GROUNDING.yaml'
        self.doc = {'sources': {'s.session': {'name': 'Public session', 'asked': 'Evaluate proposal', 'read': '2026-09-14'}},
                    'known': {'base.fact': {'v': 1, 'from': 's.session'}}}
        self.save()
        M.git(self.root, 'add', 'GROUNDING.yaml')
        M.git(self.root, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Public base')
        self.env = patch.dict(os.environ, {'KPOPPER_PRIVATE_HOME': str(self.base / 'private')})
        self.env.start()
        self.addCleanup(self.env.stop)

    def save(self):
        self.record.write_text(P.yaml.safe_dump(self.doc))

    def hypothesis(self, head=None, body=None):
        path = Path(P.hypothesis_path([str(self.record)], 'secret'))
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(P.yaml.safe_dump({'hypothesis': head or {'claim': 'Public proposal'},
            'known': {'new.fact': body or {'v': 42, 'from': 's.session'}}}))
        return path

    def attempt(self, operation):
        output = io.StringIO()
        with contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
            try:
                operation()
            except P.Refused as error:
                output.write(str(error))
                return True, output.getvalue()
        return False, output.getvalue()

    def assert_private_retained(self, action, path):
        before, hypothesis = self.record.read_bytes(), path.read_bytes()
        refused, output = self.attempt(action)
        self.assertTrue(refused, output)
        self.assertNotIn(SECRET, output)
        self.assertEqual(before, self.record.read_bytes())
        self.assertEqual(hypothesis, path.read_bytes())
        drafts = list((self.base / 'private').glob('*/*.json'))
        self.assertTrue(drafts)
        decoded = G._decode(json.loads(drafts[0].read_bytes()))
        self.assertIn('hypothesis', decoded['action'])
        return decoded

    def test_private_refutation_cannot_become_publishable_negative_fact(self):
        path = self.hypothesis({'claim': SECRET, 'privacy': 'private'},
                               {'v': 42, 'privacy': 'private'})
        before = self.record.read_bytes()
        refused, output = self.attempt(lambda: C.refute([str(self.record)], 'secret', 'does not hold'))
        # Exercise the second leg of the historical exploit, rather than only checking
        # the refusal text: a created unmarked negative fact could then enter Git.
        if 'hyp.secret' in P.bodies(P.load([str(self.record)], read_mode='frozen')):
            self.attempt(lambda: P.apply([str(self.record)], {'kind': 'set', 'id': 'hyp.secret',
                'value': 'refuted', 'scope': 'project', 'environment': 'project', 'shareability': 'project'}))
        blobs = M.git(self.root, 'cat-file', '--batch-all-objects', '--batch').stdout
        self.assertNotIn(SECRET.encode(), blobs)
        self.assertTrue(refused, output)
        self.assertNotIn(SECRET, output)
        self.assertEqual(before, self.record.read_bytes())
        self.assertTrue(path.exists())
        self.assertIsNone(G.Store(self.root).head())

    def test_head_only_private_refutation_retains_original(self):
        path = self.hypothesis({'claim': SECRET, 'private': True})
        self.assert_private_retained(lambda: C.refute([str(self.record)], 'secret', 'public reason'), path)

    def test_body_only_private_refutation_retains_original(self):
        path = self.hypothesis(body={'v': SECRET, 'visibility': 'private'})
        self.assert_private_retained(lambda: C.refute([str(self.record)], 'secret', 'public reason'), path)

    def test_head_only_private_fold_retains_claim_and_original(self):
        path = self.hypothesis({'claim': SECRET, 'privacy': 'private'})
        draft = self.assert_private_retained(lambda: C.fold([str(self.record)]), path)
        self.assertIn(SECRET, str(draft))

    def test_base_source_closure_private_fold_is_retained(self):
        self.doc['sources']['s.hidden'] = {'name': SECRET, 'privacy': 'private'}
        self.save()
        path = self.hypothesis(body={'v': 42, 'from': 's.hidden'})
        draft = self.assert_private_retained(lambda: C.fold([str(self.record)]), path)
        self.assertIn('s.hidden', str(draft))

    def test_refutation_private_session_source_is_retained(self):
        self.doc['sources']['s.session']['privacy'] = 'private'
        self.save()
        path = self.hypothesis()
        self.assert_private_retained(lambda: C.refute([str(self.record)], 'secret', 'public reason'), path)

    def test_head_reference_closure_is_checked_before_refutation(self):
        self.doc['sources']['s.hidden'] = {'name': SECRET, 'privacy': 'private'}
        self.save()
        for claim in ('Based on {{s.hidden}}', 'Based on s.hidden'):
            with self.subTest(claim=claim):
                path = self.hypothesis({'claim': claim})
                self.assert_private_retained(lambda: C.refute([str(self.record)], 'secret', 'public reason'), path)

    def test_imported_branch_keeps_unpruned_source_privacy_context(self):
        self.doc['sources']['s.vendor'] = {'name': 'Vendor'}
        self.save()
        M.git(self.root, 'add', 'GROUNDING.yaml')
        M.git(self.root, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Public vendor')
        M.git(self.root, 'checkout', '-b', 'feature')
        self.doc['sources']['s.vendor']['privacy'] = 'private'
        self.doc['known']['new.fact'] = {'v': 42, 'from': 's.vendor'}
        self.save()
        M.git(self.root, 'add', 'GROUNDING.yaml')
        M.git(self.root, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Scoped proposal')
        M.git(self.root, 'checkout', 'trunk')
        before = self.record.read_bytes()
        refused, output = self.attempt(lambda: C.fold([str(self.record)], refs=['feature']))
        self.assertTrue(refused, output)
        self.assertEqual(before, self.record.read_bytes())
        self.assertTrue(list((self.base / 'private').glob('*/*.json')))

    def test_imported_record_metadata_permission_is_not_pruned(self):
        M.git(self.root, 'checkout', '-b', 'feature')
        self.doc['meta'] = {'privacy': 'private', 'name': SECRET}
        self.doc['known']['new.fact'] = {'v': 42, 'from': 's.session'}
        self.save()
        M.git(self.root, 'add', 'GROUNDING.yaml')
        M.git(self.root, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Private record metadata')
        M.git(self.root, 'checkout', 'trunk')
        before = self.record.read_bytes()
        refused, output = self.attempt(lambda: C.fold([str(self.record)], refs=['feature']))
        self.assertTrue(refused, output)
        self.assertNotIn(SECRET, output)
        self.assertEqual(before, self.record.read_bytes())

    def test_imported_hypothesis_head_privacy_survives_ref_loading(self):
        M.git(self.root, 'checkout', '-b', 'feature')
        path = self.hypothesis({'claim': SECRET, 'visibility': 'private'})
        M.git(self.root, 'add', str(path.relative_to(self.root)))
        M.git(self.root, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Private hypothesis')
        M.git(self.root, 'checkout', 'trunk')
        before = self.record.read_bytes()
        refused, output = self.attempt(lambda: C.fold([str(self.record)], refs=['feature']))
        self.assertTrue(refused, output)
        self.assertNotIn(SECRET, output)
        self.assertEqual(before, self.record.read_bytes())

    def test_public_refutation_still_writes_and_deletes(self):
        path = self.hypothesis()
        refused, output = self.attempt(lambda: C.refute([str(self.record)], 'secret', 'public reason'))
        self.assertFalse(refused, output)
        self.assertFalse(path.exists())
        self.assertEqual(P.bodies(P.load([str(self.record)]))['hyp.secret']['v'], 'refuted')

    def test_public_fold_ignores_unrelated_private_base_entry(self):
        self.doc['known']['unrelated.fact'] = {'v': SECRET, 'private': True}
        self.save()
        path = self.hypothesis()
        refused, output = self.attempt(lambda: C.fold([str(self.record)]))
        self.assertFalse(refused, output)
        self.assertFalse(path.exists())
        self.assertEqual(P.bodies(P.load([str(self.record)]))['new.fact']['v'], 42)
