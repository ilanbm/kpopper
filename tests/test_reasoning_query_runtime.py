"""Installed native KP4 acceptance, including mixed KP2/KP3/KP4 batches."""
import copy
import os
from pathlib import Path
import unittest

from scripts.reasoning.evaluate import Evaluator
from scripts.reasoning.runtime import Runtime
from scripts.reasoning.snapshot import Snapshot


def document():
    return {
        'meta': {'reasoning': {'version': 2, 'profile': 'core/v1',
                               'requires': ['arithmetic/v1', 'composition/v1', 'query/v1']}},
        'parameters': {'p.base': {'v': 3}},
        'items': {
            'item.a': {'amount': 2, 'enabled': True},
            'item.b': {'amount': 5, 'enabled': False},
            'item.c': {'amount': 7, 'enabled': True},
        },
        'scopes': {'scope.items': {'collection_scope': {
            'collection': 'items', 'fields': ['amount', 'enabled'],
        }}},
        'calculations': {'m.enabled': {'rule': {'query': {
            'version': 1, 'scope': 'scope.items', 'op': 'count',
            'where': {'column': 'enabled'},
        }}}},
    }


def query(op, **fields):
    return {'query': {'version': 1, 'scope': 'scope.items', 'op': op, **fields}}


def authored(value):
    if value['type'] == 'number':
        return int(value['numerator']) if value['denominator'] == '1' else (
            int(value['numerator']), int(value['denominator']))
    if value['type'] == 'boolean':
        return value['value']
    if value['type'] == 'text':
        return value['value']
    if value['type'] == 'null':
        return None
    if value['type'] == 'list':
        return [authored(item) for item in value['items']]
    if value['type'] == 'record':
        return {key: authored(item) for key, item in value['fields'].items()}
    raise AssertionError(value)


class RetainingRuntime:
    def __init__(self, inner, *, corrupt_last=False):
        self.inner = inner
        self.corrupt_last = corrupt_last
        self.requests = None
        self.responses = None

    def request_many(self, requests):
        self.requests = requests
        self.responses = self.inner.request_many(requests)
        if self.corrupt_last:
            self.responses[-1]['request_id'] = '0' * 64
        return self.responses

    def implementation_for(self, request):
        return self.inner.implementation_for(request)


class NativeQueryAcceptance(unittest.TestCase):
    def setUp(self):
        archive = os.environ.get('KPOPPER_QUERY_ARCHIVE')
        self.runtime = Runtime(Path(archive).resolve()) if archive else Runtime()
        self.snapshot = Snapshot.from_data(document())
        self.engine = Evaluator(self.snapshot, runtime=self.runtime)

    def evaluate(self, operation):
        return self.engine.evaluate(operation, declared=['scope.items'])

    def test_all_operations_and_empty_semantics_are_native(self):
        enabled = {'column': 'enabled'}
        amount = {'column': 'amount'}
        cases = [
            (query('filter', where=enabled), ['item.a', 'item.c']),
            (query('project', value=amount), [2, 5, 7]),
            (query('select', where=enabled, value=amount), [2, 7]),
            (query('count', where=enabled), 2),
            (query('sum', where=enabled, value=amount), 9),
            (query('all', where=enabled), False),
            (query('any', where=enabled), True),
        ]
        for operation, expected in cases:
            with self.subTest(op=operation['query']['op']):
                result = self.evaluate(operation)
                self.assertEqual(result['status'], 'ok')
                self.assertEqual(authored(result['value']['fields']['result']), expected)
                self.assertEqual(result['query_counts']['input_count'], 3)
                self.assertEqual(result['basis']['query_counts'], result['query_counts'])
                self.assertEqual(result['implementation']['protocol'], 'KP4')

        empty = document()
        empty['items'] = {}
        engine = Evaluator(Snapshot.from_data(empty), runtime=self.runtime)
        expectations = {'filter': [], 'project': [], 'select': [], 'count': 0,
                        'sum': 0, 'all': True, 'any': False}
        for op, expected in expectations.items():
            fields = ({'value': amount} if op == 'project' else
                      {'where': enabled, 'value': amount} if op in ('select', 'sum') else
                      {'where': enabled} if op != 'filter' else {'where': enabled})
            result = engine.evaluate(query(op, **fields), declared=['scope.items'])
            self.assertEqual(result['status'], 'ok')
            self.assertEqual(authored(result['value']['fields']['result']), expected)

    def test_stored_root_and_mixed_protocol_batch(self):
        stored = self.engine.evaluate({'ref': 'm.enabled'}, declared=['m.enabled'])
        self.assertEqual(stored['status'], 'ok')
        self.assertEqual([item['kind'] for item in stored['executed_reads']], ['node', 'scope'])
        self.assertEqual(stored['potential_ids'], ['m.enabled', 'scope.items'])
        self.assertEqual(stored['cost']['node_evaluations'], {'m.enabled': 1})

        results = self.engine.evaluate_many([
            ({'ref': 'p.base'}, ['p.base']),
            ({'list': [{'num': '1'}, {'text': 'x'}]}, []),
            (query('count', where={'column': 'enabled'}), ['scope.items']),
        ])
        self.assertEqual([item['implementation']['protocol'] for item in results],
                         ['KP2', 'KP3', 'KP4'])
        self.assertEqual([item['status'] for item in results], ['ok', 'ok', 'ok'])

    def test_batch_validates_every_response_before_acceptance_and_detaches_outputs(self):
        operations = [
            (query('count', where={'column': 'enabled'}), ['scope.items']),
            (query('sum', value={'column': 'amount'}), ['scope.items']),
        ]
        retaining = RetainingRuntime(self.runtime)
        accepted = Evaluator(self.snapshot, runtime=retaining).evaluate_many(operations)
        stable = copy.deepcopy(accepted)
        retaining.responses[0]['value']['fields']['result']['numerator'] = '999'
        retaining.responses[0]['query_counts']['input_count'] = 999
        retaining.requests[0]['scope']['members'][0]['fields']['amount']['value']['numerator'] = '999'
        self.assertEqual(accepted, stable)

        corrupting = RetainingRuntime(self.runtime, corrupt_last=True)
        refused = Evaluator(self.snapshot, runtime=corrupting).evaluate_many(operations)
        self.assertEqual([item['status'] for item in refused],
                         ['operational_error', 'operational_error'])
        self.assertTrue(all(item['value'] is None and item['basis'] is None for item in refused))

    def test_semantic_growth_refuses_before_native_scan(self):
        operation = query('project', value={'op': 'mul', 'args': [
            {'column': 'amount'}, {'column': 'amount'},
        ]})
        result = Evaluator(self.snapshot, runtime=self.runtime, limits={'digits': 1}).evaluate(
            operation, declared=['scope.items'])
        self.assertEqual(result['status'], 'limit')
        self.assertIsNone(result['query_counts'])
        self.assertIsNone(result['basis'])
        self.assertEqual([item['code'] for item in result['diagnostics']], ['digit_limit'])


if __name__ == '__main__':
    unittest.main()
