"""Capability declarations follow accepted history, not unaccepted proposals."""
import unittest

from scripts import history_authoring as A, history_contract as C
from tests import test_history_store as fixtures
from tests.test_history_authoring import claim


class ProposalCapabilities(unittest.TestCase):
    def setUp(self):
        fixture = fixtures.Storage()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        self.fixture, self.entry, self.store = fixture, fixture.entry, fixture.store
        self.root = claim(body={'v': 1})
        fixture.publish([self.root], op='bootstrap')

    @staticmethod
    def record_after(mutation):
        raw = next(item['after'] for item in mutation.files if item['role'] == 'record')
        return C.decode_document(raw)

    def publish(self, mutation):
        A.commit(self.entry, mutation, verify=lambda data: None)

    def test_composed_proposal_declares_only_its_hypothetical_world_until_acceptance(self):
        proposed = A.prepare_proposal(self.entry, 'p.composed', {
            'rule': {'expr': '[p.input, {"ok": true}]'}}, 'readings',
            because='retain as an explicit proposal', operation='composed-proposal',
            recorded_at='2026-09-18T00:00:00Z')
        proposal_receipt = proposed.to_data()['receipt']

        self.assertEqual(self.record_after(proposed)['meta']['reasoning']['requires'],
                         ['arithmetic/v1'])
        self.assertEqual(proposal_receipt['after']['document']['meta']['reasoning']['requires'],
                         ['arithmetic/v1'])
        self.assertEqual(proposal_receipt['capabilities']['requires'], ['arithmetic/v1'])
        hypothetical = proposal_receipt['after']['proposal']
        self.assertEqual(hypothetical['document']['meta']['reasoning']['requires'],
                         ['arithmetic/v1', 'composition/v1'])
        computation = hypothetical['assessment']['nodes']['p.composed']['computation']
        self.assertEqual(computation['status'], 'ok')
        self.assertEqual(computation['modules'], ['arithmetic/v1', 'composition/v1'])
        A.verify_prepared(self.entry, proposed)

        self.publish(proposed)
        pending = self.store.capture()
        target = pending.state['subjects']['p.composed']['proposals'][0]
        self.assertEqual(pending.document['meta']['reasoning']['requires'], ['arithmetic/v1'])
        self.assertEqual(pending.state['subjects']['p.composed']['acceptance'], 'proposed')
        retained_objects = dict(pending.object_bytes)
        retained_commits = dict(pending.commits)

        accepted = A.prepare_act(self.entry, {'kind': 'accept', 'id': 'p.composed',
            'of': target, 'over': [], 'because': 'explicitly accept the composed proposal'},
            operation='accept-composed-proposal', recorded_at='2026-09-18T00:01:00Z')
        acceptance_receipt = accepted.to_data()['receipt']
        self.assertEqual(self.record_after(accepted)['meta']['reasoning']['requires'],
                         ['arithmetic/v1', 'composition/v1'])
        self.assertEqual(acceptance_receipt['capabilities']['requires'],
                         ['arithmetic/v1', 'composition/v1'])
        self.assertEqual(
            acceptance_receipt['after']['assessment']['nodes']['p.composed']['computation']['status'],
            'ok')

        self.publish(accepted)
        final = self.store.capture()
        self.assertEqual(final.state['subjects']['p.composed']['acceptance'], 'accepted')
        self.assertEqual(final.document['meta']['reasoning']['requires'],
                         ['arithmetic/v1', 'composition/v1'])
        for key, raw in retained_objects.items():
            self.assertEqual(final.object_bytes[key], raw)
        for operation, raw in retained_commits.items():
            self.assertEqual(final.commits[operation], raw)


if __name__ == '__main__':
    unittest.main()
