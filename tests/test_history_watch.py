import copy
import datetime
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest import mock

from scripts import history_authoring as A
from scripts import history_adapter
from scripts import history_contract as C
from scripts import history_hypotheses as HH
from scripts import history_store as H
from scripts import history_watch as W
from scripts import knowledge_views as V
from scripts import watch
from scripts.reasoning.snapshot import Snapshot
from tests import test_history_store as fixtures


class HistoryWatchTests(unittest.TestCase):
    def setUp(self):
        self.fixture = fixtures.Storage()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        claim = fixtures.claim()
        claim['authored']['fields'] = {'deps': 'rests_on', 'snapshot': 'seen', 'predicate': 'wrong_if'}
        claim['id'] = C.object_identity(claim)
        self.fixture.publish([C.validate_object(claim)], op='bootstrap')
        self.before = self.fixture.store.capture()

    def rec(self, captured):
        adapted = history_adapter.from_store_capture(captured)
        data = adapted.snapshot(as_of=None).to_data()
        return {'doc': data['document'], 'hypotheses': [
                    {'name': name, 'doc': item['document'], 'head': item['head'],
                     **({'kind': item['kind']} if item.get('kind') else {})}
                    for name, item in data['hypotheses'].items()],
                'history': V.history_evidence(captured), 'snapshot': data}

    def snap(self, ancestor, main, working):
        return {'ancestor': self.rec(ancestor), 'main': self.rec(main), 'working': self.rec(working),
                'versions': {}, 'identity': 'fixture'}

    def physical(self, record, name, document, head):
        record = copy.deepcopy(record)
        captured = W._capture(record['history'])
        hypotheses = {item['name']: {'doc': item['doc'], 'head': item['head'],
                                      **({'kind': item['kind']} if item.get('kind') else {})}
                      for item in record['hypotheses']}
        hypotheses[name] = {'doc': document, 'head': head}
        data = history_adapter.from_store_capture(captured).snapshot(
            hypotheses=hypotheses, as_of=record['snapshot']['as_of']).to_data()
        record['doc'] = data['document']
        record['snapshot'] = data
        record['hypotheses'] = [{'name': key, 'doc': item['document'], 'head': item['head'],
                                 **({'kind': item['kind']} if item.get('kind') else {})}
                                for key, item in data['hypotheses'].items()]
        return record

    def core_layer(self, document):
        return {**document, 'meta': {'reasoning': copy.deepcopy(
            self.before.document['meta']['reasoning'])}}

    def test_unchanged_history_is_clear(self):
        result = watch.compare(self.snap(self.before, self.before, self.before))
        self.assertEqual(result['state'], 'clear')

    def test_newer_main_is_retained_when_local_is_unchanged(self):
        mutation = A.prepare(self.fixture.entry, {'kind': 'set', 'id': 'p.input', 'value': 2,
                                                   'as_of': '2026-09-18'}, operation='main-change')
        self.fixture.store.commit(mutation, verify=lambda data: None)
        after = self.fixture.store.capture()
        merged = W.merged_snapshot(self.snap(self.before, after, self.before))
        self.assertEqual(merged.to_data()['document']['readings']['p.input']['v'], 2)
        self.assertEqual(W.compare(self.snap(self.before, after, self.before))['state'], 'clear')

    def test_missing_ancestor_commit_refuses_without_reads(self):
        mutation = A.prepare(self.fixture.entry, {'kind': 'set', 'id': 'p.input', 'value': 2,
                                                   'as_of': '2026-09-18'}, operation='main-change')
        self.fixture.store.commit(mutation, verify=lambda data: None)
        after = self.fixture.store.capture()
        bad = self.snap(after, after, self.before)
        result = W.compare(bad)
        self.assertEqual(result['state'], 'attention')
        self.assertIn('ancestor', result['findings'][0]['reason'])

    def branch(self, name):
        target = Path(self.fixture.temp.name).with_name(Path(self.fixture.temp.name).name + '-' + name)
        shutil.copytree(self.fixture.entry.parent, target)
        self.addCleanup(shutil.rmtree, target, True)
        return H.Store(target / self.fixture.entry.name)

    def commit(self, store, action, operation):
        mutation = A.prepare(store.entry, action, operation=operation,
                             recorded_at='2026-09-19T00:00:00+00:00')
        store.commit(mutation, verify=lambda data: None)
        return store.capture()

    def test_disjoint_changes_union_and_report_only_worktree_delta(self):
        main_store, work_store = self.branch('main'), self.branch('work')
        main = self.commit(main_store, {'kind': 'add', 'id': 'p.main', 'body': {'v': 2},
                                       'into': 'readings'}, 'main-add')
        working = self.commit(work_store, {'kind': 'add', 'id': 'p.local', 'body': {'v': 3},
                                           'into': 'readings'}, 'local-add')
        snap = self.snap(self.before, main, working)
        merged = W.merged_snapshot(snap).to_data()
        self.assertEqual(merged['document']['readings']['p.main']['v'], 2)
        self.assertEqual(merged['document']['readings']['p.local']['v'], 3)
        self.assertEqual(W.compare(snap)['changed'], ['p.local'])

    def test_concurrent_same_subject_disagreement_is_attention(self):
        main = self.commit(self.branch('main-conflict'),
                           {'kind': 'set', 'id': 'p.input', 'value': 2}, 'main-set')
        working = self.commit(self.branch('work-conflict'),
                              {'kind': 'set', 'id': 'p.input', 'value': 3}, 'work-set')
        result = W.compare(self.snap(self.before, main, working))
        self.assertEqual(result['state'], 'attention')
        self.assertEqual(result['changed'], ['p.input'])
        self.assertTrue(any('contested' in item['reason'] for item in result['findings']))

    def test_refutation_is_retained_without_synthetic_replacement(self):
        store = self.branch('refute')
        target = self.before.state['subjects']['p.input']['head']
        mutation = A.prepare_act(store.entry, {'kind': 'refute', 'id': 'p.input', 'of': target,
                                               'over': [], 'because': 'source disproved'},
                                 operation='local-refute', recorded_at='2026-09-19T00:00:00+00:00')
        store.commit(mutation, verify=lambda data: None)
        working = store.capture()
        snap = self.snap(self.before, self.before, working)
        merged = W.merged_snapshot(snap).to_data()
        self.assertNotIn('p.input', merged['document']['readings'])
        self.assertEqual(W.compare(snap)['changed'], ['p.input'])

    def test_named_layers_regenerate_and_physical_layers_merge_as_delta(self):
        named_store = self.branch('named-main')
        mutation = HH.prepare(named_store.entry, 'future',
                              {'kind': 'set', 'id': 'p.input', 'value': 4},
                              head={'claim': 'future', 'folds': 'never'}, operation='named-main')
        named_store.commit(mutation, verify=lambda data: None)
        snap = self.snap(self.before, named_store.capture(), self.before)
        merged = W.merged_snapshot(snap).to_data()
        self.assertEqual(merged['hypotheses']['future']['document']['readings']['p.input']['v'], 4)

        physical = {'name': 'manual', 'doc': {'readings': {'p.manual': {'v': 9}}},
                    'head': {'claim': 'manual', 'folds': 'never'}}
        snap['working'] = self.physical(snap['working'], physical['name'], physical['doc'],
                                        physical['head'])
        merged = W.merged_snapshot(snap).to_data()
        self.assertEqual(merged['hypotheses']['manual']['document']['readings']['p.manual']['v'], 9)
        self.assertIn('p.manual', W.compare(snap)['changed'])

    def test_pure_compare_rejects_missing_closure_and_union_size(self):
        mutation = A.prepare(self.fixture.entry, {'kind': 'set', 'id': 'p.input', 'value': 2},
                             operation='later')
        self.fixture.store.commit(mutation, verify=lambda data: None)
        after = self.fixture.store.capture()
        snap = self.snap(self.before, after, self.before)
        before = self.fixture.entry.read_bytes()
        original_read = Path.read_bytes

        def no_source_read(path):
            if path == self.fixture.entry or self.fixture.entry.parent in path.parents:
                raise AssertionError('source read')
            return original_read(path)
        with mock.patch.object(H.Store, 'capture', side_effect=AssertionError('source capture')), \
             mock.patch.object(Path, 'read_bytes', no_source_read):
            self.assertEqual(W.compare(snap)['state'], 'clear')
        self.assertEqual(self.fixture.entry.read_bytes(), before)

        broken = copy.deepcopy(snap)
        commit_path = next(name for name in broken['main']['history']['files'] if 'commits/' in name)
        broken['main']['history']['files'].pop(commit_path)
        broken['main']['history']['sha256'].pop(commit_path)
        self.assertEqual(W.compare(broken)['state'], 'attention')
        with mock.patch.object(H, 'MAX_CAPTURE_BYTES', 1):
            result = W.compare(snap)
        self.assertEqual(result['state'], 'attention')
        self.assertIn('byte limit', result['findings'][0]['reason'])

    def test_shared_and_mixed_history_contexts_are_explicitly_incomplete(self):
        snap = self.snap(self.before, self.before, self.before)
        snap['shared'] = {'doc': {'known': {'external': {'v': 1}}}}
        result = watch.compare(snap)
        self.assertEqual(result['state'], 'attention')
        self.assertIn('shared facts', result['findings'][0]['reason'])
        snap = self.snap(self.before, self.before, self.before)
        del snap['working']['history']
        result = watch.compare(snap)
        self.assertEqual(result['state'], 'attention')
        self.assertIn('missing on one branch', result['findings'][0]['reason'])

    def test_tampered_public_snapshot_as_of_is_rejected(self):
        snap = self.snap(self.before, self.before, self.before)
        snap['main']['snapshot']['as_of'] = '2026-01-01'
        result = W.compare(snap)
        self.assertEqual(result['state'], 'attention')
        self.assertIn('snapshot', result['findings'][0]['reason'])

    def test_new_named_hypothesis_condition_is_never_silently_clear(self):
        store = self.branch('condition')
        mutation = HH.prepare(store.entry, 'future',
                              {'kind': 'set', 'id': 'p.input', 'value': 2},
                              head={'claim': 'future', 'wrong_if': {'expr': 'p.input > 0'}},
                              operation='condition')
        store.commit(mutation, verify=lambda data: None)
        result = W.compare(self.snap(self.before, self.before, store.capture()))
        self.assertEqual(result['state'], 'attention')
        self.assertTrue(any(item['id'] == 'future' and item['kind'] == 'falsified'
                            for item in result['findings']))

    def test_safe_named_what_if_is_assessed_without_folding(self):
        store = self.branch('safe-condition')
        mutation = HH.prepare(store.entry, 'future',
                              {'kind': 'set', 'id': 'p.input', 'value': 2},
                              head={'claim': 'future', 'folds': 'never',
                                    'wrong_if': {'expr': 'p.input > 5'}},
                              operation='safe-condition')
        store.commit(mutation, verify=lambda data: None)
        source = store.capture()
        result = W.compare(self.snap(self.before, self.before, source))
        self.assertEqual(result['state'], 'clear', result)
        self.assertEqual(store.capture().baseline, source.baseline)

    def test_safe_physical_what_if_is_assessed_without_acceptance(self):
        snap = self.snap(self.before, self.before, self.before)
        snap['working'] = self.physical(snap['working'], 'manual',
            self.core_layer({'readings': {'p.input': {'v': 2}}}),
            {'claim': 'manual', 'folds': 'never', 'wrong_if': {'expr': 'p.input > 5'}})
        result = W.compare(snap)
        self.assertEqual(result['state'], 'clear', result)

    def test_main_change_rechecks_standing_head_without_stale_alternative(self):
        mutation = HH.prepare(self.fixture.entry, 'future',
            {'kind': 'set', 'id': 'p.input', 'value': 0},
            head={'folds': 'never', 'wrong_if': {'expr': 'p.input > 2'}}, operation='standing')
        self.fixture.store.commit(mutation, verify=lambda data: None)
        ancestor = self.fixture.store.capture()
        main = self.commit(self.branch('standing-main'),
            {'kind': 'set', 'id': 'p.input', 'value': 3}, 'new-main')
        result = W.compare(self.snap(ancestor, main, ancestor))
        self.assertTrue(any(item['id'] == 'future' and item['kind'] == 'falsified'
                            for item in result['findings']), result)
        replay = Snapshot.from_json(result['scenarios'][0]['snapshot'])
        self.assertEqual(replay.to_data()['document']['readings']['p.input']['v'], 3)

    def test_new_main_hypothesis_keeps_its_own_authored_delta(self):
        store = self.branch('main-hypothesis')
        mutation = HH.prepare(store.entry, 'future',
            {'kind': 'set', 'id': 'p.input', 'value': 3},
            head={'folds': 'never', 'wrong_if': {'expr': 'p.input > 2'}}, operation='main-hypothesis')
        store.commit(mutation, verify=lambda data: None)
        result = W.compare(self.snap(self.before, store.capture(), self.before))
        self.assertTrue(any(item['id'] == 'future' and item['kind'] == 'falsified'
                            for item in result['findings']), result)
        self.assertEqual(result['changed'], [])

    def test_hypothetical_reading_can_falsify_a_committed_judgment(self):
        before = self.commit(self.fixture.store, {'kind': 'add', 'id': 'd.limit',
            'into': 'judgments', 'body': {'verdict': 'safe', 'rests_on': ['p.input'],
                'wrong_if': {'expr': 'p.input > 2'}}}, 'base-judgment')
        store = self.branch('judgment-hypothesis')
        mutation = HH.prepare(store.entry, 'future',
            {'kind': 'set', 'id': 'p.input', 'value': 3},
            head={'folds': 'never'}, operation='hypothetical-reading')
        store.commit(mutation, verify=lambda data: None)
        result = W.compare(self.snap(before, before, store.capture()))
        self.assertTrue(any(item['id'] == 'd.limit' and item['kind'] == 'falsified'
                            and item.get('perspective') == 'scenario'
                            for item in result['findings']), result)

    def test_captured_shared_history_is_computed_without_adoption(self):
        shared_store = self.branch('shared-context')
        shared = self.commit(shared_store, {'kind': 'add', 'id': 'p.shared',
            'into': 'readings', 'body': {'v': 3}}, 'shared-reading')
        snap = self.snap(self.before, self.before, self.before)
        snap['working'] = self.physical(snap['working'], 'manual',
            self.core_layer({'readings': {'p.manual': {'v': 1}}}),
            {'folds': 'never', 'wrong_if': {'expr': 'p.shared > 2'}})
        snap['shared'] = self.rec(shared)
        result = watch.compare(snap)
        self.assertTrue(any(item['id'] == 'manual' and item['kind'] == 'falsified'
                            for item in result['findings']), result)
        self.assertEqual(shared_store.capture().baseline, shared.baseline)
        self.assertNotIn('p.shared', self.fixture.store.capture().state['subjects'])

    def test_real_watch_process_persists_typed_date_scenario(self):
        root = self.fixture.entry.parent
        day = datetime.date(2026, 9, 1)
        self.commit(self.fixture.store, {'kind': 'add', 'id': 'p.dated', 'into': 'readings',
            'body': {'v': 1, 'sampled_on': day}}, 'dated-source')
        subprocess.run(['git', 'init', '-b', 'main'], cwd=root, check=True, capture_output=True)
        subprocess.run(['git', 'add', '.'], cwd=root, check=True, capture_output=True)
        subprocess.run(['git', '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.test',
                        'commit', '-m', 'base'], cwd=root, check=True, capture_output=True)
        mutation = HH.prepare(self.fixture.entry, 'future',
            {'kind': 'set', 'id': 'p.input', 'value': 2},
            head={'folds': 'never', 'wrong_if': {'expr': 'p.input > 5'}}, operation='dated-scenario')
        self.fixture.store.commit(mutation, verify=lambda data: None)
        with tempfile.TemporaryDirectory() as state, mock.patch.dict(os.environ, {'XDG_STATE_HOME': state}):
            observed = watch.Watch(root)
            observed.setup(base_ref='refs/heads/main')
            result = observed.process()
            self.assertEqual(result['state'], 'clear', result)
            retained = json.loads((observed.state / 'result.json').read_text())
            replay = Snapshot.from_json(retained['scenarios'][0]['snapshot'])
            self.assertEqual(replay.to_data()['document']['readings']['p.dated']['sampled_on'], day)

    def test_new_executable_error_is_uncheckable(self):
        judgment = fixtures.claim('d.broken', kind='judgment', op='broken', body={
            'verdict': 'bad', 'rests_on': [], 'wrong_if': {'expr': '1 / 0 > 0'}})
        judgment['authored']['fields'] = {
            'deps': 'rests_on', 'snapshot': 'seen', 'predicate': 'wrong_if'}
        judgment['id'] = C.object_identity(judgment)
        self.fixture.publish([C.validate_object(judgment)], op='broken')
        result = W.compare(self.snap(self.before, self.before, self.fixture.store.capture()))
        self.assertEqual(result['state'], 'attention')
        self.assertTrue(any(item['id'] == 'd.broken' and item['kind'] == 'uncheckable'
                            for item in result['findings']))

    def test_real_watch_snapshot_and_worker_keep_source_bytes(self):
        root = self.fixture.entry.parent
        subprocess.run(['git', 'init', '-b', 'main'], cwd=root, check=True, capture_output=True)
        subprocess.run(['git', 'config', 'user.email', 'fixture@example.test'], cwd=root, check=True)
        subprocess.run(['git', 'config', 'user.name', 'Fixture'], cwd=root, check=True)
        subprocess.run(['git', 'add', '.'], cwd=root, check=True)
        subprocess.run(['git', 'commit', '-m', 'initial'], cwd=root, check=True, capture_output=True)
        work = root.parent / (root.name + '-git-work')
        self.addCleanup(shutil.rmtree, work, True)
        subprocess.run(['git', 'worktree', 'add', '-b', 'watch-work', str(work)], cwd=root,
                       check=True, capture_output=True)
        state = tempfile.TemporaryDirectory()
        self.addCleanup(state.cleanup)
        with mock.patch.dict(os.environ, {'XDG_STATE_HOME': state.name}):
            observed = watch.Watch(work)
            observed.setup(base_ref='refs/heads/main')
            before = {path.relative_to(work): path.read_bytes() for path in work.rglob('*') if path.is_file()}
            self.assertFalse(watch.P._CORE_READS.get())
            snapshot = observed.snapshot()
            result = observed.process()
            self.assertFalse(watch.P._CORE_READS.get())
            after = {path.relative_to(work): path.read_bytes() for path in work.rglob('*') if path.is_file()}
        self.assertEqual(result['state'], 'clear')
        self.assertIsNone(snapshot['working']['snapshot']['as_of'])
        self.assertEqual(before, after)

        main_store, work_store = H.Store(self.fixture.entry), H.Store(work / self.fixture.entry.name)
        self.commit(main_store, {'kind': 'set', 'id': 'p.input', 'value': 2}, 'git-main-set')
        subprocess.run(['git', 'add', '.'], cwd=root, check=True)
        subprocess.run(['git', 'commit', '-m', 'main change'], cwd=root, check=True, capture_output=True)
        self.commit(work_store, {'kind': 'set', 'id': 'p.input', 'value': 3}, 'git-work-set')
        subprocess.run(['git', 'add', '.'], cwd=work, check=True)
        subprocess.run(['git', 'commit', '-m', 'work change'], cwd=work, check=True, capture_output=True)
        with mock.patch.dict(os.environ, {'XDG_STATE_HOME': state.name}):
            conflict = observed.process()
        self.assertEqual(conflict['state'], 'attention')
        self.assertTrue(conflict['findings'])
        self.assertTrue(all(item.get('fingerprint') for item in conflict['findings']))


if __name__ == '__main__':
    unittest.main()
