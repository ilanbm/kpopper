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
    """A reading as the tool writes one: a root stands by itself; a later one is accepted by
    its writer - over nothing, since seeing the earlier reading is not replacing it; the
    source's own clock, or an explicit act, is what lays one reading over another - and the
    acceptance names the open acts on what it is laid over, answering them."""
    v = V.version(subject, "reading", by, {"v": value}, saw=saw, at=at, on=on)
    store.keep(v)
    if saw or over:
        store.keep(V.act(subject, by, "accept", of=v["id"], over=list(over), on=on,
                         saw=store.open_acts(subject, list(over))))
    return v["id"]


def judgment(store, subject, verdict, rests_on, wrong_if, by, on, saw=(), over=None, because="", accept=True):
    body = {"verdict": verdict, "because": because, "wrong_if": wrong_if, "rests_on": dict(rests_on)}
    v = V.version(subject, "judgment", by, body, saw=saw, on=on)
    store.keep(v)
    if accept and (over is not None or saw):
        over = list(over if over is not None else saw)
        store.keep(V.act(subject, by, "accept", of=v["id"], over=over, because=because, on=on,
                         saw=store.open_acts(subject, over)))
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
            a = reading(s, "report.revenue", 100, "s.a", T(1), at={"revision": 7})
            # b saw a's reading, read version 7 again a day later, and found 120
            reading(s, "report.revenue", 120, "s.b", T(2), saw=[a], at={"revision": 7})
            self.assertEqual(status(s, "report.revenue")["status"], "contested")

    def test_3_a_correction_the_source_publishes_the_same_day_replaces_and_marks(self):
        with tempfile.TemporaryDirectory() as d:
            s = V.Store(d)
            a = reading(s, "report.revenue", 100, "s.a", T(1), at={"revision": 7})
            j = judgment(s, "d.on_track", "on track", {"report.revenue": a}, "report.revenue < 90", "s.a", T(1))
            # the source's own correction, an hour later: version 8 corrects 7
            b = reading(s, "report.revenue", 120, "rule:source", "2026-09-01T13:00:00+00:00", saw=[a],
                        at={"revision": 8}, over=[])
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
            s.keep(V.act("d.x", "s.c", "accept", of=back["id"], over=[jb], because="x.other moved after all", on=T(3),
                         saw=s.open_acts("d.x", [jb])))
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
            r24 = reading(s, "revenue.latest", 100, "s.a", T(1), at={"revision": "2024"})
            j = judgment(s, "d.growth", "growing", {"revenue.latest": r24}, "revenue.latest < 50", "s.a", T(1))
            reading(s, "revenue.latest", 130, "s.a", T(2), saw=[r24], at={"revision": "2025"})
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

    def test_a_file_that_carries_no_heads_is_an_unknown_base(self):
        with tempfile.TemporaryDirectory() as d:
            s, r0, j0 = base(d)
            text, stamp = V.render(s)
            doc = V.yaml.safe_load(text)
            del doc["meta"]["heads"]
            doc["subjects"]["brief.runs"]["v"] = 2
            minted = V.ingest(s, V.yaml.safe_dump(doc, sort_keys=False), by="hand", on=T(2))
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


class TheCostOfHistory(unittest.TestCase):
    """More history behind the same state must not make the next open read it all again."""

    def test_a_retry_of_one_operation_is_one_version_and_a_new_observation_is_another(self):
        with tempfile.TemporaryDirectory() as d:
            s = V.Store(d)
            a = V.version("x", "reading", "s.a", {"v": 1}, on=T(1), op="op-1")
            again = V.version("x", "reading", "s.a", {"v": 1}, on=T(1), op="op-1")
            self.assertEqual(a["id"], again["id"])
            s.keep(a)
            s.keep(again)
            self.assertEqual(len(s.read()["x"]), 1)
            # the same actor finding the same value again is a new observation
            fresh = V.version("x", "reading", "s.a", {"v": 1}, on=T(2))
            self.assertNotEqual(fresh["id"], a["id"])
            s.keep(fresh)
            self.assertEqual(len(s.read()["x"]), 2)
            # a retry sends the same envelope: another moment under the same operation is a
            # different version, so a writer keeps what it first sent
            self.assertNotEqual(V.version("x", "reading", "s.a", {"v": 1}, on=T(3), op="op-1")["id"], a["id"])

    def test_a_retry_in_two_replicas_merges_clean_under_git_and_in_both_orders(self):
        with tempfile.TemporaryDirectory() as d:
            repo(d)
            s = V.Store(d)
            r0 = reading(s, "x", 0, "s.map", T(1), at={"revision": 1})
            V.write_entry(s)
            commit(d, "base")
            same = V.version("x", "reading", "s.a", {"v": 1}, saw=[r0], at={"revision": 2}, on=T(2), op="op-7")
            for br in ("a", "b"):
                git(d, "switch", "-qc", br, "main")
                s.keep(same)
                s.keep(V.act("x", "s.a", "accept", of=same["id"], on=T(2), op="op-7-accept"))
                commit(d, br)
            git(d, "switch", "-q", "main")
            self.assertEqual(git(d, "merge", "-q", "a").returncode, 0)
            self.assertEqual(git(d, "merge", "-q", "b").returncode, 0, "the same version in both replicas merges clean")
            ab, ba = V.Store(os.path.join(d, "ab")), V.Store(os.path.join(d, "ba"))
            for st, order in ((ab, ("a", "b")), (ba, ("b", "a"))):
                for br in order:
                    git(d, "switch", "-q", br)
                    st.union_from(d)
            git(d, "switch", "-q", "main")
            self.assertEqual(V.state(ab.read()), V.state(ba.read()))
            self.assertEqual(V.state(ab.read())["subjects"]["x"]["body"]["v"], 1)

    def test_an_open_after_a_thousand_contributions_parses_nothing_it_already_knows(self):
        n = int(os.environ.get("VERSIONS_BENCH", "1000"))
        with tempfile.TemporaryDirectory() as d:
            import time
            s = V.Store(d)
            rnd = random.Random(3)
            subjects = ["m.reading_%d" % i for i in range(20)]
            heads = {}
            for i, subj in enumerate(subjects):
                heads[subj] = reading(s, subj, i, "s.map", T(1), at={"revision": 0})
            js = []
            for i in range(3):
                js.append(judgment(s, "d.judgment_%d" % i, "verdict %d" % i,
                                   {subjects[i]: heads[subjects[i]]}, subjects[i] + " > 1000", "s.map", T(1)))
            t0 = time.time()
            written = 0
            while written < n:
                kind = rnd.random()
                if kind < 0.6:
                    subj = rnd.choice(subjects)
                    heads[subj] = reading(s, subj, rnd.randrange(100), "s.%d" % rnd.randrange(9),
                                          "2026-09-%02dT%02d:%02d:00+00:00" % (2 + written // 500, (written // 60) % 24, written % 60),
                                          saw=[heads[subj]], at={"revision": written + 1})
                    written += 2
                else:
                    j = rnd.choice(js)     # many reviews on few judgments
                    s.keep(V.act("d.judgment_%d" % js.index(j), "s.%d" % rnd.randrange(9), "review", of=j,
                                 on="2026-09-%02dT%02d:%02d:00+00:00" % (2 + written // 500, (written // 60) % 24, written % 60)))
                    written += 1
            wrote = time.time() - t0
            s.settle()                      # as the tool does after its writes
            files = sum(s.subjects().values())
            V.Store.parsed = 0
            t0 = time.time(); full = s.state(fresh=True); t_full = time.time() - t0
            parsed_full = V.Store.parsed
            V.Store.parsed = 0
            t0 = time.time(); again = s.state(); t_open = time.time() - t0
            parsed_open = V.Store.parsed
            self.assertEqual(again, full)
            self.assertEqual(parsed_open, 0)
            # one more reading parses that subject's files only
            reading(s, subjects[0], 1, "s.z", T(30), saw=[heads[subjects[0]]], at={"revision": 99999999})
            V.Store.parsed = 0
            t0 = time.time(); one = s.state(); t_one = time.time() - t0
            parsed_one = V.Store.parsed
            self.assertLessEqual(parsed_one, s.subjects()[subjects[0]])
            self.assertEqual(one["subjects"][subjects[0]]["body"]["v"], 1)
            t0 = time.time(); rebuilt = s.state(fresh=True); t_rebuild = time.time() - t0
            self.assertEqual(one, rebuilt)
            s.settle()                      # the act the update implied, recorded as the tool would
            # the whole path: a rendering, and an unchanged file taken in again - no snapshot
            # of the state is kept anywhere, and nothing already known is parsed
            t0 = time.time(); V.write_entry(s); t_render = time.time() - t0
            V.Store.parsed = 0
            t0 = time.time(); V.take_in(s); t_take = time.time() - t0
            self.assertEqual(V.Store.parsed, 0)
            self.assertFalse(os.path.isdir(os.path.join(s.dir, ".stamps")))
            size = sum(os.path.getsize(os.path.join(r, f)) for r, _, fs in os.walk(s.dir) for f in fs)
            sys.stderr.write("\n  history: %d files, %.1f KiB, written in %.1fs; full state %.2fs (%d parsed); "
                             "open %.3fs (%d parsed); one update %.3fs (%d parsed); rebuild %.2fs; "
                             "render %.3fs; unchanged take_in %.3fs (0 parsed)\n"
                             % (files, size / 1024, wrote, t_full, parsed_full, t_open, parsed_open, t_one,
                                parsed_one, t_rebuild, t_render, t_take))


class TheCounterexamples(unittest.TestCase):
    """The probes of two review rounds, each a case the contract names, kept as tests."""

    def test_two_roots_that_never_met_are_two_heads_and_no_moment_decides(self):
        with tempfile.TemporaryDirectory() as d:
            a, b = V.Store(os.path.join(d, "a")), V.Store(os.path.join(d, "b"))
            reading(a, "x", 100, "s.a", T(2), at={"revision": 7})
            reading(b, "x", 120, "s.b", T(1), at={"revision": 7})     # recorded earlier
            a.union_from(b.root)
            self.assertEqual(status(a, "x")["status"], "contested")
            # and two roots that say the same are one claim held twice
            c = V.Store(os.path.join(d, "c"))
            reading(c, "y", 1, "s.a", T(2), at={"revision": 7})
            reading(c, "y", 1, "s.b", T(1), at={"revision": 7})
            e = status(c, "y")
            self.assertEqual((e["status"], e["agreed"], e["body"]["v"]), ("accepted", 2, 1))

    def test_a_later_clock_of_another_source_supersedes_nothing(self):
        with tempfile.TemporaryDirectory() as d:
            s = V.Store(d)
            v = V.version("x", "reading", "s.a", {"v": 100, "from": "source-a"}, at={"revision": 1}, on=T(1))
            s.keep(v)
            w = V.version("x", "reading", "s.b", {"v": 120, "from": "source-b"}, saw=[v["id"]], at={"revision": 2}, on=T(2))
            s.keep(w)
            s.keep(V.act("x", "s.b", "accept", of=w["id"], on=T(2)))
            self.assertEqual(status(s, "x")["status"], "contested")
            # the same source, a later revision: superseded by rule, and settle records it
            u = V.version("x", "reading", "s.c", {"v": 130, "from": "source-b"}, saw=[w["id"]], at={"revision": 3}, on=T(3))
            s.keep(u)
            s.keep(V.act("x", "s.c", "accept", of=u["id"], on=T(3)))
            st = s.state()
            self.assertEqual([i["superseded"] for i in st["implied"]], [w["id"]])
            written = s.settle()
            self.assertEqual(len(written), 1)
            self.assertEqual(s.state()["implied"], [])
            self.assertEqual(s.read()["x"][written[0]]["by"], V.RULE_ACTOR)

    def test_a_reservation_travels_down_a_chain_and_a_fired_premise_is_one(self):
        with tempfile.TemporaryDirectory() as d:
            s, r0, j0 = base(d)
            r1 = reading(s, "brief.runs", 1, "s.a", T(4), saw=[r0], at={"day": "2026-09-04"})
            j1 = judgment(s, "d.not_done", "done", {"brief.runs": r1}, "brief.runs < 1", "s.a", T(4), saw=[j0])
            k = judgment(s, "d.k", "k", {"d.not_done": j1}, "brief.runs < 1", "s.a", T(4))
            l = judgment(s, "d.l", "l", {"d.k": k}, "brief.runs < 1", "s.a", T(4))
            self.assertEqual(status(s, "d.k")["reservations"], ["d.not_done"])
            self.assertEqual(status(s, "d.l")["deps"], {"d.k": "reserved"})
            self.assertEqual(status(s, "d.l")["reservations"], ["d.k"])
            # a fired premise: what rests on it says so
            reading(s, "brief.runs", 0, "s.b", T(5), saw=[r1], at={"day": "2026-09-05"})
            self.assertEqual(status(s, "d.not_done")["status"], "fired")
            self.assertEqual(status(s, "d.k")["deps"], {"d.not_done": "fired"})
            # a dependency version nothing holds leaves the judgment unresolved
            m = judgment(s, "d.m", "m", {"brief.runs": "nothing-like-this"}, "brief.runs < 1", "s.a", T(5))
            e = status(s, "d.m")
            self.assertEqual((e["status"], e["deps"]), ("unresolved", {"brief.runs": "unresolved"}))

    def test_a_file_that_does_not_say_what_its_name_says_is_a_problem_not_a_version(self):
        with tempfile.TemporaryDirectory() as d:
            s = V.Store(d)
            r = reading(s, "x", 1, "s.a", T(1))
            p = os.path.join(s.dir, "x", r + ".yaml")
            pathlib.Path(p).write_text(pathlib.Path(p).read_text().replace("v: 1", "v: 999"), encoding="utf-8")
            held = s.read()
            self.assertNotIn(r, held.get("x", {}))
            self.assertTrue(any("does not say what its name says" in m for m in s.problems))
            v = V.version("x", "reading", "s.a", {"v": 1}, on=T(1), op="k")
            s.keep(v)
            other = dict(v, body={"v": 2})
            with self.assertRaises(V.IntegrityError):
                s.keep(other)

    def test_an_acceptance_and_a_refutation_that_never_met_are_a_dispute_between_acts(self):
        with tempfile.TemporaryDirectory() as d:
            s, r0, j0 = base(d)
            j1 = V.version("d.not_done", "judgment", "s.a", {"verdict": "x", "wrong_if": "brief.runs > 9",
                                                             "rests_on": {"brief.runs": r0}}, saw=[j0], on=T(2))
            s.keep(j1)
            accept = V.act("d.not_done", "s.a", "accept", of=j1["id"], over=[j0], on=T(2))
            refute = V.act("d.not_done", "s.b", "refute", of=j1["id"], because="no", on=T(2))
            s.keep(accept)
            s.keep(refute)
            e = status(s, "d.not_done")
            self.assertEqual(e["status"], "contested")
            self.assertEqual(e["disputed_acts"], [j1["id"]])
            # the refutation that saw the acceptance answers it
            t = V.Store(os.path.join(d, "t"))
            t.keep(s.read()["brief.runs"][r0]); t.keep(s.read()["d.not_done"][j0]); t.keep(j1); t.keep(accept)
            refute2 = V.act("d.not_done", "s.b", "refute", of=j1["id"], because="no", saw=[accept["id"]], on=T(3))
            t.keep(refute2)
            e = status(t, "d.not_done")
            self.assertEqual(e["marks"][j1["id"]], "refuted")
            # refuting the replacement restores nothing by itself: what it replaced stays replaced,
            # and the subject stands empty until someone returns to it with a reason
            self.assertEqual((e["status"], e["heads"]), ("empty", []))
            back = V.act("d.not_done", "ilan", "accept", of=j0, over=[j1["id"]], because="the first reading held",
                         saw=[accept["id"], refute2["id"]], on=T(4))
            t.keep(back)
            e = status(t, "d.not_done")
            self.assertEqual((e["status"], e["head"]), ("accepted", j0))

    def test_a_thousand_equal_observations_are_one_claim_and_cost_nothing_much(self):
        import time
        with tempfile.TemporaryDirectory() as d:
            s = V.Store(d)
            for i in range(1000):
                reading(s, "x", 1, "s.%d" % i, T(1), at={"revision": 7})
            t0 = time.time(); e = status(s, "x"); took = time.time() - t0
            self.assertEqual((e["status"], e["agreed"]), ("accepted", 1000))
            self.assertLess(took, 3.0)

    def test_a_rendering_keeps_no_snapshot_beside_the_files(self):
        with tempfile.TemporaryDirectory() as d:
            s, r0, j0 = base(d)
            for i in range(50):
                reading(s, "n.%d" % i, i, "s.a", T(1))
                V.write_entry(s)
            names = {n for _, _, fs in os.walk(s.dir) for n in fs}
            self.assertEqual({n for n in names if not n.endswith(".yaml")}, {".index.json", ".gitignore", "README"})

    def test_a_deleted_subject_is_refused_and_the_file_left_as_it_is(self):
        with tempfile.TemporaryDirectory() as d:
            s, r0, j0 = base(d)
            V.write_entry(s)
            entry = pathlib.Path(d) / V.ENTRY
            doc = V.yaml.safe_load(entry.read_text(encoding="utf-8"))
            del doc["subjects"]["brief.runs"]
            edited = V.yaml.safe_dump(doc, sort_keys=False)
            entry.write_text(edited, encoding="utf-8")
            with self.assertRaises(V.EditRefused):
                V.take_in(s)
            self.assertEqual(entry.read_text(encoding="utf-8"), edited)
            self.assertIn("brief.runs", status(s, "brief.runs") and s.ids())

    def test_a_stamp_is_ordered_as_an_instant_not_as_text(self):
        self.assertEqual(V.clock_order({"stamp": "2026-09-01T09:00:00+03:00"}, {"stamp": "2026-09-01T08:00:00+00:00"}), -1)
        self.assertEqual(V.clock_order({"stamp": "2026-09-01T09:00:00Z"}, {"stamp": "2026-09-01T09:00:00+00:00"}), 0)
        self.assertIsNone(V.clock_order({"stamp": "yesterday"}, {"stamp": "2026-09-01T09:00:00Z"}))

    def test_a_review_covers_its_version_while_what_it_read_still_stands(self):
        with tempfile.TemporaryDirectory() as d:
            s, r0, j0 = base(d)
            r1 = reading(s, "brief.runs", 1, "s.a", T(4), saw=[r0], at={"day": "2026-09-04"})
            j1 = judgment(s, "d.not_done", "done", {"brief.runs": r1}, "brief.runs < 1", "s.a", T(4), saw=[j0])
            s.keep(V.act("d.not_done", "ilan", "review", of=j1, read={"brief.runs": r1}, on=T(5)))
            self.assertEqual(status(s, "d.not_done")["status"], "accepted")
            reading(s, "brief.runs", 2, "s.c", T(6), saw=[r1], at={"day": "2026-09-06"})
            self.assertEqual(status(s, "d.not_done")["status"], "unreviewed")

    def test_the_index_is_checked_against_the_files_not_their_number(self):
        with tempfile.TemporaryDirectory() as d:
            repo(d)
            s = V.Store(d)
            r0 = reading(s, "x", 0, "s.map", T(1), at={"revision": 0})
            commit(d, "base")
            git(d, "switch", "-qc", "a")
            reading(s, "x", 1, "s.a", T(2), saw=[r0], at={"revision": 1})
            commit(d, "a")
            git(d, "switch", "-qc", "b", "main")
            reading(s, "x", 2, "s.b", T(2), saw=[r0], at={"revision": 1})
            commit(d, "b")
            git(d, "switch", "-q", "a")
            self.assertEqual(s.state()["subjects"]["x"]["body"]["v"], 1)          # the index warms on a
            git(d, "switch", "-q", "b")
            self.assertEqual(s.subjects(), {"x": 3})                              # as many files as on a
            self.assertEqual(s.state()["subjects"]["x"]["body"]["v"], 2)
            self.assertEqual(s.state(), s.state(fresh=True))


class TheActsThatRemainOpen(unittest.TestCase):
    """A claim is decided by the acts no later act answered - checked on short histories, in
    sequence and in branches, against a declarative reading of the same rule."""

    def setting(self, d):
        s = V.Store(d)
        r = reading(s, "x", 1, "s.map", T(1))
        return s, r

    def test_a_refutation_that_saw_a_root_refutes_it(self):
        with tempfile.TemporaryDirectory() as d:
            s, r = self.setting(d)
            s.keep(V.act("x", "s.b", "refute", of=r, because="misread", saw=[r], on=T(2)))
            e = status(s, "x")
            self.assertEqual((e["status"], e["marks"].get(r)), ("empty", "refuted"))

    def test_a_chain_of_three_acts_ends_where_the_last_one_says(self):
        with tempfile.TemporaryDirectory() as d:
            s = V.Store(d)
            v = V.version("x", "reading", "s.a", {"v": 5}, saw=["nothing"], on=T(1))
            s.keep(v)
            a1 = V.act("x", "s.a", "accept", of=v["id"], on=T(1))
            r = V.act("x", "s.b", "refute", of=v["id"], because="no", saw=[a1["id"]], on=T(2))
            a2 = V.act("x", "s.c", "accept", of=v["id"], because="yes after all", saw=[r["id"]], on=T(3))
            for a in (a1, r, a2):
                s.keep(a)
            e = status(s, "x")
            self.assertEqual((e["status"], e["head"], e["accepted_by"]), ("accepted", v["id"], "s.c"))

    def test_a_refutation_that_saw_one_of_two_acceptances_answers_only_that_one(self):
        with tempfile.TemporaryDirectory() as d:
            s = V.Store(d)
            v = V.version("x", "reading", "s.a", {"v": 5}, saw=["nothing"], on=T(1))
            s.keep(v)
            a1 = V.act("x", "s.a", "accept", of=v["id"], on=T(1))
            r = V.act("x", "s.b", "refute", of=v["id"], because="no", saw=[a1["id"]], on=T(2))
            a2 = V.act("x", "s.c", "accept", of=v["id"], on=T(2))          # never met the refutation
            for a in (a1, r, a2):
                s.keep(a)
            e = status(s, "x")
            self.assertEqual(e["status"], "contested")
            self.assertEqual(e["disputed_acts"], [v["id"]])

    def test_short_histories_against_the_declarative_rule(self):
        rnd = random.Random(11)
        for trial in range(120):
            with tempfile.TemporaryDirectory() as d:
                s = V.Store(d)
                root = rnd.random() < 0.5
                v = V.version("x", "reading", "s.w", {"v": 1}, saw=[] if root else ["elsewhere"], on=T(1))
                s.keep(v)
                acts, words = [], []
                for i in range(rnd.randrange(0, 5)):
                    kind = rnd.choice(["accept", "refute"])
                    saw = [a["id"] for a in acts if rnd.random() < 0.5]
                    a = V.act("x", "s.%d" % i, kind, of=v["id"], saw=saw, on=T(2 + i))
                    acts.append(a)
                    words.append((a["id"], "stands" if kind == "accept" else "out", set(saw)))
                # the rule, said once more in other words: an act is answered when a later act
                # names it; the open acts decide, and a root stands with no act at all
                answered = {i for i, _, _ in words for _, _, saw in words if i in saw}
                open_kinds = {w for i, w, _ in words if i not in answered}
                if not words:
                    expect = "accepted" if root else "proposed"
                elif open_kinds == {"stands"}:
                    expect = "accepted"
                elif open_kinds == {"out"}:
                    expect = "empty"
                else:
                    expect = "contested"
                order = list(acts)
                rnd.shuffle(order)
                for a in order:
                    s.keep(a)
                e = status(s, "x")
                got = "proposed" if e["proposals"] == [v["id"]] and e["status"] == "empty" else e["status"]
                self.assertEqual(got, expect, "trial %d: root=%s words=%s" % (trial, root, words))

    def test_an_untouched_rendering_of_an_agreement_minted_nothing_after_the_source_moved(self):
        with tempfile.TemporaryDirectory() as d:
            s = V.Store(d)
            a = reading(s, "x", 1, "s.a", T(1), at={"revision": 7})
            b = reading(s, "x", 1, "s.b", T(1), at={"revision": 7})
            text, stamp = V.render(s)
            self.assertEqual(status(s, "x")["agreed"], 2)
            reading(s, "x", 2, "s.c", T(2), saw=[a, b], at={"revision": 8})
            self.assertEqual(V.ingest(s, text, by="hand", on=T(3)), [])
            self.assertEqual((status(s, "x")["status"], status(s, "x")["body"]["v"]), ("accepted", 2))

    def test_a_problem_the_files_show_is_still_shown_from_the_index(self):
        with tempfile.TemporaryDirectory() as d:
            s = V.Store(d)
            good = reading(s, "x", 1, "s.a", T(1))
            bad = V.version("x", "reading", "s.b", {"v": 1}, on=T(2))
            s.keep(bad)
            p = os.path.join(s.dir, "x", bad["id"] + ".yaml")
            pathlib.Path(p).write_text(pathlib.Path(p).read_text().replace("v: 1", "v: 999"), encoding="utf-8")
            first = s.state(fresh=True)
            self.assertEqual(len(first["problems"]), 1)
            again = V.Store(d).state()                      # a new reader, the index warm, no file changed
            self.assertEqual(again["problems"], first["problems"])
            self.assertEqual(again["subjects"], first["subjects"])


    def test_an_edit_answers_only_the_acts_its_copy_showed(self):
        with tempfile.TemporaryDirectory() as d:
            s = V.Store(d)
            a = reading(s, "x", 1, "s.a", T(1), at={"revision": 7})
            j = judgment(s, "d.j", "j", {"x": a}, "x > 5", "s.a", T(1))
            text, stamp = V.render(s)
            # after the copy was rendered, the reading is refuted - what rests on it says so
            s.keep(V.act("x", "s.b", "refute", of=a, because="misread", saw=[a], on=T(2)))
            self.assertEqual(status(s, "d.j")["deps"], {"x": "refuted"})
            # the old copy is edited and taken in: its acceptance answers nothing it never saw
            V.ingest(s, text.replace("v: 1", "v: 3"), by="hand", on=T(3))
            e = status(s, "x")
            self.assertEqual((e["status"], e["body"]["v"], e["marks"][a]), ("accepted", 3, "refuted"))
            self.assertEqual(status(s, "d.j")["deps"], {"x": "refuted"})
            self.assertEqual(status(s, "d.j")["reservations"], ["x"])

    def test_a_corroborating_observation_does_not_unreview_a_decision(self):
        with tempfile.TemporaryDirectory() as d:
            s = V.Store(d)
            r1 = reading(s, "x", 1, "s.a", T(1), at={"revision": 7})
            j = judgment(s, "d.j", "j", {"x": r1}, "x > 5", "s.a", T(1), saw=["elsewhere"])
            s.keep(V.act("d.j", "ilan", "review", of=j, read={"x": r1}, on=T(2)))
            self.assertEqual(status(s, "d.j")["status"], "accepted")
            r2 = reading(s, "x", 1, "s.b", T(3), at={"revision": 7})     # the same, seen again
            self.assertEqual(sorted(status(s, "x")["heads"]), sorted([r1, r2]))
            self.assertEqual(status(s, "d.j")["status"], "accepted")


if __name__ == "__main__":
    unittest.main()
