"""The record is parsed when it changes, not when it is read. Every command reads the whole
record and the hooks read it at every prompt and every edit, so a file's parsed document is
kept under the user's own state directory and taken again the moment the file differs.

What these hold the cache to: it is a copy of what a file said, never an authority over it.
A file that changed is parsed again, an entry that cannot be read is parsed again, an entry
about another file is not this file's, and nothing in an entry is ever built into an object.

    python3 -m unittest discover -s tests
"""
import io
import os
import pathlib
import pickle
import stat
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPTS = ROOT / "scripts"

sys.path.insert(0, str(SCRIPTS))
import provenance as P  # noqa: E402

RECORD = """meta:
  updated: 2026-09-11

sources:
  doc.plan: {name: "the plan", file: "plan.md", read: "2026-09-10"}

known:
  room.seats: {v: 12, name: "seats in the room", from: doc.plan}

judgments:
  room.fits:
    rests_on: [room.seats]
    verdict: "the room holds the standup"
    because: "twelve seats"
    wrong_if: "room.seats < 9"
    seen: {room.seats: 12}
"""


class Counted:
    """The parser, with a count of how often it was asked to read a file."""

    def __init__(self):
        self.n, self.was = 0, P.yaml.safe_load

    def __enter__(self):
        def counting(text):
            self.n += 1
            return self.was(text)
        P.yaml.safe_load = counting
        return self

    def __exit__(self, *exc):
        P.yaml.safe_load = self.was


class TheParseIsKept(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.state = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.dir.name)
        self.record = self.root / "PROVENANCE.yaml"
        self.record.write_text(RECORD, encoding="utf-8")
        self.was = os.environ.get("XDG_STATE_HOME")
        os.environ["XDG_STATE_HOME"] = self.state.name
        os.environ.pop(P.NO_CACHE, None)
        P._PARSED.clear()
        P._SWEPT = False

    def tearDown(self):
        if self.was is None:
            os.environ.pop("XDG_STATE_HOME", None)
        else:
            os.environ["XDG_STATE_HOME"] = self.was
        os.environ.pop(P.NO_CACHE, None)
        P._PARSED.clear()
        self.dir.cleanup()
        self.state.cleanup()

    def entry(self, path=None):
        return pathlib.Path(P._cache_file(os.path.abspath(str(path or self.record))))

    def read(self):
        """The record as a fresh process would read it: nothing remembered in this one."""
        P._PARSED.clear()
        return P.load([str(self.record)])

    def test_the_second_read_of_an_unchanged_file_does_not_parse_it(self):
        self.read()
        self.assertTrue(self.entry().exists(), "the parse was not kept")
        with Counted() as c:
            doc = self.read()
        self.assertEqual(c.n, 0, "the file was parsed again although nothing changed")
        self.assertEqual(doc["known"]["room.seats"]["v"], 12)

    def test_what_is_read_back_is_what_the_file_says(self):
        with Counted() as c:
            first = P.load([str(self.record)])
            self.assertEqual(c.n, 1)
        second = self.read()
        self.assertEqual(first, second)
        self.assertEqual(P.infer(second)[0], P.infer(first)[0])

    def test_the_document_is_this_reader_s_own(self):
        """What a caller does to the document it was handed cannot reach the next caller."""
        first = self.read()
        first["known"]["room.seats"]["v"] = 99
        first["known"].pop("room.fits", None)
        self.assertEqual(self.read()["known"]["room.seats"]["v"], 12)

    def test_a_longer_file_is_parsed_again(self):
        self.read()
        self.record.write_text(RECORD.replace("v: 12", "v: 1200"), encoding="utf-8")
        with Counted() as c:
            doc = self.read()
        self.assertEqual(c.n, 1)
        self.assertEqual(doc["known"]["room.seats"]["v"], 1200)

    def test_a_file_written_again_with_the_same_bytes_is_parsed_again(self):
        self.read()
        identity = P._identity(str(self.record))
        self.record.write_text(RECORD, encoding="utf-8")
        self.assertNotEqual(P._identity(str(self.record)), identity, "the file was not rewritten")
        with Counted() as c:
            self.read()
        self.assertEqual(c.n, 1, "a file of the same length was taken for the file that parsed")

    def test_other_content_of_the_same_length_is_parsed_again(self):
        self.read()
        was = P._identity(str(self.record))
        self.record.write_text(RECORD.replace("v: 12", "v: 21"), encoding="utf-8")
        self.assertEqual(P._identity(str(self.record))[0], was[0], "the test did not keep the length")
        with Counted() as c:
            doc = self.read()
        self.assertEqual(c.n, 1)
        self.assertEqual(doc["known"]["room.seats"]["v"], 21)

    def test_a_file_of_the_same_length_written_in_the_same_nanosecond_is_still_the_file_it_was(self):
        """The one thing a length and a nanosecond cannot tell apart, said out loud: a write
        this tool does not make, of the same length, stamped back to the nanosecond the parse
        was taken at. Every write this tool makes drops the entry instead of relying on it."""
        self.read()
        was = P._identity(str(self.record))
        self.record.write_text(RECORD.replace("v: 12", "v: 21"), encoding="utf-8")
        os.utime(self.record, ns=(was[1], was[1]))
        self.assertEqual(P._identity(str(self.record)), was)
        self.assertEqual(self.read()["known"]["room.seats"]["v"], 12)
        P.forget(str(self.record))
        self.assertEqual(self.read()["known"]["room.seats"]["v"], 21)

    def test_an_entry_that_cannot_be_read_is_parsed_instead(self):
        self.read()
        for damage in (b"", b"not a pickle at all", pickle.dumps({"form": 99})[:-3]):
            with self.subTest(damage=damage[:12]):
                self.entry().write_bytes(damage)
                P._PARSED.clear()
                with Counted() as c:
                    doc = P.load([str(self.record)])
                self.assertEqual(c.n, 1)
                self.assertEqual(doc["known"]["room.seats"]["v"], 12)

    def test_an_entry_about_another_file_is_not_this_file_s(self):
        other = self.root / "other.yaml"
        other.write_text(RECORD.replace("v: 12", "v: 77"), encoding="utf-8")
        P.load([str(other)])
        self.entry().write_bytes(self.entry(other).read_bytes())
        P._PARSED.clear()
        with Counted() as c:
            doc = P.load([str(self.record)])
        self.assertEqual(c.n, 1)
        self.assertEqual(doc["known"]["room.seats"]["v"], 12)

    def test_nothing_in_an_entry_is_built_into_an_object(self):
        """An entry holds what the parser returned and nothing else, so a file under the state
        directory can never become something this process runs."""
        self.read()
        identity = P._identity(str(self.record))
        self.entry().write_bytes(pickle.dumps(
            {"form": P.CACHE_FORM, "path": os.path.abspath(str(self.record)),
             "identity": identity, "doc": subprocess.run}))
        P._PARSED.clear()
        with Counted() as c:
            doc = P.load([str(self.record)])
        self.assertEqual(c.n, 1)
        self.assertEqual(doc["known"]["room.seats"]["v"], 12)

    def test_a_date_the_record_holds_survives_the_keeping(self):
        self.record.write_text(RECORD + "  born: 2026-09-11\n", encoding="utf-8")
        first = self.read()
        again = self.read()
        self.assertEqual(first["judgments"]["born"], again["judgments"]["born"])
        self.assertEqual(str(again["judgments"]["born"]), "2026-09-11")

    def test_the_entries_are_the_user_s_own(self):
        self.read()
        self.assertEqual(stat.S_IMODE(self.entry().stat().st_mode), 0o600)
        self.assertEqual(stat.S_IMODE(pathlib.Path(P.cache_dir()).stat().st_mode), 0o700)

    def test_two_records_of_one_name_never_share_an_entry(self):
        other_dir = pathlib.Path(tempfile.mkdtemp(dir=self.root))
        other = other_dir / "PROVENANCE.yaml"
        other.write_text(RECORD.replace("v: 12", "v: 77"), encoding="utf-8")
        self.assertNotEqual(self.entry(), self.entry(other))
        self.assertEqual(P.load([str(other)])["known"]["room.seats"]["v"], 77)
        self.assertEqual(self.read()["known"]["room.seats"]["v"], 12)
        self.assertEqual(P.load([str(other)])["known"]["room.seats"]["v"], 77)

    def test_a_directory_that_cannot_be_written_changes_no_answer(self):
        os.environ["XDG_STATE_HOME"] = os.path.join(self.state.name, "nothing", "here")
        os.chmod(self.state.name, 0o500)
        try:
            self.assertIsNone(P._cache_ready())
            self.assertEqual(self.read()["known"]["room.seats"]["v"], 12)
        finally:
            os.chmod(self.state.name, 0o700)

    def test_a_directory_anyone_can_write_to_is_not_used(self):
        os.makedirs(P.cache_dir(), mode=0o700, exist_ok=True)
        os.chmod(P.cache_dir(), 0o777)
        self.assertIsNone(P._cache_ready())
        self.assertEqual(self.read()["known"]["room.seats"]["v"], 12)
        self.assertFalse(self.entry().exists())

    def test_the_switch_parses_every_time(self):
        os.environ[P.NO_CACHE] = "1"
        with Counted() as c:
            self.read()
            self.read()
        self.assertEqual(c.n, 2)
        self.assertFalse(self.entry().exists(), "nothing is kept while the switch is on")

    def test_an_entry_no_read_renewed_for_a_fortnight_goes(self):
        self.read()
        old = self.entry()
        long_ago = old.stat().st_mtime - (P.CACHE_DAYS + 1) * 86400
        os.utime(old, (long_ago, long_ago))
        os.remove(os.path.join(P.cache_dir(), P.SWEEP))
        P._SWEPT = False
        other = self.root / "other.yaml"
        other.write_text(RECORD, encoding="utf-8")
        P.load([str(other)])
        self.assertFalse(old.exists(), "a stale entry was left behind")
        self.assertTrue(self.entry(other).exists())


class AWriteIsSeenByTheNextRead(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.state = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.dir.name)
        self.record = self.root / "PROVENANCE.yaml"
        self.record.write_text(RECORD, encoding="utf-8")
        self.was = os.environ.get("XDG_STATE_HOME")
        os.environ["XDG_STATE_HOME"] = self.state.name
        P._PARSED.clear()
        P._SWEPT = False

    def tearDown(self):
        if self.was is None:
            os.environ.pop("XDG_STATE_HOME", None)
        else:
            os.environ["XDG_STATE_HOME"] = self.was
        P._PARSED.clear()
        self.dir.cleanup()
        self.state.cleanup()

    def run_reader(self, *args):
        p = subprocess.run([sys.executable, str(SCRIPTS / "provenance.py")] + [str(a) for a in args],
                           capture_output=True, text=True, cwd=self.root,
                           env=dict(os.environ, XDG_STATE_HOME=self.state.name))
        return p.returncode, p.stdout, p.stderr

    def test_a_set_in_this_process_is_what_the_next_read_reads(self):
        self.assertEqual(P.load([str(self.record)])["known"]["room.seats"]["v"], 12)
        P.apply([str(self.record)], {"kind": "set", "id": "room.seats", "value": 9,
                                     "as_of": "2026-09-11", "why": None, "into": None,
                                     "hypothesis": None, "source": None, "at": None})
        self.assertEqual(P.load([str(self.record)])["known"]["room.seats"]["v"], 9)

    def test_a_set_by_another_process_is_what_the_next_read_reads(self):
        P.load([str(self.record)])
        code, out, err = self.run_reader("set", "room.seats", "9", "--as-of", "2026-09-11",
                                         str(self.record))
        self.assertEqual(code, 0, out + err)
        P._PARSED.clear()
        self.assertEqual(P.load([str(self.record)])["known"]["room.seats"]["v"], 9)

    def test_the_file_a_pointer_names_is_read_as_it_is_now(self):
        (self.root / "rooms.yaml").write_text(
            "known:\n  room.rows: {v: 3, name: rows, from: doc.plan}\n", encoding="utf-8")
        self.record.write_text(RECORD + "\nrecord: rooms.yaml\n", encoding="utf-8")
        self.assertEqual(P.load([str(self.record)])["known"]["room.rows"]["v"], 3)
        code, out, err = self.run_reader("set", "room.rows", "4", "--as-of", "2026-09-11",
                                         str(self.record))
        self.assertEqual(code, 0, out + err)
        P._PARSED.clear()
        self.assertEqual(P.load([str(self.record)])["known"]["room.rows"]["v"], 4)

    def test_the_switch_is_taken_from_the_command_line_too(self):
        code, out, err = self.run_reader("--no-cache", "check", str(self.record))
        self.assertEqual(code, 0, out + err)
        self.assertIn("1 judgments", out)
        self.assertFalse(list(pathlib.Path(self.state.name).glob("kpopper/cache/*.parse")))


if __name__ == "__main__":
    unittest.main()
