"""Older contribution formats cannot silently discard history authority."""
import unittest

from scripts import pending_grounding as G
from scripts.reasoning.contract import CapabilityError
from tests.test_history_contract import baseline, claim


class HistoryContributionGuard(unittest.TestCase):
    def test_prepare_refuses_history_instead_of_dropping_its_baseline(self):
        scope = {'kind': 'external', 'environment': 'fixture'}
        document = {'readings': {'p.input': {'v': 1, 'scope': scope}},
                    'meta': {'history': baseline(claim())}}
        with self.assertRaisesRegex(CapabilityError, 'history'):
            G.prepare(document, ['p.input'], scope=scope, shareability='project')

    def test_ordinary_contribution_keeps_existing_version_and_identity(self):
        scope = {'kind': 'external', 'environment': 'fixture'}
        document = {'readings': {'p.input': {'v': 1, 'scope': scope}}}
        bundle = G.prepare(document, ['p.input'], scope=scope, shareability='project')
        self.assertEqual(bundle['manifest']['version'], 2)
        self.assertEqual(bundle['revision'], G.identity(bundle['manifest']))
        G.validate_bundle(bundle)


if __name__ == '__main__':
    unittest.main()
