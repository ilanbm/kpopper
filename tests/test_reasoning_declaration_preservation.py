"""Reasoning declaration edits preserve hand-authored YAML around metadata."""
import unittest

import yaml

from scripts import history_authoring, provenance as P
from scripts.reasoning import authoring


class DeclarationPreservation(unittest.TestCase):
    def rewrite(self, text):
        lines = text.split('\n')
        authoring.declare(lines, P)
        return '\n'.join(lines)

    def test_block_version_upgrade_preserves_following_comments_and_blank_lines(self):
        source = (
            'meta:\n'
            '  reasoning:\n'
            '    version: 1\n'
            '    profile: core/v1\n'
            '    requires: [arithmetic/v1]\n'
            '  # retained metadata comment\n'
            '\n'
            '  note: keep\n'
            '# retained comment before the next root key\n'
            '\n'
            'known:\n'
            '  p.x: {v: 2}\n')
        expected = source.replace('    version: 1\n', '    version: 2\n')

        self.assertEqual(self.rewrite(source), expected)

    def test_block_requirement_extension_preserves_item_and_trailing_comments(self):
        source = (
            'meta:\n'
            '  reasoning:\n'
            '    version: 2\n'
            '    profile: core/v1\n'
            '    requires:\n'
            '      - arithmetic/v1 # retained item comment\n'
            '    # retained declaration comment\n'
            '\n'
            '  note: keep\n'
            'known:\n'
            '  p.x: {v: 2}\n'
            "  p.items: {rule: {expr: '[p.x]'}}\n")
        expected = source.replace(
            '      - arithmetic/v1 # retained item comment\n',
            '      - arithmetic/v1 # retained item comment\n'
            '      - composition/v1\n')

        rewritten = self.rewrite(source)
        self.assertEqual(rewritten, expected)
        self.assertEqual(yaml.safe_load(rewritten)['meta']['reasoning']['requires'],
                         ['arithmetic/v1', 'composition/v1'])

    def test_flow_declaration_updates_only_target_value_spans(self):
        source = (
            'meta: {reasoning: {version: 1, profile: core/v1, requires: [arithmetic/v1]}, note: keep}\n'
            '# retained before known\n'
            "known: {p.items: {rule: {expr: '[1, 2]'}}}\n")
        expected = source.replace('version: 1', 'version: 2').replace(
            'requires: [arithmetic/v1]',
            'requires: [arithmetic/v1, composition/v1]')

        self.assertEqual(self.rewrite(source), expected)

    def test_alias_declaration_is_still_refused(self):
        source = (
            'declaration: &core {version: 1, profile: core/v1, requires: [arithmetic/v1]}\n'
            'meta: {reasoning: *core}\n')

        with self.assertRaisesRegex(P.Refused, 'aliased reasoning declaration'):
            self.rewrite(source)

    def test_generated_metadata_and_no_change_are_stable(self):
        source = 'known:\n  p.x: {v: 2}\n'
        rewritten = self.rewrite(source)
        self.assertEqual(yaml.safe_load(rewritten)['meta']['reasoning'], authoring.DECLARATION)

        stable = (
            'meta:\n'
            '  reasoning: {version: 2, profile: core/v1, requires: [arithmetic/v1]}\n'
            '  # retained while already current\n'
            'known: {p.x: {v: 2}}\n')
        self.assertEqual(self.rewrite(stable), stable)

    def test_invalid_structured_rule_reaches_world_refusal(self):
        document = {
            'meta': {'reasoning': dict(authoring.DECLARATION)},
            'known': {'p.bad': {'rule': {'op': 'add', 'args': []}}},
        }

        self.assertEqual(authoring.declaration(document), authoring.DECLARATION)
        with self.assertRaisesRegex(P.Refused, r'p\.bad\.rule: invalid operator arity'):
            history_authoring._world(history_authoring._destination(document))


if __name__ == '__main__':
    unittest.main()
