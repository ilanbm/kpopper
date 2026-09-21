#!/usr/bin/env python3
"""Generate pending-control command outputs and exact local state from Python 1.8."""
import contextlib
import json
from pathlib import Path
import sys
import tempfile
from unittest import mock

HERE = Path(__file__).resolve().parent
BASELINE = Path('/Users/ilanbm/docs/kpopper/rust-runtime-spike/baseline-f480ea6')
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(BASELINE))
from generate_public_knowledge_oracle import git, invoke, normalized, restore  # noqa: E402
from scripts import pending_cli, pending_publication  # noqa: E402

ORIGINAL_PUBLISHER = pending_publication.Publisher


def state_files(root):
    common = Path(git(root, 'rev-parse', '--git-common-dir').decode().strip())
    if not common.is_absolute():
        common = root / common
    state = common / 'kpopper/project'
    return {
        path.relative_to(state).as_posix(): path.read_text()
        for path in sorted(state.rglob('*')) if path.is_file()
    }


def fixed_publisher(project):
    return ORIGINAL_PUBLISHER(project, clock=lambda: 123.5)


def run(argv, root):
    with mock.patch.object(pending_cli.U, 'Publisher', fixed_publisher):
        return invoke(pending_cli, argv, root)


def main():
    ledgers = json.loads((HERE / 'pending-state.json').read_text())
    cases = {case['name']: case for case in ledgers}
    output = []
    with tempfile.TemporaryDirectory() as raw:
        temp = Path(raw).resolve()

        root = temp / 'configure'
        root.mkdir()
        git(root, 'init', '-b', 'trunk')
        bare = temp / 'remote.git'
        git(temp, 'init', '--bare', str(bare))
        git(root, 'remote', 'add', 'team', str(bare))
        for name, argv in [
            ('configure_initial', ['configure', '--remote', 'team', '--target', 'trunk']),
            ('configure_grant', ['configure', '--grant']),
            ('configure_revoke', ['configure', '--revoke']),
        ]:
            actual = invoke(pending_cli, argv, root)
            output.append({'name': name, 'setup': 'configure', 'argv': argv,
                           'actual': normalized(actual, [(root, '$ROOT'), (bare, '$REMOTE')]),
                           'state_files': normalized(state_files(root), [(bare, '$REMOTE')])})

        missing = temp / 'missing-config'
        missing.mkdir()
        git(missing, 'init', '-b', 'trunk')
        argv = ['configure']
        output.append({'name': 'configure_missing', 'setup': 'git', 'argv': argv,
                       'actual': invoke(pending_cli, argv, missing),
                       'state_files': state_files(missing)})

        simple = temp / 'simple'
        simple.mkdir()
        argv = ['pause']
        output.append({'name': 'simple_pause', 'setup': 'simple', 'argv': argv,
                       'actual': invoke(pending_cli, argv, simple),
                       'state_files': {}})

        for fixture_name in ('one', 'retired'):
            root = temp / fixture_name
            root.mkdir()
            restore(root, cases[fixture_name])
            snapshot = __import__('scripts.pending_grounding', fromlist=['Store']).Store(root).snapshot()
            revision = next(iter(snapshot['bundles']))
            commands = [('pause', ['pause'])]
            if fixture_name == 'one':
                commands.append(('resume_empty', ['resume']))
            else:
                commands.append(('resume_revision', ['resume', revision]))
            for suffix, argv in commands:
                actual = run(argv, root)
                output.append({'name': fixture_name + '_' + suffix, 'setup': fixture_name,
                               'argv': argv, 'actual': normalized(actual, [(root, '$ROOT')]),
                               'state_files': state_files(root)})

        for action in ('withdraw', 'reject'):
            root = temp / action
            root.mkdir()
            restore(root, cases['one'])
            snapshot = __import__('scripts.pending_grounding', fromlist=['Store']).Store(root).snapshot()
            revision = next(iter(snapshot['bundles']))
            argv = [action, revision, '--reason', 'fixture decision']
            output.append({'name': action, 'setup': 'one', 'argv': argv,
                           'actual': run(argv, root), 'state_files': state_files(root)})

        root = temp / 'supersede'
        root.mkdir()
        restore(root, cases['two'])
        revisions = list(__import__('scripts.pending_grounding', fromlist=['Store']).Store(root).snapshot()['bundles'])
        argv = ['supersede', revisions[0], '--replacement', revisions[1], '--reason', 'new evidence']
        output.append({'name': 'supersede', 'setup': 'two', 'argv': argv,
                       'actual': run(argv, root), 'state_files': state_files(root)})

        root = temp / 'retry'
        root.mkdir()
        restore(root, cases['one'])
        project = __import__('scripts.project_modes', fromlist=['Project']).Project(root)
        project.state.mkdir(parents=True, exist_ok=True)
        (project.state / 'publication.json').write_text(json.dumps({
            'version': 1, 'scope': None, 'decisions': {}, 'receipts': [],
            'expected_head': None, 'pr': None, 'cycle': 0, 'proposed': [],
            'intent': None, 'failures': 4, 'retry_at': 9999, 'paused': False,
            'states': {},
        }))
        argv = ['retry']
        output.append({'name': 'retry', 'setup': 'one-retry', 'argv': argv,
                       'actual': run(argv, root), 'state_files': state_files(root)})

        root = temp / 'terminal-invalid'
        root.mkdir()
        restore(root, cases['one'])
        revision = next(iter(__import__('scripts.pending_grounding', fromlist=['Store']).Store(root).snapshot()['bundles']))
        argv = ['withdraw', revision]
        output.append({'name': 'withdraw_missing_reason', 'setup': 'one', 'argv': argv,
                       'actual': run(argv, root), 'state_files': state_files(root)})

        guarded = temp / 'guarded-scope'
        guarded.mkdir()
        restore(guarded, cases['one'])
        bare = temp / 'guarded.git'
        git(temp, 'init', '--bare', str(bare))
        git(guarded, 'remote', 'add', 'team', str(bare))
        invoke(pending_cli, ['configure', '--remote', 'team', '--target', 'trunk'], guarded)
        argv = ['configure', '--target', 'other']
        output.append({'name': 'configure_pending_scope_change', 'setup': 'one-configured',
                       'argv': argv,
                       'actual': normalized(invoke(pending_cli, argv, guarded), [(bare, '$REMOTE')]),
                       'state_files': normalized(state_files(guarded), [(bare, '$REMOTE')])})

    target = HERE / 'pending-controls-oracle.json'
    target.write_text(json.dumps(output, ensure_ascii=False, indent=2) + '\n')
    print(target)


if __name__ == '__main__':
    main()
