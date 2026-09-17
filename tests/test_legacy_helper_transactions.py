"""Legacy helper commands publish through the recoverable record boundary."""
from pathlib import Path
import tempfile
import unittest
from unittest import mock

from scripts import expression_cli as E


class ExpressionTransactions(unittest.TestCase):
    def test_expression_migration_retains_prepared_bytes_after_publication_failure(self):
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
