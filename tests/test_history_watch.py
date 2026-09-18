import copy
import unittest

from scripts import history_authoring as A
from scripts import history_contract as C
from scripts import history_watch as W
from scripts import knowledge_views as V
from scripts import watch
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
        return {'doc': captured.document, 'hypotheses': [], 'history': V.history_evidence(captured)}

    def snap(self, ancestor, main, working):
        return {'ancestor': self.rec(ancestor), 'main': self.rec(main), 'working': self.rec(working),
                'versions': {}, 'identity': 'fixture'}

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


if __name__ == '__main__':
    unittest.main()
