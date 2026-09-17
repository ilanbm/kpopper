"""Legacy helper commands publish through the recoverable record boundary."""
from pathlib import Path
import tempfile
import os
import unittest
from unittest import mock

from scripts import expression_cli as E


class ExpressionTransactions(unittest.TestCase):
    def require_checked_core(self):
        try:
            core = E.P.E._core_type()()
            core.ensure_program()
        except (ValueError, ImportError, OSError):
            if os.environ.get('KPOPPER_REQUIRE_CORE_TESTS') == '1':
                raise
            self.skipTest('expression migration publication recovery requires the optional checked core')

    def test_expression_migration_retains_prepared_bytes_after_publication_failure(self):
        self.require_checked_core()
        with tempfile.TemporaryDirectory() as directory:
            record = Path(directory).resolve() / 'GROUNDING.yaml'
            record.write_text('known:\n  p.x: {v: 2}\n  p.y: {rule: "p.x * 3"}\n')
            before = record.read_bytes()
            preview = E.migrate(record)
            self.assertTrue(preview['changes'])
            T = E.P._peer('history_transaction')
            replace = T._replace
            def fail(path, data):
                if path == record:
                    raise OSError('helper publication failure')
                return replace(path, data)
            with mock.patch.object(T, '_replace', side_effect=fail):
                with self.assertRaisesRegex(OSError, 'helper publication failure'):
                    E.migrate(record, apply=True)
            self.assertEqual(record.read_bytes(), before)
            journal = record.parent / T.journal_for(record)
            mutation = T.PreparedMutation.from_bytes(journal.read_bytes())
            with self.assertRaisesRegex((ValueError, E.P.Refused), 'recovery_required'):
                E.P.load([str(record)])
            E.P.recover_direct([str(record)])
            intended = next(i['after'] for i in mutation.files if i['role'] == 'record')
            self.assertEqual(record.read_bytes(), intended)
            self.assertFalse(journal.exists())
            self.assertEqual(E.migrate(record)['changes'], [])

    def test_expression_migration_does_not_publish_without_checked_meaning_validation(self):
        with tempfile.TemporaryDirectory() as directory:
            record = Path(directory).resolve() / 'GROUNDING.yaml'
            record.write_text('known:\n  p.x: {v: 2}\n  p.y: {rule: "p.x * 3"}\n')
            before = record.read_bytes()
            previous = E.P.E._CORE
            with mock.patch.object(E.P.E, '_core_type', side_effect=ValueError('checked core unavailable')):
                E.P.E._CORE = None
                try:
                    result = E.migrate(record, apply=True)
                finally:
                    E.P.E._CORE = previous
            self.assertTrue(result['changes'])
            self.assertFalse(result['applied'])
            self.assertTrue(result['problems'])
            self.assertEqual(record.read_bytes(), before)
            transaction = E.P._peer('history_transaction')
            self.assertFalse((record.parent / transaction.journal_for(record)).exists())
