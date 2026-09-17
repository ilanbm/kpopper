"""Direct recovery keeps a history identity change and its brief coherent."""
import contextlib
import io
import json
from pathlib import Path
import subprocess
import sys
import unittest
from unittest import mock

from scripts import history_direct as D, history_transaction as T, provenance as P
from tests import test_history_identity as fixtures


class DirectAuxiliaryRecovery(unittest.TestCase):
    def setUp(self):
        fixture = fixtures.Identity()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        self.fixture, self.entry, self.store = fixture, fixture.entry, fixture.store
        self.view = Path(self.store.layout['view'])
        self.original_view = b'sections: [{text: "{{p.other}}", seen: {p.other: 1}}]\n'
        self.view.write_bytes(self.original_view)
        self.before = self.store.capture()

    def same(self):
        with contextlib.redirect_stdout(io.StringIO()):
            return D.identity_write([str(self.entry)], 'p.input', 'p.other', kind='same')

    def recover_cli(self, *options):
        result = subprocess.run([sys.executable, '-B', str(Path(P.__file__).with_name('cli.py')),
                                 'recover', '--record', str(self.entry), '--json', *options],
                                capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return json.loads(result.stdout)

    def test_cli_forward_recovery_finishes_the_same_manifest_and_brief(self):
        replace = T._replace
        def interrupt(path, raw):
            if Path(path).resolve() == self.view.resolve():
                raise OSError('brief publication interrupted')
            return replace(path, raw)
        with mock.patch.object(T, '_replace', side_effect=interrupt), self.assertRaisesRegex(
                OSError, 'brief publication interrupted'):
            self.same()
        pending = self.entry.parent / D.journal(self.entry)
        mutation, _ = D._read_envelope(pending.read_bytes())
        with self.assertRaisesRegex(ValueError, 'recovery_required'):
            self.store.capture()
        receipt = self.recover_cli()
        self.assertEqual(receipt['operation'], mutation.to_data()['operation'])
        after = self.store.capture()
        self.assertEqual(len(after.commits), len(self.before.commits) + 1)
        self.assertEqual(after.state['subjects']['p.other']['acceptance'], 'retired')
        self.assertEqual(self.view.read_bytes(), self.original_view.replace(b'p.other', b'p.input'))
        for version, original in self.before.objects.items():
            self.assertEqual(after.objects[version], original)
        self.assertFalse(pending.exists())
        self.assertFalse((self.entry.parent / T.journal_for(self.entry)).exists())

    def test_cli_rollback_before_manifest_restores_readability_without_an_act(self):
        publish = T.publish_immutable
        commits = Path(self.store.layout['history_commits']).resolve()
        def interrupt(path, raw, **options):
            if Path(path).resolve().parent == commits:
                raise OSError('manifest publication interrupted')
            return publish(path, raw, **options)
        with mock.patch.object(T, 'publish_immutable', side_effect=interrupt), self.assertRaisesRegex(
                OSError, 'manifest publication interrupted'):
            self.same()
        with self.assertRaisesRegex(ValueError, 'recovery_required'):
            self.store.capture()
        self.assertEqual(self.recover_cli('--rollback')['state'], 'restored')
        after = self.store.capture()
        self.assertEqual(after.commits, self.before.commits)
        self.assertEqual(after.objects, self.before.objects)
        self.assertEqual(after.entry_bytes, self.before.entry_bytes)
        self.assertEqual(self.view.read_bytes(), self.original_view)
        self.assertFalse((self.entry.parent / D.journal(self.entry)).exists())
        self.assertFalse((self.entry.parent / T.journal_for(self.entry)).exists())

    def test_mismatched_retry_envelope_cannot_bypass_the_reader_guard(self):
        from scripts import history_contract as C, history_identity as I
        alternate = I.prepare_same(self.entry, 'p.input', 'p.other', operation='different-operation')
        replace = T._replace
        def interrupt(path, raw):
            if Path(path).resolve() == self.view.resolve():
                raise OSError('brief publication interrupted')
            return replace(path, raw)
        with mock.patch.object(T, '_replace', side_effect=interrupt), self.assertRaises(OSError):
            self.same()
        pending = self.entry.parent / D.journal(self.entry)
        _, routing = D._read_envelope(pending.read_bytes())
        pending.write_bytes(C.encode_document(D._envelope(alternate, routing)))
        primary = self.entry.parent / T.journal_for(self.entry)
        before = (self.entry.read_bytes(), self.view.read_bytes(), primary.read_bytes(), pending.read_bytes())
        with self.assertRaisesRegex(ValueError, 'invalid_history_auxiliary'):
            P.recover_direct([str(self.entry)])
        self.assertEqual(before, (self.entry.read_bytes(), self.view.read_bytes(),
                                  primary.read_bytes(), pending.read_bytes()))
        with self.assertRaisesRegex(ValueError, 'recovery_required'):
            self.store.capture()


if __name__ == '__main__':
    unittest.main()
