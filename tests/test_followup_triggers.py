"""Pure trigger behavior, including uncertain inputs and stable occurrences."""
import datetime as dt
import unittest

from scripts import followup_triggers as T


class FollowupTriggers(unittest.TestCase):
    now = dt.datetime(2026, 9, 10, 12, tzinfo=dt.timezone.utc)

    def evaluate(self, trigger, **kwargs):
        args = dict(values={}, baseline={}, completed=set(), observations={}, now=self.now)
        args.update(kwargs)
        return T.evaluate(trigger, **args)

    def test_date_uses_local_midnight_and_offset_timestamps(self):
        self.assertEqual(T.parse_time('2026-09-10', 'Asia/Jerusalem').isoformat(),
                         '2026-09-09T21:00:00+00:00')
        self.assertEqual(T.parse_time('2026-09-10T15:00:00+03:00'), self.now)
        for value in ('2026-09-10T12:00:00', 'yesterday', '2026-02-30'):
            with self.assertRaises(ValueError):
                T.parse_time(value)
        with self.assertRaises(ValueError):
            T.parse_time('2026-09-10', 'Not/AZone')

    def test_at_inputs_stable_after_crossing_and_before(self):
        trigger = {'at': '2026-09-11'}
        early = self.evaluate(trigger)
        self.assertFalse(early['value'])
        self.assertEqual(early['next_at'], '2026-09-11T00:00:00Z')
        self.assertEqual(early['inputs'], self.evaluate(trigger, now=self.now + dt.timedelta(hours=1))['inputs'])
        late = self.evaluate(trigger, now=self.now + dt.timedelta(days=1))
        self.assertTrue(late['value'])
        self.assertIsNone(late['next_at'])
        self.assertNotEqual(early['inputs'], late['inputs'])

    def test_changed_uses_semantics_and_revert_disarms(self):
        trigger = {'changed': 'f.count'}
        baseline = {'f.count': {'x': 1, 'y': [2]}}
        same = self.evaluate(trigger, values={'f.count': {'y': [2], 'x': 1}}, baseline=baseline)
        self.assertFalse(same['value'])
        changed = self.evaluate(trigger, values={'f.count': {'x': 2, 'y': [2]}}, baseline=baseline)
        self.assertTrue(changed['value'])
        self.assertFalse(self.evaluate(trigger, values=baseline, baseline=baseline)['value'])
        self.assertNotEqual(changed['inputs'], same['inputs'])
        self.assertIsNone(self.evaluate(trigger, values=baseline)['value'])
        self.assertIsNone(self.evaluate(trigger, baseline=baseline)['value'])

    def test_typed_equality_and_finite_numeric_ordering(self):
        for op, expected in [('==', False), ('!=', True), ('>', None)]:
            self.assertEqual(self.evaluate({'condition': {'id': 'f.a', 'op': op, 'value': 1}},
                                           values={'f.a': True})['value'], expected)
        self.assertTrue(self.evaluate({'condition': {'id': 'f.a', 'op': '>=', 'value': 1}},
                                      values={'f.a': 1.0})['value'])
        self.assertIsNone(self.evaluate({'condition': {'id': 'f.a', 'op': '<', 'value': 4}},
                                       values={'f.a': '3'})['value'])
        self.assertTrue(self.evaluate({'condition': {'id': 'f.a', 'op': '==', 'value': None}},
                                      values={'f.a': None})['value'])
        self.assertIsNone(self.evaluate({'condition': {'id': 'f.a', 'op': '==', 'value': None}})['value'])

    def test_three_valued_composites(self):
        yes, no, unknown = {'completed': 'done'}, {'completed': 'pending'}, {'manual': 'Ask owner'}
        for kind, children, expected in [('all', [yes, unknown], None), ('all', [no, unknown], False),
                                          ('any', [yes, unknown], True), ('any', [no, unknown], None)]:
            self.assertEqual(self.evaluate({kind: children}, completed={'done'})['value'], expected)
        self.assertTrue(self.evaluate({'all': [yes, {'any': [no, yes]}]}, completed={'done'})['value'])

    def test_external_observations_expire_and_missing_is_unknown(self):
        trigger = {'external': {'ref': 'pr:42', 'equals': 'merged', 'max_age_hours': 2}}
        obs = {'pr:42': {'value': 'merged', 'observed_at': '2026-09-10T11:00:00Z', 'evidence': 'Verified PR'}}
        fresh = self.evaluate(trigger, observations=obs)
        self.assertTrue(fresh['value'])
        self.assertEqual(fresh['next_at'], '2026-09-10T13:00:00Z')
        expired = self.evaluate(trigger, observations=obs, now=self.now + dt.timedelta(hours=1))
        self.assertIsNone(expired['value'])
        self.assertNotEqual(fresh['inputs'], expired['inputs'])
        self.assertIsNone(self.evaluate(trigger)['value'])
        self.assertIsNone(self.evaluate(trigger, observations=obs, now=self.now - dt.timedelta(hours=2))['value'])
        self.assertIsNone(self.evaluate(trigger, observations={'pr:42': {'value': 'merged'}})['value'])

    def test_default_age_and_earliest_future_wake(self):
        trigger = {'all': [{'at': '2026-09-12'},
                           {'external': {'ref': 'event', 'equals': True}}]}
        obs = {'event': {'value': True, 'observed_at': '2026-09-10T11:00:00Z', 'evidence': 'receipt'}}
        result = self.evaluate(trigger, observations=obs)
        self.assertFalse(result['value'])
        self.assertEqual(result['next_at'], '2026-09-11T11:00:00Z')
        self.assertEqual(result['inputs'], self.evaluate(trigger, observations=obs,
                         now=self.now + dt.timedelta(hours=1))['inputs'])
        self.assertIsNone(self.evaluate({'external': {'ref': 'event', 'equals': True}},
                         observations=obs, now=self.now + dt.timedelta(days=1))['value'])

    def test_invalid_graph_data_is_unknown_and_nested_bool_is_typed(self):
        trigger = {'changed': 'f.a'}
        self.assertIsNone(self.evaluate(trigger, values={'f.a': float('nan')}, baseline={'f.a': 1})['value'])
        self.assertTrue(self.evaluate(trigger, values={'f.a': {'x': [True]}},
                                       baseline={'f.a': {'x': [1]}})['value'])
        with self.assertRaises(ValueError):
            self.evaluate({'manual': 'confirm'}, now=dt.datetime(2026, 9, 10))

    def test_normalize_dates_and_reject_invalid_data(self):
        self.assertEqual(T.normalize({'d': dt.date(2026, 9, 10), 't': self.now}),
                         {'d': '2026-09-10', 't': '2026-09-10T12:00:00Z'})
        for value in (float('nan'), float('inf'), object(), {3: 'invalid'}, dt.datetime(2026, 9, 10)):
            with self.assertRaises(ValueError):
                T.normalize(value)
        loop = []; loop.append(loop)
        with self.assertRaises(ValueError):
            T.normalize(loop)

    def test_validation_rejects_unknowns_types_and_bounds(self):
        invalid = [{}, {'at': '2026-09-10', 'manual': 'extra'}, {'exec': 'rm'}, {'all': []},
                   {'all': {}}, {'manual': ''}, {'changed': 'undeclared'}, {'completed': 4},
                   {'condition': {'id': 'f.a', 'op': 'eval', 'value': 2}},
                   {'condition': {'id': 'f.a', 'op': '==', 'value': [1]}},
                   {'condition': {'id': 'f.a', 'op': '==', 'value': 1, 'extra': True}},
                   {'external': {'ref': 'x', 'equals': 1, 'max_age_hours': True}},
                   {'external': {'ref': 'x', 'equals': float('inf')}},
                   {'external': {'ref': 'x', 'equals': 1, 'max_age_hours': 0}},
                   {'at': '2026-09-10T12:00:00'}, {'any': [{'manual': 'x'}] * 100}]
        deep = {'manual': 'x'}
        for _ in range(8):
            deep = {'all': [deep]}
        invalid.append(deep)
        cyclic = {'all': []}; cyclic['all'].append(cyclic); invalid.append(cyclic)
        for trigger in invalid:
            with self.subTest(trigger=str(trigger)[:100]), self.assertRaises(ValueError):
                T.validate(trigger, ['f.a'])
        T.validate({'all': [{'changed': 'f.a'}, {'at': dt.date(2026, 9, 10)}]}, ['f.a'])

    def test_reference_helpers(self):
        trigger = {'all': [{'changed': 'f.a'}, {'condition': {'id': 'f.b', 'op': '==', 'value': 1}},
                           {'any': [{'completed': 'task.a'}, {'external': {'ref': 'pr:42', 'equals': True}}]}]}
        self.assertEqual(T.referenced_graph(trigger), {'f.a', 'f.b'})
        self.assertEqual(T.referenced_tasks(trigger), {'task.a'})
        self.assertEqual(T.referenced_external(trigger), {'pr:42'})


if __name__ == '__main__':
    unittest.main()
