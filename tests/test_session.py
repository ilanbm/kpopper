"""The checked reader preserves source fields, epistemic distinctions and exact routes."""
import copy
import asyncio
import hashlib
import importlib.util
import json
import os
import re
import shlex
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
from scripts.session.model import MapTree
from scripts.session.core import Core

try:
    import tiktoken
    from scripts.session.view import GroundingService
    from scripts.session.store import native_record
    CORE_READY = bool(Core())
except (ImportError, ValueError):
    CORE_READY = False

if os.environ.get('KPOPPER_REQUIRE_CORE_TESTS') == '1' and not CORE_READY:
    raise RuntimeError('session integration tests require the installed extras and compiled core')


NATIVE = '''schema:
  deps: based_on
  snapshot: reviewed_values
  predicate: reject_if
sources:
  s.observation: {read: 2026-09-08}
known:
  m.reading: {v: 3, from: s.observation}
  m.scalar: 0
  prior.assumption: {v: 0.7, from: s.observation}
judgments:
  d.choice:
    based_on: [m.reading, prior.assumption]
    reviewed_values: {m.reading: 1, prior.assumption: 0.7}
    reject_if: m.reading > 5
    verdict: A recorded choice, subject to review.
    status: Superseded pending a new investigation.
    reopened_by: A person finds a counterexample.
'''


class ModelContract(unittest.TestCase):
    def test_unaddressable_identifiers_are_rejected_before_projection(self):
        for nid in ['x#y', '/x', 'x:y', 'x\ny']:
            with self.assertRaises(ValueError):
                MapTree({'nodes': {nid: {'kind': 'known', 'states': [], 'body': {}}}, 'topics': {nid: []}, 'edges': []})

    def test_wide_tree_covers_every_id_once_and_preserves_leaf_topics(self):
        data = {"nodes": {}, "topics": {}, "edges": []}
        for n in range(400):
            nid = "item." + str(n)
            data["nodes"][nid] = {"kind": "known", "states": [], "body": {"v": n}}
            data["topics"][nid] = ["wide"]
        tree = MapTree(data)
        for level in tree.layers():
            covered = [nid for entry in level for nid in tree.members(entry)]
            self.assertCountEqual(covered, data["nodes"])
        self.assertEqual(set(tree.leaves.values()), {"/wide"})


@unittest.skipUnless(CORE_READY, "install session extras and run session setup for kernel integration tests")
class CheckedSessionContract(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.folder = Path(self.temp.name).resolve()
        environment = patch.dict(os.environ, {'KPOPPER_SESSION_CONFIG': str(self.folder / 'settings.json'),
                                               'XDG_CONFIG_HOME': str(self.folder / 'config'),
                                               'XDG_STATE_HOME': str(self.folder / 'state-home')})
        environment.start()
        self.addCleanup(environment.stop)
        self.native = self.folder / "PROVENANCE.yaml"
        self.native.write_text(NATIVE)
        self.reader = ROOT / "scripts" / "provenance.py"

    def native_service(self):
        return GroundingService("example", self.native, self.folder / "state", self.reader)

    def read_hint(self, text):
        prefix = 'Read via MCP kpopper_read, or (POSIX shell): '
        line = next(line for line in text.splitlines() if line.startswith(prefix))
        shell_command = line[len(prefix):]
        command = shlex.split(shell_command)
        self.assertEqual(Path(command[0]).resolve(), Path(sys.executable).resolve())
        self.assertEqual(Path(command[1]).resolve(), (ROOT / 'scripts/session_cli.py').resolve())
        self.assertEqual(command[2], 'read')
        self.assertIn('--no-settings', command)
        return shell_command, command

    def data(self):
        return native_record(self.native, self.reader)

    def json_service(self, data):
        data = copy.deepcopy(data)
        source = json.dumps({'nodes': data['nodes'], 'edges': data['edges']}, sort_keys=True)
        data['sources'] = {'record': {'text': source, 'sha256': hashlib.sha256(source.encode()).hexdigest(),
                                     'location': 'synthetic:session-contract'}}
        path = self.folder / "record.json"
        path.write_text(json.dumps(data))
        return GroundingService("example", path, self.folder / "json-state")

    def test_inferred_fields_reach_kernel_without_overwriting_original_spelling(self):
        service = self.native_service()
        graph, revision = service.graph()
        body = graph.nodes["d.choice"]["body"]
        self.assertIn("based_on", body)
        self.assertNotIn("rests_on", body)
        bundle = json.loads(service.reading("checked:d.choice", revision))["value"]
        self.assertTrue(bundle["premise_changed"])
        self.assertFalse(bundle["falsifier"]["holds_on_current_values"])
        premise = next(p for p in bundle["premises"] if p["id"] == "m.reading")
        self.assertEqual((premise["current_recorded_value"], premise["value_at_review"]), (3, 1))
        source = json.loads(service.reading("source:record", revision, 3000))["value"]
        original_bytes = self.native.read_bytes()
        self.assertEqual(source["text"].encode('utf-8'), original_bytes)
        self.assertEqual(hashlib.sha256(original_bytes).hexdigest(), source["sha256"])

    def test_utf8_cli_preserves_original_crlf_source_bytes(self):
        raw = (NATIVE.replace('\n', '\r\n') + '    because: "שלום 世界 😀"\r\n').encode('utf-8')
        self.native.write_bytes(raw)
        service = self.native_service()
        _, revision = service.graph()
        command = [sys.executable, str(ROOT / 'scripts/session_cli.py'), 'read', '--no-settings',
                   '--input', str(self.native), '--project', 'example', '--state', str(self.folder / 'state'),
                   '--revision', revision, '--ref', 'source:record', '--tokens', '3000']
        # The CLI owns its UTF-8 wire encoding rather than inheriting this codepage.
        environment = dict(os.environ, PYTHONIOENCODING='cp1252', PYTHONUTF8='0',
                           PYTHONCOERCECLOCALE='0', LC_ALL='C')
        result = subprocess.run(command, env=environment, capture_output=True, text=True,
                                encoding='utf-8', timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr)
        source = json.loads(result.stdout)['value']
        self.assertEqual(source['text'].encode('utf-8'), raw)
        self.assertEqual(source['sha256'], hashlib.sha256(raw).hexdigest())
        self.assertEqual(self.native.read_bytes(), raw)

    def test_scalar_values_and_recorded_status_remain_visible(self):
        service = self.native_service()
        graph, revision = service.graph()
        check = service.core.assess(graph.data, "d.choice", [{"kind": "current", "id": "m.scalar", "expected": 0}])
        self.assertTrue(check["assertions_accepted"])
        bundle = json.loads(service.reading("checked:d.choice", revision))["value"]
        self.assertEqual(bundle["recorded_status"], "Superseded pending a new investigation.")
        card = json.loads(service.reading("d.choice", revision))["value"]["epistemic_card"]
        self.assertIn("RECORDED STATUS (not evaluated)", card)

    def test_default_judgment_read_preserves_all_original_source_fields(self):
        self.native.write_text(NATIVE + '    because: The reviewer who receives the change decides whether to merge it.\n'
            + '    broken_by: A later account replaced the former diagnosis.\n')
        service = self.native_service()
        graph, revision = service.graph()
        response = json.loads(service.reading('d.choice', revision, 1600))
        self.assertTrue(response['complete'])
        self.assertEqual(response['value']['body'], graph.nodes['d.choice']['body'])
        self.assertIn('because', response['value']['body'])
        self.assertIn('broken_by', response['value']['body'])

    def test_native_snapshot_is_stable_across_process_hash_seeds(self):
        self.native.write_text(NATIVE.replace('  m.scalar: 0', '''  m.scalar: 0
  m.a: {v: 1}
  m.b: {v: 2}
  m.c: {v: 3}
  m.d: {v: 4}
  m.total: {v: "m.a + m.b + m.c + m.d"}'''))
        command = [sys.executable, '-c',
            'from pathlib import Path; import sys; from scripts.session.store import native_record, digest; '
            'print(digest(native_record(Path(sys.argv[1]), Path(sys.argv[2]))))',
            str(self.native), str(self.reader)]
        snapshots = []
        for seed in ('1', '2', '3', '4'):
            snapshots.append(subprocess.check_output(command, cwd=ROOT,
                env=dict(os.environ, PYTHONHASHSEED=seed), text=True).strip())
        self.assertEqual(len(set(snapshots)), 1, snapshots)

    def test_missing_current_stays_unknown_while_historical_value_survives(self):
        data = self.data()
        data["nodes"]["m.reading"]["body"].pop("v")
        service = self.json_service(data)
        graph, revision = service.graph()
        scan = service.core.scan(graph.data)
        event = next(e for e in scan["events"].values() if e["kind"] == "uncheckable")
        comparison = event["comparison_details"][0]
        self.assertIsNone(comparison["current_recorded_value"])
        self.assertEqual(comparison["value_at_review"], 1)
        self.assertIsNone(event["falsifier"]["holds_on_current_values"])
        rejected = service.core.assess(graph.data, "d.choice", [{"kind": "falsifier_holds", "id": "d.choice", "expected": False}])
        self.assertFalse(rejected["assertions_accepted"])

    def test_conflict_survives_folding_default_card_and_explicit_checked_read(self):
        data = self.data()
        data["nodes"]["d.choice"]["states"].append("contested")
        service = self.json_service(data)
        opening = service.opening_packet(700)
        revision = opening["packet"]["revision"]
        self.assertIn("CONTESTED", opening["text"])
        self.assertTrue(json.loads(service.reading("checked:d.choice#/declared_conflict", revision))["value"])
        self.assertIn("RECORDED CONFLICT: YES", json.loads(service.reading("d.choice", revision))["value"]["epistemic_card"])

    def test_no_declared_test_or_reopener_is_an_explicit_attention_event(self):
        self.native.write_text(NATIVE.replace('    reject_if: m.reading > 5\n', '')
            .replace('    reopened_by: A person finds a counterexample.\n', '')
            .replace('reviewed_values: {m.reading: 1', 'reviewed_values: {m.reading: 3'))
        service = self.native_service()
        result = service.opening_packet(700)
        event = next(e for e in result["packet"]["events"].values() if e["id"] == "d.choice")
        self.assertEqual(event["kind"], "uncheckable")
        self.assertEqual(event["falsifier"]["reason"], "not_declared")

    def test_unreadable_claim_does_not_erase_its_prose_declaration(self):
        self.native.write_text(NATIVE.replace('    verdict: A recorded choice, subject to review.\n', ''))
        service = self.native_service()
        graph, revision = service.graph()
        scan = service.core.scan(graph.data)
        self.assertEqual(scan["counts"]["errors"], 1)
        self.assertEqual(scan["counts"]["human_reopener_declared"], 1)
        condition = json.loads(service.reading("conditions:/#/d.choice", revision))["value"]
        self.assertEqual(condition["reopened_by"]["declaration"], "A person finds a counterexample.")

    def test_stale_normalized_body_cannot_mask_a_source_change(self):
        data = self.data()
        data['nodes']['d.choice']['body']['verdict'] = 'The source changed.'
        with self.assertRaises(ValueError):
            MapTree(data)
        data = self.data()
        data['nodes']['d.choice']['assessment_body']['reviewed_values']['m.reading'] = True
        # Python's True == 1 must not make different JSON types interchangeable.
        data['nodes']['d.choice']['assessment_body']['seen']['m.reading'] = True
        with self.assertRaises(ValueError):
            MapTree(data)

    def test_deep_dotted_names_retain_the_full_namespace(self):
        self.native.write_text(NATIVE.replace('  m.scalar: 0', '  api.auth.token.limit: 0'))
        data = self.data()
        self.assertEqual(data['topics']['api.auth.token.limit'], ['known', 'api', 'auth', 'token'])

    def test_metadata_names_do_not_shadow_explicit_node_references(self):
        data = self.data()
        data['nodes']['orientation'] = {'kind': 'known', 'states': [], 'body': {'v': 'an entry'}}
        data['topics']['orientation'] = ['known']
        service = self.json_service(data)
        revision = service.graph()[1]
        self.assertEqual(json.loads(service.reading('node:orientation#/body/v', revision))['value'], 'an entry')
        self.assertIn('text', json.loads(service.reading('orientation', revision))['value'])
        self.assertIn('node:orientation', service.opening(1600))

    def test_guard_rejects_lost_coverage_bad_refs_values_and_links(self):
        service = self.native_service()
        graph, revision = service.graph()
        original = service.opening_packet(1000)["packet"]
        mutations = [lambda p: p["cells"].clear(),
                     lambda p: p["cells"][0].update(key="/not-a-route"),
                     lambda p: p.update(links=[]),
                     lambda p: p.update(events={}),
                     lambda p: p.update(conditions_ref="elsewhere"),
                     lambda p: p.update(revision="0" * 64)]
        for mutate in mutations:
            packet = copy.deepcopy(original)
            mutate(packet)
            with self.assertRaises(ValueError):
                service.core.guard(graph.data, packet)

    def test_directed_links_exact_fields_and_budget_limits(self):
        service = self.native_service()
        graph, revision = service.graph()
        for budget in [500, 700, 1600]:
            result = service.opening_packet(budget)
            self.assertLessEqual(result["tokens"], budget)
            self.assertCountEqual([n for cell in result["packet"]["cells"] for n in cell["members"]], graph.nodes)
        text = service.reading("links:d.choice", revision)
        self.assertIn("d.choice rests_on m.reading", text)
        self.assertEqual(json.loads(service.reading("node:d.choice#/body/based_on", revision))["value"], ["m.reading", "prior.assumption"])
        with self.assertRaises(ValueError):
            service.opening(64)

    def test_revision_invalidation_and_pending_proposal_staleness(self):
        service = self.native_service()
        revision = service.graph()[1]
        proposal = json.loads(service.proposing(revision, "question", "What needs another observation?", [], ""))
        self.assertFalse(proposal["canonical_record_changed"])
        self.assertEqual(self.native.read_text(), NATIVE)
        self.native.write_text(NATIVE.replace("v: 3", "v: 4"))
        with self.assertRaises(ValueError):
            service.reading("d.choice", revision)
        current = service.graph()[1]
        held = json.loads(service.reading(proposal["read"], current))["value"]
        self.assertTrue(held["stale_base"])

    def test_cli_checkout_entry_uses_same_revision_and_reader(self):
        command = [sys.executable, str(ROOT / "scripts" / "cli.py"), "session", "open",
                   "--input", str(self.native), "--project", "example", "--state", str(self.folder / "cli-state")]
        run = subprocess.run(command, text=True, encoding='utf-8', capture_output=True)
        self.assertEqual(run.returncode, 0, run.stderr)
        self.assertIn("revision=", run.stdout)
        self.assertIn("seen=1 current=3", run.stdout)

    def test_invalid_predicate_type_is_unreadable_not_silently_absent(self):
        self.native.write_text(NATIVE.replace('reject_if: m.reading > 5', 'reject_if: false'))
        service = self.native_service()
        graph, revision = service.graph()
        scan = service.core.scan(graph.data)
        self.assertEqual(scan['counts']['errors'], 1)
        condition = json.loads(service.reading('conditions:/#/d.choice', revision))['value']
        self.assertEqual(condition['executable']['reason'], 'assessment_error')
        self.assertIs(condition['executable']['expression'], False)

    def test_project_enable_disable_and_hook_budget_are_reversible(self):
        from scripts.session import settings
        base = [sys.executable, str(ROOT / 'scripts' / 'session_cli.py')]
        enabled = subprocess.run(base + ['enable', '--tokens', '900'], cwd=self.folder, text=True, encoding='utf-8', capture_output=True)
        self.assertEqual(enabled.returncode, 0, enabled.stderr)
        self.assertTrue(settings.current()['enabled'])
        hook = subprocess.run(['sh', str(ROOT / 'scripts' / 'session_open.sh')], input='{}', cwd=self.folder,
                              text=True, encoding='utf-8', capture_output=True)
        self.assertEqual(hook.returncode, 0, hook.stderr)
        self.assertIn('executable falsifiers:', hook.stdout)
        self.read_hint(hook.stdout)
        self.assertIn('MAP / —', hook.stdout)
        self.assertLessEqual(len(tiktoken.get_encoding('o200k_base').encode(hook.stdout)), 900)
        disabled = subprocess.run(base + ['disable'], cwd=self.folder, text=True, encoding='utf-8', capture_output=True)
        self.assertEqual(disabled.returncode, 0, disabled.stderr)
        self.assertFalse(settings.current()['enabled'])
        legacy = subprocess.run(['sh', str(ROOT / 'scripts' / 'session_open.sh')], input=b'{}', cwd=self.folder,
                                capture_output=True)
        self.assertEqual(legacy.returncode, 0, legacy.stderr)
        self.assertNotIn(b'Read via MCP', legacy.stdout)

    def test_explicit_input_uses_target_project_settings_from_another_repository(self):
        target, unrelated = self.folder / 'target', self.folder / 'unrelated'
        for directory in (target, unrelated):
            directory.mkdir()
            subprocess.run(['git', 'init', '-q', str(directory)], check=True)
        record = target / 'PROVENANCE.yaml'
        record.write_text(NATIVE)
        profile = target / 'navigation.json'
        profile.write_text(json.dumps({'groups': {'observations': ['m.reading']}}))
        for directory, python in ((target, sys.executable), (unrelated, str(unrelated / 'missing-python'))):
            (directory / '.git' / 'kpopper-session.json').write_text(json.dumps({
                'schema': 1, 'enabled': True, 'python': python, 'tokens': 1000,
                'project': directory.name, 'state': str(directory / 'pending'),
                'profile': str(directory / 'navigation.json')}))
        environment = dict(os.environ)
        environment.pop('KPOPPER_SESSION_CONFIG', None)
        environment.pop('KPOPPER_SESSION_BOOTSTRAPPED', None)
        result = subprocess.run([sys.executable, str(ROOT / 'scripts' / 'session_cli.py'),
            'hook-open', '--input=' + str(record), '--tokens', '1000'],
            cwd=unrelated, env=environment, text=True, encoding='utf-8', capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        _, command = self.read_hint(result.stdout)
        self.assertEqual(command[command.index('--project') + 1], 'target')
        self.assertEqual(Path(command[command.index('--state') + 1]), target / 'pending')
        self.assertEqual(Path(command[command.index('--profile') + 1]), profile)
        self.assertNotIn(str(unrelated / 'pending'), command)

    def test_checked_hook_finds_root_record_from_repository_subdirectory(self):
        repository = self.folder / 'repository'
        child = repository / 'src'
        child.mkdir(parents=True)
        subprocess.run(['git', 'init', '-q', str(repository)], check=True)
        (repository / 'PROVENANCE.yaml').write_text(NATIVE)
        (repository / '.git' / 'kpopper-session.json').write_text(json.dumps({
            'schema': 1, 'enabled': True, 'python': sys.executable, 'tokens': 1000,
            'project': 'nested-project', 'state': str(repository / 'pending')}))
        environment = dict(os.environ)
        environment.pop('KPOPPER_SESSION_CONFIG', None)
        hook = subprocess.run(['sh', str(ROOT / 'scripts' / 'session_open.sh')], input='{}',
            cwd=child, env=environment, text=True, encoding='utf-8', capture_output=True)
        self.assertEqual(hook.returncode, 0, hook.stderr)
        self.assertIn('revision=', hook.stdout, hook.stderr)
        _, command = self.read_hint(hook.stdout)
        self.assertEqual(command[command.index('--project') + 1], 'nested-project')
        self.assertEqual(Path(command[command.index('--state') + 1]), repository / 'pending')

        external = self.folder / 'external'
        external.mkdir()
        subprocess.run(['git', 'init', '-q', str(external)], check=True)
        external_record = external / 'PROVENANCE.yaml'
        (repository / 'PROVENANCE.yaml').rename(external_record)
        (repository / '.git' / 'kpopper-record').write_text(str(external_record) + '\n')
        config_path = repository / '.git' / 'kpopper-session.json'
        config = json.loads(config_path.read_text())
        config['state'] = str(repository / 'registered-pending')
        config_path.write_text(json.dumps(config))
        (external / '.git' / 'kpopper-session.json').write_text(json.dumps({
            'schema': 1, 'enabled': True, 'python': str(external / 'missing-python'),
            'tokens': 1000, 'project': 'wrong-project', 'state': str(external / 'pending'),
            'profile': str(external / 'missing-profile.json')}))
        hook = subprocess.run(['sh', str(ROOT / 'scripts' / 'session_open.sh')], input='{}',
            cwd=child, env=environment, text=True, encoding='utf-8', capture_output=True)
        self.assertEqual(hook.returncode, 0, hook.stderr)
        self.assertIn('revision=', hook.stdout, hook.stderr)
        hint, command = self.read_hint(hook.stdout)
        self.assertEqual(command[command.index('--project') + 1], 'nested-project')
        self.assertEqual(Path(command[command.index('--state') + 1]), repository / 'registered-pending')
        self.assertNotIn('wrong-project', command)
        revision = re.search(r'revision=([a-f0-9]{64})', hook.stdout).group(1)
        hint = hint.replace(' --ref REF --revision REV_FROM_ABOVE',
                            ' --ref checked:d.choice --revision ' + revision)
        replay = subprocess.run(['sh', '-c', hint], cwd=self.folder, env=environment,
                                text=True, encoding='utf-8', capture_output=True)
        self.assertEqual(replay.returncode, 0, replay.stderr)
        self.assertTrue(json.loads(replay.stdout)['value']['premise_changed'])

    @unittest.skipUnless(importlib.util.find_spec('mcp'), 'install the MCP session dependency')
    def test_real_mcp_transport_outside_checkout(self):
        from mcp import Client
        from mcp.client.stdio import StdioServerParameters
        (self.folder / 'settings.json').write_text(json.dumps({
            'schema': 1, 'enabled': True, 'python': str(self.folder / 'missing-python'), 'tokens': 1000}))
        async def scenario():
            params = StdioServerParameters(command=sys.executable,
                args=[str(ROOT / 'scripts' / 'session_cli.py'), 'serve', '--input', str(self.native),
                      '--project', 'example', '--state', str(self.folder / 'mcp-state')], cwd=self.folder)
            async with Client(params) as client:
                def text(result):
                    self.assertFalse(result.is_error, str(result))
                    return ''.join(part.text for part in result.content if part.type == 'text')
                opening = text(await client.call_tool('kpopper_open', {'tokens': 700}))
                import re
                revision = re.search(r'revision=([a-f0-9]{64})', opening).group(1)
                bundle = json.loads(text(await client.call_tool('kpopper_read', {
                    'ref': 'checked:d.choice', 'revision': revision, 'tokens': 1600})))['value']
                self.assertEqual(bundle['recorded_status'], 'Superseded pending a new investigation.')
                rejected = await client.call_tool('kpopper_verify_claims', {
                    'judgment': 'd.choice', 'revision': revision,
                    'assertions': [{'kind': 'current', 'id': 'm.reading', 'expected': 1}]})
                self.assertTrue(rejected.is_error)
                proposal = json.loads(text(await client.call_tool('kpopper_propose', {
                    'revision': revision, 'kind': 'question', 'text': 'What should be observed next?',
                    'basis': []})))
                self.assertFalse(proposal['canonical_record_changed'])
                self.assertEqual(self.native.read_text(), NATIVE)
                self.native.write_text(NATIVE.replace('v: 3', 'v: 4'))
                stale = await client.call_tool('kpopper_read', {'ref': 'd.choice', 'revision': revision})
                self.assertTrue(stale.is_error)
        asyncio.run(scenario())


class SetupContract(unittest.TestCase):
    def test_relative_toolchain_cache_reuse_and_explicit_repair(self):
        from scripts.session import core
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, {'KPOPPER_CORE_CACHE': directory}):
            def compile_file(command, cwd, **kwargs):
                self.assertTrue(Path(command[0]).is_absolute())
                target = Path(cwd)
                if command[-1] == 'Main.lean':
                    (target / 'Main.c').write_text('compiled fixture')
                    (target / 'Main.olean').write_bytes(b'fixture')
                else:
                    (target / core.binary_name()).write_bytes(b'fixture binary')
                return subprocess.CompletedProcess(command, 0)
            with patch.object(core.subprocess, 'check_output', return_value='Lean (version 4.33.1, test)'), \
                    patch.object(core.platform, 'platform', return_value='test-platform'), \
                    patch.object(core.subprocess, 'run', side_effect=compile_file):
                built = core.setup('relative-toolchain')
                self.assertTrue(Path(built['cache']).is_dir())
                with patch.object(core.shutil, 'which', return_value=None) as search:
                    self.assertEqual(core.setup()['cache'], built['cache'])
                    search.assert_not_called()
                cache = Path(built['cache'])
                (cache / 'build.json').write_text('{}')
                with self.assertRaisesRegex(ValueError, '--rebuild'):
                    core.setup('relative-toolchain')
                repaired = core.setup('relative-toolchain', rebuild=True)
                self.assertTrue(Path(repaired['previous_cache']).is_dir())
                self.assertEqual(core.Core().build['source_sha256'], core.source_hash())


if __name__ == "__main__":
    unittest.main()
