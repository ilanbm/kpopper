"""A record divided by subject: a pointer index naming one file per domain. Runs against the
fixture in tests/fixtures/shards - the same record as one file and as two shards - with no
browser and no network:

    python3 -m unittest discover -s tests

Two things are held here. Reading: every command answers the same on both shapes, so dividing
a record costs nothing a reader can see. Writing: a new entry lands in the file its own
subject is in, and a write to an entry lands in the file that holds it.
"""
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPTS = ROOT / "scripts"
FIXTURE = ROOT / "tests" / "fixtures" / "shards"
WHOLE = FIXTURE / "whole"
SPLIT = FIXTURE / "split"


def run(*args, cwd=None):
    """The scripts as a session runs them: a subprocess, its exit code and both streams."""
    p = subprocess.run([sys.executable] + [str(a) for a in args], cwd=cwd,
                       capture_output=True, text=True)
    return p.returncode, p.stdout, p.stderr


def copy(which, into):
    shutil.copytree(which, into, dirs_exist_ok=True)
    return into / "PROVENANCE.yaml"


def holder(root, nid):
    """Which file of the record carries the entry - and it must be exactly one."""
    files = sorted(f for f in root.glob("*.yaml")
                   if re.search(r"^\s{2}" + re.escape(nid) + r":", f.read_text(encoding="utf-8"), re.M))
    return files[0].name if len(files) == 1 else [f.name for f in files]


class BothShapesReadAlike(unittest.TestCase):
    """The same content, as one file and as two under a pointer: what a reader gets back."""

    def both(self, *args):
        a = run(SCRIPTS / "provenance.py", *args, "PROVENANCE.yaml", cwd=WHOLE)
        b = run(SCRIPTS / "provenance.py", *args, "PROVENANCE.yaml", cwd=SPLIT)
        return a, b

    def test_check_says_the_same_of_both(self):
        (code, out, err), (code2, out2, err2) = self.both("check")
        self.assertEqual(code, 0, out + err)
        self.assertEqual((code, out), (code2, out2))
        self.assertIn("2 judgments, 8 entries, 0 problems", out)

    def test_the_opening_differs_only_in_naming_the_files(self):
        (code, out, _), (code2, out2, _) = self.both("open")
        self.assertEqual(code, code2)
        pointer = "  record: meetings.yaml | also: rooms.yaml\n"
        self.assertIn(pointer, out2)
        self.assertEqual(out, out2.replace(pointer, "", 1))

    def test_pull_reads_a_subject_across_the_files(self):
        (code, out, err), (code2, out2, _) = self.both("pull", "room.seats")
        self.assertEqual(code, 0, out + err)
        self.assertEqual((code, out), (code2, out2))
        self.assertIn("mtg.outgrown_the_room", out)      # the judgment sits in the other file

    def test_affects_reaches_the_other_file(self):
        (code, out, err), (code2, out2, _) = self.both("affects", "room.seats")
        self.assertEqual(code, 0, out + err)
        self.assertEqual((code, out), (code2, out2))
        self.assertIn("mtg.outgrown_the_room", out)

    def test_the_page_is_the_same_page(self):
        a = run(SCRIPTS / "render_page.py", "PROVENANCE.yaml", cwd=WHOLE)
        b = run(SCRIPTS / "render_page.py", "PROVENANCE.yaml", cwd=SPLIT)
        self.assertEqual(a[0], 0, a[1] + a[2])
        self.assertEqual(a[1], b[1])

    def test_page_verify_passes_on_both(self):
        for where in (WHOLE, SPLIT):
            with self.subTest(record=where.name):
                code, out, err = run(SCRIPTS / "render_page.py", "--verify", "PROVENANCE.yaml", cwd=where)
                self.assertEqual(code, 0, out + err)
                self.assertIn("6 entries, 2 judgments", out)

    def test_untouched_whole_and_sharded_records_both_stay_silent(self):
        with tempfile.TemporaryDirectory() as d:
            said = []
            for where in (WHOLE, SPLIT):
                state = pathlib.Path(d) / (where.name + ".json")
                code, out, err = run(SCRIPTS / "provenance.py", "mark", state, "PROVENANCE.yaml", cwd=where)
                self.assertEqual(code, 0, out + err)
                said.append(run(SCRIPTS / "provenance.py", "gate", state, "PROVENANCE.yaml",
                                "--turns", "40", "--host", "claude", cwd=where))
            self.assertEqual(said[0], said[1])
            self.assertEqual(said[0], (0, "", ""))


class AWriteLandsWhereItsSubjectIs(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.dir.name)
        self.record = copy(SPLIT, self.root)

    def tearDown(self):
        self.dir.cleanup()

    def add(self, nid, *fields, expect=0):
        code, out, err = run(SCRIPTS / "provenance.py", "add", nid, *fields,
                             "--as-of", "2026-09-11", self.record)
        self.assertEqual(code, expect, out + err)
        return out

    def test_a_new_entry_joins_the_shard_that_holds_its_subject(self):
        self.add("mtg.chairs", "v=3", "name=chairs carried in", "from=mtg.minutes")
        self.add("room.window", "v=true", "name=the room has a window", "from=room.plan")
        self.assertEqual(holder(self.root, "mtg.chairs"), "meetings.yaml")
        self.assertEqual(holder(self.root, "room.window"), "rooms.yaml")
        self.assertEqual(run(SCRIPTS / "provenance.py", "check", self.record)[0], 0)

    def test_a_subject_the_record_has_not_met_lands_in_the_first_file_that_holds_the_collection(self):
        self.add("new.thing", "v=1", "name=a subject nothing here shares a head with",
                 "from=mtg.minutes")
        self.assertEqual(holder(self.root, "new.thing"), "meetings.yaml")
        self.assertEqual(self.record.read_text(encoding="utf-8"),
                         (SPLIT / "PROVENANCE.yaml").read_text(encoding="utf-8"),
                         "the pointer index was written into")

    def test_a_new_judgment_joins_the_judgments_of_its_own_shard(self):
        self.add("room.too_few_seats", "rests_on=[room.seats]",
                 "verdict=the room is short of seats", "because=twelve against fourteen",
                 "wrong_if=room.seats > 13")
        self.assertEqual(holder(self.root, "room.too_few_seats"), "rooms.yaml")
        text = (self.root / "rooms.yaml").read_text(encoding="utf-8")
        self.assertLess(text.index("judgments:"), text.index("room.too_few_seats:"))

    def test_a_named_collection_still_lands_in_the_shard_that_holds_the_subject(self):
        """--in says which collection, never which file; the subject still says which file."""
        self.add("mtg.late", "v=3", "name=people who arrive after it starts",
                 "from=mtg.minutes", "--in", "known")
        self.assertEqual(holder(self.root, "mtg.late"), "meetings.yaml")

    def test_a_source_joins_the_sources_of_its_own_shard(self):
        self.add("room.survey", "name=the seating survey", "file=rooms/survey.md",
                 "read=2026-09-11")
        self.assertEqual(holder(self.root, "room.survey"), "rooms.yaml")

    def test_a_shard_without_the_collection_opens_it_rather_than_lose_the_subject(self):
        """A domain file that has no sources: yet is still where that domain's first source
        belongs - filing it among the meetings is how a division comes undone."""
        rooms = self.root / "rooms.yaml"
        rooms.write_text(rooms.read_text(encoding="utf-8").split("known:", 1)[1]
                         .join(("known:", "")).strip() + "\n", encoding="utf-8")
        self.assertNotIn("sources:", rooms.read_text(encoding="utf-8"))
        self.add("room.survey", "name=the seating survey", "file=rooms/survey.md",
                 "read=2026-09-11")
        self.assertEqual(holder(self.root, "room.survey"), "rooms.yaml")
        self.assertIn("sources:", rooms.read_text(encoding="utf-8"))

    def test_a_record_divided_by_kind_keeps_each_kind_where_it_is(self):
        """The head of an id chooses a file only among the files that could hold the entry. A
        record divided by kind rather than by subject has each collection in one file, and a
        subject it has not met goes to the file that holds its kind."""
        with tempfile.TemporaryDirectory() as d:
            root = pathlib.Path(d)
            (root / "PROVENANCE.yaml").write_text(
                "meta:\n  updated: 2026-09-11\n\nrecord: sources.yaml\nalso: known.yaml\n",
                encoding="utf-8")
            (root / "sources.yaml").write_text(
                'sources:\n  mtg.minutes: {name: "the minutes", file: "m.md", read: "2026-09-10"}\n'
                '  room.plan: {name: "the plan", file: "r.pdf", read: "2026-09-09"}\n', encoding="utf-8")
            (root / "known.yaml").write_text(
                'known:\n  lab.bench: {v: 2, name: "benches", from: room.plan}\n'
                '  mtg.attendees: {v: 14, name: "people", from: mtg.minutes}\n', encoding="utf-8")
            code, out, err = run(SCRIPTS / "provenance.py", "add", "survey.floor",
                                 "name=the seating survey", "file=survey/floor.md",
                                 "read=2026-09-11", "--as-of", "2026-09-11",
                                 root / "PROVENANCE.yaml")
            self.assertEqual(code, 0, out + err)
            self.assertEqual(holder(root, "survey.floor"), "sources.yaml")
            # and a reading of a subject one file holds alone still joins that subject
            code, out, err = run(SCRIPTS / "provenance.py", "add", "lab.stools", "v=6",
                                 "name=stools at the bench", "from=room.plan",
                                 "--as-of", "2026-09-11", root / "PROVENANCE.yaml")
            self.assertEqual(code, 0, out + err)
            self.assertEqual(holder(root, "lab.stools"), "known.yaml")

    def test_the_pointer_index_is_not_a_place_to_keep_entries(self):
        for nid, fields in (("mtg.verdict", ("rests_on=[mtg.attendees]", "verdict=it is decided",
                                             "because=because", "wrong_if=mtg.attendees < 0")),
                            ("new.verdict", ("rests_on=[mtg.attendees]", "verdict=so is this",
                                             "because=because", "wrong_if=mtg.attendees < 0"))):
            with self.subTest(nid=nid):
                with tempfile.TemporaryDirectory() as d:
                    root = pathlib.Path(d)
                    record = copy(SPLIT, root)
                    text = (root / "meetings.yaml").read_text(encoding="utf-8")
                    (root / "meetings.yaml").write_text(text.split("judgments:")[0], encoding="utf-8")
                    text = (root / "rooms.yaml").read_text(encoding="utf-8")
                    (root / "rooms.yaml").write_text(text.split("judgments:")[0], encoding="utf-8")
                    code, out, err = run(SCRIPTS / "provenance.py", "add", nid, *fields,
                                         "--as-of", "2026-09-11", record)
                    self.assertEqual(code, 0, out + err)
                    self.assertEqual(holder(root, nid), "meetings.yaml")
                    self.assertEqual(record.read_text(encoding="utf-8"),
                                     (SPLIT / "PROVENANCE.yaml").read_text(encoding="utf-8"))

    def test_an_entry_is_set_where_it_is_held(self):
        was = (self.root / "meetings.yaml").read_text(encoding="utf-8")
        code, out, err = run(SCRIPTS / "provenance.py", "set", "room.seats", "10",
                             "--as-of", "2026-09-11", self.record)
        self.assertEqual(code, 0, out + err)
        self.assertIn("v: 10", (self.root / "rooms.yaml").read_text(encoding="utf-8"))
        self.assertEqual((self.root / "meetings.yaml").read_text(encoding="utf-8"), was)
        self.assertIn("MUTED     mtg.outgrown_the_room", out)     # reached across the files

    def test_a_judgment_is_reviewed_where_it_is_held(self):
        run(SCRIPTS / "provenance.py", "set", "room.seats", "10", "--as-of", "2026-09-11", self.record)
        was = (self.root / "rooms.yaml").read_text(encoding="utf-8")
        code, out, err = run(SCRIPTS / "provenance.py", "review", "mtg.outgrown_the_room",
                             "--as-of", "2026-09-11", self.record)
        self.assertEqual(code, 0, out + err)
        self.assertIn("room.seats: 10", (self.root / "meetings.yaml").read_text(encoding="utf-8"))
        self.assertEqual((self.root / "rooms.yaml").read_text(encoding="utf-8"), was)

    def test_a_superseding_judgment_replaces_the_one_in_its_own_file(self):
        # a bigger room breaks the standing judgment by its own sign, which is the one door a
        # second verdict under the same id goes through
        run(SCRIPTS / "provenance.py", "set", "room.seats", "20", "--as-of", "2026-09-11", self.record)
        was = (self.root / "rooms.yaml").read_text(encoding="utf-8")
        code, out, err = run(SCRIPTS / "provenance.py", "add", "mtg.outgrown_the_room",
                             "rests_on=[mtg.attendees, room.seats]",
                             "verdict=the standup fits the room again",
                             "because=twenty seats against fourteen people",
                             "wrong_if=mtg.attendees > room.seats",
                             "--as-of", "2026-09-11", "--drop", "room.spare: the seats alone decide it",
                             self.record)
        self.assertEqual(code, 0, out + err)
        self.assertIn("supersede mtg.outgrown_the_room", out)
        self.assertEqual(holder(self.root, "mtg.outgrown_the_room"), "meetings.yaml")
        self.assertEqual((self.root / "rooms.yaml").read_text(encoding="utf-8"), was)

    def test_the_same_record_in_one_file_still_takes_every_write(self):
        with tempfile.TemporaryDirectory() as d:
            record = copy(WHOLE, pathlib.Path(d))
            for nid, fields in (("mtg.chairs", ("v=3", "name=chairs carried in", "from=mtg.minutes")),
                                ("room.window", ("v=true", "name=a window", "from=room.plan")),
                                ("new.thing", ("v=1", "name=a new subject", "from=mtg.minutes"))):
                code, out, err = run(SCRIPTS / "provenance.py", "add", nid, *fields,
                                     "--as-of", "2026-09-11", record)
                self.assertEqual(code, 0, out + err)
                self.assertEqual(holder(pathlib.Path(d), nid), "PROVENANCE.yaml")
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", record)[0], 0)


class TheFoldPlacesWhatItCarries(unittest.TestCase):
    """A hypothesis folded into a divided record is a write like any other: the entry it
    proposes goes where its subject is, not into whichever file comes first."""

    def test_a_folded_entry_lands_in_the_shard_that_holds_its_subject(self):
        with tempfile.TemporaryDirectory() as d:
            root = pathlib.Path(d)
            record = copy(SPLIT, root)
            (root / "PROVENANCE.d").mkdir()
            (root / "PROVENANCE.d" / "window.yaml").write_text(
                'hypothesis: {born: "2026-09-11", claim: "the room has a window"}\n'
                'known:\n  room.window:\n    v: true\n    name: "the room has a window"\n'
                '    from: room.plan\n', encoding="utf-8")
            code, out, err = run(SCRIPTS / "consolidate.py", "window", "--as-of", "2026-09-11", record)
            self.assertEqual(code, 0, out + err)
            self.assertEqual(holder(root, "room.window"), "rooms.yaml")
            self.assertIn("rooms.yaml", out)
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", record)[0], 0)


class OneSubjectUnderTwoIdsAcrossTheFiles(unittest.TestCase):
    """`same` and `distinct` on a record that is more than one file: the reviewer of the
    written layer asked whether a rename reaches every file, and this is the answer."""

    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.dir.name)
        self.record = copy(SPLIT, self.root)

    def tearDown(self):
        self.dir.cleanup()

    def test_a_rename_reaches_every_file_of_the_record_and_the_brief(self):
        code, out, err = run(SCRIPTS / "provenance.py", "add", "room.chairs", "v=12",
                             "name=chairs in the room", "from=room.plan", "--in", "known",
                             "--as-of", "2026-09-11", self.record)
        self.assertEqual(code, 0, out + err)
        code, out, err = run(SCRIPTS / "provenance.py", "same", "room.chairs", "room.seats",
                             "--keep", "a", "--as-of", "2026-09-11", self.record)
        self.assertEqual(code, 0, out + err)
        self.assertIn("mentions in meetings.yaml", out)
        self.assertIn("mentions in rooms.yaml", out)
        self.assertIn("mention in PROVENANCE.view.yaml", out)
        meetings = (self.root / "meetings.yaml").read_text(encoding="utf-8")
        rooms = (self.root / "rooms.yaml").read_text(encoding="utf-8")
        brief = (self.root / "PROVENANCE.view.yaml").read_text(encoding="utf-8")
        for text, where in ((meetings, "meetings.yaml"), (brief, "the brief")):
            with self.subTest(file=where):
                self.assertNotIn("room.seats", text)
                self.assertIn("room.chairs", text)
        self.assertIn("rests_on: [mtg.attendees, room.chairs, room.spare]", meetings)
        self.assertIn("{{room.chairs}}", meetings)
        self.assertIn("wrong_if: \"mtg.attendees <= room.chairs\"", meetings)
        self.assertIn("rule: \"room.chairs - mtg.attendees\"", rooms)
        # the survivor keeps the retired id, so a later writer of it is pointed here
        self.assertIn("also: [room.seats]", rooms)
        self.assertEqual(run(SCRIPTS / "provenance.py", "check", self.record)[0], 0)

    def test_two_subjects_are_told_apart_in_the_file_that_holds_the_first(self):
        was = (self.root / "rooms.yaml").read_text(encoding="utf-8")
        code, out, err = run(SCRIPTS / "provenance.py", "distinct", "mtg.attendees", "room.seats",
                             "one counts people, the other counts chairs",
                             "--as-of", "2026-09-11", self.record)
        self.assertEqual(code, 0, out + err)
        self.assertIn("distinct_from: room.seats", (self.root / "meetings.yaml").read_text(encoding="utf-8"))
        self.assertEqual((self.root / "rooms.yaml").read_text(encoding="utf-8"), was)
        self.assertEqual(run(SCRIPTS / "provenance.py", "check", self.record)[0], 0)


if __name__ == "__main__":
    unittest.main()
