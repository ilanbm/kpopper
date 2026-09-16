"""Routing changes re-prove migration bytes and retain reconciliation guards."""
from pathlib import Path
import unittest
from unittest.mock import patch

from scripts import core_migration as C, project_modes as M, pending_grounding as G
from scripts.reasoning.authoring import DECLARATION
from tests.test_pending_grounding import fixture_bundle
from tests import test_core_migration as Fixtures


class MigrationCutover(unittest.TestCase):
    setUp = Fixtures.CoreMigration.setUp
    fixture = Fixtures.CoreMigration.fixture

    def test_explicit_verified_cutover_and_exact_restore(self):
        original = self.fixture()
        project = M.Project(original.parent)
        destination = self.root / 'candidate'
        C.prepare(original).publish(destination)
        candidate = destination / original.name
        receipt = destination / C.ARTIFACTS / 'receipt.json'
        with self.assertRaisesRegex(ValueError, 'differs'):
            project.configure('simple', str(candidate))
        result = project.configure('simple', str(candidate), migration_receipt=receipt)
        self.assertEqual(project.record(), candidate.resolve())
        restored = project.configure('simple', str(original), migration_receipt=receipt, rollback=True,
                                     expected_generation=result['generation'])
        self.assertEqual(project.record(), original.resolve())
        self.assertGreater(restored['generation'], result['generation'])

    def test_tampered_candidate_and_receipt_refuse_without_configuration_change(self):
        original = self.fixture()
        project = M.Project(original.parent)
        destination = self.root / 'candidate'
        C.prepare(original).publish(destination)
        candidate = destination / original.name
        receipt = destination / C.ARTIFACTS / 'receipt.json'
        before = project.config()
        old = candidate.read_bytes()
        candidate.write_bytes(old + b'# later edit\n')
        with self.assertRaises(ValueError):
            project.configure('simple', str(candidate), migration_receipt=receipt)
        self.assertEqual(project.config(), before)
        candidate.write_bytes(old)
        receipt.write_text('{"equivalent":true}\n')
        with self.assertRaises(ValueError):
            project.configure('simple', str(candidate), migration_receipt=receipt)
        self.assertEqual(project.config(), before)

    def test_hypothesis_copy_is_not_permission_to_cut_over(self):
        original = self.fixture('hypothesis')
        destination = self.root / 'candidate'
        C.prepare(original).publish(destination)
        project = M.Project(original.parent)
        before = project.config()
        with self.assertRaisesRegex(ValueError, 'hypotheses need reconciliation'):
            project.configure('simple', str(destination / original.name),
                              migration_receipt=destination / C.ARTIFACTS / 'receipt.json')
        self.assertEqual(project.config(), before)

    def test_receipt_binds_the_complete_entry_not_another_copied_file(self):
        original = self.fixture()
        document = M.I.P.yaml.safe_load(original.read_bytes())
        inputs = {'known': {'p.input': document['known'].pop('p.input')}}
        part = original.parent / 'part.yaml'
        part.write_text(M.I.P.yaml.safe_dump(document))
        original.write_text(M.I.P.yaml.safe_dump(dict(inputs, also='part.yaml')))
        project = M.Project(original.parent)
        destination = self.root / 'candidate'
        C.prepare(original).publish(destination)
        receipt = destination / C.ARTIFACTS / 'receipt.json'
        before = project.config()
        with self.assertRaisesRegex(ValueError, 'entry'):
            project.configure('simple', str(destination / 'part.yaml'), migration_receipt=receipt)
        self.assertEqual(project.config(), before)
        project.configure('simple', str(destination / original.name), migration_receipt=receipt)
        current = project.config()
        with self.assertRaises(ValueError):
            project.configure('simple', str(part), migration_receipt=receipt, rollback=True)
        self.assertEqual(project.config(), current)

    def test_advanced_registration_is_relative_and_prepared_in_every_worktree(self):
        original = self.fixture(mode='advanced')
        project = M.Project(original.parent)
        other = self.root / 'other-worktree'
        M.git(project.root, 'worktree', 'add', '-b', 'other', str(other))
        destination = project.root / 'candidate'
        C.prepare(original).publish(destination)
        receipt = destination / C.ARTIFACTS / 'receipt.json'
        before = project.config()
        with self.assertRaisesRegex(ValueError, 'relative|inside'):
            project.configure('advanced', str(destination / original.name), migration_receipt=receipt)
        with self.assertRaisesRegex(ValueError, 'unavailable|worktree'):
            project.configure('advanced', 'candidate/' + original.name, migration_receipt=receipt)
        self.assertEqual(project.config(), before)
        M.git(project.root, 'add', 'candidate')
        M.git(project.root, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Candidate record')
        M.git(other, 'merge', '--ff-only', 'main')
        project.configure('advanced', 'candidate/' + original.name, migration_receipt=receipt)
        self.assertEqual(project.record(), destination / original.name)

    def test_return_to_advanced_rechecks_accepted_overlay_profile(self):
        original = self.fixture(mode='advanced')
        project = M.Project(original.parent)
        store = G.Store(project)
        bundle = fixture_bundle()
        store.capture(bundle, event_id='old', contribution_id='old', shareability='project')
        shared = self.root / 'shared.yaml'
        document = M.I.P.yaml.safe_load(original.read_bytes())
        document['meta'] = {'reasoning': DECLARATION}
        shared.write_text(M.I.P.yaml.safe_dump(document))
        original.write_bytes(shared.read_bytes())
        config = dict(project.config(), mode='simple', record=str(shared.resolve()), generation=1)
        M.I._save(project.config_path, config)
        # Unit boundary: an accepted, content-bound pending proof settles the
        # publication obligation, but cannot settle incompatible interpretation.
        proof = dict(verified=True, ledger_ref=store.head(), generation=1, scope=None,
                     unresolved=[], terminal={bundle['revision']: 'accepted'})
        with self.assertRaisesRegex(ValueError, 'pending_profile_reconciliation_required'):
            project.transition_report('advanced', original.name, _pending_proof=proof)
        self.assertEqual(project.config(), config)

    def test_accepted_contribution_reverification_allows_cutover(self):
        from scripts import pending_publication as Pub
        from tests.test_pending_publication import PublicationTests

        case = PublicationTests()
        self.addCleanup(case.doCleanups)
        case.setUp()
        record = case.root / 'GROUNDING.yaml'
        record.write_text(M.I.P.yaml.safe_dump({'meta': {'reasoning': DECLARATION},
            'known': {'p.input': {'v': 10, 'from': 'measured'}}}))
        M.git(case.root, 'add', '.')
        M.git(case.root, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Core record')
        M.git(case.root, 'push', 'team', 'trunk')
        scope = {'kind': 'external', 'environment': 'API v2'}
        bundle = G.prepare({'meta': {'reasoning': DECLARATION},
            'known': {'p.result': {'rule': {'expr': '10 / 3'}, 'scope': scope}}},
            ['p.result'], scope=scope, shareability='project')
        case.store.capture(bundle, event_id='core', contribution_id='core', shareability='project')
        case.run_ok()
        case.merge()
        M.git(case.root, 'pull', '--ff-only', 'team', 'trunk')
        with patch.object(Pub.Publisher, '_provider', lambda _self, _scope: case.provider):
            proof = Pub.Publisher(case.project).verify_obligations()
            self.assertEqual(proof['terminal'][bundle['revision']], 'accepted')
            plan = C.prepare(record)
            destination = case.root / 'candidate'
            plan.publish(destination)
            Pub.Publisher(case.project).verify_obligations()
            repeated = C.prepare(record)
            self.assertEqual(plan.original.snapshot_id, repeated.original.snapshot_id)
            self.assertEqual(plan.files[C.ARTIFACTS + '/receipt.json'], repeated.files[C.ARTIFACTS + '/receipt.json'])
            M.git(case.root, 'add', 'candidate')
            M.git(case.root, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Candidate')
            result = case.project.configure('advanced', 'candidate/GROUNDING.yaml',
                migration_receipt=destination / C.ARTIFACTS / 'receipt.json')
            self.assertEqual(result['record'], 'candidate/GROUNDING.yaml')
