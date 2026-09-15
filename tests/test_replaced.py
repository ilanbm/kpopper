"""A replacement leaves a trail: one line on the judgment that replaced another, and the
body it replaced kept whole beside the record - by add when the door admits the replacement
in place, and by the fold. Runs against the consolidate fixture, no browser, no network:

    python3 -m unittest tests.test_replaced
"""
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPTS = ROOT / "scripts"
FIXTURE = ROOT / "tests" / "fixtures" / "consolidate"

sys.path.insert(0, str(SCRIPTS))
import provenance as P  # noqa: E402

OPPOSITE = ("rests_on=[heat.boiler_kw, heat.loss_kw]", "verdict=the old boiler holds on the coldest night",
            "wrong_if=heat.loss_kw > heat.boiler_kw")
DROP = ("--drop", "heat.deficit_kw: worked out from the two it rests on")


def run(*args, cwd=None):
    p = subprocess.run([sys.executable] + [str(a) for a in args], cwd=cwd, capture_output=True, text=True)
    return p.returncode, p.stdout, p.stderr


def copy_fixture(into):
    shutil.copytree(FIXTURE, into, dirs_exist_ok=True)
    return into / "PROVENANCE.yaml"


def kept(rec):
    return P.read_replaced([str(rec)])


class TheTrailOfAReplacement(unittest.TestCase):
    """What add leaves when a judgment replaces the standing one under its id."""

    def test_a_supersede_keeps_the_body_it_replaced_and_names_what_it_dropped(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            run(SCRIPTS / "provenance.py", "set", "heat.loss_kw", "20", "--as-of", "2026-09-04", rec)
            # the door admits the replacement - the falsifier fired - but a replacement that
            # rests on less is a decision with a reason, named per dropped dependency
            code, out, err = run(SCRIPTS / "provenance.py", "add", "c.boiler_short", *OPPOSITE,
                                 "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("no longer rests on heat.deficit_kw - a dependency dropped is a decision with a "
                          "reason: add c.boiler_short", out + err)
            self.assertEqual(kept(rec), {})
            code, out, err = run(SCRIPTS / "provenance.py", "add", "c.boiler_short", *OPPOSITE,
                                 "--as-of", "2026-09-04", *DROP, rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("  kept: the replaced verdict, because, in PROVENANCE.replaced.yaml (version 1)\n"
                          "  no longer rests on heat.deficit_kw: worked out from the two it rests on\n", out)
            body = P.bodies(P.load([str(rec)]))["c.boiler_short"]
            self.assertEqual(body["replaced"],
                             ["its wrong_if holds (heat.loss_kw <= heat.boiler_kw) on 2026-09-04"])
            versions = kept(rec)["c.boiler_short"]
            self.assertEqual(len(versions), 1)
            v = versions[0]
            self.assertEqual(v["verdict"], "the old boiler cannot hold 12°C on the coldest February night")
            self.assertIn("shortfall of {{heat.deficit_kw}} kW", v["because"])
            self.assertEqual(v["rests_on"], ["heat.boiler_kw", "heat.loss_kw", "heat.deficit_kw"])
            self.assertEqual(v["wrong_if"], "heat.loss_kw <= heat.boiler_kw")
            self.assertEqual(v["seen"]["heat.loss_kw"], 31)
            self.assertEqual(v["ended"], "its wrong_if holds (heat.loss_kw <= heat.boiler_kw)")
            self.assertEqual(v["day"], "2026-09-04")
            self.assertEqual(v["dropped"], {"heat.deficit_kw": "worked out from the two it rests on"})
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", rec)[0], 0)

    def test_a_return_to_a_kept_version_is_named_and_kept_as_a_pointer(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            run(SCRIPTS / "provenance.py", "set", "heat.loss_kw", "20", "--as-of", "2026-09-04", rec)
            run(SCRIPTS / "provenance.py", "add", "c.boiler_short", *OPPOSITE, "--as-of", "2026-09-04",
                *DROP, rec)
            # the world moves back: the replacement's own condition fires, and the first
            # verdict is written again - the record says it stood before, and why it fell
            run(SCRIPTS / "provenance.py", "set", "heat.loss_kw", "45", "--as-of", "2026-09-05", rec)
            first = kept(rec)["c.boiler_short"][0]
            back = ("rests_on=[" + ", ".join(first["rests_on"]) + "]", "verdict=" + first["verdict"],
                    "because=" + first["because"], "wrong_if=" + first["wrong_if"])
            code, out, err = run(SCRIPTS / "provenance.py", "add", "c.boiler_short", *back,
                                 "--as-of", "2026-09-05", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("  returns to version 1, which stood until 2026-09-04 and fell because its "
                          "wrong_if holds (heat.loss_kw <= heat.boiler_kw)\n", out)
            body = P.bodies(P.load([str(rec)]))["c.boiler_short"]
            self.assertEqual(body["replaced"],
                             ["its wrong_if holds (heat.loss_kw <= heat.boiler_kw) on 2026-09-04",
                              "its wrong_if holds (heat.loss_kw > heat.boiler_kw) on 2026-09-05"])
            versions = kept(rec)["c.boiler_short"]
            self.assertEqual([v.get("verdict") for v in versions],
                             ["the old boiler cannot hold 12°C on the coldest February night",
                              "the old boiler holds on the coldest night"])
            # and once more: the body that now leaves is the one version 1 already keeps
            run(SCRIPTS / "provenance.py", "set", "heat.loss_kw", "20", "--as-of", "2026-09-06", rec)
            code, out, err = run(SCRIPTS / "provenance.py", "add", "c.boiler_short", *OPPOSITE,
                                 "--as-of", "2026-09-06", *DROP, rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("  returns to version 2, which stood until 2026-09-05", out)
            versions = kept(rec)["c.boiler_short"]
            self.assertEqual(len(versions), 3)
            self.assertEqual({k: versions[2][k] for k in ("same_as", "day")}, {"same_as": 1, "day": "2026-09-06"})
            self.assertNotIn("verdict", versions[2])
            self.assertEqual(P.version_at(versions, 3)["verdict"],
                             "the old boiler cannot hold 12°C on the coldest February night")

    def test_the_trail_is_written_by_the_tool_and_drop_is_shaped(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = run(SCRIPTS / "provenance.py", "add", "c.margin", "rests_on=[heat.loss_kw]",
                                 "verdict=there is a margin", "wrong_if=heat.loss_kw > 40",
                                 "replaced=[a history a session made up]", "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 1)
            self.assertIn("replaced is written by this tool, when a decision replaces another - leave it out",
                          out + err)
            code, out, err = run(SCRIPTS / "provenance.py", "add", "c.boiler_short", *OPPOSITE,
                                 "--drop", "heat.deficit_kw", rec)
            self.assertEqual(code, 1)
            self.assertIn('--drop takes "<id>: <why>"', out + err)
            code, out, err = run(SCRIPTS / "provenance.py", "set", "heat.loss_kw", "20", *DROP, rec)
            self.assertEqual(code, 1)
            self.assertIn("--drop goes with add", out + err)
            run(SCRIPTS / "provenance.py", "set", "heat.loss_kw", "20", "--as-of", "2026-09-04", rec)
            code, out, err = run(SCRIPTS / "provenance.py", "add", "c.boiler_short", *OPPOSITE,
                                 "--as-of", "2026-09-04", *DROP, "--drop", "heat.boiler_kw: still there", rec)
            self.assertEqual(code, 1)
            self.assertIn("--drop names heat.boiler_kw, which the new judgment still rests on", out + err)


class TheTrailOfAFold(unittest.TestCase):
    """The fold leaves the same trail add does."""

    def test_the_fold_keeps_what_it_replaced(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            run(SCRIPTS / "provenance.py", "set", "heat.loss_kw", "20", "--as-of", "2026-09-04", rec)
            code, out, err = run(SCRIPTS / "provenance.py", "add", "c.boiler_short", *OPPOSITE,
                                 "--as-of", "2026-09-04", "--hypothesis", "repair", rec)
            self.assertEqual(code, 0, out + err)
            self.assertEqual(kept(rec), {})
            code, out, err = run(SCRIPTS / "consolidate.py", "repair", "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("replace c.boiler_short with what repair holds, where it stands - what it replaced "
                          "kept\n  kept: the replaced verdict, because, in PROVENANCE.replaced.yaml\n"
                          "  no longer rests on heat.deficit_kw\n", out)
            self.assertIn("files to commit: PROVENANCE.yaml, PROVENANCE.replaced.yaml, "
                          "PROVENANCE.d/repair.yaml (deleted)\n", out)
            body = P.bodies(P.load([str(rec)]))["c.boiler_short"]
            self.assertEqual(body["replaced"],
                             ["its wrong_if holds (heat.loss_kw <= heat.boiler_kw) on 2026-09-04"])
            v = kept(rec)["c.boiler_short"][0]
            self.assertEqual(v["verdict"], "the old boiler cannot hold 12°C on the coldest February night")
            self.assertEqual(v["ended"], "its wrong_if holds (heat.loss_kw <= heat.boiler_kw)")
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", rec)[0], 0)


class TheLayout(unittest.TestCase):
    def test_the_layout_names_where_kept_versions_live(self):
        self.assertEqual(os.path.relpath(P.layout("/p/GROUNDING.yaml")["replaced"], "/p"), ".kpopper/replaced.yaml")
        self.assertEqual(os.path.relpath(P.layout("/p/PROVENANCE.yaml")["replaced"], "/p"), "PROVENANCE.replaced.yaml")
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            (pathlib.Path(d) / ".kpopper").mkdir()
            (pathlib.Path(d) / ".kpopper" / "replaced.yaml").write_text("c.x: []\n", encoding="utf-8")
            self.assertIn((".kpopper/replaced.yaml", "PROVENANCE.replaced.yaml"), P.leftovers([str(rec)]))


if __name__ == "__main__":
    unittest.main()
