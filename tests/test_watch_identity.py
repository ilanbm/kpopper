"""Shared identity includes provenance, even when a branch's value did not change."""
import copy
import unittest

from scripts import watch as W


class SharedIdentityTests(unittest.TestCase):
    def snapshot(self):
        base = {'schema': {'deps': 'rests_on', 'snapshot': 'seen', 'predicate': 'wrong_if'},
                'known': {'api.timeout': {'v': 5, 'of': '2026-09-01'}},
                'judgments': {'c.positive': {'rests_on': ['api.timeout'],
                                            'seen': {'api.timeout': 5}, 'verdict': 'Positive',
                                            'wrong_if': 'api.timeout < 0'}}}
        shared = {
            'sources': {'s.vendor': {'name': 'Vendor documentation',
                                    'url': 'https://example.test/limits', 'read': '2026-09-10'}},
            'known': {'vendor.limit': {'name': 'Vendor limit', 'v': 10, 'of': '2026-09-10',
                                       'from': 's.vendor', 'at': 'Limits table',
                                       'scope': {'kind': 'external', 'environment': 'production'}}},
        }
        return {**{key: {'doc': copy.deepcopy(base), 'hypotheses': []}
                   for key in ('ancestor', 'main', 'working')},
                'shared': {'doc': shared, 'hypotheses': []}, 'versions': {}, 'identity': 'fixture'}

    def collisions(self, snapshot, identity='vendor.limit'):
        return [f for f in W.compare(snapshot)['findings']
                if f['kind'] == 'collision' and f['id'] == identity]

    def test_main_shared_identity_includes_every_field_and_scalar_type(self):
        for field, value in [('name', 'Different name'), ('of', '2026-09-11'),
                             ('from', 's.different'), ('at', 'Another table'),
                             ('scope', {'kind': 'external', 'environment': 'staging'}),
                             ('v', 10.0), ('confidence', 'high')]:
            with self.subTest(field=field):
                snap = self.snapshot()
                fact = copy.deepcopy(snap['shared']['doc']['known']['vendor.limit'])
                fact[field] = value
                snap['main']['doc']['known']['vendor.limit'] = fact
                self.assertTrue(self.collisions(snap))

    def test_inherited_worktree_conflict_is_visible_without_authored_delta(self):
        snap = self.snapshot()
        for side in ('ancestor', 'working'):
            snap[side]['doc']['known']['vendor.limit'] = copy.deepcopy(
                snap['shared']['doc']['known']['vendor.limit'])
        snap['shared']['doc']['known']['vendor.limit']['v'] = 99
        self.assertTrue(self.collisions(snap))

    def test_identical_worktree_shared_entry_is_not_a_collision(self):
        snap = self.snapshot()
        snap['working']['doc']['known'].update(copy.deepcopy(snap['shared']['doc']['known']))
        self.assertEqual(W.compare(snap)['state'], 'clear')

    def test_shared_source_provenance_conflict_is_visible_on_both_sides(self):
        for side in ('main', 'working'):
            with self.subTest(side=side):
                snap = self.snapshot()
                source = copy.deepcopy(snap['shared']['doc']['sources']['s.vendor'])
                source['read'] = '2026-09-11'
                snap[side]['doc']['sources'] = {'s.vendor': source}
                if side == 'working':
                    snap['ancestor']['doc']['sources'] = copy.deepcopy(snap[side]['doc']['sources'])
                self.assertTrue(self.collisions(snap, 's.vendor'))

    def test_mapping_order_does_not_change_shared_identity(self):
        snap = self.snapshot()
        fact = snap['shared']['doc']['known']['vendor.limit']
        for side in ('main', 'working'):
            snap[side]['doc']['known']['vendor.limit'] = dict(reversed(list(fact.items())))
        self.assertFalse(self.collisions(snap))

    def test_collection_difference_is_a_shared_identity_conflict(self):
        snap = self.snapshot()
        snap['main']['doc']['observations'] = copy.deepcopy(snap['shared']['doc']['known'])
        self.assertTrue(self.collisions(snap))

    def test_hypothesis_metadata_conflict_is_visible_including_inherited(self):
        for inherited in (False, True):
            with self.subTest(inherited=inherited):
                snap = self.snapshot()
                fact = copy.deepcopy(snap['shared']['doc']['known']['vendor.limit'])
                fact['name'] = 'Different reading'
                hypothesis = {'name': 'experiment', 'head': {'folds': 'never'},
                              'doc': {'known': {'vendor.limit': fact}}}
                snap['working']['hypotheses'] = [hypothesis]
                if inherited:
                    snap['ancestor']['hypotheses'] = [copy.deepcopy(hypothesis)]
                collisions = self.collisions(snap)
                self.assertTrue(any('experiment' in f['reason'] for f in collisions))

    def test_identical_hypothesis_shared_entry_is_not_a_collision(self):
        snap = self.snapshot()
        snap['working']['hypotheses'] = [
            {'name': 'experiment', 'head': {'folds': 'never'},
             'doc': {'known': copy.deepcopy(snap['shared']['doc']['known'])}}]
        self.assertEqual(W.compare(snap)['state'], 'clear')
