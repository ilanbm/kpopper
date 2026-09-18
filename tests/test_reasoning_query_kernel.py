"""Independent query/v1 oracle and optional direct Lean kernel conformance."""
from fractions import Fraction
import json
import os
from pathlib import Path
import random
import re
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
CORPUS = json.loads((ROOT / 'tests/reasoning/query-v1.json').read_text(encoding='utf-8'))
LEAN_ROOT = ROOT / 'scripts/reasoning/lean'
LEAN = os.environ.get('KPOPPER_QUERY_LEAN')


def number(value):
    return ('number', Fraction(value))


def typed_value(data):
    kind = data['type']
    if kind == 'number':
        return ('number', Fraction(int(data['numerator']), int(data['denominator'])))
    if kind == 'boolean':
        return ('boolean', data['value'])
    if kind == 'text':
        return ('text', data['value'])
    if kind == 'null':
        return ('null', None)
    if kind == 'list':
        return ('list', tuple(typed_value(value) for value in data['items']))
    if kind == 'record':
        return ('record', tuple((key, typed_value(value))
                                for key, value in sorted(data['fields'].items())))
    raise AssertionError(kind)


def canonical_number(value):
    return {'type': 'number', 'numerator': str(value.numerator),
            'denominator': str(value.denominator)}


def render_value(value):
    kind, payload = value
    if kind == 'number':
        return f'n:{payload.numerator}/{payload.denominator}'
    if kind == 'boolean':
        return 'b:true' if payload else 'b:false'
    if kind == 'text':
        return 's:' + payload
    if kind == 'null':
        return 'null'
    if kind == 'list':
        return '[' + ','.join(render_value(item) for item in payload) + ']'
    if kind == 'record':
        return '{' + ','.join(key + '=' + render_value(item) for key, item in payload) + '}'
    raise AssertionError(kind)


def result_record(counts, result):
    return ('record', (
        ('definite_match_count', number(counts[1])),
        ('error_count', number(counts[4])),
        ('input_count', number(counts[0])),
        ('result', result),
        ('unknown_membership_count', number(counts[2])),
        ('unknown_value_count', number(counts[3])),
    ))


def add_diag(diagnostics, code, candidate, column, phase):
    diagnostics.add((code, candidate, column, phase))


def unavailable(code, candidate, column, phase, diagnostics):
    add_diag(diagnostics, code, candidate, column, phase)
    return ('unknown', None)


def expression(expr, member, candidate, phase, diagnostics):
    if 'column' in expr:
        column = expr['column']
        field = member[column]
        status = field['status']
        if status == 'known':
            return ('known', typed_value(field['value']))
        code = {'missing': 'missing_column', 'contested': 'contested_column',
                'unavailable': 'column_type_error'}[status]
        return unavailable(code, candidate, column, phase, diagnostics)
    if 'num' in expr:
        return ('known', number(Fraction(expr['num'])))
    if 'bool' in expr:
        return ('known', ('boolean', expr['bool']))
    if 'text' in expr:
        return ('known', ('text', expr['text']))
    if 'null' in expr:
        return ('known', ('null', None))
    if 'op' in expr:
        op = expr['op']
        args = [expression(arg, member, candidate, phase, diagnostics)
                for arg in expr['args']]
        if op == 'not':
            state, value = args[0]
            if state != 'known':
                return (state, None)
            if value[0] != 'boolean':
                add_diag(diagnostics, 'type_error', candidate, '', phase)
                return ('error', None)
            return ('known', ('boolean', not value[1]))
        if op in ('and', 'or'):
            normalized = []
            for state, value in args:
                if state == 'known' and value[0] != 'boolean':
                    add_diag(diagnostics, 'type_error', candidate, '', phase)
                    state, value = 'error', None
                normalized.append((state, value))
            dominant = False if op == 'and' else True
            if any(state == 'known' and value == ('boolean', dominant)
                   for state, value in normalized):
                return ('known', ('boolean', dominant))
            if any(state == 'error' for state, _ in normalized):
                return ('error', None)
            if any(state == 'unknown' for state, _ in normalized):
                return ('unknown', None)
            values = [value[1] for _, value in normalized]
            return ('known', ('boolean', all(values) if op == 'and' else any(values)))
        left, right = args
        if op in ('eq', 'ne'):
            if left[0] == 'error' or right[0] == 'error':
                return ('error', None)
            if left[0] == 'unknown' or right[0] == 'unknown':
                return ('unknown', None)
            if left[1][0] != right[1][0]:
                add_diag(diagnostics, 'type_error', candidate, '', phase)
                return ('error', None)
            equal = left[1] == right[1]
            return ('known', ('boolean', equal if op == 'eq' else not equal))
        for state, value in (left, right):
            if state == 'known' and value[0] != 'number':
                add_diag(diagnostics, 'type_error', candidate, '', phase)
                return ('error', None)
        if left[0] == 'error' or right[0] == 'error':
            return ('error', None)
        if left[0] == 'unknown' or right[0] == 'unknown':
            return ('unknown', None)
        x, y = left[1][1], right[1][1]
        if op == 'div' and y == 0:
            add_diag(diagnostics, 'division_by_zero', candidate, '', phase)
            return ('error', None)
        if op in ('add', 'sub', 'mul', 'div'):
            value = {'add': x + y, 'sub': x - y, 'mul': x * y, 'div': x / y}[op]
            return ('known', number(value))
        value = {'lt': x < y, 'le': x <= y, 'gt': x > y, 'ge': x >= y}[op]
        return ('known', ('boolean', value))
    if 'if' in expr:
        condition = expression(expr['if'], member, candidate, phase, diagnostics)
        if condition[0] != 'known':
            return (condition[0], None)
        if condition[1][0] != 'boolean':
            add_diag(diagnostics, 'type_error', candidate, '', phase)
            return ('error', None)
        branch = expr['then'] if condition[1][1] else expr['else']
        return expression(branch, member, candidate, phase, diagnostics)
    if 'list' in expr:
        values = [expression(item, member, candidate, phase, diagnostics)
                  for item in expr['list']]
        if any(state == 'error' for state, _ in values):
            return ('error', None)
        if any(state == 'unknown' for state, _ in values):
            return ('unknown', None)
        return ('known', ('list', tuple(value for _, value in values)))
    if 'record' in expr:
        values = [(key, expression(value, member, candidate, phase, diagnostics))
                  for key, value in sorted(expr['record'].items())]
        if any(result[0] == 'error' for _, result in values):
            return ('error', None)
        if any(result[0] == 'unknown' for _, result in values):
            return ('unknown', None)
        return ('known', ('record', tuple((key, result[1]) for key, result in values)))
    if 'field' in expr:
        record = expression(expr['field'], member, candidate, phase, diagnostics)
        if record[0] != 'known':
            return (record[0], None)
        if record[1][0] != 'record':
            add_diag(diagnostics, 'type_error', candidate, '', phase)
            return ('error', None)
        fields = dict(record[1][1])
        if expr['key'] not in fields:
            add_diag(diagnostics, 'missing_field', candidate, expr['key'], phase)
            return ('unknown', None)
        return ('known', fields[expr['key']])
    raise AssertionError(expr)


def query_oracle(case):
    op = case['operation']
    members = case['members']
    diagnostics = set()
    counts = [len(members), 0, 0, 0, 0]
    values = []
    total = Fraction(0)
    dominated = False
    unknown_codes = {'missing_column', 'contested_column', 'column_type_error', 'missing_field'}
    for item in members:
        candidate, member = item['id'], item['fields']
        name = op['op']
        if name == 'project':
            counts[1] += 1
            before = set(diagnostics)
            projected = expression(op['value'], member, candidate, 'value', diagnostics)
            added = diagnostics - before
            value_unknown = projected[0] == 'unknown' or any(row[0] in unknown_codes for row in added)
            value_error = projected[0] == 'error' or any(row[0] not in unknown_codes for row in added)
            if projected[0] == 'known':
                values.append(projected[1])
            if value_unknown:
                counts[3] += 1
                add_diag(diagnostics, 'value_error', candidate, '', 'value')
            if value_error:
                counts[4] += 1
                add_diag(diagnostics, 'value_error', candidate, '', 'value')
            continue
        before = set(diagnostics)
        membership = (('known', ('boolean', True)) if name == 'sum' and 'where' not in op
                      else expression(op['where'], member, candidate, 'where', diagnostics))
        added = diagnostics - before
        where_unknown = membership[0] == 'unknown' or any(row[0] in unknown_codes for row in added)
        where_error = membership[0] == 'error' or any(row[0] not in unknown_codes for row in added)
        if where_unknown:
            counts[2] += 1
            add_diag(diagnostics, 'where_unknown', candidate, '', 'where')
        if where_error:
            add_diag(diagnostics, 'where_error', candidate, '', 'where')
        value_error = False
        if membership == ('known', ('boolean', True)):
            counts[1] += 1
            if name == 'filter':
                values.append(('text', candidate))
            elif name == 'select':
                before = set(diagnostics)
                projected = expression(op['value'], member, candidate, 'value', diagnostics)
                added = diagnostics - before
                value_unknown = projected[0] == 'unknown' or any(row[0] in unknown_codes for row in added)
                value_error = projected[0] == 'error' or any(row[0] not in unknown_codes for row in added)
                if projected[0] == 'known':
                    values.append(projected[1])
                if value_unknown:
                    counts[3] += 1
                    add_diag(diagnostics, 'value_error', candidate, '', 'value')
                if value_error:
                    add_diag(diagnostics, 'value_error', candidate, '', 'value')
            elif name == 'sum':
                before = set(diagnostics)
                projected = expression(op['value'], member, candidate, 'value', diagnostics)
                added = diagnostics - before
                value_unknown = projected[0] == 'unknown' or any(row[0] in unknown_codes for row in added)
                value_error = projected[0] == 'error' or any(row[0] not in unknown_codes for row in added)
                if projected[0] == 'known' and projected[1][0] == 'number':
                    total += projected[1][1]
                elif projected[0] == 'known':
                    value_error = True
                if value_unknown:
                    counts[3] += 1
                    add_diag(diagnostics, 'value_error', candidate, '', 'value')
                if value_error:
                    add_diag(diagnostics, 'value_error', candidate, '', 'value')
            elif name == 'any':
                dominated = True
        elif membership == ('known', ('boolean', False)):
            if name == 'all':
                dominated = True
        if where_error or value_error:
            counts[4] += 1
    name = op['op']
    if dominated:
        status = 'ok'
    elif counts[4]:
        status = 'error'
    elif counts[2] or counts[3]:
        status = 'unknown'
    else:
        status = 'ok'
    if name in ('filter', 'project', 'select'):
        result = ('list', tuple(values))
    elif name == 'count':
        result = number(counts[1])
    elif name == 'sum':
        result = number(total)
    elif name == 'all':
        result = ('boolean', counts[0] == counts[1])
    else:
        result = ('boolean', counts[1] > 0)
    return {'status': status, 'counts': counts,
            'result': result if status == 'ok' else None,
            'diagnostic_codes': [item[0] for item in sorted(diagnostics)]}


def lean_string(value):
    return json.dumps(value, ensure_ascii=False)


def lean_value(value):
    kind = value['type']
    if kind == 'number':
        return f'.scalar (.number (mkRat ({value["numerator"]} : Int) {value["denominator"]}))'
    if kind == 'boolean':
        return f'.scalar (.boolean {str(value["value"]).lower()})'
    if kind == 'text':
        return f'.scalar (.text {lean_string(value["value"])})'
    if kind == 'null':
        return '.scalar .null'
    if kind == 'list':
        return 'makeArray [' + ','.join(lean_value(item) for item in value['items']) + ']'
    if kind == 'record':
        return 'makeObject [' + ','.join(
            f'({lean_string(key)}, {lean_value(item)})'
            for key, item in sorted(value['fields'].items())) + ']'
    raise AssertionError(kind)


def lean_field(field):
    status = field['status']
    if status == 'known':
        return f'.known ({lean_value(field["value"])})'
    if status in ('missing', 'contested'):
        return '.' + status
    reason = {'unsupported_type': 'unsupportedType', 'formula_value': 'formulaValue',
              'invalid_value': 'invalidValue'}[field['reason']]
    return f'.unavailable .{reason}'


def lean_expr(expr):
    if 'column' in expr:
        return f'.column {lean_string(expr["column"])}'
    if 'num' in expr:
        value = Fraction(expr['num'])
        return f'.literal (.number (mkRat ({value.numerator} : Int) {value.denominator}))'
    if 'bool' in expr:
        return f'.literal (.boolean {str(expr["bool"]).lower()})'
    if 'text' in expr:
        return f'.literal (.text {lean_string(expr["text"])})'
    if 'null' in expr:
        return '.literal .null'
    if 'op' in expr:
        name, args = expr['op'], expr['args']
        if name == 'not':
            return f'.negate ({lean_expr(args[0])})'
        if name in ('and', 'or'):
            return f'.logic {str(name == "and").lower()} ({lean_expr(args[0])}) ({lean_expr(args[1])})'
        return f'.binary .{name} ({lean_expr(args[0])}) ({lean_expr(args[1])})'
    if 'if' in expr:
        return (f'.conditional ({lean_expr(expr["if"])}) ({lean_expr(expr["then"])}) '
                f'({lean_expr(expr["else"])})')
    if 'list' in expr:
        return '.array [' + ','.join(lean_expr(item) for item in expr['list']) + ']'
    if 'record' in expr:
        return '.object [' + ','.join(
            f'({lean_string(key)}, {lean_expr(value)})'
            for key, value in sorted(expr['record'].items())) + ']'
    if 'field' in expr:
        return f'.field ({lean_expr(expr["field"])}) {lean_string(expr["key"])}'
    raise AssertionError(expr)


def lean_operation(operation):
    name = operation['op']
    if name in ('filter', 'count', 'all', 'any'):
        return f'.{name} ({lean_expr(operation["where"])})'
    if name == 'project':
        return f'.project ({lean_expr(operation["value"])})'
    if name == 'select':
        return f'.select ({lean_expr(operation["where"])}) ({lean_expr(operation["value"])})'
    where = ('none' if 'where' not in operation
             else f'some ({lean_expr(operation["where"])})')
    return f'.sum ({where}) ({lean_expr(operation["value"])})'


def lean_scope(case):
    members = []
    for member in case['members']:
        fields = ','.join(f'({lean_string(name)}, {lean_field(member["fields"][name])})'
                          for name in case['fields'])
        members.append(f'{{ id := {lean_string(member["id"])}, fields := [{fields}] }}')
    field_list = ','.join(lean_string(name) for name in case['fields'])
    return ('{ id := "scope.items", fields := [' + field_list + '], witness := witness, '
            'members := [' + ','.join(members) + '] }')


def harness_source(cases):
    body = []
    for index, case in enumerate(cases):
        body.extend([
            f'  let operation{index} : Operation := {lean_operation(case["operation"])}',
            f'  let scope{index} : Scope := {lean_scope(case)}',
            f'  let request{index} : Request := {{ requestId := d, resources := {{}}, '
            f'    requiredModules := requiredModules operation{index}, '
            f'    query := {{ scope := scope{index}.id, operation := operation{index} }}, scope := scope{index} }}',
            f'  IO.println ({lean_string(case["id"] + chr(9))} ++ runRequest request{index})',
        ])
    body.extend([
        '  let candidateLimit : Request := { request0 with resources := ({ candidates := 2 } : Resources) }',
        '  IO.println ("__candidate_limit\\t" ++ runRequest candidateLimit)',
        '  let fieldLimit : Request := { request2 with resources := ({ fieldReads := 3 } : Resources) }',
        '  IO.println ("__field_read_limit\\t" ++ runRequest fieldLimit)',
        '  let stepLimit : Request := { request0 with resources := ({ steps := 1 } : Resources) }',
        '  IO.println ("__step_limit\\t" ++ runRequest stepLimit)',
        '  let valueLimit : Request := { request0 with resources := ({ valueNodes := 6 } : Resources) }',
        '  IO.println ("__value_limit\\t" ++ runRequest valueLimit)',
        '  let unsupported : Request := { request3 with requiredModules := ["query/v1"] }',
        '  IO.println ("__unsupported\\t" ++ runRequest unsupported)',
        '  let invalidOperation : Operation := .project (.column "not_granted")',
        '  let invalidColumnModules : Request := { request0 with requiredModules := requiredModules invalidOperation }',
        '  let invalidColumn : Request := { invalidColumnModules with',
        '    query := { scope := scope0.id, operation := invalidOperation } }',
        '  IO.println ("__invalid_column\\t" ++ runRequest invalidColumn)',
    ])
    return '\n'.join([
        'import Kpopper.QueryProtocol',
        'open Kpopper',
        'open Kpopper.Query',
        'def d : String := "' + 'a' * 64 + '"',
        'def witness : ScopeWitness := {',
        '  scopeId := "scope.items", definitionDigest := d,',
        '  membershipDigest := d, projectedInputsDigest := d }',
        'def main : IO Unit := do',
        *body,
        '',
    ])


class QueryCorpusTests(unittest.TestCase):
    def test_public_corpus_matches_independent_oracle(self):
        self.assertEqual(CORPUS['schema_version'], 1)
        self.assertEqual({case['operation']['op'] for case in CORPUS['cases']},
                         {'filter', 'project', 'select', 'count', 'sum', 'all', 'any'})
        for case in CORPUS['cases']:
            with self.subTest(case=case['id']):
                actual = query_oracle(case)
                expected = case['expected']
                self.assertEqual(actual['status'], expected['status'])
                self.assertEqual(actual['counts'], expected['counts'])
                self.assertEqual(actual['diagnostic_codes'], expected['diagnostic_codes'])
                expected_result = None if expected['result'] is None else typed_value(expected['result'])
                self.assertEqual(actual['result'], expected_result)

    def test_random_simple_truth_tables_have_exact_counts(self):
        rng = random.Random(5122026)
        for _ in range(500):
            states = [rng.choice((True, False, None)) for _ in range(rng.randrange(0, 25))]
            members = []
            for index, state in enumerate(states):
                field = ({'status': 'missing'} if state is None else
                         {'status': 'known', 'value': {'type': 'boolean', 'value': state}})
                members.append({'id': f'{index:02d}', 'fields': {'enabled': field}})
            for op in ('filter', 'count', 'all', 'any'):
                case = {'fields': ['enabled'], 'members': members,
                        'operation': {'op': op, 'where': {'column': 'enabled'}}}
                result = query_oracle(case)
                self.assertEqual(result['counts'], [len(states), states.count(True),
                                                     states.count(None), 0, 0])
                dominated = (op == 'all' and False in states) or (op == 'any' and True in states)
                self.assertEqual(result['status'], 'ok' if dominated or None not in states else 'unknown')

    def test_new_kernel_sources_have_no_admissions(self):
        for name in ('QueryTypes.lean', 'Query.lean', 'QueryProtocol.lean', 'QueryWire.lean'):
            source = (LEAN_ROOT / 'Kpopper' / name).read_text(encoding='utf-8')
            self.assertIsNone(re.search(r'\b(?:axiom|sorry|admit)\b', source), name)


@unittest.skipUnless(LEAN, 'set KPOPPER_QUERY_LEAN to Lean 4.33.1 for direct kernel conformance')
class LeanQueryKernelTests(unittest.TestCase):
    def test_corpus_matches_compiled_lean_kernel(self):
        compiler = Path(LEAN).resolve()
        self.assertIn('4.33.1', subprocess.check_output([compiler, '--version'], text=True))
        with tempfile.TemporaryDirectory(prefix='kpopper-query-lean-') as directory:
            work = Path(directory)
            (work / 'Kpopper').mkdir()
            for name in ('Kernel.lean', 'Protocol.lean', 'CompositionTypes.lean',
                         'Composition.lean', 'CompositionProtocol.lean', 'Main.lean'):
                shutil.copyfile(LEAN_ROOT / name, work / name)
            for name in ('QueryTypes.lean', 'Query.lean', 'QueryProtocol.lean', 'QueryWire.lean'):
                shutil.copyfile(LEAN_ROOT / 'Kpopper' / name, work / 'Kpopper' / name)
            env = dict(os.environ, LEAN_PATH=str(work))
            modules = [
                ('Kernel.lean', 'Kernel.olean'),
                ('Protocol.lean', 'Protocol.olean'),
                ('CompositionTypes.lean', 'CompositionTypes.olean'),
                ('Composition.lean', 'Composition.olean'),
                ('CompositionProtocol.lean', 'CompositionProtocol.olean'),
                ('Kpopper/QueryTypes.lean', 'Kpopper/QueryTypes.olean'),
                ('Kpopper/Query.lean', 'Kpopper/Query.olean'),
                ('Kpopper/QueryProtocol.lean', 'Kpopper/QueryProtocol.olean'),
                ('Kpopper/QueryWire.lean', 'Kpopper/QueryWire.olean'),
                ('Main.lean', 'Main.olean'),
            ]
            for source, output in modules:
                subprocess.run([compiler, '-o', output, source], cwd=work, env=env,
                               check=True, capture_output=True, text=True)
            (work / 'QueryAudit.lean').write_text(
                'import Kpopper.QueryProtocol\n'
                '#print axioms Kpopper.Query.prepare\n'
                '#print axioms Kpopper.Query.execute\n'
                '#print axioms Kpopper.Query.responseFor\n', encoding='utf-8')
            audit = subprocess.run([compiler, 'QueryAudit.lean'], cwd=work, env=env,
                                   check=True, capture_output=True, text=True).stdout
            declarations = {'Kpopper.Query.prepare', 'Kpopper.Query.execute',
                            'Kpopper.Query.responseFor'}
            seen = set()
            for name, values in re.findall(r"'([^']+)' depends on axioms: \[([^]]*)\]", audit):
                if name in declarations:
                    seen.add(name)
                    axioms = {value.strip() for value in values.split(',') if value.strip()}
                    self.assertLessEqual(axioms, {'propext', 'Classical.choice', 'Quot.sound'})
            self.assertEqual(seen, declarations, audit)
            (work / 'Harness.lean').write_text(harness_source(CORPUS['cases']), encoding='utf-8')
            run = subprocess.run([compiler, '--run', 'Harness.lean'], cwd=work, env=env,
                                 capture_output=True, text=True)
            self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
            first = run.stdout
            second = subprocess.run([compiler, '--run', 'Harness.lean'], cwd=work, env=env,
                                    check=True, capture_output=True, text=True).stdout
            self.assertEqual(first, second, 'kernel output/cost must be deterministic')
            lines = {line.split('\t', 1)[0]: line.split('\t')[1:]
                     for line in first.splitlines()}
            boundaries = {
                '__candidate_limit': ('limit', 'candidate_limit'),
                '__field_read_limit': ('limit', 'field_read_limit'),
                '__step_limit': ('limit', 'step_limit'),
                '__value_limit': ('limit', 'value_limit'),
                '__unsupported': ('unsupported_capability', 'unsupported_capability'),
                '__invalid_column': ('error', 'invalid_expression'),
            }
            self.assertEqual(set(lines), {case['id'] for case in CORPUS['cases']} | set(boundaries))
            for case in CORPUS['cases']:
                with self.subTest(case=case['id']):
                    response = lines[case['id']]
                    oracle = query_oracle(case)
                    self.assertEqual(response[0], oracle['status'])
                    self.assertEqual([int(value) for value in response[1].split(',')], oracle['counts'])
                    expected_value = ('-' if oracle['result'] is None else
                                      render_value(result_record(oracle['counts'], oracle['result'])))
                    self.assertEqual(response[2], expected_value)
                    codes = [] if not response[3] else [item.split('@', 1)[0]
                                                        for item in response[3].split(';')]
                    self.assertEqual(codes, oracle['diagnostic_codes'])
                    self.assertTrue(all(int(value) >= 0 for value in response[4:]))
            for name, (status, code) in boundaries.items():
                with self.subTest(boundary=name):
                    response = lines[name]
                    self.assertEqual(response[0], status)
                    self.assertEqual(response[1:3], ['-', '-'])
                    self.assertEqual([item.split('@', 1)[0] for item in response[3].split(';')], [code])

            # Exercise the actual mixed-stream Main boundary with canonical
            # Python KP4 bytes, including non-ASCII and expression diagnostics.
            from scripts.reasoning import query as Q
            from tests.test_reasoning_query_ir import authored, prepared
            wire_cases = [
                prepared(authored('project', value={'text': '\b\f\té'})),
                prepared(authored('project', value={'record': {
                    'é': {'text': 'last'}, 'a': {'text': 'first'},
                }})),
                prepared(authored('project', value={'column': 'when'})),
            ]
            for item in wire_cases:
                process = subprocess.run([compiler, '--run', 'Main.lean'], cwd=work, env=env,
                                         input=Q.encode_request(item['request']), capture_output=True)
                self.assertEqual(process.returncode, 0, process.stderr.decode('utf-8', 'replace'))
                response = Q.decode_frame(process.stdout, 'KR4')
                self.assertEqual(Q.validate_response(response, item), response)
            self.assertEqual(Q.decode_frame(process.stdout, 'KR4')['status'], 'unknown')

            scalar = ('KP2\t7\t8\t9\t0\t0\tn\t1\n').encode()
            malformed_body = b'{"request_id": null}'
            malformed = b'KP4 ' + str(len(malformed_body)).encode() + b'\n' + malformed_body + b'\n'
            recovered = subprocess.run(
                [compiler, '--run', 'Main.lean'], cwd=work, env=env,
                input=malformed + scalar, capture_output=True, check=True).stdout
            header_end = recovered.index(b'\n')
            body_size = int(recovered[4:header_end])
            frame_end = header_end + 1 + body_size + 1
            wire_error = Q.decode_frame(recovered[:frame_end], 'KR4')
            self.assertEqual((wire_error['status'], wire_error['request_id']), ('error', None))
            self.assertTrue(recovered[frame_end:].startswith(b'KR2\t'))


if __name__ == '__main__':
    unittest.main()
