"""Interrupted transitions retain precise recovery state instead of generic errors."""
import unittest
from unittest import mock

from scripts import history_activation as A, history_contract as C, history_transaction as T, provenance as P
from tests import test_history_activation as activation


class RecoveryDiagnostics(unittest.TestCase):
    def test_durable_but_unfinalized_transition_names_its_required_recovery(self):
        fixture = activation.Activation()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        # This is a publication/diagnostic test, not an installed launcher proof.
        with mock.patch.object(A, '_probe', return_value={'fixture': 'not a runtime attestation'}):
            mutation = fixture.prepare()
            verify = A._verify
            count = []
            def late_failure(*args, **kwargs):
                count.append(None)
                if len(count) == 2:
                    raise ValueError('temporary final verification failure')
                return verify(*args, **kwargs)
            with mock.patch.object(A, '_verify', side_effect=late_failure):
                with self.assertRaisesRegex(C.HistoryError, 'transition_unfinalized'):
                    fixture.publish(mutation)
            self.assertTrue((fixture.entry.parent / T.journal_for(fixture.entry)).exists())
            with self.assertRaisesRegex(C.HistoryError, 'transition_recovery_required'):
                P.recover_direct([str(fixture.entry)])
            result = A.recover(fixture.entry, deployment_guard=fixture.guard)
            self.assertEqual(result['state'], 'recovered')
            self.assertFalse((fixture.entry.parent / T.journal_for(fixture.entry)).exists())
