import contextlib
import io
import multiprocessing
import os
import pathlib
import queue
import sys
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPTS = ROOT / "scripts"


def _first_add(directory, entry, pause_birth, publish_before_wait, attempted,
               finished_locked_write, started, result):
    """Run one writer in its own process, optionally stopping inside the newborn write."""
    sys.path.insert(0, str(SCRIPTS))
    import provenance as P

    os.chdir(directory)
    if pause_birth:
        write = P._write_text
        locked = P._locked
        under_lock = [False]

        @contextlib.contextmanager
        def observed_lock(path, **kwargs):
            with locked(path, **kwargs):
                under_lock[0] = True
                try:
                    yield
                finally:
                    under_lock[0] = False

        P._locked = observed_lock

        def held_write(path, text):
            if text.startswith(P.HEAD_LINE + "meta:\n  updated:"):
                if publish_before_wait:
                    write(path, text)
                started.set()
                # The old path reaches birth outside the record lock, so let writer two
                # finish its locked mutation before this replace. The fixed path reaches
                # birth while holding the lock, so let writer two attempt that lock before
                # continuing. This gives each implementation a deterministic schedule and
                # makes the old overwrite happen without relying on process timing.
                wait_for = attempted if under_lock[0] else finished_locked_write
                if not wait_for.wait(10):
                    raise RuntimeError("the other writer did not reach the synchronized boundary")
                if publish_before_wait:
                    return None
            return write(path, text)

        P._write_text = held_write
    else:
        locked = P._locked

        @contextlib.contextmanager
        def announced_lock(path, **kwargs):
            # The attempted event releases a first writer that already owns the fixed
            # birth lock. The finished event releases a historical first writer whose
            # newborn replace still sits outside the lock.
            started.set()
            attempted.set()
            with locked(path, **kwargs):
                yield
            finished_locked_write.set()

        P._locked = announced_lock

    output = io.StringIO()
    try:
        with contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
            code = P.write_command("add", entry)
    except BaseException as error:
        result.put(("error", type(error).__name__, str(error), output.getvalue()))
    else:
        result.put(("ok", code, output.getvalue()))


class ConcurrentBirth(unittest.TestCase):
    def setUp(self):
        if os.name == "nt" or "fork" not in multiprocessing.get_all_start_methods():
            self.skipTest("record locking and this process harness require POSIX fcntl/fork")
        try:
            import fcntl  # noqa: F401
        except ImportError:
            self.skipTest("record locking requires fcntl")
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.directory = pathlib.Path(self.tmp.name).resolve()
        self.context = multiprocessing.get_context("fork")

    def writers(self, first, second, publish_before_wait=False):
        attempted = self.context.Event()
        finished_locked_write = self.context.Event()
        first_started = self.context.Event()
        second_started = self.context.Event()
        result = self.context.Queue()
        one = self.context.Process(
            target=_first_add,
            args=(str(self.directory), first, True, publish_before_wait, attempted, finished_locked_write,
                  first_started, result),
        )
        two = self.context.Process(
            target=_first_add,
            args=(str(self.directory), second, False, False, attempted, finished_locked_write,
                  second_started, result),
        )
        one.start()
        self.assertTrue(first_started.wait(10), "first writer did not reach the newborn write")
        two.start()
        self.assertTrue(second_started.wait(10), "second writer did not start")
        for process in (one, two):
            process.join(10)
            if process.is_alive():
                process.terminate()
                process.join()
                self.fail("a synchronized writer did not finish")
        outcomes = []
        try:
            outcomes.extend(result.get(timeout=2) for _ in range(2))
        except queue.Empty:
            self.fail("a writer exited without reporting its result")
        return outcomes

    def test_two_successful_first_adds_are_serialized_without_losing_either_entry(self):
        outcomes = self.writers(
            ["fact.first", "v=1", "--as-of", "2026-09-12"],
            ["fact.second", "v=2", "--as-of", "2026-09-12"],
        )
        self.assertEqual([outcome[0] for outcome in outcomes], ["ok", "ok"], outcomes)
        text = (self.directory / "GROUNDING.yaml").read_text(encoding="utf-8")
        self.assertIn("  fact.first:", text)
        self.assertIn("  fact.second:", text)

    def test_waiting_writer_can_create_after_the_first_add_is_refused_and_cleaned_up(self):
        outcomes = self.writers(
            ["d.first", "verdict=a conclusion", "because=nothing", "rests_on=[s.nowhere]",
             "reopened_by=anything", "--as-of", "2026-09-12"],
            ["fact.second", "v=2", "--as-of", "2026-09-12"],
        )
        self.assertEqual(sorted(outcome[0] for outcome in outcomes), ["error", "ok"], outcomes)
        refused = next(outcome for outcome in outcomes if outcome[0] == "error")
        self.assertIn("refused", (refused[2] + refused[3]).lower())
        text = (self.directory / "GROUNDING.yaml").read_text(encoding="utf-8")
        self.assertIn("  fact.second:", text)
        self.assertNotIn("  d.first:", text)

    def test_waiting_writer_rechecks_after_a_published_newborn_is_refused_and_cleaned_up(self):
        outcomes = self.writers(
            ["d.first", "verdict=a conclusion", "because=nothing", "rests_on=[s.nowhere]",
             "reopened_by=anything", "--as-of", "2026-09-12"],
            ["fact.second", "v=2", "--as-of", "2026-09-12"],
            publish_before_wait=True,
        )
        self.assertEqual(sorted(outcome[0] for outcome in outcomes), ["error", "ok"], outcomes)
        text = (self.directory / "GROUNDING.yaml").read_text(encoding="utf-8")
        self.assertIn("  fact.second:", text)
        self.assertNotIn("  d.first:", text)


if __name__ == "__main__":
    unittest.main()
