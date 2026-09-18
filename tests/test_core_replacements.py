"""Core writers retain the ordinary decision-replacement guards."""
import unittest

from scripts import provenance as P
from tests import test_core_authoring as fixtures


class CoreReplacements(unittest.TestCase):
    setUp = fixtures.CoreAuthoring.setUp
    save = fixtures.CoreAuthoring.save
    read = fixtures.CoreAuthoring.read
    write = fixtures.CoreAuthoring.write

    def standing(self, *, fired=False):
        self.write('add', 'c.price', body={
            'rests_on': ['p.price', 'p.quantity'], 'verdict': 'Price fits',
            'because': 'Both readings were considered', 'wrong_if': 'p.price > 100'},
            profile='core/v1')
        if fired:
            self.write('set', 'p.price', value=200)

    def test_same_verdict_new_grounds_need_the_standing_condition(self):
        self.standing()
        before = self.path.read_bytes()
        with self.assertRaises(P.Refused):
            self.write('add', 'c.price', body={
                'rests_on': ['p.price', 'p.quantity'], 'verdict': 'Price fits',
                'because': 'A new reason', 'wrong_if': 'p.price > 100'})
        self.assertEqual(self.path.read_bytes(), before)
        self.assertEqual(P.read_replaced([str(self.path)]), {})

    def test_tool_written_trail_cannot_be_supplied_to_core_writer(self):
        before = self.path.read_bytes()
        with self.assertRaisesRegex(P.Refused, 'replaced is written by this tool'):
            self.write('add', 'c.price', body={
                'rests_on': ['p.price'], 'verdict': 'Price fits',
                'wrong_if': 'p.price > 100', 'replaced': ['invented history']},
                profile='core/v1')
        self.assertEqual(self.path.read_bytes(), before)
        self.assertEqual(P.read_replaced([str(self.path)]), {})

    def test_core_replacement_requires_each_dropped_dependency_reason(self):
        self.standing(fired=True)
        before = self.path.read_bytes()
        body = {'rests_on': ['p.price'], 'verdict': 'Price exceeds the limit',
                'wrong_if': 'p.price < 100'}
        with self.assertRaisesRegex(P.Refused, 'no longer rests on p.quantity'):
            self.write('add', 'c.price', body=body)
        self.assertEqual(self.path.read_bytes(), before)
        self.assertEqual(P.read_replaced([str(self.path)]), {})
        self.write('add', 'c.price', body=body,
                   drops={'p.quantity': 'The price limit does not depend on quantity'})
        version = P.read_replaced([str(self.path)])['c.price'][0]
        self.assertEqual(version['dropped'], {
            'p.quantity': 'The price limit does not depend on quantity'})
        self.assertEqual(version['seen']['p.price']['computed']['value'], {
            'type': 'number', 'numerator': '20', 'denominator': '1'})

    def test_core_replacement_rejects_a_reason_for_a_retained_dependency(self):
        self.standing(fired=True)
        before = self.path.read_bytes()
        with self.assertRaisesRegex(P.Refused, 'still rests on'):
            self.write('add', 'c.price', body={
                'rests_on': ['p.price', 'p.quantity'], 'verdict': 'Price exceeds the limit',
                'wrong_if': 'p.price < 100'}, drops={'p.quantity': 'Actually still used'})
        self.assertEqual(self.path.read_bytes(), before)
        self.assertEqual(P.read_replaced([str(self.path)]), {})
