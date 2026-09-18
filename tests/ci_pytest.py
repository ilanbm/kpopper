"""Live CI progress and execution evidence for the final shard coverage gate."""
import json
from pathlib import Path
import sys
import threading
import time

import pytest


def pytest_addoption(parser):
    parser.addoption('--ci-manifest', default=None)


@pytest.hookimpl(hookwrapper=True, tryfirst=True)
def pytest_collection_modifyitems(config, items):
    collected = sorted(item.nodeid for item in items)
    yield
    config._ci_collection = {'collected': collected, 'selected': sorted(item.nodeid for item in items)}


def pytest_configure(config):
    if config.getoption('ci_manifest') and not hasattr(config, 'workerinput'):
        config.pluginmanager.register(Progress(config), 'ci-progress')


@pytest.hookimpl(trylast=True)
def pytest_sessionfinish(session):
    if hasattr(session.config, 'workeroutput'):
        session.config.workeroutput['ci_collection'] = getattr(session.config, '_ci_collection', None)


class Progress:
    def __init__(self, config):
        self.config, self.active, self.executed, self.collections = config, {}, [], []
        self.lock, self.stop = threading.Lock(), threading.Event()
        self.thread = None
        self.total = 0

    def pytest_sessionstart(self):
        self.thread = threading.Thread(target=self.report_progress, daemon=True)
        self.thread.start()

    def pytest_xdist_node_collection_finished(self, node, ids):
        self.total = len(ids)

    def pytest_runtest_logstart(self, nodeid, location):
        with self.lock:
            self.active[nodeid] = time.monotonic()

    def pytest_runtest_logreport(self, report):
        with self.lock:
            if report.when == 'call' or (report.when == 'setup' and (report.skipped or report.failed)):
                self.executed.append(report.nodeid)
            if report.when == 'teardown':
                self.active.pop(report.nodeid, None)

    def pytest_testnodedown(self, node, error):
        self.collections.append(getattr(node, 'workeroutput', {}).get('ci_collection'))

    def report_progress(self):
        while not self.stop.wait(30):
            with self.lock:
                running = sorted((time.monotonic() - start, name) for name, start in self.active.items())
                done = len(self.executed)
            reporter = self.config.pluginmanager.get_plugin('terminalreporter')
            if reporter:
                reporter.write_line('[CI progress] %s/%s completed; active: %s' %
                    (done, self.total or '?', '; '.join('%s (%.0fs)' % (name, seconds)
                     for seconds, name in reversed(running)) or 'waiting for collection or workers'))

    @pytest.hookimpl(trylast=True)
    def pytest_sessionfinish(self, session):
        self.stop.set()
        if self.thread:
            self.thread.join(timeout=1)
        if self.config.option.collectonly:
            return
        samples = self.collections or [getattr(self.config, '_ci_collection', None)]
        expected_workers = self.config.getoption('numprocesses') or 1
        consistent = len(samples) == expected_workers and bool(samples[0]) and all(s == samples[0] for s in samples)
        data = samples[0] or {'collected': [], 'selected': []}
        data = dict(data, python='%s.%s' % sys.version_info[:2],
                    group=self.config.getoption('group', default=None) or 1,
                    splits=self.config.getoption('splits', default=None) or 1,
                    executed=sorted(self.executed), consistent=bool(consistent))
        path = Path(self.config.getoption('ci_manifest'))
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(data, sort_keys=True) + '\n', encoding='utf-8')
        if (not consistent or len(self.executed) != len(set(self.executed))
                or set(data['selected']) != set(self.executed)):
            if session.exitstatus == 0:
                session.exitstatus = 1
            self.config.pluginmanager.get_plugin('terminalreporter').write_line(
                'CI execution manifest is incomplete, duplicated, or workers disagree about collection', red=True)
