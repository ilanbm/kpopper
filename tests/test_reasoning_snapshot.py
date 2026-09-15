"""Strict capture witnesses the whole source closure and portable read context."""
import copy
import datetime
import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from scripts import provenance as P
from scripts.reasoning.snapshot import Snapshot, SnapshotError, SnapshotView
from tests.test_pending_grounding import Repository, fixture_bundle


def source():
    return {'schema': {'deps': 'rests_on', 'snapshot': 'reviewed', 'predicate': 'wrong_if'},
            'items': {'a': {'v': 1}, 'b': {'v': 2}},
            'scopes': {'scope.items': {'collection_scope': {'collection': 'items', 'fields': ['absent', 'v']},
                                       'scope': {'kind': 'project', 'environment': 'fixture', 'commit': 'abc'}}}}


class SnapshotTests(unittest.TestCase):
    def test_identity_detachment_and_types(self):
        document = source()
        snapshot = Snapshot.from_data(document)
        document['items']['a']['v'] = 90
        snapshot.to_data()['document']['items']['a']['v'] = 80
        self.assertEqual(snapshot.to_data()['document']['items']['a']['v'], 1)
        ids = set()
        for value in [False, 0, 0.0, '0', None]:
            document['items']['a']['v'] = value
            ids.add(Snapshot.from_data(document).snapshot_id)
        self.assertEqual(len(ids), 5)

    def test_replay_validates_schema_and_digest(self):
        data = Snapshot.from_data(source()).to_data()
        self.assertEqual(Snapshot.from_snapshot(data).to_data(), data)
        data['document']['items']['a']['v'] = 100
        data['nodes']['a']['body']['v'] = 100
        with self.assertRaisesRegex(SnapshotError, 'snapshot'):
            Snapshot.from_snapshot(data)

    def test_completed_format_rewrite_keeps_semantic_identity_separate(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'GROUNDING.yaml'
            path.write_text('known:\n  a: {v: 1}\n')
            before = Snapshot.capture([str(path)], read_mode='frozen')
            path.write_text('# comment and layout only\nknown: {a: {v: 1}}\n')
            after = Snapshot.capture([str(path)], read_mode='frozen')
            self.assertEqual(before.snapshot_id, after.snapshot_id)
            self.assertNotEqual(before.to_data()['authored_revision']['digest'],
                                after.to_data()['authored_revision']['digest'])

    def test_authored_revision_is_independently_validated_on_replay(self):
        from scripts.reasoning.contract import digest
        supplied = Snapshot.from_data(source()).to_data()
        valid = {'files': [{'origin': 'origin:GROUNDING.yaml', 'status': 'read', 'sha256': 'a' * 64}]}
        valid['digest'] = digest(valid)
        supplied['authored_revision'] = valid
        self.assertEqual(Snapshot.from_snapshot(supplied).snapshot_id, supplied['snapshot_id'])
        invalid_revisions = [False, {}, {'files': [], 'digest': 'a' * 64},
                             dict(valid, extra=True), dict(valid, files='invalid'),
                             {'files': [{'origin': 'origin:GROUNDING.yaml', 'status': 'read', 'sha256': None}]}]
        malformed = copy.deepcopy(valid)
        malformed['files'][0]['status'] = 'unknown'
        malformed['digest'] = digest({'files': malformed['files']})
        invalid_revisions.append(malformed)
        stale = copy.deepcopy(valid)
        stale['files'][0]['sha256'] = 'b' * 64
        invalid_revisions.append(stale)
        for revision in invalid_revisions:
            with self.subTest(revision=revision):
                invalid = copy.deepcopy(supplied)
                invalid['authored_revision'] = revision
                with self.assertRaisesRegex(SnapshotError, 'revision'):
                    Snapshot.from_snapshot(invalid)

    def test_snapshot_identity_still_binds_history_and_context(self):
        document = source()
        original = Snapshot.from_data(document).snapshot_id
        document['items']['a']['reviewed'] = {'prior': 1}
        history = Snapshot.from_data(document).snapshot_id
        self.assertNotEqual(original, history)
        self.assertNotEqual(history, Snapshot.from_data(document, context={
            'read_mode': 'supplied', 'generation': 1}).snapshot_id)

    def test_scope_definition_fields_must_be_sorted_and_unique(self):
        for fields in (['v', 'absent'], ['v', 'v']):
            document = source()
            document['scopes']['scope.items']['collection_scope']['fields'] = fields
            with self.assertRaisesRegex(SnapshotError, 'invalid_scope'):
                Snapshot.from_data(document).capture_scope('scope.items')

    def test_node_view_copies_snapshot_once_and_keeps_returned_values_detached(self):
        from scripts.reasoning import contract
        snapshot = Snapshot.from_data(source())
        original_to_data = Snapshot.to_data
        with mock.patch.object(Snapshot, 'to_data', autospec=True, side_effect=original_to_data) as copies, \
                mock.patch.object(contract, 'node_basis', wraps=contract.node_basis) as bases:
            view = SnapshotView(snapshot, nodes=['a', 'b'])
            first = view.read_node('a')
            first['body']['v'] = 99
            first['fields']['snapshot'] = 'changed'
            snapshot.to_data()['nodes']['a']['body']['v'] = 88
            repeated = view.read_node('a')
            view.read_node('b')
            self.assertEqual(repeated['body']['v'], 1)
            self.assertEqual(repeated['fields']['snapshot'], 'reviewed')
            # One view initialization copy plus the explicit caller export above.
            self.assertEqual(copies.call_count, 2)
            self.assertEqual(bases.call_count, 2)
            witnesses = view.executed_reads
            witnesses[0]['fingerprint'] = 'changed'
            self.assertNotIn('changed', [item['fingerprint'] for item in view.executed_reads])
            self.assertEqual(len(view.executed_reads), 2)

    def test_scope_view_reuses_capture_and_detaches_each_field_read(self):
        document = source()
        document['items']['a']['v'] = {'nested': [1]}
        snapshot = Snapshot.from_data(document)
        original_capture = Snapshot.capture_scope
        with mock.patch.object(Snapshot, 'capture_scope', autospec=True, side_effect=original_capture) as captures:
            view = SnapshotView(snapshot, scopes=['scope.items'])
            first = view.read_scope('scope.items', 'v')
            first[0]['value']['nested'][0] = 99
            missing = view.read_scope('scope.items', 'absent')
            repeated = view.read_scope('scope.items', 'v')
            self.assertEqual(repeated[0]['value']['nested'], [1])
            self.assertEqual(missing[0]['status'], 'missing')
            self.assertEqual(captures.call_count, 1)
            self.assertEqual(len(view.executed_reads), 1)
            with self.assertRaisesRegex(SnapshotError, 'undeclared_dependency'):
                view.read_scope('scope.items', 'secret')
            self.assertEqual(captures.call_count, 1)

    def test_typed_json_roundtrip_preserves_yaml_types_without_source_io(self):
        from scripts.reasoning import snapshot as S
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'GROUNDING.yaml'
            path.write_text("known:\n  p.date: {v: 2026-09-15}\n"
                            "  p.timestamp: {v: 2026-09-15T12:34:56+03:00}\n"
                            "  p.bool: {v: false}\n  p.int: {v: 0}\n  p.float: {v: 0.0}\n"
                            "  p.null: {v: null}\n  p.text: {v: '2026-09-15'}\n")
            snapshot = Snapshot.capture([str(path)], read_mode='frozen')
            serialized = snapshot.to_json()
            self.assertIsInstance(serialized, str)
            path.unlink()
            with mock.patch.object(P, 'load', side_effect=AssertionError('must not read')), \
                    mock.patch.object(S, '_observation', side_effect=AssertionError('must not observe')):
                replay = Snapshot.from_json(serialized)
                binary_replay = Snapshot.from_json(serialized.encode('utf-8'))
            self.assertEqual(snapshot.snapshot_id, replay.snapshot_id)
            self.assertEqual(snapshot.to_data(), binary_replay.to_data())
            expected = {'p.date': datetime.date, 'p.timestamp': datetime.datetime,
                        'p.bool': bool, 'p.int': int, 'p.float': float, 'p.null': type(None), 'p.text': str}
            for name, kind in expected.items():
                self.assertIs(type(replay.to_data()['nodes'][name]['body']['v']), kind)

    def test_typed_json_rejects_duplicate_keys_malformed_tags_and_limits(self):
        from scripts.reasoning import snapshot as S
        malformed = ['{"x":1,"x":2}', '["map",[["x",["null"]],["x",["null"]]]]',
                     '["bool",0]', '["null",0]', '["text",1]', '["int","01"]',
                     '["float","nan"]', '["date","2026-99-99"]', '["list",{}]',
                     '["map",[["x"]]]', '["unknown",0]', '["int"]', 'NaN', '[']
        for serialized in malformed:
            with self.subTest(serialized=serialized), self.assertRaises(SnapshotError):
                Snapshot.from_json(serialized)
        with self.assertRaises(SnapshotError):
            Snapshot.from_json(b'\xff')
        oversized = Snapshot.from_data(source())
        with mock.patch.object(S, 'MAX_REQUEST_BYTES', 16):
            with self.assertRaisesRegex(SnapshotError, 'limit'):
                Snapshot.from_json(' ' * 17)
            with self.assertRaisesRegex(SnapshotError, 'limit'):
                oversized.to_json()
        with self.assertRaisesRegex(SnapshotError, 'limit'):
            Snapshot.from_json(json.dumps(['int', '1' * 4097]))
        deep = '["null"]'
        for _ in range(130):
            deep = '["list",[' + deep + ']]'
        with self.assertRaisesRegex(SnapshotError, 'limit'):
            Snapshot.from_json(deep)

    def test_typed_json_replay_still_rejects_stale_snapshot_digest(self):
        from scripts.pending_grounding import _encode
        data = Snapshot.from_data(source()).to_data()
        data['document']['items']['a']['v'] = 99
        data['nodes']['a']['body']['v'] = 99
        with self.assertRaisesRegex(SnapshotError, 'stale_snapshot'):
            Snapshot.from_json(json.dumps(_encode(data)))

    def test_scope_candidate_membership_fields_and_grants(self):
        data = source()
        initial = Snapshot.from_data(data).capture_scope('scope.items')
        self.assertEqual(initial.value['fields']['member_count']['numerator'], '2')
        self.assertEqual(initial.view().read_scope('scope.items', 'absent')[0]['status'], 'missing')
        with self.assertRaisesRegex(SnapshotError, 'undeclared_dependency'):
            initial.view().read_scope('scope.items', 'secret')
        with self.assertRaisesRegex(SnapshotError, 'undeclared_dependency'):
            SnapshotView(Snapshot.from_data(data), nodes=['a']).read_node('b')
        data['items']['b']['v'] = 5
        changed = Snapshot.from_data(data).capture_scope('scope.items')
        self.assertNotEqual(initial.witness['projected_inputs_digest'], changed.witness['projected_inputs_digest'])
        data['items']['c'] = {'v': 0}
        added = Snapshot.from_data(data).capture_scope('scope.items')
        self.assertNotEqual(changed.witness['membership_digest'], added.witness['membership_digest'])
        del data['items']['a']
        removed = Snapshot.from_data(data).capture_scope('scope.items')
        self.assertNotEqual(added.witness['membership_digest'], removed.witness['membership_digest'])
        with self.assertRaisesRegex(SnapshotError, 'limit'):
            Snapshot.from_data(data).capture_scope('scope.items', limits={'members': 1})

    def test_mapped_historical_seen_is_excluded_from_scope_basis(self):
        doc = source()
        doc['items']['a']['reviewed'] = {'x': 1}
        first = Snapshot.from_data(doc).capture_scope('scope.items').basis
        doc['items']['a']['reviewed']['x'] = 2
        self.assertEqual(first, Snapshot.from_data(doc).capture_scope('scope.items').basis)

    def test_capture_frozen_hypotheses_and_replay(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'GROUNDING.yaml'
            path.write_text('known:\n  a: {v: 1}\n')
            hyp = path.parent / '.kpopper/hypotheses'
            hyp.mkdir(parents=True)
            (hyp / 'broken.yaml').write_text('known: [')
            snapshot = Snapshot.capture([str(path)], read_mode='frozen')
            data = snapshot.to_data()
            self.assertIsNotNone(data['hypotheses']['broken']['error'])
            self.assertEqual(len(data['authored_revision']['files']), 2)
            path.unlink()
            self.assertEqual(Snapshot.from_snapshot(data).snapshot_id, snapshot.snapshot_id)

    def test_supplied_hypotheses_contest_without_adopting_and_types_stay_distinct(self):
        document = source()
        one = {'left': {'doc': {'items': {'a': {'v': False}}}}}
        single = Snapshot.from_data(document, hypotheses=one)
        self.assertEqual(single.capture_scope('scope.items').candidates['a']['v']['status'], 'known')
        one['right'] = {'doc': {'items': {'a': {'v': 0}}}}
        contested = Snapshot.from_data(document, hypotheses=one)
        candidate = contested.capture_scope('scope.items').candidates['a']['v']
        self.assertEqual(candidate['status'], 'contested')
        self.assertEqual(candidate['value'], 1)
        self.assertEqual(len(candidate['alternatives']), 2)

    def test_contested_evidence_retains_history_without_fingerprinting_it(self):
        document = source()
        context = {'read_mode': 'supplied', 'conflicts': {'a': [
            ['left', {'v': 1, 'reviewed': {'old': 0}}], ['right', {'v': 2}]]}}
        before = Snapshot.from_data(document, context=context).capture_scope('scope.items')
        context['conflicts']['a'][0][1]['reviewed']['old'] = 99
        after = Snapshot.from_data(document, context=context).capture_scope('scope.items')
        self.assertEqual(before.witness, after.witness)
        self.assertNotEqual(before.candidates, after.candidates)

    def test_computed_field_basis_changes_with_transitive_input(self):
        document = source()
        document['items']['b'] = {'rule': {'expr': 'a * 0'}}
        # Identifier syntax uses dotted authored names.
        document['items']['p.a'] = document['items'].pop('a')
        document['items']['b']['rule']['expr'] = 'p.a * 0'
        before = Snapshot.from_data(document).capture_scope('scope.items').candidates['b']['v']['fingerprint']
        document['items']['p.a']['v'] = 42
        after = Snapshot.from_data(document).capture_scope('scope.items').candidates['b']['v']['fingerprint']
        self.assertNotEqual(before, after)

    def test_config_and_pending_observation_changes_refuse(self):
        from scripts.reasoning import snapshot as S
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'GROUNDING.yaml'
            path.write_text('known:\n  a: {v: 1}\n')
            stable = S._observation([str(path)], 'frozen')
            for mutation in ({'pending_ref': 'new-ref'}, {'config': dict(stable['config'], generation=1)}):
                changed = dict(stable, **mutation)
                with mock.patch.object(S, '_observation', side_effect=[stable, changed]):
                    with self.assertRaisesRegex(SnapshotError, 'snapshot_changed'):
                        Snapshot.capture([str(path)], read_mode='frozen')

    def test_unreadable_hypothesis_has_inventory_and_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'GROUNDING.yaml'
            path.write_text('known:\n  a: {v: 1}\n')
            hyp = path.parent / '.kpopper/hypotheses'
            hyp.mkdir(parents=True)
            denied = hyp / 'denied.yaml'
            denied.write_text('known: {}')
            original_open = P.io.open
            def deny(file, *args, **kwargs):
                if str(file) == str(denied):
                    raise PermissionError('denied')
                return original_open(file, *args, **kwargs)
            with mock.patch.object(P.io, 'open', deny):
                result = Snapshot.capture([str(path)], read_mode='frozen').to_data()
            self.assertEqual(result['hypotheses']['denied']['error'], 'PermissionError')
            self.assertIn('unreadable', [item['status'] for item in result['authored_revision']['files']])

    def test_captured_live_replay_has_no_live_io_and_differs_from_frozen(self):
        document = source()
        context = {'read_mode': 'captured-live', 'original_read_mode': 'live',
                   'pending': {'ref': 'fixed', 'bundles': {'one': {'document': {'items': {'a': {'v': 8}}}}}}}
        live = Snapshot.from_data(document, context=context)
        frozen = Snapshot.from_data(document, context={'read_mode': 'frozen'})
        self.assertNotEqual(live.snapshot_id, frozen.snapshot_id)
        with mock.patch.object(P, 'load', side_effect=AssertionError('replay must not load')):
            self.assertEqual(Snapshot.from_snapshot(live.to_data()).snapshot_id, live.snapshot_id)

    def test_scope_fields_are_literal_keys_and_empty_collection_is_known(self):
        document = source()
        document['items']['a']['nested.value'] = 8
        document['items']['a']['nested'] = {'value': 9}
        document['scopes']['scope.items']['collection_scope']['fields'] = ['nested.value']
        captured = Snapshot.from_data(document).capture_scope('scope.items')
        self.assertEqual(captured.candidates['a']['nested.value']['value'], 8)
        document['items'] = {}
        empty = Snapshot.from_data(document).capture_scope('scope.items')
        self.assertEqual(empty.value['fields']['member_count']['numerator'], '0')
        del document['items']
        with self.assertRaisesRegex(SnapshotError, 'scope_unavailable'):
            Snapshot.from_data(document).capture_scope('scope.items')

    def test_removed_pointer_source_or_same_yaml_rewrite_refuses(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'GROUNDING.yaml'
            shard = Path(tmp) / 'part.yaml'
            path.write_text('also: part.yaml\n')
            original = P.load
            for mutation in ('remove', 'rewrite'):
                shard.write_text('known:\n  a: {v: 1}\n')
                calls = []
                def changing(*args, **kwargs):
                    result = original(*args, **kwargs)
                    if not calls:
                        if mutation == 'remove':
                            shard.unlink()
                        else:
                            shard.write_text('# changed bytes; identical parsed value\nknown:\n  a: {v: 1}\n')
                    calls.append(1)
                    return result
                with mock.patch.object(P, 'load', changing):
                    with self.assertRaisesRegex(SnapshotError, 'snapshot_changed'):
                        Snapshot.capture([str(path)], read_mode='frozen')

    def test_source_addition_during_capture_refuses(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'GROUNDING.yaml'
            path.write_text('known:\n  a: {v: 1}\n')
            original = P.load
            calls = []
            def changing(*args, **kwargs):
                result = original(*args, **kwargs)
                if not calls:
                    hyp = path.parent / '.kpopper/hypotheses'
                    hyp.mkdir(parents=True)
                    (hyp / 'new.yaml').write_text('known:\n  b: {v: 2}\n')
                calls.append(1)
                return result
            with mock.patch.object(P, 'load', changing):
                with self.assertRaisesRegex(SnapshotError, 'snapshot_changed'):
                    Snapshot.capture([str(path)], read_mode='frozen')


class ReaderCapabilityTests(unittest.TestCase):
    def test_ordinary_reader_refuses_declared_core_but_capture_succeeds(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'GROUNDING.yaml'
            doc = {'meta': {'reasoning': {'version': 1, 'profile': 'core/v1', 'requires': ['arithmetic/v1']}},
                   'known': {'p.one': {'v': 1}}}
            path.write_text(P.yaml.safe_dump(doc))
            for read in (lambda: P.load([str(path)], read_mode='frozen'), lambda: P.check([str(path)])):
                with self.assertRaisesRegex(P.Refused, 'unsupported_capability: use core/v1 consumer'):
                    read()
            self.assertEqual(Snapshot.capture([str(path)], read_mode='frozen').to_data()['document'], doc)
            self.assertFalse(P._CORE_READS.get())
            with self.assertRaisesRegex(P.Refused, 'unsupported_capability'):
                P.load([str(path)], read_mode='frozen')

    def test_unknown_and_malformed_declarations_remain_raw_but_reads_refuse(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'GROUNDING.yaml'
            for declaration, code in [(None, 'invalid_capability'),
                    ({'version': 1, 'profile': 'core/v900', 'requires': ['arithmetic/v1']}, 'unsupported_capability')]:
                doc = {'meta': {'reasoning': declaration}, 'known': {'p.one': {'v': 1}}}
                path.write_text(P.yaml.safe_dump(doc))
                self.assertEqual(P.parse(path), doc)
                with self.assertRaisesRegex(P.Refused, code):
                    P.load([str(path)], read_mode='frozen')
                with self.assertRaisesRegex(P.Refused, code):
                    Snapshot.capture([str(path)], read_mode='frozen')
                self.assertFalse(P._CORE_READS.get())
                self.assertIsNone(P._CAPTURE_READS.get())

    def test_tagged_hypothesis_cannot_enter_ordinary_reader(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'GROUNDING.yaml'
            path.write_text('known: {p.one: {v: 1}}')
            self.assertEqual(P.load([str(path)], read_mode='frozen')['known']['p.one']['v'], 1)
            hypothesis = path.parent / '.kpopper/hypotheses/core.yaml'
            hypothesis.parent.mkdir(parents=True)
            hypothesis.write_text('meta: {reasoning: {version: 1, profile: core/v1, requires: [arithmetic/v1]}}\n'
                                  'known: {p.one: {v: 2}}\n')
            with self.assertRaisesRegex(P.Refused, 'unsupported_capability'):
                P.load([str(path)], read_mode='frozen')
            self.assertIn('core', Snapshot.capture([str(path)], read_mode='frozen').to_data()['hypotheses'])


class LiveCaptureTests(Repository):
    def test_portable_metadata_does_not_rewrite_authored_evidence(self):
        from scripts.reasoning.snapshot import _portable
        value = {'standing_permission': True, 'record': '/private/record.yaml',
                 'manifest': {'document': {'known': {'p.permission': {
                     'standing_permission': True, 'v': '/authored/quoted/path'}}}}}
        portable = _portable(value, lambda _: 'sanitized-origin')
        self.assertNotIn('standing_permission', portable)
        self.assertEqual(portable['record'], 'sanitized-origin')
        self.assertEqual(portable['manifest'], value['manifest'])

    def test_full_bundle_preserved_and_replay_never_resolves_ref(self):
        path = self.root / 'GROUNDING.yaml'
        path.write_text('known:\n  local.one: {v: 1}\n')
        receipt = self.capture()
        live = Snapshot.capture([str(path)], read_mode='live')
        data = live.to_data()
        self.assertEqual(data['context']['read_mode'], 'captured-live')
        self.assertEqual(data['context']['pending']['ref'], receipt['ledger_commit'])
        bundle = data['context']['pending']['bundles'][receipt['revision']]
        self.assertEqual(bundle['manifest']['document'], fixture_bundle()['manifest']['document'])
        self.assertEqual(bytes.fromhex(bundle['files']['evidence/vendor.txt']['data']), b'The limit is 10.\n')
        frozen = Snapshot.capture([str(path)], read_mode='frozen')
        self.assertNotEqual(live.snapshot_id, frozen.snapshot_id)
        self.assertEqual(frozen.to_data()['context']['pending']['bundles'], {})
        self.assertNotIn(str(self.root), str(data))
        self.capture(fixture_bundle(value=20), event_id='second')
        with mock.patch.object(P, 'load', side_effect=AssertionError('must not read')):
            replayed = Snapshot.from_snapshot(data)
            portable_replay = Snapshot.from_json(live.to_json())
        self.assertEqual(replayed.snapshot_id, live.snapshot_id)
        self.assertEqual(portable_replay.to_data(), data)



if __name__ == '__main__':
    unittest.main()
