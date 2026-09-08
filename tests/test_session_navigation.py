"""Complete navigation stays useful when only some sibling groups fit."""
import json
import os
from pathlib import Path
import tempfile
import unittest

try:
    import tiktoken
    from scripts.session.core import Core
    from scripts.session.view import GroundingService
    CORE_READY = bool(Core())
except (ImportError, ValueError):
    CORE_READY = False

if os.environ.get('KPOPPER_REQUIRE_CORE_TESTS') == '1' and not CORE_READY:
    raise RuntimeError('navigation integration tests require the session extras and compiled core')


@unittest.skipUnless(CORE_READY, 'install session extras and run session setup for kernel integration tests')
class MixedDepthNavigation(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.folder = Path(self.temp.name)
        self.record = self.folder / 'record.json'
        self.data = {'nodes': {}, 'topics': {}, 'edges': [], 'scope': 'Navigation fixture.'}
        for number in range(36):
            key = 'large.' + 'lengthy_identifier_segment_' * 5 + str(number)
            self.add_node(key, ['large'], number)
        for number in range(2):
            self.add_node('small.item' + str(number), ['small'], number)
        self.data['nodes']['s.evidence'] = {'kind': 'source', 'states': [], 'body': {'read': '2026-09-09'}}
        self.data['topics']['s.evidence'] = ['sources']
        self.data['edges'] = [{'from': 'small.item0', 'rel': 'from', 'to': 's.evidence'}]
        self.write_record()

    def add_node(self, key, topic, value):
        self.data['nodes'][key] = {'kind': 'known', 'states': [], 'body': {'v': value}}
        self.data['topics'][key] = topic

    def write_record(self):
        self.record.write_text(json.dumps(self.data))
        self.service = GroundingService('fixture', self.record, self.folder / 'state')

    def test_opening_expands_small_siblings_without_dropping_the_large_group(self):
        result = self.service.opening_packet(700)
        cells = result['packet']['cells']
        visible = {cell['key'] for cell in cells if cell['kind'] == 'node'}
        self.assertTrue({'small.item0', 'small.item1', 's.evidence'} <= visible)
        self.assertIn(('group', '/large'), result['frontier'])
        members = [key for cell in cells for key in cell['members']]
        self.assertCountEqual(members, self.data['nodes'])
        self.assertEqual(len(members), len(set(members)))
        self.assertTrue(result['guard']['accepted'])
        self.assertLessEqual(result['tokens'], 700)

    def test_branch_read_can_reveal_a_small_sibling_within_its_budget(self):
        _, revision = self.service.graph()
        text = self.service.reading('/', revision, 160)
        self.assertIn('node:small.item0', text)
        self.assertIn('node:small.item1', text)
        self.assertIn('+ /large', text)
        self.assertLessEqual(len(self.service.encoder.encode(text)), 160)

    def test_displayed_leaf_handles_are_directly_readable_and_counts_are_explicit(self):
        result = self.service.opening_packet(700)
        self.assertIn('source_count=1', result['text'])
        handles = [line[2:].split()[0] for line in result['text'].splitlines() if line.startswith('- ')]
        self.assertGreaterEqual(len(handles), 3)
        for handle in handles:
            self.assertTrue(handle.startswith('node:'), handle)
            response = json.loads(self.service.reading(handle, result['packet']['revision']))
            self.assertTrue(response['complete'])
            self.assertIn('body', response['value'])

    def test_unordered_input_does_not_change_navigation_selection(self):
        first = self.service.opening_packet(700)
        self.data['nodes'] = dict(reversed(list(self.data['nodes'].items())))
        self.data['topics'] = dict(reversed(list(self.data['topics'].items())))
        self.write_record()
        second = self.service.opening_packet(700)
        self.assertEqual(first['text'], second['text'])
        self.assertEqual(first['frontier'], second['frontier'])

    def test_source_entry_error_names_its_actual_node_handle(self):
        _, revision = self.service.graph()
        with self.assertRaisesRegex(ValueError, 'read node:s.evidence'):
            self.service.reading('source:s.evidence', revision)
        response = json.loads(self.service.reading('node:s.evidence', revision))
        self.assertEqual(response['value']['body']['read'], '2026-09-09')

    def test_unknown_routes_offer_the_valid_root_without_guessing_an_id(self):
        _, revision = self.service.graph()
        for ref in ['/not-listed', 'node:not-listed', 'links:/not-listed',
                    'events:/not-listed', 'conditions:/not-listed']:
            with self.subTest(ref=ref), self.assertRaisesRegex(ValueError, 'read /'):
                self.service.reading(ref, revision)
        self.assertIn('MAP /', self.service.reading('/', revision, 160))


if __name__ == '__main__':
    unittest.main()
