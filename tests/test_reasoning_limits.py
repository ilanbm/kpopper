"""Operational refusal preserves complete evidence and deterministic identity."""
import json
import subprocess
import sys
import unittest
from unittest.mock import patch

from scripts.reasoning.assessment import assess
from scripts.reasoning.evaluate import Evaluator
from scripts.reasoning.snapshot import Snapshot
from test_reasoning_contract import record


class NativeStub:
    implementation = {'protocol': 'fixture'}

    def __init__(self):
        self.calls = 0

    def request_many(self, requests):
        self.calls += 1
        return [{'status': 'ok', 'value': {'type': 'number', 'numerator': '30', 'denominator': '1'},
                 'diagnostics': [], 'executed_reads': sorted(request['nodes']),
                 'potential_reads': request['declared'], 'steps': 1, 'preflight_steps': 1,
                 'node_evaluations': {nid: 1 for nid in request['nodes']}}
                for request in requests]


class OperationalBoundaryTests(unittest.TestCase):
    def test_timeout_has_distinct_diagnostic(self):
        runtime = NativeStub()
        with patch.object(runtime, 'request_many', side_effect=subprocess.TimeoutExpired('fixture', 30)):
            result = Evaluator(Snapshot.from_data(record()), runtime=runtime).evaluate({'num': '1'}, declared=[])
        self.assertEqual(result['status'], 'operational_error')
        self.assertEqual(result['diagnostics'][0]['code'], 'runtime_timeout')
        self.assertIsNone(result['value'])
        self.assertEqual(result['operational_limits']['timeout_seconds'], 30)

    def test_actual_read_fingerprints_match_potential_witnesses(self):
        snapshot = Snapshot.from_data(record())
        result = Evaluator(snapshot, runtime=NativeStub()).evaluate({'ref': 'm.total'}, declared=['m.total'])
        self.assertTrue(result['executed_reads'])
        potential = {w['id']: w for w in result['potential_dependencies']}
        for read in result['executed_reads']:
            self.assertEqual(read, potential[read['id']])
            self.assertEqual(read['fingerprint'], snapshot._input_basis().summary(read['id'])['fingerprint'])

    def test_oversized_iterator_batch_refuses_before_runtime(self):
        from scripts.reasoning.contract import OperationalLimit
        runtime = NativeStub()
        consumed = []
        def requests():
            for n in range(100):
                consumed.append(n)
                yield {'num': '1'}, []
        with self.assertRaisesRegex(OperationalLimit, 'batch_request_limit'):
            Evaluator(Snapshot.from_data(record()), runtime=runtime,
                      operational_limits={'batch_requests': 2}).evaluate_many(requests())
        self.assertEqual(len(consumed), 3)
        self.assertEqual(runtime.calls, 0)

    def test_batch_budget_counts_repeated_closures_without_clipping(self):
        from scripts.reasoning.contract import OperationalLimit
        runtime = NativeStub()
        snapshot = Snapshot.from_data(record())
        with self.assertRaisesRegex(OperationalLimit, 'output_limit'):
            Evaluator(snapshot, runtime=runtime, operational_limits={'output_bytes': 200}).evaluate_many(
                [({'ref': 'm.total'}, ['m.total'])] * 50)
        self.assertEqual(runtime.calls, 0)

    def test_shared_computation_repeated_in_report_is_budgeted(self):
        from scripts.reasoning.contract import OperationalLimit
        doc = record()
        template = doc['decisions']['d.order']
        doc['decisions'] = {'d.' + str(i): template for i in range(50)}
        with self.assertRaisesRegex(OperationalLimit, 'output_limit'):
            assess(Snapshot.from_data(doc), runtime=NativeStub(), operational_limits={'output_bytes': 10000})

    def test_operational_limits_do_not_change_arithmetic_identity(self):
        snapshot = Snapshot.from_data(record())
        a = Evaluator(snapshot, runtime=NativeStub()).evaluate({'num': '1'}, declared=[])
        b = Evaluator(snapshot, runtime=NativeStub(), operational_limits={'timeout_seconds': 1}).evaluate({'num': '1'}, declared=[])
        self.assertEqual(a['computation_id'], b['computation_id'])
        self.assertNotEqual(a['operational_limits'], b['operational_limits'])

    def test_success_schema_requires_actual_read_fingerprint(self):
        try:
            import jsonschema
        except ImportError:
            import os
            if os.environ.get('KPOPPER_REQUIRE_CORE_TESTS') == '1':
                raise
            self.skipTest('jsonschema test dependency unavailable')
        from pathlib import Path
        schema = json.loads((Path(__file__).resolve().parents[1] /
                             'scripts/reasoning/assessment.schema.json').read_text())
        report = assess(Snapshot.from_data(record()), ['m.total'], runtime=NativeStub())
        jsonschema.validate(report, schema)
        del report['nodes']['m.total']['computation']['executed_reads'][0]['fingerprint']
        with self.assertRaises(jsonschema.ValidationError):
            jsonschema.validate(report, schema)

    def test_budget_counts_shared_and_unicode_json_without_copying_report(self):
        from scripts.reasoning.contract import OutputBudget, OperationalLimit
        shared = {'message': 'שלום\\n😀' * 5000}
        value = [shared, shared]
        size = len(json.dumps(value, ensure_ascii=True, separators=(',', ':')))
        OutputBudget(size).add(value)
        with self.assertRaises(OperationalLimit):
            OutputBudget(size - 1).add(value)

    def test_cli_refuses_final_oversized_projection_without_partial_output(self):
        import contextlib
        import io
        from scripts import assessment
        report = assess(Snapshot.from_data(record()), ['m.total'], runtime=NativeStub())
        report['operational_limits']['output_bytes'] = 20
        output = io.StringIO()
        with patch.object(assessment, 'load', return_value=report), \
                contextlib.redirect_stdout(output), contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit) as error:
                assessment.main(['m.total', '--profile', 'core/v1'])
        self.assertEqual(error.exception.code, 2)
        self.assertEqual(output.getvalue(), '')

    def test_real_process_output_and_stderr_are_bounded(self):
        from scripts.reasoning.runtime import _run_bounded
        from scripts.reasoning.contract import OperationalLimit
        for fd in (1, 2):
            with self.subTest(fd=fd), self.assertRaisesRegex(OperationalLimit, 'output_limit'):
                _run_bounded([sys.executable, '-c', 'import os; os.write(%s, b"x" * 100000)' % fd],
                             b'', timeout=5, output_bytes=128)

    def test_real_process_timeout_remains_operational(self):
        from scripts.reasoning.runtime import _run_bounded
        with self.assertRaises(subprocess.TimeoutExpired):
            _run_bounded([sys.executable, '-c', 'import time; time.sleep(5)'],
                         b'', timeout=0.1, output_bytes=128)

    def test_timeout_covers_inherited_pipes_after_parent_exit(self):
        import time
        from scripts.reasoning.runtime import _run_bounded
        child = ('import subprocess,sys; subprocess.Popen([sys.executable,"-c",'
                 '"import time; time.sleep(2)"])')
        started = time.monotonic()
        with self.assertRaises(subprocess.TimeoutExpired):
            _run_bounded([sys.executable, '-c', child], b'', timeout=0.15, output_bytes=128)
        self.assertLess(time.monotonic() - started, 1.5)

    def test_real_process_drains_input_and_captures_output(self):
        from scripts.reasoning.runtime import _run_bounded
        output = _run_bounded([sys.executable, '-c', 'import sys; sys.stdout.buffer.write(sys.stdin.buffer.read())'],
                              b'fixture\n', timeout=5, output_bytes=128)
        self.assertEqual(output, b'fixture\n')
