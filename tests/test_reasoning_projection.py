"""Focused findings and scoped module reads do not expose unrelated evidence."""
import json
import unittest

from scripts.reasoning.assessment import assess
from scripts.reasoning.snapshot import Snapshot
from test_reasoning_assessment import UnavailableRuntime


class ProjectionTests(unittest.TestCase):
    def test_scope_read_projects_alternative_bodies_to_granted_fields(self):
        doc = {'items': {'a': {'v': 0, 'secret': 'base-private'}},
               'scopes': {'scope.items': {'collection_scope': {'collection': 'items', 'fields': ['v']}}}}
        variants = [['one', {'v': 1, 'secret': 'hidden-one'}],
                    ['two', {'v': 2, 'secret': 'hidden-two'}]]
        snapshot = Snapshot.from_data(doc, context={'conflicts': {'a': variants}})
        rows = snapshot.capture_scope('scope.items').view().read_scope('scope.items', 'v')
        self.assertNotIn('secret', json.dumps(rows))
        self.assertEqual(snapshot.to_data()['context']['conflicts']['a'], variants)
        self.assertEqual([body['v'] for _, body in rows[0]['alternatives']], [1, 2])

    def test_selected_assessment_binds_but_does_not_print_unrelated_bodies(self):
        doc = {'items': {'a': {'v': 1}, 'b': {'v': 2}}}
        secret = 'unrelated-private-value'
        context = {'conflicts': {'b': [['other', {'v': secret}]]},
                   'pending': {'ref': 'ledger', 'bundles': {
                       'revision': {'manifest': {'document': {'items': {'b': {'v': secret}}},
                                                'roots': ['b'], 'scope': {'kind': 'project'}},
                                    'files': {'private.txt': secret}}}}}
        hypotheses = {'h': {'doc': {'items': {'b': {'v': secret}}}, 'head': {'claim': secret}}}
        snapshot = Snapshot.from_data(doc, context=context, hypotheses=hypotheses)
        report = assess(snapshot, ['a'], runtime=UnavailableRuntime())
        self.assertNotIn(secret, json.dumps(report))
        self.assertEqual(report['snapshot_id'], snapshot.snapshot_id)
        self.assertIn(secret, json.dumps(snapshot.to_data()))


if __name__ == '__main__':
    unittest.main()
