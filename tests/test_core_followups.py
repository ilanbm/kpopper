"""Core followups preserve exact readings and captured evidence independently."""
import copy
import datetime as dt
import os
import json
import subprocess
import sys
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import yaml

from scripts import followups as F, followup_core as C, followup_triggers as T
from scripts.reasoning.context import CapturedAssessment
from scripts.reasoning.snapshot import Snapshot
from tests.test_reasoning_history_assessment import claim, captured, UnavailableRuntime


def document():
    return {
        'meta': {'reasoning': {'version': 1, 'profile': 'core/v1',
                              'requires': ['arithmetic/v1']}},
        'readings': {'m.x': {'v': 3}, 'm.y': {'rule': {'expr': 'm.x / 2'}},
                     'm.null': {'v': None}, 'm.bool': {'v': False}},
        'decisions': {'d.ok': {'verdict': 'Fine', 'rests_on': ['m.x'],
                              'seen': {'m.x': 3}, 'wrong_if': {'expr': 'm.x < 1'}}},
    }


def view(doc=None, **kwargs):
    return CapturedAssessment.from_snapshot(Snapshot.from_data(doc or document()), **kwargs)


def condition(values, identifier, op, expected):
    return T.evaluate({'condition': {'id': identifier, 'op': op, 'value': expected}},
                      values, {}, set(), {}, dt.datetime(2026, 9, 18, tzinfo=dt.timezone.utc))['value']


class CoreFollowups(unittest.TestCase):
    def test_declared_record_routes_to_core_exact_conditions(self):
        with tempfile.TemporaryDirectory() as root:
            record = Path(root) / 'GROUNDING.yaml'
            record.write_text(yaml.safe_dump(document()))
            before = record.read_bytes()
            values, _, error = F.graph(str(record))
            self.assertIsNone(error)
            self.assertIs(condition(values, 'm.y', '==', {'rational': ['3', '2']}), True)
            self.assertIs(condition(values, 'm.null', '==', None), True)
            self.assertIs(condition(values, 'm.bool', '==', 0), False)
            self.assertIs(condition(values, 'd.ok', '==', 'Fine'), True)
            self.assertEqual(record.read_bytes(), before)

    def test_runtime_unavailable_is_unknown_not_null_or_false(self):
        values, flags, error = C.project(view(runtime=UnavailableRuntime()))
        self.assertIsNone(error)
        self.assertTrue(flags)
        self.assertTrue(T.unavailable(values['m.x']))
        self.assertIsNone(condition(values, 'm.x', '==', None))
        self.assertIsNone(condition(values, 'm.x', '>', 0))

    def test_same_value_changed_basis_and_unrelated_changes(self):
        doc = document()
        doc['readings']['m.y']['rule'] = {'expr': 'm.x * 0'}
        first = C.project(view(doc))[0]
        doc['readings']['m.other'] = {'v': 99}
        unrelated = C.project(view(doc))[0]
        self.assertEqual(first['m.y'], unrelated['m.y'])
        self.assertEqual(first['d.ok'], unrelated['d.ok'])
        doc['readings']['m.x']['v'] = 4
        changed = C.project(view(doc))[0]
        self.assertEqual(first['m.y']['core']['value'], changed['m.y']['core']['value'])
        self.assertNotEqual(first['m.y'], changed['m.y'])

    def test_captured_replay_never_reads_source(self):
        context = view()
        replay = CapturedAssessment.from_json(context.to_json())
        with mock.patch.object(Snapshot, 'capture', side_effect=AssertionError('live read')):
            self.assertEqual(C.project(context), C.project(replay))

    def test_accepted_fired_condition_remains_accepted(self):
        head = claim('d.ok', {'verdict': 'Ready', 'rests_on': [],
                             'wrong_if': {'bool': True}}, kind='judgment')
        context = CapturedAssessment.from_snapshot(captured(head).snapshot())
        values, maintenance, error = C.project(context)
        self.assertIsNone(error)
        self.assertTrue(values['d.ok']['core']['available'])
        self.assertEqual(values['d.ok']['core']['findings']['acceptance'], 'accepted')
        self.assertEqual(values['d.ok']['core']['findings']['falsifier']['status'], 'holds')
        self.assertTrue(maintenance)

    def test_unrelated_refusal_does_not_select_core(self):
        with mock.patch.object(F, '_record_view', side_effect=F.P.Refused('invalid predicate')):
            with mock.patch.object(C, 'capture', side_effect=AssertionError('fallback')):
                values, flags, error = F.graph('unused')
        self.assertEqual((values, flags), ({}, []))
        self.assertIn('invalid predicate', error)

    def test_profile_changed_during_route_stays_unavailable(self):
        doc = document()
        del doc['meta']['reasoning']
        with mock.patch.object(CapturedAssessment, 'capture', return_value=view(doc)):
            with self.assertRaisesRegex(ValueError, 'profile changed'):
                C.capture('unused')

    def test_invalid_persisted_core_envelope_is_unknown(self):
        value = copy.deepcopy(C.project(view())[0]['m.x'])
        value['core']['version'] = 2
        self.assertIsNone(condition({'m.x': value}, 'm.x', '==', 3))

    @unittest.skipIf(os.name == 'nt', 'History fixture publication requires POSIX locking')
    def test_active_history_equal_observation_changes_evidence_without_scan_writes(self):
        from tests.test_history_authoring import Authoring
        fixture = Authoring()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        before = {str(p): p.read_bytes() for p in fixture.entry.parent.rglob('*') if p.is_file()}
        first, _, error = F.graph(str(fixture.entry))
        self.assertIsNone(error)
        self.assertIs(condition(first, 'p.input', '==', 1), True)
        self.assertEqual(before, {str(p): p.read_bytes() for p in fixture.entry.parent.rglob('*')
                                  if p.is_file()})
        fixture.publish(fixture.prepare({'kind': 'set', 'id': 'p.input', 'value': 1,
                                         'as_of': '2026-09-18'}))
        after, _, error = F.graph(str(fixture.entry))
        self.assertIsNone(error)
        self.assertEqual(first['p.input']['core']['value'], after['p.input']['core']['value'])
        self.assertNotEqual(first['p.input']['core']['evidence'], after['p.input']['core']['evidence'])

    def test_authored_core_mapping_is_not_an_envelope(self):
        raw = {'core': {'value': 3}}
        self.assertFalse(T.unavailable(raw))
        self.assertEqual(T._condition_scalar(raw), raw)

    def test_incomplete_history_and_unaccepted_claims_have_no_reading(self):
        head = claim('m.x', {'v': 3})
        for projection in (captured(head, complete=False),
                           captured(head, acceptance={'m.x': 'proposed'})):
            values, flags, error = C.project(CapturedAssessment.from_snapshot(projection.snapshot()))
            self.assertIsNone(error)
            self.assertTrue(T.unavailable(values['m.x']))
            self.assertIsNone(condition(values, 'm.x', '==', 3))
            self.assertTrue(flags)

    @unittest.skipIf(os.name == 'nt', 'Persistent followups require POSIX locking')
    def test_store_reopen_and_claim_uses_core_evidence(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            work = root / 'project'
            work.mkdir()
            record = work / 'GROUNDING.yaml'
            doc = document()
            doc['readings']['m.y']['rule'] = {'expr': 'm.x * 0'}
            record.write_text(yaml.safe_dump(doc))
            with mock.patch.dict(os.environ, {'XDG_STATE_HOME': str(root / 'state')}):
                store = F.Store(work)
                store.setup(private=True)
                store.add({'id': 'review', 'title': 'Review the calculation',
                           'why': 'Keep the basis current', 'how': 'Inspect the evidence',
                           'scope': 'Read only', 'related': ['m.y'],
                           'when': {'changed': 'm.y'}})
                self.assertEqual(store.scan()['items'][0]['state'], 'waiting')
                doc['readings']['m.x']['v'] = 4
                record.write_text(yaml.safe_dump(doc))
                reopened = F.Store(work)
                row = reopened.scan()['items'][0]
                self.assertEqual(row['state'], 'ready')
                cli = subprocess.run([sys.executable, str(Path(__file__).resolve().parents[1]
                                                          / 'scripts/cli.py'),
                                      '--workspace', str(work), 'followups', 'scan', '--json'],
                                     capture_output=True, text=True)
                self.assertEqual(cli.returncode, 0, cli.stderr + cli.stdout)
                self.assertEqual(json.loads(cli.stdout)['items'][0]['state'], 'ready')
                result = reopened.claim('review', row['occurrence'], 'test')
                self.assertTrue(result['claim']['token'])


if __name__ == '__main__':
    unittest.main()
