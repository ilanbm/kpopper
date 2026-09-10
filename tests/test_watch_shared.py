"""One explicit, sourced shared observation must survive worktree disposal and races."""
import copy
from pathlib import Path
from unittest.mock import patch
import unittest
from tests import test_watch as fixture
from scripts import watch as W, watch_shared as S, watch_hook as H


class SharedTests(fixture.WatchFixture, unittest.TestCase):
    def setUp(self):
        super().setUp()
        self.watch.setup(shared_private=True)
        self.report = {'id': 'vendor.limit', 'name': 'Vendor limit', 'value': 10, 'date': '2026-09-09',
            'scope': {'kind': 'external', 'environment': 'production API v2'},
            'source': {'url': 'https://example.test/reference', 'at': 'Limits table'},
            'source_quote': 'The limit is 10.', 'event_id': 'limit-1'}

    def capture(self, report=None):
        with patch.object(W, 'launch'):
            return S.capture(self.watch, report or self.report)

    def test_new_shared_fact_has_source_and_is_available_from_main(self):
        before = [(p / 'PROVENANCE.yaml').read_bytes() for p in (self.main, self.work)]
        self.capture()
        self.assertNotIn('vendor.limit', S.read(self.watch)['entries'])
        self.watch.process()
        other = W.Watch(self.main)
        fact = S.read(other)['entries']['vendor.limit']
        self.assertEqual(fact['v'], 10)
        self.assertEqual(fact['scope'], self.report['scope'])
        self.assertIn('s.shared_limit_1', S.read(other)['entries'])
        self.assertEqual(before, [(p / 'PROVENANCE.yaml').read_bytes() for p in (self.main, self.work)])

    def test_scope_is_never_inferred_or_changed(self):
        for scope in (None, {'kind': 'branch', 'environment': 'experiment'}, {'kind': 'external'}):
            with self.assertRaises(ValueError):
                self.capture({**self.report, 'scope': scope})
        self.capture()
        self.watch.process()
        self.capture({**self.report, 'event_id': 'limit-2', 'scope': {'kind': 'external', 'environment': 'staging'}})
        self.watch.process()
        self.assertEqual(S.read(self.watch)['entries']['vendor.limit']['scope'], self.report['scope'])
        self.assertTrue(any(r['state'] == 'needs_review' for r in S.read(self.watch)['reports']))

    def test_same_day_conflict_is_retained_but_newer_reading_updates(self):
        self.capture()
        self.watch.process()
        self.capture({**self.report, 'event_id': 'conflict', 'value': 20})
        self.watch.process()
        self.assertEqual(S.read(self.watch)['entries']['vendor.limit']['v'], 10)
        self.assertTrue(any(r['state'] == 'needs_review' for r in S.read(self.watch)['reports']))
        self.capture({**self.report, 'event_id': 'newer', 'value': 20, 'date': '2026-09-10'})
        self.watch.process()
        self.assertEqual(S.read(self.watch)['entries']['vendor.limit']['v'], 20)

    def test_two_captures_do_not_lose_independent_facts(self):
        self.capture()
        self.capture({**self.report, 'event_id': 'second', 'id': 'vendor.other', 'value': 99})
        self.watch.process()
        values = S.read(self.watch)['entries']
        self.assertEqual((values['vendor.limit']['v'], values['vendor.other']['v']), (10, 99))

    def test_event_retry_and_tampered_identity(self):
        a = self.capture()
        self.assertEqual(self.capture()['event_id'], a['event_id'])
        with self.assertRaises(ValueError):
            self.capture({**self.report, 'value': 20})
        self.watch.process()
        rec, _ = S.layout(self.watch)
        before = rec.read_bytes()
        self.watch.process()
        self.assertEqual(rec.read_bytes(), before)

    def test_background_hook_ignores_children_and_unconfigured_projects(self):
        with patch.object(W, 'launch'):
            self.assertEqual(H.handle({'cwd': str(self.work), 'session_id': 'one', 'agent_id': 'child'}, 'codex', 'wait'), ('', '', 0))
            self.changed()
            self.watch.process()
            out, err, code = H.handle({'cwd': str(self.work), 'session_id': 'one'}, 'codex', 'wait', 0)
            self.assertIn('KPOPPER_WATCH', out)
            self.assertEqual((err, code), ('', 0))
            self.assertEqual(H.handle({'cwd': str(self.work), 'session_id': 'one'}, 'codex', 'wait', 0), ('', '', 0))
            out, err, code = H.handle({'cwd': str(self.work), 'session_id': 'claude'}, 'claude', 'wait', 0)
            self.assertEqual(code, 2)
            self.assertIn('KPOPPER_WATCH', err)

    def test_shared_crash_after_replace_is_recovered_once(self):
        self.capture()
        real = W.I._replace_record
        def crash(record, data):
            real(record, data)
            raise KeyboardInterrupt('simulated process termination')
        with patch.object(W.I, '_replace_record', side_effect=crash):
            with self.assertRaises(KeyboardInterrupt):
                S.process(self.watch)
        rec, _ = S.layout(self.watch)
        before = rec.read_bytes()
        S.process(self.watch)
        self.assertEqual(rec.read_bytes(), before)
        self.assertTrue(S.read(self.watch)['reports'][0]['recovered'])

    def test_changed_target_and_corrupt_report_are_not_overwritten(self):
        self.capture()
        rec, root = S.layout(self.watch)
        event_path = next((root / 'events').glob('*.json'))
        event = W.I._load(event_path)
        event['report']['value'] = 999
        W.I._save(event_path, event)
        before = rec.read_bytes()
        S.process(self.watch)
        self.assertEqual(rec.read_bytes(), before)
        self.assertIn('integrity', S.read(self.watch)['reports'][0]['reason'])

    def test_shared_updates_flag_main_decisions_without_rewriting_seen(self):
        self.capture();self.watch.process()
        main = W.P.yaml.safe_load((self.main / 'PROVENANCE.yaml').read_text())
        main['judgments']['c.vendor'] = {'rests_on': ['vendor.limit', 'api.timeout'],
            'seen': {'vendor.limit': 10, 'api.timeout': 5}, 'verdict': 'Fits external limit',
            'wrong_if': 'api.timeout > vendor.limit'}
        self.save(self.main, main);self.commit(self.main)
        self.capture({**self.report, 'event_id': 'reduced', 'value': 1, 'date': '2026-09-10'})
        self.watch.process()
        result = self.watch.status()
        self.assertEqual(result['state'], 'attention', result)
        self.assertTrue(any('c.vendor' in f['reason'] for f in result['findings']))
        current = W.P.yaml.safe_load((self.main / 'PROVENANCE.yaml').read_text())
        self.assertEqual(current['judgments']['c.vendor']['seen'], main['judgments']['c.vendor']['seen'])

    def test_pause_stops_writes_but_preserves_shared_reads(self):
        self.capture();self.watch.process()
        config=self.watch.config();config['enabled']=False
        W.I._save(self.watch.config_path,config)
        self.assertEqual(S.read(self.watch)['entries']['vendor.limit']['v'],10)
        with self.assertRaises(ValueError):
            self.capture({**self.report,'event_id':'paused','value':20})

    def test_resolving_a_report_does_not_apply_it_or_leave_a_stale_notice(self):
        self.capture();self.watch.process()
        self.capture({**self.report,'event_id':'conflict','value':20});self.watch.process()
        self.assertTrue(any(f['kind']=='shared_review' for f in self.watch.status()['findings']))
        with patch.object(W,'launch'):
            S.resolve(self.watch,'conflict','Inspected the source; the earlier reading was correct')
        self.watch.process()
        self.assertEqual(S.read(self.watch)['entries']['vendor.limit']['v'],10)
        self.assertFalse(self.watch.status()['findings'])

    def test_opening_names_shared_destination_without_loading_claims(self):
        self.capture();self.watch.process()
        with patch.object(W,'launch'), patch.object(W.Watch,'status',side_effect=AssertionError('opening must not compare')):
            out,_,_=H.handle({'cwd':str(self.main),'session_id':'main-reader'},'codex','start')
            self.assertIn('KPOPPER_WATCH_CONTEXT',out)
            self.assertIn('watch shared',out)
            self.assertNotIn('vendor.limit',out)

    def test_main_record_cannot_be_selected_as_shared_destination(self):
        # Use a fresh config to exercise destination validation, rather than relocation.
        config=self.watch.config();config['shared_record']=None
        W.I._save(self.watch.config_path,config)
        with self.assertRaisesRegex(ValueError,'checkouts'):
            self.watch.setup(shared_record=str(self.main/'PROVENANCE.yaml'))

    def test_existing_independent_notes_repository_can_be_the_shared_store(self):
        notes=self.root/'notes';notes.mkdir();self.git(notes,'init','-b','main')
        record=notes/'PROVENANCE.yaml';record.write_text('sources:\n  s.notes: {file: notes.md}\n')
        config=self.watch.config();config['shared_record']=None
        W.I._save(self.watch.config_path,config)
        self.watch.setup(shared_record=str(record))
        self.assertEqual(S.read(self.watch)['record'],str(record))
        self.capture();self.watch.process()
        self.assertEqual(S.read(self.watch)['entries']['vendor.limit']['v'],10)

    def test_cached_failure_never_becomes_clear_on_the_next_pass(self):
        self.doc['notes']={'api.timeout':{'v':7}}
        self.save(self.work,self.doc)
        self.assertEqual(self.watch.process()['state'],'unavailable')
        self.assertEqual(self.watch.process()['state'],'unavailable')
        self.assertEqual(self.watch.status()['state'],'unavailable')

    def test_transient_comparison_failure_is_retried(self):
        with patch.object(W,'compare',side_effect=ValueError('temporary read failure')):
            self.watch.process()
        self.assertEqual(self.watch.status()['state'],'unavailable')
        self.watch.process()
        self.assertEqual(self.watch.status()['state'],'clear')

    def test_setup_never_recreates_a_missing_pinned_shared_record(self):
        record,_=S.layout(self.watch)
        record.unlink();record.parent.chmod(0o755)
        for options in ({},{'shared_private':True}):
            with self.assertRaisesRegex(ValueError,'unavailable'):
                self.watch.setup(**options)
            self.assertFalse(record.exists())
            self.assertEqual(record.parent.stat().st_mode & 0o777,0o755)

    def test_review_follows_worktree_identity_across_subdirectories(self):
        self.capture();self.watch.process()
        self.capture({**self.report,'event_id':'conflict','value':20});self.watch.process()
        sub=self.work/'scripts';sub.mkdir()
        child=W.Watch(sub);child.process()
        self.assertTrue(any(f['kind']=='shared_review' for f in child.status()['findings']))
        with patch.object(W,'launch'):
            S.resolve(child,'conflict','Inspected original report')
        self.assertEqual(self.watch.status()['state'],'pending')
        child.process()
        self.assertEqual(self.watch.status()['state'],'clear')

    def test_concurrent_unrelated_write_survives_shared_commit(self):
        self.capture();self.watch.process()
        self.capture({**self.report,'event_id':'next','id':'vendor.next','value':77})
        record,_=S.layout(self.watch)
        original=W.I._save;injected=[]
        def race(path,value):
            original(path,value)
            if Path(path).parent.name=='journals' and not injected:
                injected.append(True)
                W.P.apply([str(record)],{'kind':'set','id':'vendor.limit','value':11,'as_of':'2026-09-10'})
        with patch.object(W.I,'_save',side_effect=race):S.process(self.watch)
        entries=S.read(self.watch)['entries']
        self.assertEqual((entries['vendor.limit']['v'],entries['vendor.next']['v']),(11,77))
        self.assertTrue(injected)

    def test_concurrent_target_write_is_retained_for_review(self):
        self.capture();self.watch.process()
        self.capture({**self.report,'event_id':'next','value':20,'date':'2026-09-10'})
        record,_=S.layout(self.watch)
        original=W.I._save;injected=[]
        def race(path,value):
            original(path,value)
            if Path(path).parent.name=='journals' and not injected:
                injected.append(True)
                W.P.apply([str(record)],{'kind':'set','id':'vendor.limit','value':11,'as_of':'2026-09-10'})
        with patch.object(W.I,'_save',side_effect=race):S.process(self.watch)
        self.assertEqual(S.read(self.watch)['entries']['vendor.limit']['v'],11)
        self.assertEqual(next(r for r in S.read(self.watch)['reports'] if r['id']=='next')['state'],'needs_review')
