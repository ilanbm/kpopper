#!/usr/bin/env python3
"""Generate native public knowledge/pending fixtures from Python 1.8.

The input ledger images are already pinned Python fixtures.  This script restores
them in temporary repositories, invokes the public Python command entrypoints,
and records complete JSON output plus every materialized file byte.
"""
import base64
import contextlib
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
BASELINE = Path('/Users/ilanbm/docs/kpopper/rust-runtime-spike/baseline-f480ea6')
sys.path.insert(0, str(BASELINE))
from scripts import knowledge_cli, pending_cli  # noqa: E402


def git(root, *args, data=None):
    return subprocess.run(
        ['git', '-C', str(root), *args], input=data, check=True,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    ).stdout


def restore(root, case):
    git(root, 'init', '-b', 'trunk', '--object-format=' + case['object_format'])
    for item in case['objects']:
        actual = git(root, 'hash-object', '-w', '--stdin', '-t', item['kind'],
                     data=base64.b64decode(item['raw'])).decode().strip()
        assert actual == item['oid']
    if case.get('head'):
        git(root, 'update-ref', 'refs/kpopper/pending_grounding', case['head'])
    for name, raw in case['files'].items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(raw)
    common = Path(git(root, 'rev-parse', '--git-common-dir').decode().strip())
    if not common.is_absolute():
        common = root / common
    state = common / 'kpopper/project'
    if case.get('config') is not None:
        state.mkdir(parents=True, exist_ok=True)
        (state / 'project.json').write_text(case['config'])
    if case.get('target_ref'):
        git(root, 'update-ref', 'refs/remotes/origin/trunk', case['target_ref'])
    if case.get('publication') is not None:
        state.mkdir(parents=True, exist_ok=True)
        (state / 'publication.json').write_text(case['publication'])


@contextlib.contextmanager
def environment(cwd, **values):
    before = Path.cwd()
    old = {key: os.environ.get(key) for key in values}
    os.chdir(cwd)
    try:
        for key, value in values.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
        yield
    finally:
        os.chdir(before)
        for key, value in old.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value


def invoke(module, argv, cwd, **env):
    out, err = io.StringIO(), io.StringIO()
    with environment(cwd, **env), contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        code = module.main(argv)
    return {'code': code, 'stdout': out.getvalue(), 'stderr': err.getvalue()}


def normalized(value, replacements):
    if isinstance(value, str):
        for old, new in replacements:
            value = value.replace(str(old), new)
        return value
    if isinstance(value, list):
        return [normalized(item, replacements) for item in value]
    if isinstance(value, dict):
        return {key: normalized(item, replacements) for key, item in value.items()}
    return value


def image(root):
    return {
        path.relative_to(root).as_posix(): base64.b64encode(path.read_bytes()).decode()
        for path in sorted(root.rglob('*')) if path.is_file()
    }


def main():
    ledgers = json.loads((HERE / 'pending-state.json').read_text())
    by_name = {case['name']: case for case in ledgers}
    result = {'status': [], 'pending': [], 'materialize': []}
    with tempfile.TemporaryDirectory() as raw_temp:
        temp = Path(raw_temp).resolve()
        private = temp / 'private'
        private.mkdir()

        simple = temp / 'simple'
        simple.mkdir()
        (simple / 'GROUNDING.yaml').write_text('p:\n  p.local: {v: local}\n')
        for mode in ('live', 'frozen'):
            actual = invoke(knowledge_cli, ['status'], simple,
                            KPOPPER_READ_MODE=mode, KPOPPER_PRIVATE_HOME=str(private))
            actual = normalized(actual, [(simple, '$ROOT'), (private, '$PRIVATE')])
            result['status'].append({'name': 'simple_' + mode, 'setup': 'simple',
                                     'read_mode': mode, 'actual': actual})
        result['pending'].append({'name': 'simple', 'setup': 'simple',
                                  'actual': invoke(pending_cli, ['status'], simple)})

        for name in ('empty', 'one', 'target_missing'):
            root = temp / name
            root.mkdir()
            restore(root, by_name[name])
            # Retain one discoverable private obligation without reading its body.
            if name == 'one':
                common = Path(git(root, 'rev-parse', '--git-common-dir').decode().strip())
                if not common.is_absolute():
                    common = root / common
                key = hashlib.sha256(str(common.resolve()).encode()).hexdigest()
                draft = private / key / 'private-one.json'
                draft.parent.mkdir(parents=True)
                draft.write_text('{}')
            for mode in ('live', 'frozen'):
                actual = invoke(knowledge_cli, ['status'], root,
                                KPOPPER_READ_MODE=mode, KPOPPER_PRIVATE_HOME=str(private))
                actual = normalized(actual, [(root, '$ROOT'), (private, '$PRIVATE')])
                result['status'].append({'name': name + '_' + mode, 'setup': name,
                                         'read_mode': mode, 'actual': actual})
            actual = invoke(pending_cli, ['status'], root)
            result['pending'].append({'name': name, 'setup': name,
                                      'actual': normalized(actual, [(root, '$ROOT')])})

        # Explicit registered absolute record is found from a nested working directory.
        registered = temp / 'registered'
        registered.mkdir()
        git(registered, 'init', '-b', 'trunk')
        external = temp / 'shared' / 'facts.yaml'
        external.parent.mkdir()
        external.write_text('p:\n  p.shared: {v: shared}\n')
        common = Path(git(registered, 'rev-parse', '--git-common-dir').decode().strip())
        if not common.is_absolute():
            common = registered / common
        state = common / 'kpopper/project'
        state.mkdir(parents=True)
        (state / 'project.json').write_text(json.dumps({
            'version': 1, 'mode': 'simple', 'record': str(external),
            'publication': None, 'generation': 0,
        }))
        nested = registered / 'nested'
        nested.mkdir()
        actual = invoke(knowledge_cli, ['status'], nested,
                        KPOPPER_READ_MODE='live', KPOPPER_PRIVATE_HOME=str(private))
        result['status'].append({'name': 'registered_other_cwd', 'setup': 'registered',
                                 'read_mode': 'live',
                                 'actual': normalized(actual, [(registered, '$ROOT'),
                                                               (external, '$RECORD'),
                                                               (private, '$PRIVATE')])})

        root = temp / 'materialize'
        root.mkdir()
        restore(root, by_name['one'])
        revision = next(iter(__import__('scripts.pending_grounding', fromlist=['Store'])
                             .Store(root).snapshot()['bundles']))
        out = temp / 'snapshot'
        actual = invoke(knowledge_cli,
                        ['materialize', revision, '--out', str(out)], root)
        result['materialize'].append({
            'name': 'complete', 'setup': 'one', 'revision': revision,
            'actual': normalized(actual, [(root, '$ROOT'), (out, '$OUT')]),
            'files': image(out),
        })
        result['materialize'].append({
            'name': 'overwrite', 'setup': 'one', 'revision': revision,
            'actual': normalized(invoke(knowledge_cli,
                ['materialize', revision, '--out', str(out)], root), [(out, '$OUT')]),
        })
        missing = '0' * 64
        missing_out = temp / 'missing-snapshot'
        result['materialize'].append({
            'name': 'unavailable', 'setup': 'one', 'revision': missing,
            'actual': normalized(invoke(knowledge_cli,
                ['materialize', missing, '--out', str(missing_out)], root),
                [(missing_out, '$OUT')]),
        })

    target = HERE / 'public-knowledge-oracle.json'
    target.write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n')
    print(target)


if __name__ == '__main__':
    main()
