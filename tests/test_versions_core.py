"""The versions core, under examination: every scenario the branch verdict conflicts raised,
run against the isolated module with real git where a merge is the subject. Nothing here
touches the record's existing paths.

    python3 -m unittest tests.test_versions_core
"""
import os
import pathlib
import random
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
import versions as V  # noqa: E402

T = lambda n: "2026-09-%02dT12:00:00+00:00" % n     # a recorded moment, day n of September


def git(d, *args):
    return subprocess.run(["git", "-C", str(d)] + list(args), capture_output=True, text=True)


def repo(d):
    git(d, "init", "-q", "-b", "main")
    git(d, "config", "user.email", "t@t.t")
    git(d, "config", "user.name", "t")
    return d


def commit(d, msg):
    git(d, "add", "-A")
    git(d, "commit", "-qm", msg)
    return git(d, "rev-parse", "HEAD").stdout.strip()


def reading(store, subject, value, by, on, saw=(), at=None, over=()):
    """A reading as the tool writes one: the version, and the writer's acceptance of it - over
    nothing, since seeing the earlier reading is not replacing it; the source's own clock, or
    an explicit act, is what lays one reading over another."""
    v = V.version(subject, "reading", by, {"v": value}, saw=saw, at=at, on=on)
    store.keep(v)
    store.keep(V.act(subject, by, "accept", of=v["id"], over=list(over), on=on))
    return v["id"]


def judgment(store, subject, verdict, rests_on, wrong_if, by, on, saw=(), over=None, because="", accept=True):
    body = {"verdict": verdict, "because": because, "wrong_if": wrong_if, "rests_on": dict(rests_on)}
    v = V.version(subject, "judgment", by, body, saw=saw, on=on)
    store.keep(v)
    if accept and (over is not None or saw):
        store.keep(V.act(subject, by, "accept", of=v["id"], over=list(over if over is not None else saw),
                         because=because, on=on))
    return v["id"]


def base(d):
    """The base every branch starts from: a counter at 0 and a judgment resting on it."""
    s = V.Store(d)
    r0 = reading(s, "brief.runs", 0, "s.map", T(1), at={"day": "2026-09-01"})
    j0 = judgment(s, "d.not_done", "not demonstrated: no brief arrived", {"brief.runs": r0},
                  "brief.runs > 0", "s.map", T(1), because="zero briefs so far")
    return s, r0, j0


def status(s, subject):
    return V.state(s.read())["subjects"][subject]


class TheSixExperiments(unittest.TestCase):

    def test_1a_an_inherited_reading_is_not_evidence_against_a_re_reading(self):
        with tempfile.TemporaryDirectory() as d:
            s, r0, j0 = base(d)
            a, b = os.path.join(d, "a"), os.path.join(d, "b")
            shutil.copytree(d, a, ignore=shutil.ignore_patterns("a", "b"))
            shutil.copytree(d, b, ignore=shutil.ignore_patterns("a", "b"))
            reading(V.Store(a), "brief.runs", 1, "s.contra_a", T(4), saw=[r0], at={"day": "2026-09-04"})
            reading(V.Store(b), "brief.note", 1, "s.contra_b", T(4))     # b never touched the counter
            s.union_from(a)
            s.union_from(b)
            e = status(s, "brief.runs")
            self.assertEqual((e["status"], e["body"]["v"]), ("accepted", 1))
            self.assertEqual(len(e["heads"]), 1)

    def test_1b_a_re_reading_of_the_same_state_is_a_second_head(self):
        with tempfile.TemporaryDirectory() as d:
            s, r0, j0 = base(d)
            a, b = os.path.join(d, "a"), os.path.join(d, "b")
            shutil.copytree(d, a, ignore=shutil.ignore_patterns("a", "b"))
            shutil.copytree(d, b, ignore=shutil.ignore_patterns("a", "b"))
            reading(V.Store(a), "brief.runs", 1, "s.contra_a", T(4), saw=[r0], at={"day": "2026-09-04"})
            reading(V.Store(b), "brief.runs", 0, "s.contra_b", T(4), saw=[r0], at={"day": "2026-09-04"})
            s.union_from(a)
            s.union_from(b)
            e = status(s, "brief.runs")
            self.assertEqual(e["status"], "contested")
            self.assertEqual(len(e["heads"]), 2)
            # and the judgment resting on the counter carries the dispute
            self.assertEqual(status(s, "d.not_done")["deps"], {"brief.runs": "contested"})

    def test_2_the_same_source_version_read_on_another_day_is_not_an_update(self):
        with tempfile.TemporaryDirectory() as d:
            s = V.Store(d)
            a = reading(s, "report.revenue", 100, "s.a", T(1), at={"version": 7})
            # b saw a's reading, read version 7 again a day later, and found 120
            reading(s, "report.revenue", 120, "s.b", T(2), saw=[a], at={"version": 7})
            self.assertEqual(status(s, "report.revenue")["status"], "contested")

    def test_3_a_correction_the_source_publishes_the_same_day_replaces_and_marks(self):
        with tempfile.TemporaryDirectory() as d:
            s = V.Store(d)
            a = reading(s, "report.revenue", 100, "s.a", T(1), at={"version": 7})
            j = judgment(s, "d.on_track", "on track", {"report.revenue": a}, "report.revenue < 90", "s.a", T(1))
            # the source's own correction, an hour later: version 8 corrects 7
            b = reading(s, "report.revenue", 120, "rule:source", "2026-09-01T13:00:00+00:00", saw=[a],
                        at={"version": 8}, over=[])
            s.keep(V.act("report.revenue", "rule:source", "correct", of=b, over=[a],
                         because="the source's revision 8 corrects revision 7", on="2026-09-01T13:00:00+00:00"))
            e = status(s, "report.revenue")
            self.assertEqual((e["status"], e["body"]["v"]), ("accepted", 120))
            self.assertEqual(e["marks"][a], "corrected")
            # what rested on the corrected reading carries the reservation; a plain later
            # version would only have moved it
            self.assertEqual(status(s, "d.on_track")["deps"], {"report.revenue": "corrected"})
            self.assertEqual(status(s, "d.on_track")["reservations"], ["report.revenue"])

    def test_4_two_commits_measure_two_code_states_and_neither_proves_the_merge(self):
        with tempfile.TemporaryDirectory() as d:
            repo(d)
            (pathlib.Path(d) / "f").write_text("0\n")
            root = commit(d, "root")
            git(d, "switch", "-qc", "a")
            (pathlib.Path(d) / "f").write_text("a\n")
            ca = commit(d, "a")
            git(d, "switch", "-q", "main")
            git(d, "switch", "-qc", "b")
            (pathlib.Path(d) / "g").write_text("b\n")
            cb = commit(d, "b")
            git(d, "switch", "-q", "main")
            git(d, "merge", "-q", "--no-ff", "a", "-m", "m")
            git(d, "merge", "-q", "--no-ff", "b", "-m", "m")
            cm = git(d, "rev-parse", "HEAD").stdout.strip()

            def ancestry(x, y):
                return git(d, "merge-base", "--is-ancestor", x, y).returncode == 0
            s = V.Store(os.path.join(d, "rec"))
            r = reading(s, "tests.pass", True, "ci", T(1), at={"commit": root})
            reading(s, "tests.pass", True, "ci", T(2), saw=[r], at={"commit": ca})
            reading(s, "tests.pass", False, "ci", T(2), saw=[r], at={"commit": cb})
            st = V.state(s.read(), ancestry=ancestry)
            self.assertEqual(st["subjects"]["tests.pass"]["status"], "divergent")
            # the merged tree measured again settles it: a commit that descends from both
            reading(s, "tests.pass", True, "ci", T(3), at={"commit": cm})
            e = V.state(s.read(), ancestry=ancestry)["subjects"]["tests.pass"]
            self.assertEqual((e["status"], e["body"]["v"]), ("accepted", True))

    def test_5_a_fallen_conclusion_does_not_admit_its_replacement_by_itself(self):
        with tempfile.TemporaryDirectory() as d:
            s, r0, j0 = base(d)
            r1 = reading(s, "brief.runs", 1, "s.a", T(4), saw=[r0], at={"day": "2026-09-04"})
            self.assertEqual(status(s, "d.not_done")["status"], "fired")
            # the wide replacement, written and accepted by the same session
            j1 = judgment(s, "d.not_done", "every wave-1 requirement is complete", {"brief.runs": r1},
                          "brief.runs < 1", "s.a", T(4), saw=[j0], because="one brief rendered")
            e = status(s, "d.not_done")
            self.assertEqual((e["head"], e["status"]), (j1, "unreviewed"))
            self.assertEqual(e["marks"][j0], "replaced")
            # a review by another party is what makes it stand; the writer's own does not
            s.keep(V.act("d.not_done", "s.a", "review", of=j1, on=T(4)))
            self.assertEqual(status(s, "d.not_done")["status"], "unreviewed")
            s.keep(V.act("d.not_done", "ilan", "review", of=j1, on=T(5)))
            self.assertEqual(status(s, "d.not_done")["status"], "accepted")

    def test_6_replace_and_return_without_losing_a_reason(self):
        with tempfile.TemporaryDirectory() as d:
            s = V.Store(d)
            r = reading(s, "x.reading", 1, "s.a", T(1))
            q = reading(s, "x.other", 1, "s.a", T(1))
            ja = judgment(s, "d.x", "a", {"x.reading": r, "x.other": q}, "x.reading > 5", "s.a", T(1))
            jb = judgment(s, "d.x", "b", {"x.reading": r}, "x.reading > 5", "s.b", T(2), saw=[ja],
                          because="x.other no longer decides it: the margin comes from x.reading alone")
            # back to a, on the same grounds: a new version with a's body, laid over b - the
            # return is a decision of its own, and the period b stood is not erased by it
            body = s.read()["d.x"][ja]["body"]
            back = V.version("d.x", "judgment", "s.c", body, saw=[jb], on=T(3))
            s.keep(back)
            s.keep(V.act("d.x", "s.c", "accept", of=back["id"], over=[jb], because="x.other moved after all", on=T(3)))
            e = status(s, "d.x")
            self.assertEqual(e["head"], back["id"])
            self.assertEqual(e["body"], s.read()["d.x"][ja]["body"])
            self.assertEqual(e["marks"], {ja: "replaced", jb: "replaced"})
            hist = V.history(s.read(), "d.x")
            reasons = [v["body"]["because"] for v in hist if v["kind"] == "act"]
            self.assertIn("x.other no longer decides it: the margin comes from x.reading alone", reasons)
            self.assertIn("x.other moved after all", reasons)
            self.assertEqual([v["id"] for v in hist if v["kind"] == "judgment"], [ja, jb, back["id"]])


class ThePromises(unittest.TestCase):

    def contributions(self, d):
        """Three branches' worth of versions and acts, as files."""
        s, r0, j0 = base(d)
        r1 = reading(s, "brief.runs", 1, "s.a", T(4), saw=[r0], at={"day": "2026-09-04"})
        j1 = judgment(s, "d.not_done", "done", {"brief.runs": r1}, "brief.runs < 1", "s.a", T(4), saw=[j0])
        s.keep(V.act("d.not_done", "ilan", "review", of=j1, on=T(5)))
        reading(s, "brief.note", "x", "s.b", T(4))
        jb = judgment(s, "d.note", "noted", {"brief.note": [k for k in s.read()["brief.note"]][0]},
                      "brief.runs > 9", "s.b", T(4))
        s.keep(V.act("d.note", "s.c", "refute", of=jb, because="never mattered", on=T(6)))
        files = []
        for subject, held in s.read().items():
            for v in held.values():
                files.append(v)
        return files

    def test_the_same_contributions_give_the_same_state_in_any_order_or_grouping(self):
        with tempfile.TemporaryDirectory() as d:
            files = self.contributions(d)
            want = V.state(V.Store(d).read())
            rnd = random.Random(7)
            for trial in range(12):
                with tempfile.TemporaryDirectory() as e:
                    order = list(files)
                    rnd.shuffle(order)
                    cut = rnd.randrange(len(order) + 1)
                    first, rest = order[:cut], order[cut:]
                    left, right = V.Store(os.path.join(e, "l")), V.Store(os.path.join(e, "r"))
                    for v in first:
                        left.keep(v)
                    for v in rest:
                        right.keep(v)
                    left.union_from(right.root)
                    left.union_from(right.root)          # a second time changes nothing
                    self.assertEqual(V.state(left.read()), want, f"trial {trial}")

    def test_a_different_rule_version_is_a_different_state(self):
        with tempfile.TemporaryDirectory() as d:
            s, r0, j0 = base(d)
            r1 = reading(s, "brief.runs", 1, "s.a", T(4), saw=[r0], at={"day": "2026-09-04"})
            j1 = judgment(s, "d.not_done", "done", {"brief.runs": r1}, "brief.runs < 1", "s.a", T(4), saw=[j0])
            s.keep(V.act("d.not_done", "s.a", "review", of=j1, on=T(4)))
            self.assertEqual(V.state(s.read())["subjects"]["d.not_done"]["status"], "unreviewed")
            self.assertEqual(V.state(s.read(), {"self_review_counts": True})["subjects"]["d.not_done"]["status"],
                             "accepted")

    def test_a_branch_history_travels_whole(self):
        with tempfile.TemporaryDirectory() as d:
            s, r0, j0 = base(d)
            a = os.path.join(d, "a")
            shutil.copytree(d, a, ignore=shutil.ignore_patterns("a"))
            sa = V.Store(a)
            j1 = judgment(sa, "d.not_done", "b", {"brief.runs": r0}, "brief.runs > 0", "s.a", T(2), saw=[j0],
                          because="second thoughts")
            j2 = judgment(sa, "d.not_done", "c", {"brief.runs": r0}, "brief.runs > 0", "s.a", T(3), saw=[j1],
                          because="third thoughts")
            s.union_from(a)
            hist = V.history(s.read(), "d.not_done")
            self.assertEqual([v["id"] for v in hist if v["kind"] == "judgment"], [j0, j1, j2])
            self.assertEqual([v["body"]["because"] for v in hist if v["kind"] == "act"],
                             ["second thoughts", "third thoughts"])

    def test_what_rests_on_an_unreviewed_reversal_carries_it(self):
        with tempfile.TemporaryDirectory() as d:
            s, r0, j0 = base(d)
            r1 = reading(s, "brief.runs", 1, "s.a", T(4), saw=[r0], at={"day": "2026-09-04"})
            j1 = judgment(s, "d.not_done", "done", {"brief.runs": r1}, "brief.runs < 1", "s.a", T(4), saw=[j0])
            k = judgment(s, "d.ship", "ship it", {"d.not_done": j1}, "brief.runs < 1", "s.a", T(4))
            e = status(s, "d.ship")
            self.assertEqual(e["deps"], {"d.not_done": "unreviewed"})
            self.assertEqual(e["reservations"], ["d.not_done"])
            s.keep(V.act("d.not_done", "ilan", "review", of=j1, on=T(5)))
            self.assertEqual(status(s, "d.ship")["reservations"], [])
            # a proposal - a version no act accepted - carries the same, and contests nothing
            j2 = judgment(s, "d.not_done", "maybe", {"brief.runs": r1}, "brief.runs < 1", "s.z", T(6),
                          saw=[j1], accept=False)
            e = status(s, "d.not_done")
            self.assertEqual((e["head"], e["proposals"]), (j1, [j2]))
            k2 = judgment(s, "d.ship2", "ship", {"d.not_done": j2}, "brief.runs < 1", "s.z", T(6))
            self.assertEqual(status(s, "d.ship2")["deps"], {"d.not_done": "proposed"})

    def test_a_historical_dependency_is_moved_not_lost(self):
        with tempfile.TemporaryDirectory() as d:
            s = V.Store(d)
            r24 = reading(s, "revenue.latest", 100, "s.a", T(1), at={"version": "2024"})
            j = judgment(s, "d.growth", "growing", {"revenue.latest": r24}, "revenue.latest < 50", "s.a", T(1))
            reading(s, "revenue.latest", 130, "s.a", T(2), saw=[r24], at={"version": "2025"})
            e = status(s, "d.growth")
            self.assertEqual(e["deps"], {"revenue.latest": "moved"})
            self.assertEqual(e["reservations"], [])
            self.assertEqual(e["status"], "accepted")


class TheEntryFile(unittest.TestCase):

    def test_a_hand_edit_keeps_the_base_it_saw_and_meets_a_moved_head_as_contested(self):
        with tempfile.TemporaryDirectory() as d:
            s, r0, j0 = base(d)
            text, stamp = V.render(s)
            # meanwhile another reading lands - after the editor's copy was rendered
            r1 = reading(s, "brief.runs", 1, "s.a", T(4), saw=[r0], at={"day": "2026-09-04"})
            edited = text.replace("v: 0", "v: 2")
            minted = V.ingest(s, edited, by="hand", on=T(4))
            self.assertEqual(len(minted), 2)
            e = status(s, "brief.runs")
            self.assertEqual(e["status"], "contested")
            self.assertEqual(sorted(e["heads"]), sorted([r1, minted[0]]))
            self.assertEqual(s.read()["brief.runs"][minted[0]]["saw"], [r0])

    def test_a_hand_edit_on_the_current_state_is_an_update_and_an_unchanged_file_is_nothing(self):
        with tempfile.TemporaryDirectory() as d:
            s, r0, j0 = base(d)
            text, stamp = V.render(s)
            self.assertEqual(V.ingest(s, text, by="hand", on=T(2)), [])
            minted = V.ingest(s, text.replace("v: 0", "v: 2"), by="hand", on=T(2))
            e = status(s, "brief.runs")
            self.assertEqual((e["status"], e["body"]["v"], e["head"]), ("accepted", 2, minted[0]))

    def test_an_unknown_stamp_assumes_nothing_seen(self):
        with tempfile.TemporaryDirectory() as d:
            s, r0, j0 = base(d)
            text, stamp = V.render(s)
            stale = text.replace(stamp, "nothing-known").replace("v: 0", "v: 2")
            minted = V.ingest(s, stale, by="hand", on=T(2))
            e = status(s, "brief.runs")
            self.assertEqual(e["status"], "contested")
            self.assertEqual(s.read()["brief.runs"][minted[0]]["saw"], [])

    def test_a_real_git_merge_rebuilds_the_file_and_keeps_a_pending_edit(self):
        with tempfile.TemporaryDirectory() as d:
            repo(d)
            s, r0, j0 = base(d)
            V.write_entry(s)
            commit(d, "base")
            git(d, "switch", "-qc", "a")
            r1 = reading(s, "brief.runs", 1, "s.a", T(4), saw=[r0], at={"day": "2026-09-04"})
            V.write_entry(s)
            commit(d, "a")
            git(d, "switch", "-q", "main")
            reading(s, "brief.note", "x", "s.b", T(4))
            V.write_entry(s)
            commit(d, "main moved")
            # a pending hand edit on main, on the copy main rendered
            entry = pathlib.Path(d) / V.ENTRY
            entry.write_text(entry.read_text().replace("v: 0", "v: 3"), encoding="utf-8")
            ingested, added, stamp = V.take_in(s)          # the tool ingests before any merge
            self.assertEqual(len(ingested), 2)
            commit(d, "hand edit")
            m = git(d, "merge", "a")
            self.assertNotEqual(m.returncode, 0)
            porcelain = git(d, "status", "--porcelain").stdout
            self.assertIn("UU " + V.ENTRY, porcelain)                # only the rendering conflicts
            self.assertNotIn("UU .kpopper", porcelain)
            ingested, added, stamp = V.take_in(s)          # the file is rebuilt, never read
            text = entry.read_text(encoding="utf-8")
            self.assertNotIn("<<<<<<<", text)
            self.assertIn("state: " + stamp, text)
            e = status(s, "brief.runs")
            self.assertEqual(e["status"], "contested")     # a's 1 and the hand edit's 3, neither saw the other
            self.assertEqual(status(s, "brief.note")["body"]["v"], "x")

    def test_conflicting_acts_are_a_dispute_between_acts(self):
        with tempfile.TemporaryDirectory() as d:
            s, r0, j0 = base(d)
            a, b = os.path.join(d, "a"), os.path.join(d, "b")
            shutil.copytree(d, a, ignore=shutil.ignore_patterns("a", "b"))
            shutil.copytree(d, b, ignore=shutil.ignore_patterns("a", "b"))
            jx = judgment(V.Store(a), "d.not_done", "x", {"brief.runs": r0}, "brief.runs > 0", "s.a", T(2), saw=[j0])
            jy = judgment(V.Store(b), "d.not_done", "y", {"brief.runs": r0}, "brief.runs > 0", "s.b", T(2), saw=[j0])
            s.union_from(a)
            s.union_from(b)
            e = status(s, "d.not_done")
            self.assertEqual(e["status"], "contested")
            self.assertEqual(sorted(e["heads"]), sorted([jx, jy]))
            # the other order gives the same
            t = V.Store(os.path.join(d, "t"))
            t.union_from(b)
            t.union_from(a)
            self.assertEqual(V.state(t.read()), V.state(s.read()))


if __name__ == "__main__":
    unittest.main()
