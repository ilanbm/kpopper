"""Readable formula storage retains exact calculations and graph semantics."""
import contextlib
import copy
import io
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import yaml
from scripts import ingestion as I
from scripts.session.core import Core

P, E = I.P, I.P.E


class ReadableSyntax(unittest.TestCase):
    def test_readable_storage_lowers_without_losing_exact_tokens_or_names(self):
        source = {'expr': 'מחיר.יחידה * 0.1 + ref("item-with-hyphen")'}
        tree = E.lower(source)
        self.assertEqual(tree, {'op': 'add', 'args': [
            {'op': 'mul', 'args': [{'ref': 'מחיר.יחידה'}, {'num': '0.1'}]},
            {'ref': 'item-with-hyphen'}]})
        self.assertEqual(E.refs(source), ['item-with-hyphen', 'מחיר.יחידה'])
        self.assertEqual(source, {'expr': 'מחיר.יחידה * 0.1 + ref("item-with-hyphen")'})

    def test_literals_do_not_create_graph_links(self):
        predicate = {'expr': 'order.state == "order.price"'}
        self.assertEqual(E.refs(predicate), ['order.state'])
        self.assertEqual(E.lower(predicate, predicate=True)['args'][1], {'text': 'order.price'})

    def test_cached_compilation_does_not_share_mutable_trees(self):
        E._parse_readable.cache_clear()
        expr = {'expr': 'order.price * order.quantity'}
        first = E.lower(expr)
        first['args'][0]['ref'] = 'corrupted'
        self.assertEqual(E.lower(expr)['args'][0], {'ref': 'order.price'})
        self.assertEqual(E._parse_readable.cache_info().misses, 1)
        self.assertGreater(E._parse_readable.cache_info().hits, 0)

    def test_invalid_expressions_are_not_executed_or_silently_accepted(self):
        for expression in [{'expr': '1', 'op': 'add'}, {'expr': 3}, {'expr': 'x.value ** 2'},
                           {'expr': '__import__("os").system("echo bad")'},
                           {'expr': 'ref("x", "y")'}, {'expr': 'x.value > 1 and x.value < 10'}]:
            with self.subTest(expression=expression), self.assertRaises(ValueError):
                E.lower(expression)

    def test_identity_rewrite_changes_only_references(self):
        pred = {'expr': 'ref("x-old") == "x-old"'}
        changed = E.rename(pred, 'x-old', 'x.new', predicate=True)
        self.assertEqual(E.lower(changed, predicate=True), {
            'op': 'eq', 'args': [{'ref': 'x.new'}, {'text': 'x-old'}]})

    def test_wire_lowering_does_not_reinterpret_evidence_or_expected_events(self):
        expr = {'expr': 'order.price * 2'}
        value = {'expr': 'this is source data'}
        payload = {'record': {'nodes': {'order.double': {'body': {'rule': expr, 'description': value}},
            'c.budget': {'assessment_fields': {'predicate': 'fails', 'snapshot': 'checked'},
                'body': {'fails': {'expr': 'order.price > 50'}, 'checked': {
                    'order.double': {'computed': {'rule': expr, 'value': value}}}},
                'assessment_body': {'wrong_if': {'expr': 'order.price > 50'}}}}},
            'view': {'events': [expr]}, 'assertions': [{'expected': value}]}
        before = copy.deepcopy(payload)
        lowered = E.wire_payload(payload)
        self.assertEqual(lowered['record']['nodes']['order.double']['body']['rule'], E.lower(expr))
        body = lowered['record']['nodes']['c.budget']['body']
        self.assertEqual(body['fails']['op'], 'gt')
        self.assertEqual(body['checked']['order.double']['computed']['rule'], E.lower(expr))
        self.assertEqual(body['checked']['order.double']['computed']['value'], value)
        self.assertEqual(lowered['view'], payload['view'])
        self.assertEqual(lowered['assertions'], payload['assertions'])
        self.assertEqual(payload, before)

    def test_readable_round_trip_covers_reserved_ids_and_quoted_literals(self):
        for value in [{'ref': 'true'}, {'ref': 'class'}, {'ref': 'None'}, {'ref': 'item.True'},
                      {'ref': 'a-b'}, {'ref': 'שם עם רווח'}, {'text': 'a > b "quoted"'},
                      {'op': 'div', 'args': [{'num': '1'}, {'num': '3'}]}]:
            self.assertEqual(E.lower(E.readable(value)), value)

    def test_malformed_rule_comparison_is_local_and_nonthrowing(self):
        self.assertFalse(E.same({'expr': 'order.price * 2'}, {'op': [], 'args': []}))

    def test_identity_rewrite_preserves_block_scalar_boundaries(self):
        from scripts import sameness as S
        for style in ('|', '>', '|-', '>+'):
            source = 'known:\n  order.double:\n    rule:\n      expr: ' + style + '\n        order.old * 2\n    name: Double\n'
            rewritten, _ = S._rewrite_text(source, 'order.old', 'order.price')
            node = yaml.safe_load(rewritten)['known']['order.double']
            self.assertEqual(node['name'], 'Double')
            self.assertEqual(E.refs(node['rule']), ['order.price'])


class ReadableRuntime(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        try:
            cls.core = Core()
        except ValueError:
            if os.environ.get('KPOPPER_REQUIRE_CORE_TESTS') == '1':
                raise
            raise unittest.SkipTest('build the core for readable expression integration')

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.path = self.root / 'GROUNDING.yaml'
        self.doc = {'sources': {'s.report': {'file': 'report.md'}}, 'known': {
            'order.price': {'v': 20, 'from': 's.report'}, 'order.quantity': {'v': 5, 'from': 's.report'},
            'order.total': {'rule': {'op': 'mul', 'args': [{'ref': 'order.price'}, {'ref': 'order.quantity'}]}}},
            'judgments': {'c.budget': {'rests_on': ['order.total'], 'verdict': 'Fits',
                'wrong_if': {'op': 'gt', 'args': [{'ref': 'order.total'}, {'num': '150'}]},
                'seen': {'order.total': {'computed': {'value': 100, 'rule': {
                    'op': 'mul', 'args': [{'ref': 'order.price'}, {'ref': 'order.quantity'}]}}}}}}}
        self.save()
        (self.root / 'report.md').write_text('Price 20; quantity 5.')

    def save(self):
        self.path.write_text(yaml.safe_dump(self.doc, sort_keys=False, allow_unicode=True))

    def world(self):
        doc = P.load([str(self.path)]); ids, jud, fields = P.infer(doc)
        return doc, ids, jud, fields, P.with_builtins(doc, ids, jud, fields)

    def test_normal_authoring_stores_readable_source_and_captures_it(self):
        with contextlib.redirect_stdout(io.StringIO()):
            P.apply([str(self.path)], {'kind': 'add', 'id': 'order.discounted', 'body': {'rule': 'order.total - 0.1'}})
            P.apply([str(self.path)], {'kind': 'add', 'id': 'c.discount', 'body': {
                'rests_on': ['order.discounted'], 'verdict': 'Fits', 'wrong_if': 'order.discounted > 150'}})
        _, _, _, _, raw = self.world()
        self.assertEqual(raw['order.discounted']['rule'], {'expr': 'order.total - 0.1'})
        self.assertEqual(raw['c.discount']['wrong_if'], {'expr': 'order.discounted > 150'})
        self.assertEqual(raw['c.discount']['seen']['order.discounted']['computed'], {
            'value': {'rational': ['999', '10']}, 'rule': {'expr': 'order.total - 0.1'}})

    def test_format_changes_preserve_history_but_formula_changes_reopen_review(self):
        original = copy.deepcopy(self.doc['judgments']['c.budget']['seen'])
        self.doc['known']['order.total']['rule'] = {'expr': '(order.price * order.quantity)'}
        self.save()
        _, ids, jud, fields, raw = self.world()
        self.assertEqual(P._state('c.budget', jud['c.budget'], raw, ids, fields)[0], 'HOLDS')
        self.assertEqual(raw['c.budget']['seen'], original)
        self.doc['known']['order.total']['rule'] = {'expr': 'order.price * order.quantity + 0'}
        self.save()
        _, ids, jud, fields, raw = self.world()
        self.assertEqual(P.value_of(raw, ids, 'order.total'), 100)
        self.assertEqual(P._state('c.budget', jud['c.budget'], raw, ids, fields)[0], 'MOVED')
        self.assertEqual(raw['c.budget']['seen'], original)

    def test_direct_core_protocol_supports_readable_calculations_and_predicates(self):
        record = {'nodes': {key: {'body': body, 'states': []} for group in self.doc.values() for key, body in group.items()}}
        record['nodes']['order.total']['body'] = {'rule': {'expr': 'order.price * order.quantity'}}
        record['nodes']['c.budget']['body']['wrong_if'] = {'expr': 'order.total > 150'}
        before = copy.deepcopy(record)
        result = self.core.request({'operation': 'compute', 'record': record,
            'predicate': {'expr': 'order.total > 150'}, 'dependencies': ['order.total']})
        self.assertEqual(result['values']['order.total']['value'], 100)
        self.assertIs(result['predicate']['holds_on_current_values'], False)
        self.assertEqual(record, before)

    def test_invalid_rule_does_not_poison_an_unrelated_calculation(self):
        raw = {'order.bad': {'rule': {'expr': 'run_something()'}},
               'order.exact': {'rule': {'expr': '0.1 + 0.2'}}}
        result = E.compute(raw, set(raw))
        self.assertEqual(result['values']['order.exact']['value'], {'rational': ['3', '10']})
        self.assertIsNone(result['values']['order.bad']['value'])

    def test_checked_session_keeps_readable_bodies_and_assesses_custom_fields(self):
        from scripts.session.store import native_record
        from scripts.session.model import RecordMap
        self.doc['known']['order.total']['rule'] = {'expr': 'order.price * order.quantity'}
        judgment = self.doc['judgments']['c.budget']
        judgment['fails'] = {'expr': 'order.total > 150'}
        del judgment['wrong_if']
        self.doc['schema'] = {'predicate': 'fails', 'deps': 'rests_on', 'snapshot': 'seen'}
        self.save()
        record = native_record(self.path, Path(P.__file__))
        model = RecordMap(record)
        before = copy.deepcopy(model.data)
        bundle = self.core.assess(model.data, 'c.budget')['bundle']
        self.assertIs(bundle['falsifier']['holds_on_current_values'], False)
        self.assertEqual(model.data, before)
        self.assertEqual(model.nodes['order.total']['body']['rule'], {'expr': 'order.price * order.quantity'})
        self.assertIn({'from': 'order.total', 'rel': 'rule_reads', 'to': 'order.price'}, model.edges)

    def test_followup_change_compares_formula_structure_and_historical_value(self):
        from scripts import followup_triggers as T
        original = self.doc['judgments']['c.budget']['seen']['order.total']
        current = copy.deepcopy(original)
        current['computed']['rule'] = {'expr': ' (order.price * order.quantity) '}
        self.assertTrue(T._equal(original, current))
        current['computed']['rule'] = {'expr': 'order.price * order.quantity + 0'}
        self.assertFalse(T._equal(original, current))

    def test_real_same_rewrites_readable_refs_and_preserves_literals_and_snapshots(self):
        from scripts import sameness as S
        self.doc['known']['order.old'] = copy.deepcopy(self.doc['known']['order.price'])
        self.doc['known']['order.total']['rule'] = {'expr': 'order.old * order.quantity'}
        self.doc['known']['order.label'] = {'rule': {'expr': '"[order.price, order.price]"'}}
        self.doc['judgments']['c.budget']['seen']['order.total']['computed']['rule'] = {'expr': 'order.old * order.quantity'}
        self.save()
        source = self.path.read_text().replace('      expr: order.old * order.quantity\n',
            '      expr: |\n        order.old * order.quantity\n', 1)
        self.path.write_text(source)
        with contextlib.redirect_stdout(io.StringIO()):
            S.same([str(self.path)], 'order.price', 'order.old')
        _, _, _, _, raw = self.world()
        self.assertEqual(E.refs(raw['order.total']['rule']), ['order.price', 'order.quantity'])
        self.assertEqual(raw['order.label']['rule']['expr'], '"[order.price, order.price]"')
        history = raw['c.budget']['seen']['order.total']['computed']
        self.assertEqual(history['value'], 100)
        self.assertEqual(E.refs(history['rule']), ['order.price', 'order.quantity'])

    def test_identity_merge_and_claims_accept_equivalent_expression_representations(self):
        from scripts import sameness as S
        rule = {'expr': ' (order.price * order.quantity) '}
        self.doc['known']['order.copy'] = {'rule': rule}
        self.save()
        ast = self.doc['known']['order.total']['rule']
        self.assertTrue(P._same_claim(P.claim_of({'rule': ast}), P.claim_of({'rule': rule})))
        self.assertFalse(P._same_claim(P.claim_of({'v': ast}), P.claim_of({'v': rule})))
        with contextlib.redirect_stdout(io.StringIO()):
            S.same([str(self.path)], 'order.total', 'order.copy')
        _, _, _, _, raw = self.world()
        self.assertNotIn('order.copy', raw)
        self.assertEqual(raw['order.total']['rule'], ast)

    def test_core_rejects_an_already_loaded_parser_that_changed_on_disk(self):
        module = self.core.expressions
        with patch.object(module, 'LOADED_SOURCE_HASH', 'old-parser'):
            with self.assertRaisesRegex(ValueError, 'parser changed'):
                Core()

    def test_explicit_readable_migration_preserves_snapshots_and_current_results(self):
        from scripts.expression_cli import migrate
        before = self.path.read_bytes()
        preview = migrate(self.path, readable=True)
        self.assertEqual(len(preview['changes']), 2)
        self.assertEqual(self.path.read_bytes(), before)
        outcome = migrate(self.path, apply=True, readable=True)
        self.assertTrue(outcome['applied'], outcome)
        _, ids, jud, fields, raw = self.world()
        self.assertEqual(raw['order.total']['rule'], {'expr': 'order.price * order.quantity'})
        self.assertEqual(raw['c.budget']['seen'], self.doc['judgments']['c.budget']['seen'])
        self.assertEqual(P._state('c.budget', jud['c.budget'], raw, ids, fields)[0], 'HOLDS')
        self.assertEqual(migrate(self.path, readable=True)['changes'], [])

    def test_readable_migration_preserves_crlf_and_history(self):
        from scripts.expression_cli import migrate
        before = self.path.read_bytes().replace(b'\r\n', b'\n').replace(b'\n', b'\r\n')
        self.path.write_bytes(before)
        preview = migrate(self.path, readable=True)
        self.assertEqual(len(preview['changes']), 2)
        self.assertEqual(self.path.read_bytes(), before)
        outcome = migrate(self.path, apply=True, readable=True)
        self.assertTrue(outcome['applied'], outcome)
        after = self.path.read_bytes()
        self.assertIn(b'\r\n', after)
        self.assertNotIn(b'\n', after.replace(b'\r\n', b''))
        self.assertNotIn(b'\r\r\n', after)
        _, ids, _, _, raw = self.world()
        self.assertEqual(raw['order.total']['rule'], {'expr': 'order.price * order.quantity'})
        self.assertEqual(P.value_of(raw, ids, 'order.total'), 100)
        self.assertEqual(raw['c.budget']['seen'], self.doc['judgments']['c.budget']['seen'])

    def test_checked_revision_binds_parser_identity_and_guard_keeps_exact_events(self):
        from scripts.session.view import GroundingService
        self.doc['known']['order.total']['rule'] = {'expr': 'order.price * order.quantity'}
        self.doc['judgments']['c.budget']['wrong_if'] = {'expr': 'order.total > 150'}
        self.save()
        service = GroundingService('readable-example', self.path, self.root / 'session', reader=Path(P.__file__))
        graph, revision = service.graph()
        packet = service.opening_packet(1000)['packet']
        self.assertTrue(service.core.guard(graph.data, packet)['accepted'])
        with patch.object(service.core, 'expression_source_hash', 'different-parser'):
            _, changed = service.graph()
        self.assertNotEqual(revision, changed)
        forged = copy.deepcopy(packet)
        forged['events'] = {'expr': 'order.price * order.quantity'}
        with self.assertRaises(ValueError):
            service.core.guard(graph.data, forged)
