import unittest
import os
from pathlib import Path
import tempfile
from storage import available, purge_exports, retention_days


class StoragePolicy(unittest.TestCase):
    def test_new_export_is_available(self):
        self.assertGreater(retention_days(), 0)
        self.assertTrue(available(0))

    def test_old_export_is_removed(self):
        self.assertFalse(available(365))

    def test_retention_boundary(self):
        self.assertTrue(available(retention_days() - 1))
        self.assertFalse(available(retention_days()))

    def test_cleanup_removes_old_exports_and_keeps_new_ones(self):
        now = 1735689600
        with tempfile.TemporaryDirectory() as directory:
            new, old = Path(directory) / "new.csv", Path(directory) / "old.csv"
            new.write_text("new export")
            old.write_text("old export")
            os.utime(new, (now, now))
            os.utime(old, (now - 365 * 86400, now - 365 * 86400))
            purge_exports(directory, now)
            self.assertTrue(new.exists())
            self.assertFalse(old.exists())
