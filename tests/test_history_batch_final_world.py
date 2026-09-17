"""Final-world batches preserve admission, historical evidence and old envelopes."""
import base64
import copy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock
import zlib

from scripts import history_authoring as A, history_contract as C, history_transaction as T
from scripts import provenance as P
from scripts.reasoning.contract import digest
from tests import test_history_authoring as fixture
from tests import test_history_store as storage


def objects(mutation):
    return [C.decode_document(item['after']) for item in mutation.files if item['role'] == 'history_object']


def forged_adapter(mutation, adapter):
    data = mutation.to_data()
    receipt = copy.deepcopy(data['receipt'])
    for role in ('before', 'after'):
        for report in (receipt[role].get('assessment'), receipt[role].get('proposal', {}).get('assessment')):
            if report is None:
                continue
            for result in A._computations(report):
                if result and result.get('implementation'):
                    result['implementation']['adapter_source_sha256'] = adapter
                    result['assurance']['implementation'] = digest(result['implementation'])
            report['assessment_revision'] = digest({k: v for k, v in report.items() if k != 'assessment_revision'})
    receipt = T.semantic_receipt(**{k: receipt[k] for k in ('profile', 'capabilities', 'before', 'after')})
    files = mutation.files
    manifest_file = next(item for item in files if item['role'] == 'history_commit')
    manifest = C.decode_document(manifest_file['after']); manifest['receipt'] = receipt
    manifest_file['after'] = C.encode_document(manifest)
    return T.PreparedMutation(operation=data['operation'], authority=data['authority'], baseline=data['baseline'],
                              files=files, receipt=receipt, entry=data['entry'])


class FinalWorld(unittest.TestCase):
    def setUp(self):
        self.fixture = fixture.Authoring()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.entry, self.store = self.fixture.entry, self.fixture.store

    def batch(self, actions, **kwargs):
        return A.prepare_batch(self.entry, actions, recorded_at='2026-09-17T12:00:00Z', **kwargs)

    def test_consistent_whole_receipt_adapter_forgery_never_becomes_its_own_witness(self):
        mutations = [
            A.prepare(self.entry, {'kind': 'add', 'id': 'p.single', 'body': {'rule': {'expr': 'p.input + 1'}}}),
            self.batch([{'kind': 'add', 'id': 'p.batch', 'body': {'rule': {'expr': 'p.input * 2'}}}]),
            A.prepare_act(self.entry, {'kind': 'accept', 'id': 'p.input', 'of': self.fixture.original['id'],
                                      'over': [], 'because': 'explicit acceptance'}),
            A.prepare_proposal(self.entry, 'p.proposal', {'v': 2}, 'readings', because='proposal')]
        before = self.store.capture().inventory
        for mutation in mutations:
            forged = forged_adapter(mutation, 'f' * 64)
            receipt = forged.to_data()['receipt']
            with self.subTest(version=receipt['before']['authoring']['version']):
                with self.assertRaisesRegex(C.HistoryError, 'unknown_retained_adapter_audit'):
                    A.verify_prepared(self.entry, forged)
                with self.assertRaisesRegex(C.HistoryError, 'unknown_retained_adapter_audit'):
                    A.commit(self.entry, forged, verify=lambda data: None)
                self.assertEqual(self.store.capture().inventory, before)

    def test_already_committed_self_receipt_cannot_bootstrap_unknown_adapter_trust(self):
        mutation = A.prepare(self.entry, {'kind': 'add', 'id': 'p.self', 'body': {'v': 2}})
        forged = forged_adapter(mutation, 'e' * 64)
        # A raw fixture writer deliberately publishes the adversarial manifest.
        # A's replay trust must still exclude the operation's own receipt.
        self.store.commit(forged, verify=lambda data: None)
        before = self.store.capture().inventory
        with self.assertRaisesRegex(C.HistoryError, 'unknown_retained_adapter_audit'):
            A.verify_prepared(self.entry, forged)
        self.assertEqual(self.store.capture().inventory, before)

    def test_noncausal_sibling_does_not_witness_pending_operation_after_adapter_restart(self):
        import shutil
        import subprocess
        import sys
        pending = self.batch([{'kind': 'add', 'id': 'p.pending', 'body': {'v': 2}}])
        sibling = A.prepare(self.entry, {'kind': 'add', 'id': 'p.sibling', 'body': {'v': 3}})
        A.commit(self.entry, sibling, verify=lambda data: None)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            shutil.copytree(Path(A.__file__).resolve().parent, root / 'scripts',
                            ignore=shutil.ignore_patterns('__pycache__', '*.pyc'))
            source = root / 'scripts/reasoning/snapshot.py'
            source.write_bytes(source.read_bytes() + b'\n# Restarted adapter audit for sibling exclusion.\n')
            shutil.copytree(self.entry.parent, root / 'record')
            (root / 'pending.json').write_bytes(pending.to_bytes())
            code = """import sys
from pathlib import Path
sys.path.insert(0,sys.argv[1])
from scripts import history_authoring as A,history_contract as C,history_transaction as T
root=Path(sys.argv[1]); mutation=T.PreparedMutation.from_bytes((root/'pending.json').read_bytes())
try: A.verify_prepared(root/'record/GROUNDING.yaml',mutation)
except C.HistoryError as error:
 assert error.code=='unknown_retained_adapter_audit', str(error)
else: raise AssertionError('noncausal sibling was accepted as an audit witness')
"""
            result = subprocess.run([sys.executable, '-B', '-c', code, str(root)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_current_adapter_witness_does_not_relax_semantic_native_protocol_or_library_checks(self):
        mutation = self.batch([{'kind': 'add', 'id': 'p.new', 'body': {'v': 3}}])
        before = self.store.capture().inventory
        for change in ('semantic', 'protocol', 'native', 'library'):
            with self.subTest(change=change):
                modified = RetainedSourceOracle.tampered(self, mutation, change)
                with self.assertRaisesRegex(C.HistoryError, 'authoring_receipt_mismatch|authoring_native_audit_mismatch'):
                    A.verify_prepared(self.entry, modified)
                self.assertEqual(self.store.capture().inventory, before)

    def test_repeated_subject_preserves_each_admitted_claim_and_review_pins_final_version(self):
        reading = fixture.claim('p.multi', op='multi', body={'v': 1, 'of': '2026-09-14'})
        judgment = fixture.claim('d.multi', kind='judgment', op='multi-judgment', body={
            'verdict': 'safe', 'rests_on': ['p.multi'], 'wrong_if': {'expr': 'p.multi > 5'},
            'seen': {'p.multi': 1}}, pins={'p.multi': reading['id']})
        self.fixture.fixture.publish([reading, judgment], op='multi-bootstrap')
        mutation = self.batch([
            {'kind': 'set', 'id': 'p.multi', 'value': 2, 'as_of': '2026-09-15'},
            {'kind': 'review', 'id': 'd.multi'},
            {'kind': 'set', 'id': 'p.multi', 'value': 3, 'as_of': '2026-09-16'}])
        self.assertEqual(mutation.to_data()['receipt']['before']['authoring']['version'], 6)
        made = objects(mutation)
        readings = {obj['body']['v']: obj for obj in made if obj['kind'] == 'reading'}
        self.assertEqual(set(readings), {2, 3})
        self.assertIn(readings[2]['id'], readings[3]['saw'])
        review = next(obj for obj in made if obj['kind'] == 'act' and obj['body']['act'] == 'review')
        self.assertEqual(review['body']['of'], judgment['id'])
        self.assertEqual(review['body']['read'], {'p.multi': readings[3]['id']})
        A.commit(self.entry, mutation, verify=lambda data: None)
        self.assertEqual(self.store.capture().objects[judgment['id']], judgment)
        self.assertEqual(self.store.state()['subjects']['p.multi']['body']['v'], 3)

    def test_final_world_cannot_mask_same_day_replacement_or_invalid_source(self):
        before = self.store.capture().inventory
        cases = [
            ([{'kind': 'set', 'id': 'p.input', 'value': 2, 'as_of': '2026-09-16'}], 'same day'),
            ([{'kind': 'set', 'id': 'p.input', 'value': 2, 'as_of': '2026-09-17'},
              {'kind': 'set', 'id': 'p.input', 'value': 3, 'as_of': '2026-09-17'}], 'same day'),
            ([{'kind': 'set', 'id': 'p.input', 'value': 2, 'as_of': '2026-09-17',
               'source': 'p.future', 'at': 'line 1'},
              {'kind': 'add', 'id': 'p.future', 'body': {'v': 3}}], 'not a recorded source'),
            ([{'kind': 'add', 'id': 'p.input', 'body': {'v': 1}},
              {'kind': 'set', 'id': 'p.input', 'value': 2, 'as_of': '2026-09-17'}], 'already an entry')]
        for actions, reason in cases:
            with self.subTest(reason=reason), self.assertRaisesRegex(P.Refused, reason):
                self.batch(actions)
            self.assertEqual(self.store.capture().inventory, before)

    def test_final_candidate_does_not_grant_standing_judgment_replacement_or_review_scope(self):
        old = fixture.claim('d.standing', kind='judgment', op='standing', body={
            'verdict': 'original', 'rests_on': ['p.input'], 'wrong_if': {'expr': 'p.input < 0'},
            'seen': {'p.input': 1}}, pins={'p.input': self.fixture.original['id']})
        self.fixture.fixture.publish([old], op='standing-bootstrap')
        before = self.store.capture().inventory
        with self.assertRaisesRegex(P.Refused, 'standing judgment|already a judgment'):
            self.batch([{'kind': 'add', 'id': 'd.standing', 'body': {'verdict': 'replacement',
                         'rests_on': ['p.input'], 'wrong_if': {'expr': 'p.input > 5'}}},
                        {'kind': 'set', 'id': 'p.input', 'value': 2, 'as_of': '2026-09-17'}])
        with self.assertRaisesRegex(C.HistoryError, 'history_review_scope_change_requires_claim'):
            self.batch([{'kind': 'review', 'id': 'd.standing', '_record_scope': {'kind': 'project'}}])
        self.assertEqual(self.store.capture().inventory, before)
        self.assertEqual(self.store.capture().objects[old['id']], old)

    def test_valid_source_introduced_later_and_readable_formula_use_final_names(self):
        mutation = self.batch([
            {'kind': 'add', 'id': 'p.total', 'body': {'rule': 'p.future + 1'}},
            {'kind': 'set', 'id': 'p.input', 'value': 2, 'as_of': '2026-09-17', 'source': 's.new', 'at': 'page 2'},
            {'kind': 'add', 'id': 'p.future', 'body': {'v': 4}},
            {'kind': 'add', 'id': 's.new', 'body': {'file': 'source.txt'}}])
        made = objects(mutation)
        total = next(obj for obj in made if obj['subject'] == 'p.total' and obj['kind'] != 'act')
        self.assertEqual(total['body']['rule'], {'expr': 'p.future + 1'})
        value = mutation.to_data()['receipt']['after']['assessment']['nodes']['p.total']['computation']['value']
        self.assertEqual(value, {'type': 'number', 'numerator': '5', 'denominator': '1'})

    def test_cyclic_new_pin_identity_is_explicit_and_never_backfills_seen(self):
        before = self.store.capture().inventory
        actions = [{'kind': 'add', 'id': name, 'into': 'judgments', 'body': {
            'verdict': 'waiting', 'rests_on': [dependency], 'blocked_on': 'no scalar evidence'}}
            for name, dependency in [('d.one', 'd.two'), ('d.two', 'd.one')]]
        with self.assertRaisesRegex(C.HistoryError, 'cyclic_batch_pin_dependencies'):
            self.batch(actions)
        self.assertEqual(self.store.capture().inventory, before)

    def test_manifest_crash_retry_uses_exact_version_six_envelope(self):
        mutation = self.batch([{'kind': 'add', 'id': 'd.new', 'into': 'judgments', 'body': {
            'verdict': 'ready', 'rests_on': ['p.future'], 'wrong_if': {'expr': 'p.future > 5'}}},
            {'kind': 'add', 'id': 'p.future', 'body': {'v': 2}}])
        raw = mutation.to_bytes()
        with mock.patch.object(T, '_replace', side_effect=OSError('interruption')):
            with self.assertRaises(OSError):
                A.commit(self.entry, mutation, verify=lambda data: None)
        A.commit(self.entry, T.PreparedMutation.from_bytes(raw), verify=lambda data: None)
        A.commit(self.entry, T.PreparedMutation.from_bytes(raw), verify=lambda data: None)
        self.assertEqual(len(self.store.capture().commits), 2)
        self.assertEqual(mutation.to_bytes(), raw)

    def test_single_act_proposal_and_v6_replay_after_actual_adapter_only_restart(self):
        import shutil
        import subprocess
        import sys
        witness = A.prepare(self.entry, {'kind': 'add', 'id': 'p.audit_witness', 'body': {'v': 0}})
        A.commit(self.entry, witness, verify=lambda data: None)
        mutations = {
            'single': A.prepare(self.entry, {'kind': 'set', 'id': 'p.input', 'value': 2, 'as_of': '2026-09-17'}),
            'act': A.prepare_act(self.entry, {'kind': 'accept', 'id': 'p.input',
                'of': self.fixture.original['id'], 'over': [], 'because': 'explicit retained act'}),
            'proposal': A.prepare_proposal(self.entry, 'p.proposed', {'v': 4}, 'readings', because='retained proposal'),
            'batch': self.batch([{'kind': 'add', 'id': 'p.new', 'body': {'v': 3}}]),
            'batch-v2': self.batch([{'kind': 'add', 'id': 'p.old_v2', 'body': {'v': 2}}], _receipt_version=2),
            'batch-v3': self.batch([{'kind': 'add', 'id': 'p.old_v3', 'body': {'v': 3}}], _receipt_version=3)}
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = Path(A.__file__).resolve().parent
            shutil.copytree(source, root / 'scripts', ignore=shutil.ignore_patterns('__pycache__', '*.pyc'))
            changed = root / 'scripts/reasoning/snapshot.py'
            changed.write_bytes(changed.read_bytes() + b'\n# Adapter-only source revision for restart replay regression.\n')
            for name, mutation in mutations.items():
                shutil.copytree(self.entry.parent, root / 'records' / name)
                (root / (name + '.json')).write_bytes(mutation.to_bytes())
            code = """import sys
from pathlib import Path
sys.path.insert(0, sys.argv[1])
from scripts import history_authoring as A, history_transaction as T
root = Path(sys.argv[1])
for name in ('single', 'act', 'proposal', 'batch', 'batch-v2', 'batch-v3'):
 raw = (root / (name + '.json')).read_bytes()
 mutation = T.PreparedMutation.from_bytes(raw)
 entry = root / 'records' / name / 'GROUNDING.yaml'
 A.commit(entry, mutation, verify=lambda data: None)
 A.commit(entry, mutation, verify=lambda data: None)
 assert mutation.to_bytes() == raw
print('all retained envelopes replayed byte-exact')
"""
            result = subprocess.run([sys.executable, '-B', '-c', code, str(root)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn('all retained envelopes replayed byte-exact', result.stdout)

    def test_legacy_profile_final_world_and_admission_remain_named(self):
        f = storage.Storage(); f.setUp(); self.addCleanup(f.doCleanups)
        document = C.decode_document(f.entry.read_bytes()); document['meta'].pop('reasoning')
        f.entry.write_bytes(C.encode_document(document))
        reading = fixture.claim(body={'v': 1, 'of': '2026-09-16'})
        judgment = fixture.claim('d.old', kind='judgment', op='old', body={
            'verdict': 'safe', 'rests_on': ['p.input'], 'wrong_if': 'p.input < 0', 'seen': {'p.input': 0}},
            pins={'p.input': reading['id']})
        reading['authored']['profile'] = judgment['authored']['profile'] = 'ordinary-reader/v1'
        reading['id'] = C.object_identity(reading)
        judgment['pins']['p.input'] = reading['id']; judgment['id'] = C.object_identity(judgment)
        f.publish([reading, judgment], op='legacy-bootstrap')
        mutation = A.prepare_batch(f.entry, [
            {'kind': 'add', 'id': 'd.new', 'into': 'judgments', 'body': {'verdict': 'new value',
             'rests_on': ['p.input'], 'wrong_if': 'p.input < 2'}},
            {'kind': 'set', 'id': 'p.input', 'value': 2, 'as_of': '2026-09-17'}])
        self.assertEqual(mutation.to_data()['receipt']['profile'], 'ordinary-reader/v1')
        A.commit(f.entry, mutation, verify=lambda data: None)
        self.assertEqual(f.store.capture().objects[judgment['id']], judgment)
        with self.assertRaisesRegex(P.Refused, 'same day'):
            A.prepare_batch(f.entry, [{'kind': 'set', 'id': 'p.input', 'value': 3, 'as_of': '2026-09-17'}])


class RetainedSourceOracle(unittest.TestCase):
    """Golden bytes were generated by immutable efb5b6d, never the current writer."""
    def setUp(self):
        source = json.loads((Path(__file__).parent / 'fixtures/history_batches_efb5b6d.json').read_text())
        self.assertEqual(source['source_commit'], 'efb5b6d47456e4fe8f2ee93310f1f1d7f8cdf1dd')
        raw = zlib.decompress(base64.b64decode(source['payload']))
        self.assertEqual(hashlib.sha256(raw).hexdigest(), source['payload_sha256'])
        self.cases = json.loads(raw)
        self.target = source['native_target']
        temporary = tempfile.TemporaryDirectory(); self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)

    def case(self, version):
        root = self.root / str(version); root.mkdir()
        case = self.cases[str(version)]
        for name, encoded in case['files'].items():
            path = root / name; path.parent.mkdir(parents=True, exist_ok=True); path.write_bytes(bytes.fromhex(encoded))
        return root / 'GROUNDING.yaml', T.PreparedMutation.from_bytes(bytes.fromhex(case['mutation']))

    def test_unwitnessed_pinned_source_v2_v3_refuses_upgrade_without_rewriting_golden(self):
        for version in (2, 3):
            with self.subTest(version=version):
                entry, mutation = self.case(version)
                raw = mutation.to_bytes()
                from scripts import history_store as H
                before = H.Store(entry).capture()
                self.assertEqual({operation: A._recorded_adapter_audit(C.decode_document(data)['receipt'])
                                  for operation, data in before.commits.items()}, {'bootstrap': {}})
                with self.assertRaisesRegex(C.HistoryError, 'unknown_retained_adapter_audit'):
                    A.verify_prepared(entry, mutation)
                with self.assertRaisesRegex(C.HistoryError, 'unknown_retained_adapter_audit'):
                    A.commit(entry, mutation, verify=lambda data: None)
                self.assertEqual(mutation.to_bytes(), raw)
                self.assertEqual(H.Store(entry).capture().inventory, before.inventory)

    def tampered(self, mutation, change):
        data = mutation.to_data(); receipt = data['receipt']
        report = receipt['after']['assessment']
        result = report['nodes']['p.new']['computation']
        if change == 'semantic':
            result['value']['numerator'] = '999'
        else:
            implementation = result['implementation']
            if change == 'protocol': implementation['protocol'] = 'KP99'
            elif change == 'native': implementation['binary_sha256'] = '0' * 64
            else: implementation['libraries'] = {'other': '0' * 64}
            result['assurance']['implementation'] = digest(implementation)
        report['assessment_revision'] = digest({k: v for k, v in report.items() if k != 'assessment_revision'})
        receipt = T.semantic_receipt(**{k: receipt[k] for k in ('profile', 'capabilities', 'before', 'after')})
        files = mutation.files; manifest_file = next(item for item in files if item['role'] == 'history_commit')
        manifest = C.decode_document(manifest_file['after']); manifest['receipt'] = receipt
        manifest_file['after'] = C.encode_document(manifest)
        return T.PreparedMutation(operation=data['operation'], authority=data['authority'], baseline=data['baseline'],
                                  files=files, receipt=receipt)

    def test_recomputed_outer_hashes_do_not_hide_semantic_or_native_tampering(self):
        for index, change in enumerate(('semantic', 'protocol', 'native', 'library')):
            with self.subTest(change=change):
                entry, mutation = self.case(2 if index % 2 == 0 else 3)
                modified = self.tampered(mutation, change)
                with self.assertRaisesRegex(C.HistoryError, 'unknown_retained_adapter_audit'):
                    A.verify_prepared(entry, modified)
                # Each subsequent case needs a fresh destination with the same source bytes.
                import shutil; shutil.rmtree(entry.parent)


if __name__ == '__main__': unittest.main()
