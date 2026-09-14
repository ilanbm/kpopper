"""The capture receipt must mean a complete contribution is reachable from Git."""
import base64
import copy
import datetime
import multiprocessing
import queue
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from scripts import pending_grounding as G, project_modes as M


def capture_process(root, index, same=False):
    store = G.Store(root)
    name = 'api.limit' if same else 'api.limit' + str(index)
    bundle = fixture_bundle(name)
    store.capture(bundle, event_id='same' if same else 'event-' + str(index),
                  contribution_id='limit', shareability='project')


def conflicting_capture_process(root, value, output):
    try:
        receipt = G.Store(root).capture(fixture_bundle(value=value), event_id='contended',
                                       contribution_id='limit', shareability='project')
        output.put(('captured', receipt['revision']))
    except ValueError as error:
        output.put(('refused', str(error)))


def configure_process(root, destination, started, output):
    started.set()
    try:
        output.put(('configured', M.Project(root).configure('simple', record=destination)))
    except ValueError as error:
        output.put(('refused', str(error)))


def fixture_bundle(name='api.limit', value=10):
    doc = {'sources': {'s.vendor': {'name': 'Vendor', 'file': 'evidence/vendor.txt', 'read': '2026-09-14'}},
           'known': {name: {'name': 'Limit', 'v': value, 'from': 's.vendor', 'at': 'table 1',
                            'scope': {'kind': 'external', 'environment': 'API v2'}}}}
    return G.prepare(doc, [name], scope={'kind': 'external', 'environment': 'API v2'},
                     shareability='project', evidence={'evidence/vendor.txt': b'The limit is 10.\n'})


class Repository(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name).resolve()
        self.root = self.base / 'main'
        self.root.mkdir()
        M.git(self.root, 'init', '-b', 'trunk')
        M.git(self.root, 'config', 'user.name', 'Test')
        M.git(self.root, 'config', 'user.email', 'test@example.test')
        (self.root / 'app.txt').write_text('base\n')
        M.git(self.root, 'add', 'app.txt')
        M.git(self.root, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Initial')
        self.store = G.Store(self.root)

    def worktree(self, name='feature'):
        path = self.base / name
        M.git(self.root, 'worktree', 'add', '-b', name, str(path))
        return path

    def capture(self, bundle=None, **kw):
        return self.store.capture(bundle or fixture_bundle(), event_id=kw.get('event_id', 'event-1'),
                                  contribution_id=kw.get('contribution_id', 'limit'),
                                  shareability=kw.get('shareability', 'project'))


class IdentityTests(unittest.TestCase):
    def test_scalar_types_null_and_absence_are_distinct(self):
        readings = [{}, {'v': None}, {'v': True}, {'v': 1}, {'v': 1.0}, {'v': '1'}, {'v': ' 1 '},
                    {'v': datetime.date(2026, 9, 14)}, {'v': '2026-09-14'}]
        self.assertEqual(len({G.identity(x) for x in readings}), len(readings))
        for reading in readings:
            self.assertEqual(G.identity(reading), G.identity(G._decode(G._encode(reading))))

    def test_map_order_and_complete_source_body(self):
        bundle = fixture_bundle()
        manifest = bundle['manifest']
        self.assertEqual(G.identity(manifest), G.identity(dict(reversed(list(manifest.items())))))
        other = copy.deepcopy(manifest)
        other['document']['sources']['s.vendor']['read'] = '2026-09-13'
        self.assertNotEqual(G.identity(other), bundle['revision'])

    def test_dependency_closure_preserves_bodies_and_omits_unrelated_content(self):
        doc = fixture_bundle()['manifest']['document']
        doc['known']['private.unrelated'] = {'v': 'SECRET', 'private': True}
        doc['judgments'] = {'c.fits': {'rests_on': ['api.limit'], 'seen': {'api.limit': 10},
                                     'verdict': 'fits', 'wrong_if': 'api.limit < 0'}}
        selected = G.closure(doc, ['c.fits'])
        self.assertEqual(set(G.entries(selected)), {'s.vendor', 'api.limit', 'c.fits'})
        self.assertEqual(selected['judgments']['c.fits']['seen'], {'api.limit': 10})
        self.assertNotIn('SECRET', str(selected))

    def test_missing_source_and_dependency_are_refused(self):
        for field, value in [('from', '{{s.missing}}'), ('rests_on', ['api.missing'])]:
            with self.subTest(field=field), self.assertRaises(ValueError):
                G.closure({'known': {'api.limit': {'v': 1, field: value}}}, ['api.limit'])

    def test_existing_prose_citations_are_preserved(self):
        for citation in ('scripts/sameness.py', 'pyproject.toml', 'package.json'):
            doc = {'known': {'p.candidate_reasons': {'v': True, 'from': citation}}}
            self.assertEqual(G.closure(doc, ['p.candidate_reasons']), doc)

    def test_builtin_cannot_be_a_contribution_root(self):
        with self.assertRaisesRegex(ValueError, 'stored entries'):
            G.prepare({'known': {'api.limit': {'v': 1}}}, ['graph.entries'],
                      scope={'kind': 'project', 'environment': 'all'}, shareability='project')

    def test_evidence_path_collisions_are_rejected_in_both_orders(self):
        for files in ({'notes/x.txt': b'x', 'notes': b'n'}, {'notes': b'n', 'notes/x.txt': b'x'}):
            with self.subTest(files=list(files)), self.assertRaisesRegex(ValueError, 'collides'):
                G.prepare({'sources': {'s.one': {'file': 'notes'}, 's.two': {'file': 'notes/x.txt'}}},
                          ['s.one', 's.two'], scope={'kind': 'project', 'environment': 'all'},
                          shareability='project', evidence=files)

    def test_code_measurement_retains_exact_scope_without_becoming_a_hypothesis(self):
        scope = {'kind': 'code', 'environment': 'feature benchmark', 'commit': 'a' * 40}
        doc = {'known': {'bench.elapsed': {'v': 2.0, 'scope': scope}}}
        bundle = G.prepare(doc, ['bench.elapsed'], scope=scope, shareability='project')
        self.assertEqual(bundle['manifest']['document'], doc)
        del doc['known']['bench.elapsed']['scope']
        with self.assertRaisesRegex(ValueError, 'portable body'):
            G.prepare(doc, ['bench.elapsed'], scope=scope, shareability='project')

    def test_equivalence_checks_full_subset_and_portable_evidence(self):
        bundle = fixture_bundle()
        doc = copy.deepcopy(bundle['manifest']['document'])
        doc['known']['feature.extra'] = {'v': 99}
        self.assertTrue(G.equivalent(bundle, doc, bundle['files']))
        doc['sources']['s.vendor']['read'] = '2026-09-12'
        self.assertFalse(G.equivalent(bundle, doc, bundle['files']))
        self.assertFalse(G.equivalent(bundle, bundle['manifest']['document'], {'evidence/vendor.txt': b'changed'}))

    def test_equivalence_rejects_explicit_or_inferred_changes_to_judgment_roles(self):
        source = {'known': {'api.limit': {'v': 1}}, 'judgments': {'c.fits': {
            'rests_on': ['api.limit'], 'seen': {'api.limit': 1}, 'wrong_if': 'api.limit > 2',
            'scope': {'kind': 'project', 'environment': 'all'}}}}
        bundle = G.prepare(source, ['c.fits'], scope={'kind': 'project', 'environment': 'all'}, shareability='project')
        for explicit in (False, True):
            target = copy.deepcopy(source)
            if explicit:
                target['schema'] = {'deps': 'premises', 'snapshot': 'seen', 'predicate': 'wrong_if'}
            for name in ('local.one', 'local.two'):
                target['judgments'][name] = {'premises': ['api.limit'], 'seen': {'api.limit': 1},
                                              'wrong_if': 'api.limit < 0'}
            self.assertNotIn('c.fits', G.P.infer(target)[1])
            self.assertFalse(G.equivalent(bundle, target, {}))
        compatible = copy.deepcopy(source)
        compatible['judgments']['local.extra'] = {'rests_on': ['api.limit'], 'seen': {'api.limit': 1},
                                                  'wrong_if': 'api.limit < 0'}
        self.assertTrue(G.equivalent(bundle, compatible, {}))

    def test_source_only_closure_with_explicit_schema_can_still_match(self):
        doc = fixture_bundle()['manifest']['document']
        doc['schema'] = {'deps': 'rests_on', 'snapshot': 'seen', 'predicate': 'wrong_if'}
        bundle = G.prepare(doc, ['api.limit'], scope={'kind': 'external', 'environment': 'API v2'},
                           shareability='project', evidence=fixture_bundle()['files'])
        self.assertTrue(G.equivalent(bundle, doc, bundle['files']))

    def test_unportable_or_unpermitted_evidence_is_rejected(self):
        for name in ('/private/secret.txt', '../secret.txt', '.git/config', 'a/../../secret', 'a\\secret'):
            with self.subTest(name=name), self.assertRaises(ValueError):
                G.prepare({'sources': {'s.file': {'file': name}}}, ['s.file'], shareability='project',
                          scope={'kind': 'project', 'environment': 'all'}, evidence={name: b'secret'})
        bundle = fixture_bundle()
        with self.assertRaises(ValueError):
            G.prepare(bundle['manifest']['document'], ['api.limit'], shareability='project',
                      scope=bundle['manifest']['scope'], evidence={**bundle['files'], 'unrelated': b'SECRET'})


class StoreTests(Repository):
    def test_capture_is_one_git_receipt_without_checkout_or_index_writes(self):
        (self.root / 'staged.txt').write_text('staged')
        M.git(self.root, 'add', 'staged.txt')
        index = M.git(self.root, 'write-tree').stdout
        status = M.git(self.root, 'status', '--porcelain').stdout
        branch = M.git(self.root, 'rev-parse', 'HEAD').stdout
        receipt = self.capture()
        self.assertEqual(self.store.head(), receipt['commit'])
        self.assertEqual(M.git(self.root, 'status', '--porcelain').stdout, status)
        self.assertEqual(M.git(self.root, 'write-tree').stdout, index)
        self.assertEqual(M.git(self.root, 'rev-parse', 'HEAD').stdout, branch)
        self.assertEqual(self.store.read_bundle(receipt['revision']), fixture_bundle())
        self.assertFalse((self.root / 'GROUNDING.yaml').exists())

    def test_capture_survives_origin_worktree_removal_and_gc(self):
        path = self.worktree()
        store = G.Store(path)
        bundle = fixture_bundle()
        receipt = store.capture(bundle, event_id='feature-event', contribution_id='limit', shareability='project')
        M.git(self.root, 'worktree', 'remove', str(path))
        M.git(self.root, 'gc', '--prune=now')
        other = G.Store(self.worktree('new-feature'))
        self.assertEqual(other.read_bundle(receipt['revision']), bundle)

    def test_idempotence_conflicting_replay_and_versions(self):
        first = self.capture()
        self.assertTrue(self.capture()['replay'])
        self.assertEqual(first['commit'], self.store.head())
        for args in ({'bundle': fixture_bundle(value=20)}, {'contribution_id': 'different'}):
            with self.subTest(args=args), self.assertRaises(ValueError):
                self.capture(**args)
        second = self.capture(fixture_bundle(value=20), event_id='event-2')
        self.assertNotEqual(first['revision'], second['revision'])
        self.assertEqual(self.store.read_bundle(first['revision']), fixture_bundle())
        self.assertEqual(len(self.store.events()), 2)

    def test_real_multiprocess_captures_never_lose_independent_or_duplicate_events(self):
        context = multiprocessing.get_context('spawn')
        for same in (False, True):
            jobs = [context.Process(target=capture_process, args=(str(self.root), n, same)) for n in range(6)]
            for job in jobs:
                job.start()
            for job in jobs:
                job.join(30)
                if job.is_alive():
                    job.terminate()
                self.assertEqual(job.exitcode, 0)
        self.assertEqual(len(self.store.events()), 7)
        self.assertEqual(len(self.store.snapshot()['bundles']), 7)

    def test_real_multiprocess_conflicting_event_has_one_winner(self):
        context = multiprocessing.get_context('spawn')
        output = context.Queue()
        jobs = [context.Process(target=conflicting_capture_process, args=(str(self.root), value, output))
                for value in (10, 20)]
        for job in jobs:
            job.start()
        for job in jobs:
            job.join(30)
            if job.is_alive():
                job.terminate()
            self.assertEqual(job.exitcode, 0)
        results = [output.get(timeout=2), output.get(timeout=2)]
        output.close()
        self.assertEqual(sorted(state for state, _ in results), ['captured', 'refused'])
        snapshot = self.store.snapshot()
        self.assertEqual(len(snapshot['events']), 1)
        self.assertEqual(snapshot['events'][0]['revision'], next(value for state, value in results if state == 'captured'))

    def test_crash_before_ref_never_acknowledges_and_retry_recovers(self):
        with patch.object(self.store, '_cas', side_effect=KeyboardInterrupt):
            with self.assertRaises(KeyboardInterrupt):
                self.capture()
        self.assertIsNone(self.store.head())
        self.assertEqual(self.store.events(), [])
        self.assertEqual(self.capture()['state'], 'captured')

    def test_crash_after_ref_replays_without_second_commit(self):
        original = self.store._cas
        def crash(old, new):
            self.assertTrue(original(old, new))
            raise KeyboardInterrupt()
        with patch.object(self.store, '_cas', side_effect=crash):
            with self.assertRaises(KeyboardInterrupt):
                self.capture()
        head = self.store.head()
        self.assertIsNotNone(head)
        self.assertTrue(self.capture()['replay'])
        self.assertEqual(self.store.head(), head)

    def test_compare_and_swap_rejects_a_stale_writer(self):
        old = self.capture()['commit']
        new = self.capture(event_id='new')['commit']
        self.assertFalse(self.store._cas(old, old))
        self.assertEqual(self.store.head(), new)

    def test_cas_retry_preserves_an_intervening_ref_update(self):
        original = self.store._cas
        once = []
        def race(old, new):
            if not once:
                once.append(True)
                tree = self.store.tree(old)
                tree['recovery/other.txt'] = self.store._write_blob(b'Other ref owner\n')
                intervening = self.store._commit(tree, old)
                self.assertTrue(original(old, intervening))
                return False
            return original(old, new)
        with patch.object(self.store, '_cas', side_effect=race):
            self.capture()
        self.assertIn('recovery/other.txt', self.store.tree())
        self.assertEqual(len(self.store.events()), 1)

    def test_private_or_unknown_cannot_write_even_unreachable_git_objects(self):
        before = M.git(self.root, 'count-objects', '-v').stdout
        for permission in (None, 'unknown', 'private'):
            with self.subTest(permission=permission), self.assertRaises(ValueError):
                self.capture(shareability=permission)
        bundle = fixture_bundle()
        bundle['manifest']['document']['known']['api.limit']['private'] = True
        with self.assertRaises(ValueError):
            self.capture(bundle)
        self.assertEqual(M.git(self.root, 'count-objects', '-v').stdout, before)
        self.assertIsNone(self.store.head())

    def test_failure_during_object_write_never_acknowledges(self):
        before = self.store._write_blob
        calls = []
        def crash(data):
            calls.append(data)
            if len(calls) == 2:
                raise KeyboardInterrupt()
            return before(data)
        with patch.object(self.store, '_write_blob', side_effect=crash):
            with self.assertRaises(KeyboardInterrupt):
                self.capture()
        self.assertIsNone(self.store.head())
        self.assertEqual(self.capture()['state'], 'captured')

    def test_mutated_bundle_fails_before_git_objects(self):
        bundle = fixture_bundle()
        bundle['files']['evidence/vendor.txt'] = b'Unexpected source'
        with self.assertRaises(ValueError):
            self.capture(bundle)
        self.assertIsNone(self.store.head())

    def test_committed_snapshot_does_not_drift_with_new_arrivals(self):
        first = self.capture()
        self.capture(fixture_bundle(value=20), event_id='later')
        self.assertEqual(len(self.store.snapshot(first['commit'])['events']), 1)
        self.assertEqual(len(self.store.snapshot()['events']), 2)

    def test_replay_identifies_original_capture_and_events_follow_commit_order(self):
        first = self.capture(event_id='z-first')
        second = self.capture(event_id='a-second')
        replay = self.capture(event_id='z-first')
        self.assertEqual(replay['commit'], first['commit'])
        self.assertEqual(replay['ledger_commit'], second['commit'])
        self.assertEqual([e['event_id'] for e in self.store.events()], ['z-first', 'a-second'])

    def test_stored_bundle_read_does_not_reextract_closure(self):
        bundle = fixture_bundle()
        receipt = self.capture(bundle)
        with patch.object(G, 'closure', side_effect=ValueError('future closure semantics')):
            self.assertEqual(self.store.read_bundle(receipt['revision']), bundle)

    def test_git_signing_configuration_cannot_block_capture(self):
        M.git(self.root, 'config', 'commit.gpgsign', 'true')
        M.git(self.root, 'config', 'gpg.program', '/nonexistent/signing-program')
        receipt = self.capture()
        self.assertNotIn(b'gpgsig', M.git(self.root, 'cat-file', '-p', receipt['commit']).stdout)

    def test_tree_builder_refuses_both_orders_of_path_collision(self):
        oid = self.store._write_blob(b'x')
        for paths in ({'x': oid, 'x/y': oid}, {'x/y': oid, 'x': oid}):
            with self.subTest(paths=list(paths)), self.assertRaisesRegex(ValueError, 'collision'):
                self.store._write_tree(paths)

    def test_evidence_filename_does_not_become_a_privacy_policy(self):
        bundle = G.prepare({'sources': {'s.example': {'file': 'shareability',
                           'scope': {'kind': 'project', 'environment': 'all'}}}}, ['s.example'],
                           scope={'kind': 'project', 'environment': 'all'}, shareability='project',
                           evidence={'shareability': b'public evidence'})
        receipt = self.capture(bundle)
        self.assertEqual(self.store.read_bundle(receipt['revision']), bundle)
        self.assertEqual(len(self.store.snapshot()['events']), 1)
        self.assertTrue(self.capture(bundle)['replay'])


class ModeTests(Repository):
    def test_new_git_and_legacy_defaults_are_shared_and_not_written_by_reads(self):
        self.assertEqual(M.Project(self.root).config()['mode'], 'advanced')
        self.assertFalse(M.Project(self.root).config_path.exists())
        (self.root / 'GROUNDING.yaml').write_text('known: {api.limit: {v: 1}}\n')
        self.assertEqual(M.Project(self.worktree()).config()['mode'], 'advanced')

    def test_only_two_modes_and_simple_retains_parallel_named_hypotheses(self):
        self.assertEqual(M.MODES, ('simple', 'advanced'))
        root = self.base / 'plain'
        root.mkdir()
        (root / 'GROUNDING.yaml').write_text('known: {api.limit: {v: 1}}\n')
        directory = root / '.kpopper/hypotheses'
        directory.mkdir(parents=True)
        for name, value in [('proposal-a', 2), ('proposal-b', 3)]:
            (directory / (name + '.yaml')).write_text('known: {api.limit: {v: %d}}\n' % value)
        project = M.Project(root)
        project.configure()
        doc = G.P.load([str(project.record())])
        self.assertEqual(set(doc.hypotheses), {'proposal-a', 'proposal-b'})
        self.assertIn('api.limit', G.P.contested(doc))
        self.assertEqual(doc['known']['api.limit']['v'], 1)

    def test_simple_project_is_one_shared_ancestor_record(self):
        root = self.base / 'plain'
        root.mkdir()
        (root / 'GROUNDING.yaml').write_text('known: {api.limit: {v: 1}}\n')
        child = root / 'notes'
        child.mkdir()
        project = M.Project(child)
        self.assertEqual(project.record(), root / 'GROUNDING.yaml')
        self.assertEqual(project.config()['mode'], 'simple')
        project.configure()
        self.assertEqual(M.Project(root).config_path, project.config_path)

    def test_explicit_transition_preserves_bytes_and_rollback_evidence(self):
        (self.root / 'GROUNDING.yaml').write_text('known: {api.limit: {v: 1}}\n')
        project = M.Project(self.root)
        before = project.record().read_bytes()
        shared = self.base / 'shared.yaml'
        shared.write_bytes(before)
        self.assertEqual(project.configure('simple', record=str(shared))['mode'], 'simple')
        self.assertEqual(project.record().read_bytes(), before)
        history = list((project.state / 'mode-history').glob('*.json'))
        self.assertEqual(len(history), 1)
        self.assertEqual(base64.b64decode(M.I._load(history[0])['records'][str(self.root / 'GROUNDING.yaml')]), before)

    def test_rollback_retains_crlf_bytes_exactly(self):
        before = b'known:\r\n  api.limit: {v: 1}\r\n'
        (self.root / 'GROUNDING.yaml').write_bytes(before)
        shared = self.base / 'shared.yaml'
        shared.write_bytes(before)
        project = M.Project(self.root)
        project.configure('simple', record=str(shared))
        history = next((project.state / 'mode-history').glob('*.json'))
        self.assertEqual(base64.b64decode(M.I._load(history)['records'][str(self.root / 'GROUNDING.yaml')]), before)

    def test_registered_shared_record_defaults_simple_even_when_missing(self):
        project = M.Project(self.root)
        shared = self.base / 'external.yaml'
        (project.common / 'kpopper-record').write_text(str(shared) + '\n')
        self.assertEqual(project.config()['mode'], 'simple')
        project.configure()
        self.assertEqual(project.record(), shared)
        self.assertFalse(shared.exists())
        with self.assertRaisesRegex(ValueError, 'Simple uses'):
            self.capture()

    def test_registered_pointer_cannot_silently_replace_an_existing_branch_record(self):
        project = M.Project(self.root)
        (self.root / 'GROUNDING.yaml').write_text('known: {api.limit: {v: 1}}\n')
        shared = self.base / 'external.yaml'
        shared.write_text('known: {api.limit: {v: 2}}\n')
        (project.common / 'kpopper-record').write_text(str(shared) + '\n')
        with self.assertRaisesRegex(ValueError, 'reconcile'):
            project.config()

    def test_transition_refuses_different_destination_or_multifile_graph(self):
        record = self.root / 'GROUNDING.yaml'
        record.write_text('known: {api.limit: {v: 1}}\n')
        shared = self.base / 'shared.yaml'
        shared.write_text('known: {api.limit: {v: 2}}\n')
        project = M.Project(self.root)
        with self.assertRaisesRegex(ValueError, 'destination record differs'):
            project.configure('simple', record=str(shared))

        record.write_text('also: other.yaml\n')
        shared.write_bytes(record.read_bytes())
        with self.assertRaisesRegex(ValueError, 'multi-file record'):
            project.configure('simple', record=str(shared))

    def test_all_worktree_destinations_are_checked_and_backed_up(self):
        data = b'known: {api.limit: {v: 1}}\n'
        (self.root / 'GROUNDING.yaml').write_bytes(data)
        other = self.worktree()
        (other / 'GROUNDING.yaml').write_bytes(data)
        (self.root / 'notes.yaml').write_bytes(data)
        project = M.Project(self.root)
        with self.assertRaisesRegex(ValueError, 'destination record is unavailable'):
            project.configure('advanced', record='notes.yaml')
        (other / 'notes.yaml').write_text('known: {api.limit: {v: 999}}\n')
        with self.assertRaisesRegex(ValueError, 'destination record differs'):
            project.configure('advanced', record='notes.yaml')
        (other / 'notes.yaml').write_bytes(data)
        project.configure('advanced', record='notes.yaml')
        backup = M.I._load(next((project.state / 'mode-history').glob('*.json')))
        self.assertEqual(set(backup['records']), {str(root / name) for root in (self.root, other)
                                                  for name in ('GROUNDING.yaml', 'notes.yaml')})

    def test_mode_transition_waits_for_record_writer_before_checking_equality(self):
        record = self.root / 'GROUNDING.yaml'
        record.write_text('known: {api.limit: {v: 1}}\n')
        shared = self.base / 'shared.yaml'
        shared.write_bytes(record.read_bytes())
        context = multiprocessing.get_context('spawn')
        started, output = context.Event(), context.Queue()
        with G.P._locked(str(record)):
            job = context.Process(target=configure_process, args=(str(self.root), str(shared), started, output))
            job.start()
            self.assertTrue(started.wait(10))
            with self.assertRaises(queue.Empty):
                output.get(timeout=0.5)
            record.write_text('known: {api.limit: {v: 2}}\n')
        job.join(20)
        if job.is_alive():
            job.terminate()
        self.assertEqual(job.exitcode, 0)
        state, reason = output.get(timeout=2)
        output.close()
        self.assertEqual(state, 'refused')
        self.assertIn('destination record differs', reason)

    def test_escaping_registration_is_refused_before_first_capture_or_config(self):
        project = M.Project(self.root)
        pointer = project.common / 'kpopper-record'
        pointer.write_text('../shared.yaml\n')
        with self.assertRaisesRegex(ValueError, 'reconcile registration'):
            self.capture()
        self.assertFalse(project.config_path.exists())
        self.assertIsNone(self.store.head())
        self.assertEqual(pointer.read_text(), '../shared.yaml\n')

    def test_yml_hypothesis_blocks_actual_mode_transition(self):
        (self.root / 'GROUNDING.yaml').write_text('known: {api.limit: {v: 1}}\n')
        shared = self.base / 'shared.yaml'
        shared.write_bytes((self.root / 'GROUNDING.yaml').read_bytes())
        hyp = self.root / '.kpopper/hypotheses'
        hyp.mkdir(parents=True)
        (hyp / 'test.yml').write_text('known: {api.limit: {v: 2}}')
        with self.assertRaisesRegex(ValueError, 'hypotheses'):
            M.Project(self.root).configure('simple', record=str(shared))

    def test_transition_refuses_divergence_hypotheses_and_pending_work(self):
        (self.root / 'GROUNDING.yaml').write_text('known: {api.limit: {v: 1}}\n')
        path = self.worktree()
        project = M.Project(self.root)
        shared = self.base / 'shared.yaml'
        shared.write_bytes((self.root / 'GROUNDING.yaml').read_bytes())
        self.assertTrue(any('different' in x for x in project.transition_report('simple', str(shared))['blockers']))
        (path / 'GROUNDING.yaml').write_bytes((self.root / 'GROUNDING.yaml').read_bytes())
        hyp = path / '.kpopper/hypotheses'
        hyp.mkdir(parents=True)
        (hyp / 'test.yaml').write_text('known: {api.limit: {v: 2}}')
        with self.assertRaisesRegex(ValueError, 'hypotheses'):
            project.configure('simple', record=str(shared))
        (hyp / 'test.yaml').unlink()
        self.capture()
        with self.assertRaisesRegex(ValueError, 'durable contributions'):
            project.configure('simple', record=str(shared))

    def test_detached_and_moved_worktree_keep_policy(self):
        project = M.Project(self.root)
        project.configure('advanced')
        path = self.worktree()
        M.git(path, 'checkout', '--detach')
        moved = self.base / 'moved'
        M.git(self.root, 'worktree', 'move', str(path), str(moved))
        self.assertEqual(M.Project(moved).config(), project.config())

    def test_custom_record_path_is_explicit_and_missing_path_is_not_recreated(self):
        custom = self.root / 'notes.yaml'
        custom.write_text('known: {api.limit: {v: 1}}\n')
        project = M.Project(self.root)
        project.configure('advanced', record='notes.yaml')
        self.assertEqual(project.record(), custom)
        custom.unlink()
        self.assertEqual(project.record(), custom)
        self.assertFalse(custom.exists())

    def test_publication_config_resolves_exact_remote_target_and_permission(self):
        bare = self.base / 'remote.git'
        M.git(self.root, 'init', '--bare', str(bare))
        M.git(self.root, 'remote', 'add', 'team', str(bare))
        project = M.Project(self.root)
        scope = project.publication_config('team', 'trunk')
        self.assertEqual(scope['repository'], str(bare))
        self.assertEqual(scope['branch'], 'pending_grounding')
        self.assertFalse(scope['standing_permission'])
        self.assertEqual(M.git(self.root, 'ls-remote', 'team').stdout, b'')
        project.publication_config('team', 'trunk', grant=True)
        self.assertTrue(project.config()['publication']['standing_permission'])

    def test_malformed_config_never_infers_authority(self):
        project = M.Project(self.root)
        config = project.configure()
        config['publication'] = {'remote': 'team', 'repository': 'example', 'target': 'trunk',
                                 'branch': 'pending_grounding', 'standing_permission': 'false'}
        M.I._save(project.config_path, config)
        with self.assertRaisesRegex(ValueError, 'no authority'):
            project.config()


if __name__ == '__main__':
    unittest.main()
