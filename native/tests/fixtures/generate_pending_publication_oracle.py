#!/usr/bin/env python3
"""Generate bounded publication/reconciliation fixtures from Python 1.8."""
import contextlib
import json
import os
from pathlib import Path
import sys
from unittest import mock

HERE = Path(__file__).resolve().parent
BASELINE = Path('/Users/ilanbm/docs/kpopper/rust-runtime-spike/baseline-f480ea6')
sys.path.insert(0, str(BASELINE))
from scripts import pending_grounding as G  # noqa: E402
from tests.test_pending_publication import PublicationTests  # noqa: E402


@contextlib.contextmanager
def fixed_git_time():
    names = ('GIT_AUTHOR_DATE', 'GIT_COMMITTER_DATE')
    old = {name: os.environ.get(name) for name in names}
    for name in names:
        os.environ[name] = '2001-09-09T01:46:40Z'
    try:
        yield
    finally:
        for name, value in old.items():
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value


def refs(case):
    result = {}
    for name in ('trunk', 'pending_grounding'):
        value = case.remote_head(name)
        result[name] = value
    return result


def state(case):
    path = case.project.state / 'publication.json'
    return path.read_text() if path.exists() else None


def scenario():
    case = PublicationTests('runTest')
    case.setUp()
    case.publisher.clock = lambda: 123.5
    try:
        with mock.patch.object(G.time, 'time_ns', return_value=1), \
             mock.patch('scripts.pending_publication.uuid.uuid4', return_value=type('U', (), {'hex': '1' * 32})()):
            revision = case.capture()
            proposed = case.publisher.run()
            proposed_state = state(case)
            proposed_refs = refs(case)
            unresolved = case.publisher.verify_obligations()
            unresolved_state = state(case)
            case.merge()
            verified = case.publisher.verify_obligations()
            verified_state = state(case)
            verified_refs = refs(case)
            scope = case.project.config()['publication']
            scope_id = __import__(
                'scripts.pending_publication', fromlist=['scope_identity']
            ).scope_identity(scope)
        return {
            '_scope': scope_id,
            'revision': revision,
            'proposed': proposed,
            'proposed_state': proposed_state,
            'proposed_refs': proposed_refs,
            'unresolved': unresolved,
            'unresolved_state': unresolved_state,
            'verified': verified,
            'verified_state': verified_state,
            'verified_refs': verified_refs,
            'provider_rows': case.provider.rows,
        }
    finally:
        case.doCleanups()


def main():
    with fixed_git_time():
        value = scenario()
    scope = value.pop('_scope')
    def normalize(item):
        if isinstance(item, str):
            return item.replace(scope, '$SCOPE')
        if isinstance(item, list):
            return [normalize(value) for value in item]
        if isinstance(item, dict):
            return {key: normalize(value) for key, value in item.items()}
        return item
    value = normalize(value)
    (HERE / 'pending-publication-oracle.json').write_text(
        json.dumps(value, ensure_ascii=False, indent=2) + '\n'
    )
    print(HERE / 'pending-publication-oracle.json')


if __name__ == '__main__':
    main()
