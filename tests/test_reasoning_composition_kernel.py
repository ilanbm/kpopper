"""Versioned composition framing and native semantics, without compiler setup."""
import os
import subprocess
import unittest

from scripts.reasoning.transport import decode_response, encode_request

BINARY = os.environ.get('KPOPPER_REASONING_TEST_BINARY')
if os.environ.get('KPOPPER_REQUIRE_REASONING_TESTS') == '1' and not BINARY:
    raise RuntimeError('KPOPPER_REASONING_TEST_BINARY is required')


def n(value):
    return {'num': str(value)}


def op(name, *args):
    return {'op': name, 'args': list(args)}


def req(expression, nodes=None, declared=None, **limits):
    return {'protocol': 'KP3', 'expression': expression, 'nodes': nodes or {},
            'declared': declared or [], 'limits': limits}


class CompositionTransportTests(unittest.TestCase):
    def test_new_protocol_is_explicit_and_old_wire_stays_closed(self):
        expression = {'record': {'é': {'list': [n(1), {'bool': False}]}, '': {'null': True}}}
        encoded = encode_request(req(expression))
        self.assertTrue(encoded.startswith('KP3\t'))
        self.assertTrue(encoded.isascii())
        with self.assertRaises(ValueError):
            encode_request({'nodes': {}, 'declared': [], 'expression': expression})

    def test_recursive_values_are_typed_and_canonical(self):
        response = 'KR3\tok\tl\t2\tn\t1\t3\tm\t1\t78\tb\t1\t0\t0\t0\t1\t1\t0'
        result = decode_response(response)
        self.assertEqual(result['value'], {'type': 'list', 'items': [
            {'type': 'number', 'numerator': '1', 'denominator': '3'},
            {'type': 'record', 'fields': {'x': {'type': 'boolean', 'value': True}}}]})
        with self.assertRaises(ValueError):
            decode_response(response.replace('KR3', 'KR2'))
        for value in ('m\t2\t62\tz\t61\tz', 'm\t2\t61\tz\t61\tz', 'l\t1\tu'):
            with self.subTest(value=value), self.assertRaises(ValueError):
                decode_response('KR3\tok\t' + value + '\t0\t0\t0\t1\t1\t0')


@unittest.skipUnless(BINARY, 'set KPOPPER_REASONING_TEST_BINARY for native conformance')
class CompositionKernelTests(unittest.TestCase):
    def batch(self, requests):
        payload = ('\n'.join(encode_request(r) for r in requests) + '\n').encode('ascii')
        run = subprocess.run([BINARY], input=payload, capture_output=True, timeout=30, check=True)
        self.assertEqual(run.stderr, b'')
        lines = run.stdout.decode('ascii').splitlines()
        self.assertEqual(len(lines), len(requests))
        return [decode_response(line) for line in lines]

    def evaluate(self, expression, **kwargs):
        return self.batch([req(expression, **kwargs)])[0]

    def test_boolean_dominance_retains_unknown_and_runtime_error_diagnostics(self):
        bad = op('eq', op('div', n(1), n(0)), n(0))
        for name, dominant in [('and', False), ('or', True)]:
            for other, code, declared in [({'ref': 'missing'}, 'missing_reference', ['missing']),
                                           (bad, 'division_by_zero', [])]:
                for args in [({'bool': dominant}, other), (other, {'bool': dominant})]:
                    with self.subTest(name=name, args=args):
                        result = self.evaluate(op(name, *args), declared=declared)
                        self.assertEqual((result['status'], result['value']),
                                         ('ok', {'type': 'boolean', 'value': dominant}))
                        self.assertEqual(result['diagnostics'], [code])
        self.assertEqual(self.evaluate(op('not', bad))['status'], 'error')

    def test_conditionals_keep_potential_reads_and_validate_every_branch(self):
        expression = {'if': {'bool': True}, 'then': {'ref': 'a'}, 'else': {'ref': 'b'}}
        result = self.evaluate(expression, nodes={'a': n(1), 'b': op('div', n(1), n(0))}, declared=['a', 'b'])
        self.assertEqual(result['status'], 'ok')
        self.assertEqual(result['diagnostics'], [])
        self.assertEqual(result['executed_reads'], ['a'])
        self.assertEqual(result['potential_reads'], ['a', 'b'])
        expression['else'] = op('add', n(1), {'bool': False})
        result = self.evaluate(expression, nodes={'a': n(1)}, declared=['a'])
        self.assertEqual((result['status'], result['diagnostics'], result['steps']),
                         ('error', ['type_error'], 0))

    def test_potential_cycle_refuses_before_execution(self):
        root = {'if': {'bool': True}, 'then': n(1), 'else': {'ref': 'a'}}
        result = self.evaluate(root, nodes={'a': {'ref': 'a'}}, declared=['a'])
        self.assertEqual((result['status'], result['diagnostics'], result['steps']),
                         ('error', ['cyclic_reference'], 0))

    def test_stored_containers_are_complete_values_and_missing_field_is_unknown(self):
        record = {'record': {'a': n(1), 'b': {'ref': 'missing'}}}
        result = self.evaluate({'field': record, 'key': 'a'}, declared=['missing'])
        self.assertEqual((result['status'], result['diagnostics']), ('unknown', ['missing_reference']))
        missing = {'field': {'record': {}}, 'key': 'absent'}
        self.assertEqual(self.evaluate(missing)['diagnostics'], ['missing_field'])
        result = self.evaluate({'if': {'bool': True}, 'then': n(2), 'else': missing})
        self.assertEqual((result['status'], result['diagnostics']), ('ok', []))

    def test_container_equality_checks_all_member_types_before_truth(self):
        for left, right, status, expected in [
                ({'list': []}, {'list': []}, 'ok', True),
                ({'list': [n(1)]}, {'list': []}, 'ok', False),
                ({'record': {'a': n(1)}}, {'record': {'b': n(1)}}, 'ok', False),
                ({'list': [n(1), {'text': 'x'}]}, {'list': [n(2), n(3)]}, 'error', None)]:
            result = self.evaluate(op('eq', left, right))
            self.assertEqual(result['status'], status)
            self.assertEqual(result['value'], None if expected is None else {'type': 'boolean', 'value': expected})

    def test_expanded_nodes_and_depth_are_per_request_limits(self):
        nodes = {'x0': {'list': []}}
        for i in range(1, 201):
            nodes['x' + str(i)] = {'list': [{'ref': 'x' + str(i - 1)}]}
        result = self.evaluate({'ref': 'x200'}, nodes=nodes, declared=list(nodes), depth=1000)
        self.assertEqual((result['status'], result['diagnostics']), ('limit', ['collection_limit']))
        result = self.evaluate({'list': [n(1), n(2), n(3)]}, value_nodes=3)
        self.assertEqual((result['status'], result['diagnostics']), ('limit', ['collection_limit']))

    def test_shared_text_cannot_amplify_output_and_poison_another_request(self):
        big = req({'list': [{'ref': 'text'}] * 40}, nodes={'text': {'text': 'x' * (2 * 1024 * 1024)}}, declared=['text'])
        bad, good = self.batch([big, {'nodes': {}, 'declared': [], 'expression': n(7)}])
        self.assertEqual((bad['status'], bad['diagnostics']), ('limit', ['collection_limit']))
        self.assertEqual((good['status'], good['value']['numerator']), ('ok', '7'))


if __name__ == '__main__':
    unittest.main()
