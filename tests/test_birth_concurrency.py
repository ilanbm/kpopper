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


def _first_add(directory, entry, pause_birth, attempted, started, result):
    """Run one writer in its own process, optionally stopping inside bootstrap prepare."""
    sys.path.insert(0, str(SCRIPTS))
    import provenance as P

    os.chdir(directory)
    if pause_birth:
        bootstrap = P._peer('history_bootstrap')
        prepare = bootstrap.prepare
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

        def held_prepare(*args, **kwargs):
            if not under_lock[0]:
                raise RuntimeError("bootstrap preparation escaped the record lock")
            if pathlib.Path(directory, 'GROUNDING.yaml').exists():
                raise RuntimeError("a newborn became visible before validation completed")
            started.set()
            if not attempted.wait(10):
                raise RuntimeError("the other writer did not reach the synchronized boundary")
            return prepare(*args, **kwargs)

        bootstrap.prepare = held_prepare
    else:
        locked = P._locked

        @contextlib.contextmanager
        def announced_lock(path, **kwargs):
            # The attempted event releases a first writer that owns the bootstrap lock.
            started.set()
            attempted.set()
            with locked(path, **kwargs):
                yield

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

    def writers(self, first, second):
        attempted = self.context.Event()
        first_started = self.context.Event()
        second_started = self.context.Event()
        result = self.context.Queue()
        one = self.context.Process(
            target=_first_add,
            args=(str(self.directory), first, True, attempted, first_started, result),
        )
        two = self.context.Process(
            target=_first_add,
            args=(str(self.directory), second, False, attempted, second_started, result),
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

    def test_waiting_writer_never_observes_a_refused_newborn(self):
        outcomes = self.writers(
            ["d.first", "verdict=a conclusion", "because=nothing", "rests_on=[s.nowhere]",
             "reopened_by=anything", "--as-of", "2026-09-12"],
            ["fact.second", "v=2", "--as-of", "2026-09-12"],
        )
        self.assertEqual(sorted(outcome[0] for outcome in outcomes), ["error", "ok"], outcomes)
        text = (self.directory / "GROUNDING.yaml").read_text(encoding="utf-8")
        self.assertIn("  fact.second:", text)
        self.assertNotIn("  d.first:", text)


if __name__ == "__main__":
    unittest.main()
