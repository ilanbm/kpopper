"""Existing visible hypothesis filename stems do not disable record writes."""
import contextlib
import io
from pathlib import Path
import tempfile
import unittest

from scripts import provenance as P, history_contract as C
from scripts.reasoning.snapshot import Snapshot
from tests import test_history_snapshot_capture as histories


class HypothesisNames(unittest.TestCase):
    def test_existing_dotted_spaced_and_unicode_names_allow_ordinary_base_writes(self):
        for name in ('alt.v2', 'with space', 'חלופה.v2'):
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                record = Path(directory).resolve() / 'GROUNDING.yaml'
                record.write_text('known:\n  p.value: {v: 1}\n')
                path = Path(P.hypothesis_path([str(record)], name))
                path.parent.mkdir(parents=True)
                path.write_text('known:\n  p.other: {v: 3}\n')
                before = path.read_bytes()
                with contextlib.redirect_stdout(io.StringIO()):
                    P.apply([str(record)], {'kind': 'set', 'id': 'p.value', 'value': 2})
                self.assertEqual(P.bodies(P.load([str(record)]))['p.value']['v'], 2)
                self.assertEqual(path.read_bytes(), before)

    def test_history_group_and_command_preserve_same_name_class(self):
        fixture = histories.HistorySnapshotCapture()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        with contextlib.redirect_stdout(io.StringIO()):
            P.write_command('set', ['p.input', '5', '--hypothesis', 'with space.v2', str(fixture.entry)])
        self.assertIn('with space.v2', Snapshot.capture(fixture.entry).to_data()['hypotheses'])

    def test_names_cannot_hide_or_escape_the_hypothesis_directory(self):
        for name in ('../escape', 'nested/name', r'nested\name', '.hidden', 'line\nfeed', ''):
            with self.subTest(name=name), self.assertRaises(C.HistoryError):
                C.hypothesis_name(name)
