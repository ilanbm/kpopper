"""Exercise the compiled Lean parser, never a Python substitute for its evaluator.

Grammar: one whitespace-separated reference, comparison operator and RHS. The RHS
is a complete JSON scalar literal, a single-quoted string, or one declared bare
reference. Single quotes decode only escaped apostrophe and escaped backslash;
JSON strings retain JSON escapes. Raw U+0000..U+001F, malformed/trailing syntax,
and unsupported JSON values stay unknown. These tests do not build on reads:
run `kpopper session setup` explicitly for this checkout before integration tests.
"""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from scripts.session.core import Core

MISSING = object()
VALUE = 'kbd.gate.prod'
OTHER = 'reference.value'
JUDGMENT = 'd.literal_policy'


class PredicateLiteralContract(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        try:
            cls.core = Core()
        except ValueError:
            if os.environ.get('KPOPPER_REQUIRE_CORE_TESTS') == '1':
                raise
            raise unittest.SkipTest('run session setup for this Lean source before integration tests')

    def assess(self, expression, current=MISSING, seen='old', other='unset', dependencies=None,
               extra_nodes=None):
        body = {} if current is MISSING else {'v': current}
        nodes = {
            VALUE: {'kind': 'known', 'states': [], 'body': body},
            OTHER: {'kind': 'known', 'states': [], 'body': {'v': other}},
            JUDGMENT: {'kind': 'judgment', 'states': [], 'body': {
                'verdict': 'A recorded policy, not a claim of runtime verification.',
                'rests_on': [VALUE] if dependencies is None else dependencies,
                'seen': {VALUE: seen, OTHER: other}, 'wrong_if': expression,
                'reopened_by': 'A person supplies contrary evidence.',
            }},
        }
        nodes.update(extra_nodes or {})
        result = self.core.assess({'nodes': nodes}, JUDGMENT, [
            {'kind': 'falsifier_holds', 'id': JUDGMENT, 'expected': True},
            {'kind': 'falsifier_holds', 'id': JUDGMENT, 'expected': False},
        ])
        self.assertEqual(result['bundle']['falsifier']['expression'], expression)
        return result

    def assert_result(self, expression, current, expected, **kwargs):
        result = self.assess(expression, current, **kwargs)
        self.assertIs(result['bundle']['falsifier']['holds_on_current_values'], expected)
        self.assertEqual([row['accepted'] for row in result['assertion_checks']],
                         [expected is True, expected is False])
        return result

    def test_native_single_quoted_gate_uses_current_not_seen(self):
        expression = VALUE + " != 'unset'"
        result = self.assert_result(expression, 'enabled', True, seen='unset')
        premise = result['bundle']['premises'][0]
        self.assertEqual((premise['current_recorded_value'], premise['value_at_review']),
                         ('enabled', 'unset'))
        result = self.assert_result(expression, 'unset', False, seen='enabled')
        self.assertTrue(result['bundle']['premise_changed'])

    def test_native_record_preserves_the_expression_received_by_lean(self):
        from scripts.session.store import native_record
        import yaml

        for expression in [VALUE + " != 'unset'", VALUE + ' != "pending release"']:
            with self.subTest(expression=expression), tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / 'PROVENANCE.yaml'
                path.write_text(yaml.safe_dump({
                    'schema': {'deps': 'rests_on', 'snapshot': 'seen', 'predicate': 'wrong_if'},
                    'known': {VALUE: {'v': 'configured'}},
                    'judgments': {JUDGMENT: {'verdict': 'A captured policy.', 'rests_on': [VALUE],
                                             'seen': {VALUE: 'unset'}, 'wrong_if': expression}},
                }))
                record = native_record(path, ROOT / 'scripts' / 'provenance.py')
                result = self.core.assess(record, JUDGMENT)['bundle']
                self.assertEqual(result['falsifier']['expression'], expression)
                self.assertIs(result['falsifier']['holds_on_current_values'], True)

    def test_both_quote_styles_preserve_spaces_empty_and_operator_text(self):
        for value in ['', 'awaiting review', '  keep  spaces  ', 'a > b and c != d', 'שלום 世界']:
            for literal in [json.dumps(value, ensure_ascii=False), "'" + value + "'"]:
                with self.subTest(literal=literal):
                    self.assert_result(VALUE + ' == ' + literal, value, True)
                    self.assert_result(VALUE + ' != ' + literal, value, False)

    def test_single_quote_escapes_are_explicit_and_bounded(self):
        cases = [
            (r"'can\'t'", "can't"),
            (r"'a\\b'", 'a\\b'),
            (r"'\\\''", "\\'"),
            ("'double \" quote'", 'double " quote'),
        ]
        for literal, value in cases:
            with self.subTest(literal=literal):
                self.assert_result(VALUE + ' == ' + literal, value, True)
        for literal in [r"'\n'", r"'\t'", r"'\r'", r"'\u0041'", r"'\x41'", r"'\/'"]:
            with self.subTest(unsupported_escape=literal):
                self.assert_result(VALUE + ' != ' + literal, 'different', None)

    def test_json_string_escapes_keep_json_semantics(self):
        for value in ['line\nbreak', 'tab\there', 'quote " and slash \\', '\x00', '😀']:
            with self.subTest(value=value):
                self.assert_result(VALUE + ' == ' + json.dumps(value), value, True)
        self.assert_result(VALUE + r' == "a\/b"', 'a/b', True)

    def test_malformed_quotes_controls_and_trailing_expressions_stay_unknown(self):
        literals = ["'", '"', "'unterminated", '"unterminated', "'mixed\"", '"mixed\'',
                    "'value' trailing", '"value" trailing', "'value' 'other'", "'value'and",
                    "'value' or 1 == 1", '"value" # comment', "'ends\\'", 'plain words',
                    r'"\x41"', 'true false', '1 + 2']
        for control in ['\n', '\r', '\t', '\x00']:
            literals += ["'a" + control + "b'", '"a' + control + 'b"']
        for literal in literals:
            with self.subTest(literal=repr(literal)):
                self.assert_result(VALUE + ' != ' + literal, 'different', None)
        for expression in [VALUE + " === 'unset'", VALUE + "=='unset'", VALUE + ' ==']:
            with self.subTest(expression=expression):
                self.assert_result(expression, 'unset', None)

    def test_json_types_and_unsupported_values_never_coerce(self):
        for literal, current, expected in [
            ('true', True, True), ('false', False, True), ('0', 0, True),
            ('-1.25e2', -125, True), ('"0"', '0', True), ("'true'", 'true', True),
            ('true', 1, None), ('false', 0, None), ('0', False, None),
            ('"0"', 0, None), ("'false'", False, None), ('null', None, None),
            ('null', 'null', None), ('[1, 2]', [1, 2], None), ('{"a": 1}', {'a': 1}, None),
        ]:
            with self.subTest(literal=literal, current=current):
                self.assert_result(VALUE + ' == ' + literal, current, expected)
        self.assert_result(VALUE + ' < true', False, None)
        self.assert_result(VALUE + " > 'a'", 'b', None)
        for operator, expected in [('<', True), ('<=', True), ('>', False), ('>=', False),
                                   ('==', False), ('!=', True)]:
            self.assert_result(VALUE + ' ' + operator + ' 2', 1, expected)

    def test_missing_current_does_not_fall_back_to_review_value(self):
        for current in [MISSING, None]:
            for literal in ["'unset'", '"unset"']:
                with self.subTest(current=current, literal=literal):
                    result = self.assert_result(VALUE + ' != ' + literal, current, None, seen='unset')
                    premise = result['bundle']['premises'][0]
                    self.assertIsNone(premise['current_recorded_value'])
                    self.assertEqual(premise['value_at_review'], 'unset')

    def test_references_must_be_declared_and_cannot_rescue_malformed_literals(self):
        self.assert_result(OTHER + " == 'unset'", 'unset', None)
        self.assert_result(VALUE + ' == ' + OTHER, 'unset', None)
        self.assert_result(VALUE + ' == ' + OTHER, 'unset', True, dependencies=[VALUE, OTHER])
        self.assert_result(VALUE + ' == ' + OTHER, 'unset', None, other=None,
                           dependencies=[VALUE, OTHER])
        for token in ["'unterminated", '"unterminated', "bad'quote", 'bad\x00id', 'null', '[1, 2]']:
            with self.subTest(token=token):
                node = {'kind': 'known', 'states': [], 'body': {'v': 'unset'}}
                self.assert_result(VALUE + ' == ' + token, 'unset', None,
                                   dependencies=[VALUE, token], extra_nodes={token: node})

    def test_insignificant_ascii_whitespace_does_not_rewrite_reporting(self):
        expression = " \t" + VALUE + "\t ==  '  still  pending  ' \r\n"
        self.assert_result(expression, '  still  pending  ', True)

    def test_unicode_core_pipe_does_not_depend_on_the_host_text_codec(self):
        # Construct Unicode inside an ASCII-only child program. Disable Python's
        # locale coercion/UTF-8 mode only in this child: the real Core pipe, not a
        # test-wide UTF-8 switch, must deliver the exact value to/from Lean.
        script = '''
import json, locale
from scripts.session.core import Core
value = ''.join(chr(n) for n in [0x05e9, 0x05dc, 0x05d5, 0x05dd, 0x20, 0x4e16, 0x754c, 0x1f600])
expression = 'value.text == ' + json.dumps(value, ensure_ascii=False)
record = {'nodes': {
    'value.text': {'kind': 'known', 'states': [], 'body': {'v': value}},
    'claim.same': {'kind': 'judgment', 'states': [], 'body': {
        'verdict': 'The stored text matches.', 'rests_on': ['value.text'],
        'seen': {'value.text': value}, 'wrong_if': expression}}
}}
bundle = Core().assess(record, 'claim.same')['bundle']
print(json.dumps({'host_codec': locale.getpreferredencoding(False), 'bundle': bundle}, ensure_ascii=True))
'''
        environment = dict(os.environ, PYTHONUTF8='0', PYTHONCOERCECLOCALE='0', LC_ALL='C')
        result = subprocess.run([sys.executable, '-c', script], cwd=ROOT, env=environment,
                                capture_output=True, text=True, encoding='utf-8', timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr)
        response = json.loads(result.stdout)
        value = 'שלום 世界😀'
        self.assertEqual(response['bundle']['premises'][0]['current_recorded_value'], value)
        self.assertIs(response['bundle']['falsifier']['holds_on_current_values'], True)


if __name__ == '__main__':
    unittest.main()
