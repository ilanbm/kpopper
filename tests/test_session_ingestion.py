"""An applied ingestion update is visible through the same checked-session boundary."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import yaml

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

try:
    from scripts.session.core import Core
    from scripts.session.view import GroundingService
    CORE_READY = bool(Core())
except (ImportError, ValueError):
    CORE_READY = False

if os.environ.get('KPOPPER_REQUIRE_CORE_TESTS') == '1' and not CORE_READY:
    raise RuntimeError('session ingestion integration requires the compiled core and extras')


@unittest.skipIf(os.name == 'nt', 'ingestion writes require POSIX locking')
@unittest.skipUnless(CORE_READY, 'install session extras and run session setup')
class SessionIngestionContract(unittest.TestCase):
    def test_capture_is_not_applied_but_processing_updates_current_and_invalidates_revision(self):
        with tempfile.TemporaryDirectory() as temporary:
            folder = Path(temporary).resolve()
            record = folder / 'PROVENANCE.yaml'
            (folder / 'initial.txt').write_text('The count is three.\n')
            judgment = {'verdict': 'The count is within the threshold.', 'rests_on': ['facts.count'],
                        'seen': {'facts.count': 3}, 'wrong_if': 'facts.count > 3'}
            record.write_text(yaml.safe_dump({
                'meta': {'scope': 'Ingestion and checked-reader integration fixture.'},
                'schema': {'deps': 'rests_on', 'snapshot': 'seen', 'predicate': 'wrong_if'},
                'sources': {'s.initial': {'name': 'Initial count', 'file': 'initial.txt', 'read': '2026-09-07'}},
                'known': {'facts.count': {'v': 3, 'from': 's.initial', 'at': 'line 1', 'of': '2026-09-07'}},
                'judgments': {'c.count_limit': judgment},
            }, sort_keys=False))
            report = folder / 'report.json'
            report.write_text(json.dumps({'event_id': 'session-integration', 'session_id': 'fixture',
                'source_quote': 'The count is four now.', 'target': 'facts.count', 'value': 4,
                'date': '2026-09-09', 'kind': 'report'}))
            session_state, ingestion_state = folder / 'session-state', folder / 'ingestion-state'
            environment = {'KPOPPER_SESSION_CONFIG': str(folder / 'settings.json'),
                           'XDG_CONFIG_HOME': str(folder / 'config'), 'XDG_STATE_HOME': str(folder / 'state')}
            with patch.dict(os.environ, environment):
                def cli(*args):
                    result = subprocess.run([sys.executable, str(ROOT / 'scripts/cli.py'), *args],
                                            cwd=folder, text=True, capture_output=True)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    return json.loads(result.stdout)

                service = GroundingService('ingestion-session', record, session_state,
                                           ROOT / 'scripts/provenance.py')
                _, before = service.graph()
                original = json.loads(service.reading('checked:c.count_limit', before))['value']
                self.assertFalse(original['falsifier']['holds_on_current_values'])
                captured = cli('ingest', 'capture', '--file', str(report), '--record', str(record),
                               '--state-dir', str(ingestion_state), '--no-start')
                self.assertEqual(captured['state'], 'captured')
                self.assertEqual(service.graph()[1], before)
                self.assertEqual(json.loads(service.reading('checked:c.count_limit', before))['value'], original)

                receipts = cli('ingest', 'process', '--record', str(record), '--state-dir', str(ingestion_state))
                self.assertEqual(receipts[0]['state'], 'applied')
                self.assertEqual(receipts[0]['newly_fired_judgments'], ['c.count_limit'])
                with self.assertRaisesRegex(ValueError, 'record changed'):
                    service.reading('checked:c.count_limit', before)
                _, after = service.graph()
                self.assertNotEqual(before, after)
                checked = json.loads(service.reading('checked:c.count_limit', after))['value']
                premise = checked['premises'][0]
                self.assertEqual((premise['current_recorded_value'], premise['value_at_review']), (4, 3))
                self.assertTrue(checked['falsifier']['holds_on_current_values'])
                self.assertEqual(yaml.safe_load(record.read_text())['judgments']['c.count_limit'], judgment)
                from_cli = cli('session', 'read', '--no-settings', '--input', str(record),
                               '--project', 'ingestion-session', '--state', str(session_state),
                               '--revision', after, '--ref', 'checked:c.count_limit')
                self.assertEqual(from_cli['value'], checked)


if __name__ == '__main__':
    unittest.main()
