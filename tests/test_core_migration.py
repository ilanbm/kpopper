"""Core migration copies a complete frozen closure without live authority."""
import copy
import hashlib
import json
from pathlib import Path
import tempfile
import subprocess
import sys
import unittest
from unittest.mock import patch

from scripts import provenance as P, project_modes as M, pending_grounding as G
from scripts import core_migration as C
from scripts.reasoning.snapshot import Snapshot
from scripts.reasoning.evaluate import Evaluator


class CoreMigration(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)

    def fixture(self, shape='single', mode='simple', name='GROUNDING.yaml'):
        source = self.root / (shape + '-' + mode)
        source.mkdir()
        doc = {'known': {'p.input': {'v': 10, 'from': 'measured'},
                         'p.result': {'rule': {'expr': 'p.input + 2'}}},
               'judgments': {'d.stable': {'rests_on': ['p.result'], 'seen': {'p.result': 12},
                                          'wrong_if': {'expr': 'p.result < 0'}, 'verdict': 'Stable'}}}
        record = source / name
        if shape in ('sharded', 'pointer'):
            child = source / ('data/values.yaml' if shape == 'pointer' else 'part.yaml')
            child.parent.mkdir(parents=True, exist_ok=True)
            child.write_text(P.yaml.safe_dump(doc, sort_keys=False))
            record.write_text('record: ' + str(child.relative_to(source)) + '\n')
        else:
            record.write_text(P.yaml.safe_dump(doc, sort_keys=False))
        layout = P.layout(record)
        for key, content in [('view', 'sections: []\n'), ('measure', 'recipes: {}\n'), ('session', '{"note":"retained"}\n')]:
            path = Path(layout[key])
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content)
        if shape == 'hypothesis':
            path = Path(layout['hypotheses']) / 'alternative.yaml'
            path.parent.mkdir(parents=True)
            path.write_text(P.yaml.safe_dump({'hypothesis': {'claim': 'Alternative', 'folds': 'never'},
                                             'known': {'p.input': {'v': 11, 'from': 'measured'}}}))
        if mode == 'advanced':
            M.git(source, 'init', '-b', 'main')
            M.git(source, 'config', 'user.name', 'Fixture')
            M.git(source, 'config', 'user.email', 'fixture@example.test')
            M.git(source, 'add', '.')
            M.git(source, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Source')
        return record

    def test_complete_copy_matrix_and_captured_candidate_replay(self):
        for mode in ('simple', 'advanced'):
            for shape in ('single', 'sharded', 'pointer', 'hypothesis'):
                with self.subTest(mode=mode, shape=shape):
                    record = self.fixture(shape, mode)
                    before = {str(path): path.read_bytes() for path in record.parent.rglob('*') if path.is_file() and '.git' not in path.parts}
                    plan = C.prepare(record)
                    self.assertFalse(plan.problems, plan.problems)
                    destination = self.root / ('copy-' + shape + '-' + mode)
                    result = plan.publish(destination)
                    self.assertEqual(result['state'], 'materialized')
                    for path, data in before.items():
                        self.assertEqual(Path(path).read_bytes(), data)
                    replay = Snapshot.from_json((destination / C.ARTIFACTS / 'candidate.json').read_bytes())
                    with patch('scripts.reasoning.snapshot._observation', side_effect=AssertionError('live lookup')):
                        value = Evaluator(replay).evaluate({'ref': 'p.result'}, declared=['p.result'])
                    self.assertEqual(value['value']['numerator'], '12')
                    self.assertEqual(replay.to_data()['document']['judgments']['d.stable']['seen'], {'p.result': 12})
                    if shape == 'hypothesis':
                        self.assertIn('alternative', replay.to_data()['hypotheses'])
                    copied_layout = P.layout(destination / record.name)
                    for role in ('view', 'measure', 'session'):
                        self.assertEqual(Path(copied_layout[role]).read_bytes(), Path(P.layout(record)[role]).read_bytes())

    def test_stale_source_and_new_sidecar_refuse_without_destination(self):
        record = self.fixture()
        for change in ('source', 'hypothesis'):
            with self.subTest(change=change):
                plan = C.prepare(record)
                if change == 'source':
                    record.write_text(record.read_text() + '# changed\n')
                else:
                    path = Path(P.layout(record)['hypotheses']) / 'new.yaml'
                    path.parent.mkdir(exist_ok=True)
                    path.write_text('known: {p.new: {v: 3}}\n')
                destination = self.root / ('stale-' + change)
                with self.assertRaisesRegex(ValueError, 'changed|stale|inventory'):
                    plan.publish(destination)
                self.assertFalse(destination.exists())

    def test_generated_metadata_preserves_block_and_flow_source_layouts(self):
        for style in ('block', 'flow'):
            with self.subTest(style=style):
                record = self.fixture(shape=style)
                if style == 'block':
                    original = (b'---\n# record heading\nmeta:\n'
                                b'  updated: 2026-09-16  # retained date\n'
                                b'  name: Original name  # retained metadata\n\n' + record.read_bytes())
                else:
                    original = P.yaml.safe_dump(P.yaml.safe_load(record.read_bytes()),
                                               default_flow_style=True, width=100000).encode()
                record.write_bytes(original)
                plan = C.prepare(record)
                self.assertFalse(plan.problems, plan.problems)
                destination = self.root / ('metadata-copy-' + style)
                plan.publish(destination)
                copied = (destination / record.name).read_bytes()
                self.assertEqual(P.yaml.safe_load(copied)['meta']['reasoning']['version'], 2)
                self.assertEqual(record.read_bytes(), original)
                if style == 'block':
                    self.assertIn(b'  updated: 2026-09-16  # retained date\n', copied)
                    self.assertIn(b'  name: Original name  # retained metadata\n', copied)
                    self.assertTrue(copied.startswith(b'---\n# record heading\n'))
                plan.apply()
                self.assertEqual(record.read_bytes(), copied)

    def test_missing_pointer_and_half_moved_sidecar_block_complete_conversion(self):
        record = self.fixture()
        record.write_text(record.read_text() + 'also: missing.yaml\n')
        with self.assertRaisesRegex(ValueError, 'missing|unavailable'):
            C.prepare(record)
        record.write_text(record.read_text().replace('also: missing.yaml\n', ''))
        (record.parent / 'PROVENANCE.measure.yaml').write_text('recipes: {}\n')
        with self.assertRaisesRegex(ValueError, 'layout|leftover|moved'):
            C.prepare(record)

    def test_inplace_only_one_changed_canonical_file_and_history_unchanged(self):
        record = self.fixture()
        original = record.read_bytes()
        plan = C.prepare(record)
        result = plan.apply()
        self.assertTrue(result['applied'])
        document = P.yaml.safe_load(record.read_bytes())
        self.assertEqual(document['meta']['reasoning']['version'], 2)
        self.assertEqual(document['judgments']['d.stable']['seen'], {'p.result': 12})
        self.assertNotEqual(original, record.read_bytes())
        other = self.fixture('sharded')
        plan = C.prepare(other)
        before = other.read_bytes()
        with self.assertRaisesRegex(ValueError, 'requires_copy_migration'):
            plan.apply()
        self.assertEqual(before, other.read_bytes())

    def test_legacy_layout_and_comments_unknown_fields_and_history_bytes_survive(self):
        record = self.fixture(name='PROVENANCE.yaml')
        record.write_text(record.read_text().replace("rule:\n      expr: p.input + 2", 'rule: p.input + 2  # active formula') + '# final note\nunknown: {opaque: unchanged}\n')
        before = record.read_bytes()
        plan = C.prepare(record)
        destination = self.root / 'legacy-copy'
        plan.publish(destination)
        copied = (destination / record.name).read_bytes()
        self.assertIn(b'# active formula', copied)
        self.assertIn(b'# final note\nunknown: {opaque: unchanged}', copied)
        history = before[before.index(b'    seen:'):before.index(b'    wrong_if:')]
        self.assertIn(history, copied)
        self.assertEqual((destination / C.ARTIFACTS / 'originals' / record.name).read_bytes(), before)
        self.assertTrue(plan.validate_destination(destination)['valid'])

    def test_absolute_external_pointer_is_relocated_without_live_fallback(self):
        record = self.fixture('pointer')
        child = record.parent / 'data/values.yaml'
        external = self.root / 'external.yaml'
        external.write_bytes(child.read_bytes())
        record.write_text('record: ' + str(external) + '\n')
        child.unlink()
        plan = C.prepare(record)
        destination = self.root / 'external-copy'
        plan.publish(destination)
        pointer = P.yaml.safe_load((destination / record.name).read_bytes())['record']
        self.assertFalse(Path(pointer).is_absolute())
        self.assertTrue((destination / pointer).is_file())
        self.assertNotIn(str(external), (destination / record.name).read_text())
        self.assertTrue(plan.validate_destination(destination)['valid'])

    def test_destination_changed_artifact_pointer_and_extra_file_are_rejected(self):
        record = self.fixture()
        plan = C.prepare(record)
        destination = self.root / 'copy'
        plan.publish(destination)
        for name in (record.name, C.ARTIFACTS + '/original.json', C.ARTIFACTS + '/receipt.json'):
            with self.subTest(name=name):
                path = destination / name
                before = path.read_bytes()
                path.write_bytes(before + b' ')
                with self.assertRaisesRegex(ValueError, 'changed|match'):
                    plan.validate_destination(destination)
                path.write_bytes(before)
        (destination / 'extra').write_text('unexpected')
        with self.assertRaisesRegex(ValueError, 'inventory'):
            plan.validate_destination(destination)

    def test_removed_sidecar_and_config_bytes_make_preview_stale(self):
        record = self.fixture()
        project = M.Project(record.parent)
        project.configure(mode='simple', record=str(record))
        plan = C.prepare(record)
        project.config_path.write_text(project.config_path.read_text() + '\n')
        with self.assertRaisesRegex(ValueError, 'changed'):
            plan.publish(self.root / 'stale-config')
        plan = C.prepare(record)
        Path(P.layout(record)['measure']).unlink()
        with self.assertRaisesRegex(ValueError, 'changed|unreadable'):
            plan.publish(self.root / 'stale-measure')

    def test_hypothesis_active_fields_preserve_head_history_and_block_inplace(self):
        record = self.fixture('hypothesis')
        path = Path(P.layout(record)['hypotheses']) / 'alternative.yaml'
        doc = P.yaml.safe_load(path.read_bytes())
        doc['known']['p.result'] = {'rule': 'p.input + 2'}
        doc['judgments'] = {'d.stable': {'rests_on': ['p.result'], 'seen': {'p.result': 12},
                                      'wrong_if': 'p.result < 0', 'verdict': 'Stable'}}
        path.write_text(P.yaml.safe_dump(doc, sort_keys=False))
        plan = C.prepare(record)
        self.assertFalse(plan.problems, plan.problems)
        hyp = plan.candidate.to_data()['hypotheses']['alternative']
        self.assertEqual(hyp['head'], doc['hypothesis'])
        self.assertEqual(hyp['document']['known']['p.result']['rule'], {'expr': 'p.input + 2'})
        self.assertEqual(hyp['document']['judgments']['d.stable']['seen'], {'p.result': 12})
        with self.assertRaisesRegex(ValueError, 'requires_copy_migration'):
            plan.apply()
        plan.publish(self.root / 'hypothesis-copy')

    def test_ambiguous_and_unreadable_hypotheses_refuse_publication(self):
        record = self.fixture('hypothesis')
        path = Path(P.layout(record)['hypotheses']) / 'alternative.yaml'
        path.write_text('known:\n  p.result: {rule: graph.nodes + 2}\n')
        plan = C.prepare(record)
        self.assertTrue(plan.problems)
        with self.assertRaisesRegex(ValueError, 'incomplete'):
            plan.publish(self.root / 'invalid-copy')
        path.write_text('known: [invalid')
        with self.assertRaisesRegex(ValueError, 'unreadable'):
            C.prepare(record)

    def test_publication_failure_and_destination_race_do_not_replace_anything(self):
        record = self.fixture()
        plan = C.prepare(record)
        destination = self.root / 'failed'
        with patch.object(plan, 'validate_destination', side_effect=ValueError('forced validation failure')):
            with self.assertRaisesRegex(ValueError, 'forced'):
                plan.publish(destination)
        self.assertFalse(destination.exists())
        validate = plan.validate_destination
        def raced(staging):
            result = validate(staging)
            destination.mkdir()
            return result
        with patch.object(plan, 'validate_destination', side_effect=raced):
            with self.assertRaises(FileExistsError):
                plan.publish(destination)
        self.assertEqual(list(destination.iterdir()), [])
        self.assertFalse(list(self.root.glob('.knowledge-*')))

    def test_pending_original_evidence_is_frozen_and_ledger_changes_make_preview_stale(self):
        record = self.fixture(mode='advanced')
        project = M.Project(record.parent)
        store = G.Store(project)
        scope = {'kind': 'project', 'environment': 'fixture'}
        bundle = G.prepare({'known': {'p.pending': {'v': 7, 'from': 'measured', 'file': 'evidence.txt', 'scope': scope}}}, ['p.pending'],
            scope=scope, shareability='project',
            evidence={'evidence.txt': b'original evidence'})
        store.capture(bundle, event_id='event-one', contribution_id='one', shareability='project')
        before = store.head()
        plan = C.prepare(record)
        self.assertFalse(plan.problems, plan.problems)
        destination = self.root / 'pending-copy'
        plan.publish(destination)
        self.assertEqual(store.head(), before)
        revision = bundle['revision']
        self.assertEqual((destination / C.ARTIFACTS / 'pending' / revision / 'evidence.txt').read_bytes(), b'original evidence')
        replay = Snapshot.from_json((destination / C.ARTIFACTS / 'candidate.json').read_bytes())
        self.assertIn(revision, replay.to_data()['context']['migration']['incompatible_original_pending'])
        self.assertFalse((destination / '.git').exists())
        store.capture(bundle, event_id='event-two', contribution_id='two', shareability='project')
        self.assertNotEqual(store.head(), before)
        with self.assertRaisesRegex(ValueError, 'changed'):
            plan.publish(self.root / 'stale-ref')
        self.assertEqual(Evaluator(replay).evaluate({'ref': 'p.result'}, declared=['p.result'])['value']['numerator'], '12')

    def test_withdrawn_unsupported_archive_copies_without_interpretation(self):
        from scripts import pending_publication as Pub
        from tests.test_contribution_routes import CORE, SCOPE, retain_raw

        for capability in (dict(CORE, requires=['arithmetic/v1', 'future/v1']),
                           dict(CORE, version=999)):
            with self.subTest(capability=capability):
                record = self.fixture(mode='advanced', shape=str(capability['version']))
                store = G.Store(record.parent)
                evidence = b'Original unsupported evidence\n'
                manifest = {'version': 2, 'roots': ['p.future'], 'scope': SCOPE,
                    'evidence': {'evidence.txt': hashlib.sha256(evidence).hexdigest()},
                    'reasoning': capability,
                    'document': {'meta': {'reasoning': capability},
                                 'known': {'p.future': {'v': 1, 'scope': SCOPE}}}}
                bundle = {'manifest': manifest, 'revision': G.identity(manifest),
                          'files': {'evidence.txt': evidence}}
                retain_raw(store, bundle)
                with self.assertRaisesRegex(P.Refused, 'unsupported_capability'):
                    C.prepare(record)
                Pub.Publisher(record.parent).action('withdraw', revisions=[bundle['revision']],
                    reason='Retain as unsupported evidence')
                head = store.head()
                original = store.read_bundle(bundle['revision'])
                destination = self.root / ('archive-copy-' + str(capability['version']))
                plan = C.prepare(record)
                self.assertFalse(plan.problems, plan.problems)
                plan.publish(destination)
                replay = Snapshot.from_json((destination / C.ARTIFACTS / 'candidate.json').read_bytes())
                data = replay.to_data()
                self.assertFalse(data['hypotheses'])
                self.assertIn(bundle['revision'], data['context']['migration']['incompatible_original_pending'])
                self.assertFalse(data['context']['migration']['publication_authority'])
                self.assertIn(bundle['revision'], data['context']['pending']['bundles'])
                self.assertEqual(store.head(), head)
                self.assertEqual(store.read_bundle(bundle['revision']), original)
                self.assertEqual((destination / C.ARTIFACTS / 'pending' / bundle['revision'] / 'evidence.txt').read_bytes(), evidence)
                with patch('scripts.reasoning.snapshot._observation', side_effect=AssertionError('live lookup')):
                    self.assertEqual(Evaluator(replay).evaluate({'ref': 'p.result'}, declared=['p.result'])['value']['numerator'], '12')

    def test_local_source_evidence_is_copied_and_external_locators_not_fetched(self):
        record = self.fixture()
        document = P.yaml.safe_load(record.read_bytes())
        document['sources'] = {'s.local': {'file': '.kpopper/evidence/report.txt', 'scope': 'private'},
                               's.external': {'file': '/not/authorized/external.txt'}}
        record.write_text(P.yaml.safe_dump(document, sort_keys=False))
        evidence = record.parent / '.kpopper/evidence/report.txt'
        evidence.parent.mkdir(parents=True)
        evidence.write_bytes(b'private original evidence')
        plan = C.prepare(record)
        destination = self.root / 'evidence-copy'
        plan.publish(destination)
        self.assertEqual((destination / '.kpopper/evidence/report.txt').read_bytes(), evidence.read_bytes())
        self.assertEqual(plan.candidate.to_data()['document']['sources']['s.external']['file'], '/not/authorized/external.txt')
        evidence.write_bytes(b'new evidence')
        with self.assertRaisesRegex(ValueError, 'changed'):
            plan.validate_destination(destination)
        evidence.unlink()
        with self.assertRaisesRegex(ValueError, 'missing referenced evidence'):
            C.prepare(record)

    def test_dot_relative_evidence_is_copied_and_directory_locator_is_explicit(self):
        record = self.fixture()
        document = P.yaml.safe_load(record.read_bytes())
        document['sources'] = {'s.local': {'file': './notes.md'}}
        record.write_text(P.yaml.safe_dump(document))
        evidence = record.parent / 'notes.md'
        evidence.write_bytes(b'Retained local evidence')
        destination = self.root / 'relative-evidence-copy'
        C.prepare(record).publish(destination)
        self.assertEqual((destination / 'notes.md').read_bytes(), evidence.read_bytes())
        self.assertEqual(P.yaml.safe_load((destination / record.name).read_bytes())['sources'], document['sources'])
        document['sources']['s.local']['file'] = './notes'
        record.write_text(P.yaml.safe_dump(document))
        (record.parent / 'notes').mkdir()
        before = record.read_bytes()
        with self.assertRaisesRegex(ValueError, 'directory evidence locator.*explicit file'):
            C.prepare(record)
        self.assertEqual(record.read_bytes(), before)

    def test_admin_credentials_and_permission_are_not_copied_into_export(self):
        record = self.fixture(mode='advanced')
        project = M.Project(record.parent)
        config = project.config()
        config['publication'] = {'remote': 'origin', 'repository': 'private/repo', 'target': 'main',
                                 'branch': 'pending_grounding', 'standing_permission': True}
        project.state.mkdir(parents=True)
        project.config_path.write_text(json.dumps(config))
        plan = C.prepare(record)
        destination = self.root / 'safe-copy'
        plan.publish(destination)
        self.assertFalse((destination / '.kpopper/project.json').exists())
        self.assertNotIn(b'standing_permission', (destination / C.ARTIFACTS / 'receipt.json').read_bytes())
        replay = Snapshot.from_json((destination / C.ARTIFACTS / 'candidate.json').read_bytes())
        self.assertFalse(replay.to_data()['context']['migration']['publication_authority'])

    def test_inplace_stale_source_has_no_backup_or_mutation(self):
        record = self.fixture()
        plan = C.prepare(record)
        record.write_text(record.read_text() + '# concurrently changed\n')
        changed = record.read_bytes()
        with self.assertRaisesRegex(ValueError, 'changed'):
            plan.apply()
        self.assertEqual(record.read_bytes(), changed)
        self.assertFalse(list(record.parent.glob('*.bak')))

    def test_target_revision_change_invalidates_frozen_preview(self):
        record = self.fixture(mode='advanced')
        project = M.Project(record.parent)
        config = project.config()
        config['publication'] = {'remote': 'origin', 'repository': 'fixture/repo', 'target': 'main',
                                 'branch': 'pending_grounding', 'standing_permission': False}
        project.state.mkdir(parents=True)
        project.config_path.write_text(json.dumps(config))
        M.git(project.root, 'update-ref', 'refs/remotes/origin/main', 'HEAD')
        plan = C.prepare(record)
        M.git(project.root, '-c', 'commit.gpgsign=false', 'commit', '--allow-empty', '-m', 'Observed target changed')
        M.git(project.root, 'update-ref', 'refs/remotes/origin/main', 'HEAD')
        with self.assertRaisesRegex(ValueError, 'changed'):
            plan.publish(self.root / 'stale-target')

    def test_unreadable_sidecar_is_an_explicit_blocker(self):
        record = self.fixture()
        path = Path(P.layout(record)['measure'])
        read_bytes = Path.read_bytes
        def unreadable(instance):
            if instance.resolve() == path.resolve():
                raise PermissionError('fixture unreadable')
            return read_bytes(instance)
        with patch.object(Path, 'read_bytes', unreadable):
            with self.assertRaisesRegex(ValueError, 'unreadable migration sidecar'):
                C.prepare(record)

    def test_source_path_alias_has_identical_manifest(self):
        record = self.fixture()
        alias = self.root / 'source-alias'
        alias.symlink_to(record.parent, target_is_directory=True)
        direct = C.prepare(record.resolve())
        indirect = C.prepare(alias / record.name)
        self.assertEqual(indirect.record, record.resolve())
        self.assertEqual(direct.manifest, indirect.manifest)
        destination = self.root / 'alias-copy'
        indirect.publish(destination)
        self.assertTrue(direct.validate_destination(destination)['valid'])

    def test_file_path_cli_import_uses_the_same_captured_reader(self):
        record = self.fixture()
        result = subprocess.run([sys.executable, str(Path(C.__file__).with_name('expression_cli.py')),
                                 'migrate', '--record', str(record), '--profile', 'core/v1'],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        self.assertTrue(json.loads(result.stdout)['complete'])
