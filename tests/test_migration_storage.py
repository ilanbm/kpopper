"""Exact source inventories and exclusive atomic directory publication."""
from pathlib import Path
import tempfile
import unittest

from scripts import knowledge_views as V
from scripts.reasoning.snapshot import capture_source, SnapshotError


class MigrationStorage(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)

    def test_reader_inventory_keeps_exact_shards_and_observes_missing_pointer(self):
        entry = self.root / 'GROUNDING.yaml'
        shard = self.root / 'part.yaml'
        entry.write_bytes(b'# entry\nrecord: part.yaml\nalso: missing.yaml\n')
        shard.write_bytes(b'known:\n  p.one: {v: 1, from: measured}\n')
        captured = capture_source([str(entry)], read_mode='frozen')
        self.assertEqual(captured.files, {str(entry): entry.read_bytes(), str(shard): shard.read_bytes()})
        self.assertFalse(captured.inventory.events[('exists', str(self.root / 'missing.yaml'))])
        captured.verify()
        (self.root / 'missing.yaml').write_text('{}')
        with self.assertRaisesRegex(SnapshotError, 'inventory'):
            captured.verify()

    def test_publication_refuses_destination_created_after_validation(self):
        target = self.root / 'new'
        def populate(staging):
            (staging / 'record').write_text('candidate')
        def validate(staging):
            target.mkdir()
        with self.assertRaises(FileExistsError):
            V.publish_tree(target, populate, validate)
        self.assertTrue(target.is_dir())
        self.assertEqual(list(target.iterdir()), [])
        self.assertEqual(list(self.root.iterdir()), [target])

    def test_failed_validation_leaves_source_and_destination_untouched(self):
        source = self.root / 'source'
        source.write_text('original')
        target = self.root / 'new'
        def invalid(staging):
            raise ValueError('changed source')
        with self.assertRaisesRegex(ValueError, 'changed source'):
            V.publish_tree(target, lambda staging: (staging / 'record').write_text('candidate'), invalid)
        self.assertEqual(source.read_text(), 'original')
        self.assertFalse(target.exists())
        self.assertEqual(list(self.root.iterdir()), [source])

    def test_successful_publication_and_existing_empty_destination(self):
        target = self.root / 'new'
        V.publish_tree(target, lambda staging: (staging / 'record').write_text('candidate'))
        self.assertEqual((target / 'record').read_text(), 'candidate')
        empty = self.root / 'empty'
        empty.mkdir()
        with self.assertRaises(FileExistsError):
            V.publish_tree(empty, lambda staging: None)
