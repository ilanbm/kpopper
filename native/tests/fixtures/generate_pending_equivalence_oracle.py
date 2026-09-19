#!/usr/bin/env python3
"""Generate v2/v3 content-equivalence fixtures from Python 1.8."""
import base64
import copy
import json
from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
BASELINE = Path('/Users/ilanbm/docs/kpopper/rust-runtime-spike/baseline-f480ea6')
sys.path.insert(0, str(BASELINE))
from scripts import history_bundle as H, pending_grounding as G  # noqa: E402
from tests.test_history_advanced import HistoryAdvanced  # noqa: E402
from tests import test_history_bundles as history_fixture  # noqa: E402
from tests.test_history_snapshot_capture import act  # noqa: E402


def encoded_files(files):
    return {path: base64.b64encode(raw).decode() for path, raw in sorted(files.items())}


def main():
    pending = json.loads((HERE / 'pending-bundle.json').read_text())
    plain = next(case for case in pending['cases'] if case['name'] == 'plain')
    bundle = G._decode(plain['output'])
    document = G._decode(plain['document'])
    files = {path: base64.b64decode(raw) for path, raw in plain['files'].items()}
    bundle['files'] = files
    changed = copy.deepcopy(document)
    changed['known']['p.x']['v'] = 99
    cases = [{
        'name': 'v2-extra-target-ids', 'bundle': G._encode({k: bundle[k] for k in ('revision', 'manifest')}), 'files': plain['files'],
        'document': plain['document'], 'evidence': plain['files'],
        'expected': G.equivalent(bundle, document, files),
    }, {
        'name': 'v2-changed-root', 'bundle': G._encode({k: bundle[k] for k in ('revision', 'manifest')}), 'files': plain['files'],
        'document': G._encode(changed), 'evidence': plain['files'],
        'expected': G.equivalent(bundle, changed, files),
    }]
    file_case = next(case for case in pending['cases'] if case['name'] == 'file')
    file_bundle = G._decode(file_case['output'])
    file_bytes = {path: base64.b64decode(raw) for path, raw in file_case['files'].items()}
    file_bundle['files'] = file_bytes
    changed_files = dict(file_bytes)
    changed_files[next(iter(changed_files))] += b'changed'
    cases.append({
        'name': 'v2-changed-evidence',
        'bundle': G._encode({k: file_bundle[k] for k in ('revision', 'manifest')}),
        'files': file_case['files'], 'document': file_case['document'],
        'evidence': encoded_files(changed_files),
        'expected': G.equivalent(file_bundle, G._decode(file_case['document']), changed_files),
    })

    test = HistoryAdvanced('runTest')
    test.setUp()
    try:
        bundle = test.source.bundle()
        capture = test.source.store.capture()
        document = H.adapt(H.from_contribution(bundle)).document
        history = {
            'revision': H.export(capture, roots=bundle['manifest']['roots'],
                                 scope=bundle['manifest']['scope'], shareability='project')['revision'],
            'manifest': H.export(capture, roots=bundle['manifest']['roots'],
                                 scope=bundle['manifest']['scope'], shareability='project')['manifest'],
            'files': H.captured_files(capture),
        }
        cases.append({
            'name': 'v3-full-history',
            'bundle': G._encode({'revision': bundle['revision'], 'manifest': bundle['manifest']}),
            'files': encoded_files(bundle['files']),
            'document': G._encode(document),
            'target_history': encoded_files(history['files']),
            'expected': G.equivalent(bundle, document, {}, history=history),
        })
        other = test.new_source('other-authority').store.capture()
        other_artifact = H.export(other, roots=['p.input'], scope=bundle['manifest']['scope'],
                                  shareability='project')
        cases.append({
            'name': 'v3-other-authority',
            'bundle': G._encode({'revision': bundle['revision'], 'manifest': bundle['manifest']}),
            'files': encoded_files(bundle['files']),
            'document': G._encode(H.adapt(other_artifact).document),
            'target_history': encoded_files(other_artifact['files']),
            'expected': G.equivalent(bundle, H.adapt(other_artifact).document, {},
                                     history=other_artifact),
        })

        target_capture = test.source.store.capture()
        newer = history_fixture.reading(value=7, operation='newer')
        test.source.publish(
            [newer, act(newer, 'correct', operation='correct', over=[test.source.first['id']])],
            'newer',
        )
        bundle = test.source.bundle()
        target_artifact = H.export(target_capture, roots=['p.input'],
                                   scope=bundle['manifest']['scope'], shareability='project')
        cases.append({
            'name': 'v3-history-union',
            'bundle': G._encode({'revision': bundle['revision'], 'manifest': bundle['manifest']}),
            'files': encoded_files(bundle['files']),
            'document': G._encode(H.adapt(target_artifact).document),
            'target_history': encoded_files(target_artifact['files']),
            'expected': G.equivalent(bundle, H.adapt(target_artifact).document, {},
                                     history=target_artifact),
        })
    finally:
        test.doCleanups()
    (HERE / 'pending-equivalence-oracle.json').write_text(
        json.dumps(cases, ensure_ascii=False, separators=(',', ':')) + '\n'
    )
    print(HERE / 'pending-equivalence-oracle.json')


if __name__ == '__main__':
    main()
