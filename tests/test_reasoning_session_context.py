"""The explicit core session route retains one source-free assessment context."""
import json
import contextlib
import io
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest
import datetime
from unittest import mock

try:
    import tiktoken  # noqa: F401
    from scripts.reasoning.context import CapturedAssessment
    from scripts.reasoning.snapshot import Snapshot
    from scripts.pending_grounding import _decode as typed_decode
    from scripts.session.entry import parser
    from scripts.session.view import CoreGroundingService
    SESSION_READY = True
except ImportError:
    SESSION_READY = False


class UnavailableRuntime:
    def request_many(self, requests):
        raise OSError('deliberately unavailable')


@unittest.skipUnless(SESSION_READY, 'install the session tokenization dependency')
class CoreSessionContextTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.folder = Path(self.temporary.name)
        self.record = self.folder / 'GROUNDING.yaml'
        self.record.write_text('captured by the patched Snapshot boundary')
        self.reader = Path(__file__).resolve().parents[1] / 'scripts' / 'provenance.py'
        self.document = {
            'meta': {'reasoning': {'version': 2, 'profile': 'core/v1',
                                   'requires': ['arithmetic/v1']}},
            'readings': {'p.input': {'v': 1}},
            'judgments': {'d.choice': {
                'rests_on': ['p.input'], 'seen': {'p.input': 1},
                'wrong_if': 'p.input > 2', 'verdict': 'Keep the choice.',
            }},
        }

    def context(self, review_act):
        snapshot = Snapshot.from_data(
            self.document,
            context={'read_mode': 'supplied', 'source_collection': 'test',
                     'captured_review_act': review_act})
        return CapturedAssessment.from_snapshot(
            snapshot, runtime=UnavailableRuntime())

    def service(self, project='fixture'):
        return CoreGroundingService(
            project, self.record, self.folder / ('state-' + project), self.reader)

    def open_context(self, context):
        service = self.service()
        with mock.patch.object(CapturedAssessment, 'capture', return_value=context):
            opened = service.opening_packet(1000)
        return opened, opened['packet']['revision']

    def test_explicit_profile_has_a_separate_cli_selector(self):
        legacy = parser().parse_args(['open'])
        explicit = parser().parse_args(['open', '--assessment-profile', 'core/v1'])
        self.assertIsNone(legacy.assessment_profile)
        self.assertEqual(explicit.assessment_profile, 'core/v1')
        with self.assertRaisesRegex(ValueError, 'checked-reader/v1'):
            self.service().core.assess({}, 'd.choice', [])

    def test_declared_core_selects_session_and_hook_without_legacy_executor(self):
        import yaml
        from scripts.session import entry
        self.record.write_text(yaml.safe_dump(self.document), encoding='utf-8')
        for operation in ('open', 'hook-open'):
            output = io.StringIO()
            with self.subTest(operation=operation), contextlib.redirect_stdout(output), \
                 mock.patch('scripts.session.view.GroundingService', side_effect=AssertionError('legacy service')):
                code = entry.main([operation, '--no-settings', '--input', str(self.record),
                                   '--state', str(self.folder / 'auto-state'), '--tokens', '4000'])
            self.assertEqual(code, 0, output.getvalue())
            self.assertIn('assessment_profile=core/v1', output.getvalue())
            if operation == 'hook-open':
                self.assertIn('--assessment-profile core/v1', output.getvalue())

    def test_handle_binds_project_snapshot_findings_and_consumer_version(self):
        context = self.context('review-one')
        opened, revision = self.open_context(context)
        text = opened['text']
        self.assertIn('assessment_profile=core/v1', text)
        self.assertIn('snapshot_id=' + context.snapshot_id, text)
        self.assertIn('findings_revision=' + context.findings_revision, text)
        self.assertIn('consumer_view=reasoning-projection/v1', text)
        self.assertEqual(revision, context.session_revision({
            'version': 1, 'project': 'fixture', 'input_path': str(self.record.resolve()),
            'navigation_profile_sha256': None}))
        self.assertNotEqual(revision, context.session_revision({
            'version': 1, 'project': 'another', 'input_path': str(self.record.resolve()),
            'navigation_profile_sha256': None}))
        cache = self.folder / 'state-fixture' / ('core-context-' + revision + '.json')
        payload = json.loads(cache.read_text())
        self.assertEqual(payload['revision'], revision)
        self.assertEqual(CapturedAssessment.from_data(payload['context']).view['version'],
                         'reasoning-projection/v1')

    def test_public_context_uses_core_capture_and_keeps_legacy_session_route(self):
        import yaml
        from scripts.session import entry
        self.record.write_text(yaml.safe_dump(self.document), encoding='utf-8')
        output = io.StringIO()
        shared = ['--no-settings', '--input', str(self.record),
                  '--state', str(self.folder / 'public-context'), '--tokens', '10000']
        with contextlib.redirect_stdout(output), \
             mock.patch('scripts.session.view.GroundingService', side_effect=AssertionError('legacy service')):
            code = entry.main(['d.choice', *shared], context=True)
        self.assertEqual(code, 0, output.getvalue())
        direct = json.loads(output.getvalue())
        self.assertEqual({row['id'] for row in direct['reads']}, {'d.choice', 'p.input'})
        legacy = io.StringIO()
        with contextlib.redirect_stdout(legacy):
            code = entry.main(['context', '--id', 'd.choice', '--direction', 'support',
                               '--revision', direct['revision'], *shared])
        self.assertEqual(code, 0, legacy.getvalue())
        self.assertEqual(json.loads(legacy.getvalue()), direct)

    def test_followups_recapture_only_for_staleness_then_read_retained_context(self):
        context = self.context('review-one')
        _, revision = self.open_context(context)
        followup = self.service()
        with mock.patch.object(Snapshot, 'capture', return_value=context.snapshot) as recapture, \
                mock.patch.object(CapturedAssessment, 'capture',
                                  side_effect=AssertionError('assessment must stay retained')), \
                mock.patch('scripts.session.reader.Core',
                           side_effect=AssertionError('legacy evaluator must not run')):
            node = json.loads(followup.reading('node:d.choice', revision, 2400))['value']
            finding = json.loads(followup.reading('finding:d.choice', revision, 10000))['value']
            assessment = json.loads(followup.reading('assessment', revision, 2400))['value']
            search = json.loads(followup.searching('input', revision, 2400,
                                                   mode='lexical'))
            context_read = json.loads(followup.contextualizing(
                ['d.choice'], revision, 'support', 10000))
        self.assertEqual(recapture.call_count, 5)
        self.assertEqual(typed_decode(node['body']['value']), self.document['judgments']['d.choice'])
        self.assertEqual(node['body_encoding'], 'typed-json/v1')
        self.assertEqual(node['finding_ref'], 'finding:d.choice')
        self.assertEqual(typed_decode(finding['value']), context.assessment['nodes']['d.choice'])
        self.assertEqual(finding['encoding'], 'typed-json/v1')
        self.assertEqual(assessment['snapshot_id'], context.snapshot_id)
        self.assertEqual(assessment['findings_revision'], context.findings_revision)
        self.assertEqual(assessment['assessment_profile'], 'core/v1')
        self.assertIn('p.input', [row['id'] for row in search['hits']])
        self.assertIn('p.input', [row['id'] for row in context_read['reads']])
        with self.assertRaisesRegex(ValueError, 'checked-reader/v1'):
            with mock.patch.object(Snapshot, 'capture', return_value=context.snapshot):
                followup.reading('checked:d.choice', revision)

    def test_history_only_review_change_stales_body_equal_handle(self):
        from scripts import history_contract as HC
        from tests.test_reasoning_history_assessment import captured, claim

        source = claim('p.input', {'v': 1}, operation='input')
        before_snapshot = captured(source).snapshot()
        review = HC.make_object(
            subject='p.input', kind='act', by='reviewer', on='2026-09-17',
            operation='review', body={'act': 'review', 'of': source['id'],
                                      'over': [], 'because': 'checked', 'read': {}})
        dispositions = {'p.input': {
            'marks': {}, 'proposals': [], 'contested_claims': [],
            'reviews': [review], 'implied': [],
        }}
        after_snapshot = captured(
            source, acts=(review,), dispositions=dispositions).snapshot()
        before = CapturedAssessment.from_snapshot(
            before_snapshot, runtime=UnavailableRuntime())
        after = CapturedAssessment.from_snapshot(
            after_snapshot, runtime=UnavailableRuntime())
        self.assertEqual(before.snapshot.to_data()['nodes'], after.snapshot.to_data()['nodes'])
        self.assertNotEqual(before.snapshot_id, after.snapshot_id)
        self.assertNotEqual(before.findings_revision, after.findings_revision)
        _, revision = self.open_context(before)
        with mock.patch.object(Snapshot, 'capture', return_value=after.snapshot):
            with self.assertRaisesRegex(ValueError, 'record or history changed'):
                self.service().reading('node:d.choice', revision)

    def test_core_revision_is_not_a_legacy_graph_digest(self):
        context = self.context('review-one')
        opened, revision = self.open_context(context)
        self.assertRegex(revision, r'^[0-9a-f]{64}$')
        self.assertEqual(opened['packet']['revision'], revision)
        self.assertIsNone(re.search(r'checked:', opened['text']))

    def test_cli_open_and_read_preserve_the_explicit_core_route(self):
        self.record.write_text(
            'meta:\n'
            '  reasoning: {version: 2, profile: core/v1, requires: [arithmetic/v1]}\n'
            'known:\n'
            '  p.input: {v: 1}\n')
        state = self.folder / 'cli-state'
        command = [sys.executable, str(Path(__file__).resolve().parents[1]
                                      / 'scripts' / 'session_cli.py')]
        common = ['--no-settings', '--assessment-profile', 'core/v1',
                  '--input', str(self.record), '--project', 'cli-fixture',
                  '--state', str(state)]
        opened = subprocess.run(command + ['open', *common, '--tokens', '1000'],
                                text=True, encoding='utf-8', capture_output=True,
                                timeout=30)
        self.assertEqual(opened.returncode, 0, opened.stderr)
        revision = re.search(r'revision=([0-9a-f]{64})', opened.stdout).group(1)
        read = subprocess.run(command + ['read', *common, '--revision', revision,
                                         '--ref', 'node:p.input', '--tokens', '2000'],
                              text=True, encoding='utf-8', capture_output=True,
                              timeout=30)
        self.assertEqual(read.returncode, 0, read.stderr)
        value = json.loads(read.stdout)['value']
        self.assertEqual(typed_decode(value['body']['value']), {'v': 1})
        self.assertEqual(value['body_encoding'], 'typed-json/v1')
        self.assertEqual(value['finding_ref'], 'finding:p.input')

    def test_date_bearing_record_opens_and_reads_with_typed_body(self):
        self.record.write_text(
            'meta:\n'
            '  reasoning: {version: 2, profile: core/v1, requires: [arithmetic/v1]}\n'
            'known:\n'
            '  p.input: {v: 1, of: 2026-09-17}\n')
        service = self.service('dated')
        opened = service.opening_packet(1000)
        revision = opened['packet']['revision']
        value = json.loads(service.reading('node:p.input', revision, 2000))['value']
        body = typed_decode(value['body']['value'])
        self.assertEqual(body['of'], datetime.date(2026, 9, 17))


if __name__ == '__main__':
    unittest.main()
