import os
import pathlib
import subprocess
import sys
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPTS = ROOT / "scripts"
sys.path.insert(0, str(SCRIPTS))
import provenance as P  # noqa: E402


RECORD = "known:\n  room.seats: {v: 12}\n"
HYPOTHESIS = "hypothesis:\n  claim: more seats\nknown:\n  room.seats: {v: 14}\n"


class CustomRecordLayout(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.directory = pathlib.Path(self.tmp.name).resolve()

    def make_custom_record(self, directory):
        record = directory / "notes.yaml"
        record.write_text(RECORD, encoding="utf-8")
        (directory / "notes.view.yaml").write_text("title: Notes\n", encoding="utf-8")
        hypotheses = directory / "PROVENANCE.d"
        hypotheses.mkdir()
        (hypotheses / "capacity.yaml").write_text(HYPOTHESIS, encoding="utf-8")
        (directory / "PROVENANCE.measure.yaml").write_text("seats: [printf, 12]\n", encoding="utf-8")
        (directory / "PROVENANCE.session.json").write_text("{}\n", encoding="utf-8")
        return record

    def assert_historical_layout(self, record):
        layout = P.layout(record)
        self.assertTrue(layout["legacy"])
        self.assertEqual(pathlib.Path(layout["view"]), record.with_suffix(".view.yaml"))
        self.assertEqual(pathlib.Path(layout["hypotheses"]), record.parent / "PROVENANCE.d")
        self.assertEqual(pathlib.Path(layout["measure"]), record.parent / "PROVENANCE.measure.yaml")
        self.assertEqual(pathlib.Path(layout["session"]), record.parent / "PROVENANCE.session.json")
        self.assertEqual(P.brief_for([str(record)]), str(record.with_suffix(".view.yaml")))
        self.assertEqual(set(P.load([str(record)]).hypotheses), {"capacity"})
        self.assertEqual(P.leftovers([str(record)]), [])

    def test_an_explicit_custom_filename_keeps_its_adjacent_metadata(self):
        self.assert_historical_layout(self.make_custom_record(self.directory))

    def test_a_registered_custom_filename_keeps_its_adjacent_metadata(self):
        project = self.directory / "project"
        project.mkdir()
        subprocess.run(["git", "init", "-q"], cwd=project, check=True)
        record_home = self.directory / "record"
        record_home.mkdir()
        record = self.make_custom_record(record_home)
        (project / ".git" / "kpopper-record").write_text(str(record) + "\n", encoding="utf-8")
        before = pathlib.Path.cwd()
        os.chdir(project)
        try:
            self.assertEqual(P.default_paths(), [str(record)])
            self.assert_historical_layout(pathlib.Path(P.default_paths()[0]))
        finally:
            os.chdir(before)

    def test_grounding_yaml_still_uses_the_modern_home(self):
        record = self.directory / "GROUNDING.yaml"
        record.write_text(RECORD, encoding="utf-8")
        layout = P.layout(record)
        self.assertFalse(layout["legacy"])
        self.assertEqual(pathlib.Path(layout["view"]), self.directory / ".kpopper" / "view.yaml")
        self.assertEqual(pathlib.Path(layout["hypotheses"]), self.directory / ".kpopper" / "hypotheses")
        self.assertEqual(pathlib.Path(layout["measure"]), self.directory / ".kpopper" / "measure.yaml")
        self.assertEqual(pathlib.Path(layout["session"]), self.directory / ".kpopper" / "session.json")


if __name__ == "__main__":
    unittest.main()
