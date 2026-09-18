"""Run a synthetic revisit using an installed kpopper package and disposable files."""
import contextlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

import yaml
from kpopper import provenance
from kpopper.reasoning.evaluate import Evaluator
from kpopper.reasoning.snapshot import Snapshot


def result(snapshot, subject):
    return Evaluator(snapshot).evaluate({'ref': subject}, declared=[subject])


def number(value):
    assert value['type'] == 'number' and value['denominator'] == '1', value
    return int(value['numerator'])


def write(record, action):
    with contextlib.redirect_stdout(io.StringIO()):
        provenance.apply([str(record)], action)


def main():
    if os.name != 'posix':
        raise SystemExit('This writing exercise requires POSIX file locks (macOS or Linux).')
    with tempfile.TemporaryDirectory(prefix='kpopper-query-example-') as directory:
        root = Path(directory)
        record = root / 'GROUNDING.yaml'
        fixtures = Path(__file__).resolve().parent
        shutil.copyfile(fixtures / 'record.yaml', record)
        shutil.copyfile(fixtures / 'evidence.md', root / 'evidence.md')

        first = Snapshot.capture([str(record)], read_mode='frozen')
        retained = first.to_json()
        before = result(first, 'm.minutes')
        assert before['status'] == 'ok', before
        assert number(before['value']['fields']['result']) == 30
        assert number(result(first, 'm.people')['value']) == 16
        assert result(first, 'm.weather_margin')['status'] == 'unknown'

        # These are two explicit writes in this disposable exercise, not an
        # atomic batch. A report batch uses the separate recording protocol.
        for subject, value in [('p.guides', 3), ('p.group_size', 6)]:
            write(record, {'kind': 'set', 'id': subject, 'value': value,
                           'as_of': '2026-09-02', 'source': 's.example',
                           'at': 'revised group'})
        write(record, {'kind': 'add', 'id': 'route.garden', 'into': 'routes',
                       'body': {'minutes': 5, 'permitted': True,
                                'from': 's.example', 'at': 'later garden route'}})
        second = Snapshot.capture([str(record)], read_mode='frozen')
        after = result(second, 'm.minutes')
        assert after['status'] == 'ok', after
        assert number(after['value']['fields']['result']) == 35
        assert number(result(second, 'm.people')['value']) == 18
        assert after['basis'] != before['basis']
        assert [item['kind'] for item in after['executed_reads']] == ['node', 'scope']

        # A later process reads the written record through the ordinary CLI.
        process = subprocess.run([sys.executable, '-m', 'kpopper.cli', 'assess',
                                  'm.minutes', 'm.people', 'm.weather_margin', 'd.route_style',
                                  '--record', str(record), '--profile', 'core/v1', '--history'],
                                 cwd=root, text=True, capture_output=True, check=True)
        report = json.loads(process.stdout)
        assert report['nodes']['m.weather_margin']['computation']['status'] == 'unknown'
        assert report['nodes']['d.route_style']['state']['falsifier']['status'] == 'not_declared'
        assert 'wrong_if' not in yaml.safe_load(record.read_text())['decisions']['d.route_style']

        replay = result(Snapshot.from_json(retained), 'm.minutes')
        assert replay['basis'] == before['basis']
        assert number(replay['value']['fields']['result']) == 30
        print(json.dumps({'initial_minutes': 30, 'revised_minutes': 35,
                          'initial_people': 16, 'revised_people': 18,
                          'forecast': 'unknown', 'qualitative_falsifier': 'not_declared',
                          'scope_basis_changed': True, 'retained_snapshot_minutes': 30}, indent=2))


if __name__ == '__main__':
    main()
