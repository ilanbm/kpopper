"""Pure query/v1 IR, KP4 canonical data, and response binding tests."""
import copy
import datetime
import unittest

from scripts.reasoning import query as Q
from scripts.reasoning.snapshot import Snapshot


REQUEST_ID = '1' * 64


def document():
    return {
        'schema': {'deps': 'rests_on', 'snapshot': 'seen', 'predicate': 'wrong_if'},
        'items': {
            'a': {'amount': 2, 'enabled': True, 'note': 'one',
                  'when': datetime.date(2026, 9, 18)},
            'b': {'amount': 0.5, 'enabled': False, 'note': None},
        },
        'scopes': {'scope.items': {'collection_scope': {
            'collection': 'items',
            'fields': ['amount', 'enabled', 'note', 'when'],
        }}},
    }


def capture():
    return Snapshot.from_data(document()).capture_query_scope('scope.items')


def authored(op, **fields):
    return {'query': {'version': 1, 'scope': 'scope.items', 'op': op, **fields}}


def prepared(operation=None, root=None, limits=None):
    operation = operation or authored('count', where={'column': 'enabled'})
    return Q.prepare(capture(), operation, request_id=REQUEST_ID,
                     declared_capabilities=['arithmetic/v1', 'composition/v1', 'query/v1'],
                     root_witness=root, limits=limits)


def number(value):
    return {'type': 'number', 'numerator': str(value), 'denominator': '1'}


def counts(input_count=2, definite=1, membership=0, unknown_value=0, errors=0):
    return {'input_count': input_count, 'definite_match_count': definite,
            'unknown_membership_count': membership, 'unknown_value_count': unknown_value,
            'error_count': errors}


def response_for(item, query_counts=None, status='ok'):
    query_counts = counts() if query_counts is None else query_counts
    value = None
    if status == 'ok':
        value = {'type': 'record', 'fields': {
            'input_count': number(query_counts['input_count']),
            'definite_match_count': number(query_counts['definite_match_count']),
            'unknown_membership_count': number(query_counts['unknown_membership_count']),
            'unknown_value_count': number(query_counts['unknown_value_count']),
            'error_count': number(query_counts['error_count']),
            'result': number(query_counts['definite_match_count']),
        }}
    cost = item['basis_template']['preflight_cost']
    root = item['request']['root_witness']
    return {'version': 4, 'request_id': REQUEST_ID, 'status': status, 'value': value,
            'diagnostics': [], 'query_counts': query_counts,
            'executed_reads': copy.deepcopy(item['potential_dependencies']),
            'cost': {'steps': max(1, cost['step_upper_bound']),
                     'preflight_steps': cost['preflight_steps'],
                     'node_evaluations': {} if root is None else {root['id']: 1},
                     'candidates': cost['candidates'], 'field_reads': cost['field_reads'],
                     'evaluated_field_reads': 1}}


class QueryIRTests(unittest.TestCase):
    def test_every_operation_has_one_exact_key_schema(self):
        where = {'op': 'eq', 'args': [{'column': 'enabled'}, {'bool': True}]}
        value = {'column': 'amount'}
        cases = {
            'filter': {'where': where}, 'project': {'value': value},
            'select': {'where': where, 'value': value}, 'count': {'where': where},
            'sum': {'value': value}, 'sum-where': {'where': where, 'value': value},
            'all': {'where': where}, 'any': {'where': where},
        }
        for name, fields in cases.items():
            op = name.partition('-')[0]
            with self.subTest(op=name):
                lowered = Q.lower(authored(op, **fields),
                                  ['amount', 'enabled', 'note', 'when'])
                self.assertEqual(lowered['query']['op'], op)
                self.assertEqual(set(lowered['query']), {'version', 'scope', 'op', *fields})
                invalid = copy.deepcopy(authored(op, **fields))
                invalid['query']['extra'] = False
                with self.assertRaisesRegex(ValueError, 'keys'):
                    Q.lower(invalid, ['amount', 'enabled', 'note', 'when'])

    def test_row_grammar_covers_every_node_and_rejects_refs_dynamic_keys_and_arity(self):
        fields = ['amount', 'enabled', 'note', 'when']
        leaves = [{'column': 'amount'}, {'num': '-1.5e+2'}, {'bool': False},
                  {'text': 'x'}, {'null': True}]
        for leaf in leaves:
            self.assertEqual(Q.lower_row(leaf, fields), leaf)
        for op in sorted(Q.ARITHMETIC):
            tree = {'op': op, 'args': [{'column': 'amount'}, {'num': '1'}]}
            self.assertEqual(Q.lower_row(tree, fields), tree)
        for op in ('and', 'or'):
            Q.lower_row({'op': op, 'args': [{'bool': True}, {'column': 'enabled'}]}, fields)
        Q.lower_row({'op': 'not', 'args': [{'column': 'enabled'}]}, fields)
        composed = {'field': {'record': {'z': {'list': [
            {'if': {'column': 'enabled'}, 'then': {'text': 'yes'}, 'else': {'text': 'no'}}
        ]}}}, 'key': 'z'}
        self.assertEqual(Q.lower_row(composed, fields), composed)
        invalid = ({'ref': 'a'}, {'column': 'secret'}, {'call': 'x'},
                   {'field': {'column': 'note'}, 'key': {'column': 'amount'}},
                   {'op': 'not', 'args': [{'bool': True}, {'bool': False}]})
        for tree in invalid:
            with self.subTest(tree=tree), self.assertRaises(ValueError):
                Q.lower_row(tree, fields)

    def test_columns_and_modules_are_closure_local(self):
        operation = Q.lower(authored('select',
            where={'op': 'and', 'args': [
                {'op': 'gt', 'args': [{'column': 'amount'}, {'num': '0'}]},
                {'column': 'enabled'}]},
            value={'field': {'record': {'note': {'column': 'note'}}}, 'key': 'note'}),
            ['amount', 'enabled', 'note', 'when'])
        self.assertEqual(Q.required_columns(operation), ['amount', 'enabled', 'note'])
        self.assertEqual(Q.required_modules(operation),
                         ['arithmetic/v1', 'composition/v1', 'query/v1'])
        literal = Q.lower(authored('project', value={'column': 'note'}),
                          ['amount', 'enabled', 'note', 'when'])
        self.assertEqual(Q.required_modules(literal), ['query/v1'])

    def test_malformed_operation_types_versions_and_cycles_refuse(self):
        bad = [None, {}, {'query': []}, authored('unknown', where={'bool': True}),
               {'query': {'version': True, 'scope': 'scope.items', 'op': 'count',
                          'where': {'bool': True}}},
               {'query': {'version': 1, 'scope': 3, 'op': 'count', 'where': {'bool': True}}}]
        for value in bad:
            with self.subTest(value=value), self.assertRaises(ValueError):
                Q.lower(value, ['amount', 'enabled', 'note', 'when'])
        cycle = {'list': []}
        cycle['list'].append(cycle)
        with self.assertRaisesRegex(ValueError, 'cycle'):
            Q.lower_row(cycle, ['amount', 'enabled', 'note', 'when'])


class CanonicalKP4Tests(unittest.TestCase):
    def test_exact_string_escaping_and_raw_unicode(self):
        value = {'x': '"\\/\b\f\n\r\t\x01א'}
        encoded = Q.canonical_json_bytes(value)
        self.assertEqual(encoded, b'{"x":"\\"\\\\/\\b\\f\\n\\r\\t\\u0001\xd7\x90"}')
        self.assertEqual(Q.decode_canonical_json(encoded), value)
        for noncanonical in (b'{"x":"\\u05d0"}', b'{ "x":"a"}', b'{"b":1,"a":2}',
                             b'{"x":1,"x":1}', b'{"x":1.0}'):
            with self.subTest(payload=noncanonical), self.assertRaises(ValueError):
                Q.decode_canonical_json(noncanonical)

    def test_rationals_are_reduced_positive_and_canonical(self):
        valid = {'type': 'number', 'numerator': '-2', 'denominator': '3'}
        self.assertEqual(Q.decode_canonical_json(Q.canonical_json_bytes(valid)), valid)
        for numerator, denominator in (('2', '4'), ('-0', '1'), ('01', '1'), ('1', '0'), ('1', '-2')):
            with self.subTest(pair=(numerator, denominator)), self.assertRaises(ValueError):
                Q.canonical_json_bytes({'type': 'number', 'numerator': numerator,
                                        'denominator': denominator})

    def test_frame_length_terminator_and_payload_are_exact(self):
        value = {'a': 1}
        frame = Q.encode_frame(value)
        self.assertEqual(Q.decode_frame(frame), value)
        for invalid in (b'KP4 07\n{"a":1}\n', frame[:-1], frame + b'x',
                        b'KP4 6\n{"a":1}\n', b'KP3 7\n{"a":1}\n'):
            with self.subTest(frame=invalid), self.assertRaises(ValueError):
                Q.decode_frame(invalid)


class ScopeAndPreparationTests(unittest.TestCase):
    def test_scope_adapter_preserves_missing_contested_and_unavailable_reasons(self):
        data = capture().to_data()
        data['candidates']['b']['amount']['status'] = 'contested'
        data['candidates']['b']['amount']['alternatives'] = [['left', {'amount': 1}]]
        data['candidates']['a']['note'] = {
            'status': 'unavailable', 'reason': 'formula_value',
            'fingerprint': data['candidates']['a']['note']['fingerprint']}
        rows = Q.scope_rows(data)['members']
        self.assertEqual([row['id'] for row in rows], ['a', 'b'])
        self.assertEqual(rows[0]['fields']['amount']['value'], number(2))
        self.assertEqual(rows[0]['fields']['when'],
                         {'status': 'unavailable', 'reason': 'unsupported_type'})
        self.assertEqual(rows[0]['fields']['note'],
                         {'status': 'unavailable', 'reason': 'formula_value'})
        self.assertEqual(rows[1]['fields']['amount'], {'status': 'contested'})
        self.assertEqual(rows[1]['fields']['when'], {'status': 'missing'})

    def test_scope_adapter_consumes_authoritative_query_rows_and_keeps_empty_grants(self):
        captured = capture()
        adapted = Q.scope_rows(captured)
        self.assertEqual(adapted['fields'], ['amount', 'enabled', 'note', 'when'])
        self.assertEqual(adapted['members'], captured.query_rows())
        empty_document = document()
        empty_document['items'] = {}
        empty = Snapshot.from_data(empty_document).capture_query_scope('scope.items')
        request = Q.prepare(empty, authored('project', value={'column': 'amount'}),
                            request_id=REQUEST_ID,
                            declared_capabilities=['query/v1'])['request']
        self.assertEqual(request['scope']['fields'], ['amount', 'enabled', 'note', 'when'])
        self.assertEqual(request['scope']['members'], [])
        self.assertEqual(Q.decode_request(Q.encode_request(request)), request)

    def test_formula_backed_missing_value_and_invalid_value_are_explicit(self):
        data = capture().to_data()
        missing = data['candidates']['b']['when']
        missing['computed_basis'] = {'status': 'available'}
        invalid = []
        invalid.append(invalid)
        data['candidates']['a']['amount']['value'] = invalid
        rows = Q.scope_rows(data)['members']
        self.assertEqual(rows[0]['fields']['amount'],
                         {'status': 'unavailable', 'reason': 'invalid_value'})
        self.assertEqual(rows[1]['fields']['when'],
                         {'status': 'unavailable', 'reason': 'formula_value'})

    def test_resources_are_closed_lowerable_and_preflighted(self):
        defaults = Q.resources_v4()
        self.assertEqual(defaults['version'], 'resources/v4')
        self.assertEqual(Q.resources_v4({'candidates': 2})['candidates'], 2)
        for invalid in ({'extra': 1}, {'candidates': 0}, {'candidates': 10001},
                        {'version': 'resources/v3'}, {'steps': True}):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                Q.resources_v4(invalid)
        with self.assertRaisesRegex(ValueError, 'candidate_limit'):
            prepared(limits={'candidates': 1})
        with self.assertRaisesRegex(ValueError, 'field_read_limit'):
            prepared(limits={'field_reads': 7})
        huge_exponent = authored('count', where={'op': 'eq', 'args': [
            {'column': 'amount'}, {'num': '1e' + '9' * 1000}]})
        with self.assertRaisesRegex(ValueError, 'digit_limit'):
            prepared(huge_exponent)
        prepared(authored('count', where={'op': 'eq', 'args': [
            {'column': 'amount'}, {'num': '0.5'}]}), limits={'digits': 1})

    def test_inline_and_stored_root_requests_are_detached_and_bound(self):
        inline = prepared()
        self.assertEqual(inline['resources'], 'resources/v4')
        self.assertEqual(inline['basis_template']['recipe'], 'query-inputs/v1')
        self.assertEqual(set(inline['request']['scope']), {'id', 'witness', 'fields', 'members'})
        self.assertEqual(inline['request']['scope']['fields'],
                         ['amount', 'enabled', 'note', 'when'])
        self.assertEqual([item['kind'] for item in inline['potential_dependencies']], ['scope'])
        self.assertEqual(inline['potential_ids'], ['scope.items'])
        stored = prepared(root={'kind': 'node', 'id': 'q.total', 'fingerprint': 'a' * 64})
        self.assertEqual([item['kind'] for item in stored['potential_dependencies']], ['node', 'scope'])
        self.assertEqual(stored['potential_ids'], ['q.total', 'scope.items'])
        wire = Q.encode_request(stored['request'])
        self.assertEqual(Q.decode_request(wire), stored['request'])
        changed = capture().to_data()
        before = copy.deepcopy(stored['request'])
        changed['candidates']['a']['amount']['value'] = 99
        Q.prepare(changed, authored('count', where={'column': 'enabled'}),
                  request_id='2' * 64,
                  declared_capabilities=['arithmetic/v1', 'composition/v1', 'query/v1'])
        self.assertEqual(stored['request'], before)

    def test_prepare_requires_exact_declared_closure_and_scope(self):
        operation = authored('count', where={'op': 'eq', 'args': [
            {'column': 'amount'}, {'num': '2'}]})
        with self.assertRaisesRegex(ValueError, 'undeclared modules'):
            Q.prepare(capture(), operation, request_id=REQUEST_ID,
                      declared_capabilities=['query/v1'])
        wrong = copy.deepcopy(operation)
        wrong['query']['scope'] = 'scope.other'
        with self.assertRaisesRegex(ValueError, 'does not match'):
            Q.prepare(capture(), wrong, request_id=REQUEST_ID,
                      declared_capabilities=['arithmetic/v1', 'query/v1'])


class ResponseValidationTests(unittest.TestCase):
    def test_ok_response_counters_witnesses_and_basis_finalize(self):
        item = prepared()
        response = response_for(item)
        self.assertEqual(Q.validate_response(response, item), response)
        finalized = Q.finalize_basis(item, response)
        self.assertEqual(finalized['query_counts'], counts())
        self.assertNotEqual(finalized['digest'], item['basis_template']['digest'])
        framed = Q.encode_frame(response, 'KR4')
        self.assertEqual(Q.decode_response(framed, item), response)

    def test_diagnostics_are_sorted_unique_and_locations_are_bound(self):
        item = prepared(authored('all', where={'column': 'enabled'}))
        response = response_for(item, counts(definite=1, membership=1), status='ok')
        response['diagnostics'] = [{
            'code': 'where_unknown', 'related_ids': ['b', 'scope.items'],
            'locations': [{'candidate': 'b', 'column': 'enabled', 'phase': 'where'}],
        }]
        self.assertEqual(Q.validate_response(response, item), response)
        response['diagnostics'].append(copy.deepcopy(response['diagnostics'][0]))
        with self.assertRaisesRegex(ValueError, 'sorted and unique'):
            Q.validate_response(response, item)

    def test_response_rejects_counter_order_binding_and_cost_forgery(self):
        item = prepared(root={'kind': 'node', 'id': 'q.total', 'fingerprint': 'a' * 64})
        mutations = []
        wrong_counter = response_for(item)
        wrong_counter['value']['fields']['input_count'] = number(3)
        mutations.append(wrong_counter)
        wrong_reads = response_for(item)
        wrong_reads['executed_reads'] = wrong_reads['executed_reads'][1:]
        mutations.append(wrong_reads)
        wrong_root = response_for(item)
        wrong_root['cost']['node_evaluations'] = {}
        mutations.append(wrong_root)
        wrong_request = response_for(item)
        wrong_request['request_id'] = '2' * 64
        mutations.append(wrong_request)
        wrong_preflight = response_for(item)
        wrong_preflight['cost']['field_reads'] += 1
        mutations.append(wrong_preflight)
        over_steps = response_for(item)
        over_steps['cost']['steps'] = item['request']['resources']['steps'] + 1
        mutations.append(over_steps)
        for response in mutations:
            with self.subTest(response=response), self.assertRaises(ValueError):
                Q.validate_response(response, item)

    def test_preflight_refusals_have_no_counts_or_final_basis(self):
        item = prepared()
        response = response_for(item, query_counts=None, status='limit')
        response['query_counts'] = None
        response['value'] = None
        response['diagnostics'] = [{
            'code': 'step_limit', 'related_ids': ['scope.items'], 'locations': []}]
        self.assertEqual(Q.validate_response(response, item), response)
        self.assertIsNone(Q.finalize_basis(item, response))
        response['query_counts'] = counts()
        with self.assertRaisesRegex(ValueError, 'cannot claim'):
            Q.validate_response(response, item)

    def test_unknown_requires_completed_row_scan_counts(self):
        item = prepared()
        response = response_for(item, status='unknown')
        response['value'] = None
        response['query_counts'] = None
        with self.assertRaisesRegex(ValueError, 'needs query counts'):
            Q.validate_response(response, item)


if __name__ == '__main__':
    unittest.main()
