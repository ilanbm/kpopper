"""Explicit identity intents preserve immutable originals and current graph closure."""
import copy
from pathlib import Path
import unittest
from unittest import mock

from scripts import history_identity as I, history_hypotheses as HH
from scripts import history_contract as C, history_transaction as T, versions as V
from scripts.reasoning.snapshot import Snapshot
from tests import test_history_snapshot_capture as fixture


class Identity(unittest.TestCase):
    def setUp(self):
        source = fixture.HistorySnapshotCapture()
        source.setUp()
        self.addCleanup(source.doCleanups)
        self.fixture, self.store, self.entry = source, source.store, source.entry
        self.other = fixture.claim('p.other', operation='other', body={'v': 1})
        source.publish([self.other], 'other')

    def publish(self, *objects, operation='fixture'):
        return self.fixture.publish(list(objects), operation)

    def commit(self, mutation):
        return I.commit(self.entry, mutation, verify=lambda data: None)

    def test_same_rewrites_current_refs_and_pins_retains_all_originals(self):
        decision = fixture.claim('d.ready', kind='judgment', operation='judgment', body={
            'verdict': 'ready', 'rests_on': ['p.other'], 'wrong_if': {'expr': 'p.other > 5'},
            'seen': {'p.other': 1}, 'because': 'read {{p.other}}'}, pins={'p.other': self.other['id']})
        self.publish(decision)
        before = self.store.capture()
        mutation = I.prepare_same(self.entry, 'p.input', 'p.other', by='writer')
        self.assertEqual(before.inventory, self.store.capture().inventory)
        with mock.patch.object(V, '_evaluate', side_effect=AssertionError('float fallback')):
            self.commit(mutation)
        current = self.store.capture()
        self.assertEqual(len(current.commits), len(before.commits) + 1)
        self.assertEqual(current.state['subjects']['p.other']['acceptance'], 'retired')
        self.assertEqual(current.document['readings']['p.input']['also'], ['p.other'])
        body = current.document['decisions']['d.ready']
        self.assertEqual(body['rests_on'], ['p.input'])
        self.assertEqual(body['wrong_if'], {'expr': 'p.input > 5'})
        self.assertEqual(body['because'], 'read {{p.input}}')
        self.assertEqual(body['seen'], {'p.input': 1})
        new = current.objects[current.state['subjects']['d.ready']['head']]
        self.assertEqual(new['pins'], {'p.input': current.state['subjects']['p.input']['head']})
        for version, obj in before.objects.items():
            self.assertEqual(current.objects[version], obj)
        snapshot = Snapshot.capture(self.entry, read_mode='frozen')
        self.assertEqual(Snapshot.from_json(snapshot.to_json()).snapshot_id, snapshot.snapshot_id)

    def test_distinct_keeps_original_and_same_refuses(self):
        original = self.store.capture()
        self.commit(I.prepare_distinct(self.entry, 'p.input', 'p.other', 'Different measurements'))
        current = self.store.capture()
        self.assertEqual(current.document['readings']['p.input']['distinct_from'], 'p.other')
        self.assertEqual(current.objects[self.fixture.source['id']], original.objects[self.fixture.source['id']])
        with self.assertRaisesRegex(C.HistoryError, 'identities_declared_distinct'):
            I.prepare_same(self.entry, 'p.input', 'p.other')
        with self.assertRaisesRegex(C.HistoryError, 'invalid_identity_subjects'):
            I.prepare_distinct(self.entry, 'p.input', 'p.input', 'invalid')

    def test_keep_choice_and_newer_source_provenance_are_retained(self):
        fresh = fixture.claim('p.new', operation='fresh', body={'v': 2, 'of': '2026-09-17', 'from': 's.new', 'at': 'page 3'})
        source = fixture.claim('s.new', operation='source', body={'url': 'https://example.test/original'})
        self.publish(fresh, source)
        self.commit(I.prepare_same(self.entry, 'p.input', 'p.new', keep='b'))
        body = self.store.capture().document['readings']['p.new']
        self.assertEqual((body['v'], body['from'], body['at']), (2, 's.new', 'page 3'))
        self.assertEqual(body['also'], ['p.input'])
        self.assertEqual(self.store.state()['subjects']['p.input']['acceptance'], 'retired')

    def test_unordered_conflicting_values_refuse_without_changes(self):
        obj = fixture.claim('p.different', operation='different', body={'v': 9})
        self.publish(obj)
        before = self.store.capture().inventory
        with self.assertRaises((I.P.Refused, I.S.P.Refused)):
            I.prepare_same(self.entry, 'p.input', 'p.different')
        self.assertEqual(self.store.capture().inventory, before)

    def test_alias_self_dependency_refuses(self):
        first = fixture.claim('d.first', kind='judgment', operation='first', body={
            'verdict': 'yes', 'rests_on': ['p.input'], 'wrong_if': {'expr': 'p.input > 10'}},
            pins={'p.input': self.fixture.source['id']})
        second = fixture.claim('d.second', kind='judgment', operation='second', body={
            'verdict': 'yes', 'rests_on': ['d.first'], 'wrong_if': {'expr': '1 > 10'}},
            pins={'d.first': first['id']})
        self.publish(first, second)
        with self.assertRaisesRegex(C.HistoryError, 'identity_self_dependency|identity_pin_cycle'):
            I.prepare_same(self.entry, 'd.second', 'd.first')

    def test_unequal_seen_and_pin_collapses_refuse(self):
        decision = fixture.claim('d.two', kind='judgment', operation='two', body={
            'verdict': 'ready', 'rests_on': ['p.input', 'p.other'], 'wrong_if': {'expr': 'p.input > 5'},
            'seen': {'p.input': 1, 'p.other': 2}}, pins={'p.input': self.fixture.source['id'], 'p.other': self.other['id']})
        self.publish(decision)
        with self.assertRaisesRegex(C.HistoryError, 'identity_seen_collision'):
            I.prepare_same(self.entry, 'p.input', 'p.other')

    def test_computed_seen_basis_and_literal_expression_data_are_not_rewritten(self):
        from scripts.reasoning.contract import digest
        basis = {'version': 1, 'profile': 'core/v1', 'modules': ['arithmetic/v1'], 'dependencies': []}
        basis['digest'] = digest(basis)
        seen = {'computed': {'version': 2, 'value': {'type': 'number', 'numerator': '1', 'denominator': '1'}, 'basis': basis}}
        decision = fixture.claim('d.ready', kind='judgment', operation='decision', body={
            'verdict': 'ready', 'rests_on': ['p.other'], 'wrong_if': {'expr': 'p.other > 5'},
            'seen': {'p.other': seen}}, pins={'p.other': self.other['id']})
        literal = fixture.claim('p.literal', operation='literal', body={'rule': {'text': 'p.other'}})
        self.publish(decision, literal)
        self.commit(I.prepare_same(self.entry, 'p.input', 'p.other'))
        current = self.store.capture()
        self.assertEqual(current.document['decisions']['d.ready']['seen']['p.input'], seen)
        self.assertEqual(current.document['readings']['p.literal']['rule'], {'text': 'p.other'})
        self.assertEqual(current.objects[decision['id']]['body']['seen'], {'p.other': seen})

    def test_named_group_rewrites_stay_proposals(self):
        proposal = HH.prepare(self.entry, 'alternative', {'kind': 'set', 'id': 'p.other', 'value': 1})
        HH.commit(self.entry, proposal, verify=lambda data: None)
        self.commit(I.prepare_same(self.entry, 'p.input', 'p.other'))
        groups, index = HH.layers(I.D.from_store_capture(self.store.capture()).projection,
                                  I.D.from_store_capture(self.store.capture()).document)
        self.assertIn('p.input', groups['alternative']['doc']['readings'])
        self.assertNotIn('p.other', groups['alternative']['doc']['readings'])
        version = index['groups']['alternative']['p.input'][0]
        self.assertIn(version, self.store.state()['subjects']['p.input']['proposals'])
        self.assertNotEqual(version, self.store.state()['subjects']['p.input']['head'])

    def test_physical_groups_refuse_and_brief_references_prepare_without_writes(self):
        directory = Path(self.store.layout['hypotheses'])
        directory.mkdir(parents=True)
        physical = directory / 'old.yaml'
        physical.write_text('readings: {p.other: {v: 2}}\n')
        before = self.entry.read_bytes()
        with self.assertRaisesRegex(C.HistoryError, 'identity_physical_hypotheses_require_import'):
            I.prepare_same(self.entry, 'p.input', 'p.other')
        physical.unlink()
        Path(self.store.layout['view']).write_text('sections: [{text: "{{p.other}}"}]\n')
        mutation = I.prepare_same(self.entry, 'p.input', 'p.other')
        self.assertEqual(mutation.to_data()['receipt']['before']['identity_authoring']['version'], 2)
        self.assertTrue(any(item['role'] == 'view' for item in mutation.files))
        self.assertEqual(self.entry.read_bytes(), before)

    def test_historical_pin_to_retired_identity_refuses_instead_of_refreshing_review(self):
        updated = fixture.claim('p.other', operation='later', body={'v': 1}, saw=[self.other['id']])
        accept = fixture.act(updated, 'accept', operation='accept', over=[self.other['id']])
        old_judgment = fixture.claim('d.old', kind='judgment', operation='old-j', body={
            'verdict': 'old', 'rests_on': ['p.other'], 'wrong_if': {'expr': 'p.other > 5'}}, pins={'p.other': self.other['id']})
        self.publish(updated, accept, old_judgment)
        with self.assertRaisesRegex(C.HistoryError, 'historical_alias_pin_requires_resolution'):
            I.prepare_same(self.entry, 'p.input', 'p.other')

    def test_stale_and_interrupted_retries_keep_one_manifest(self):
        mutation = I.prepare_same(self.entry, 'p.input', 'p.other')
        stale = I.prepare_distinct(self.entry, 'p.input', 'p.other', 'a different intent')
        with mock.patch.object(T, '_replace', side_effect=OSError('view failed')), self.assertRaises(OSError):
            self.commit(mutation)
        count = len(self.store.capture().commits)
        self.commit(T.PreparedMutation.from_bytes(mutation.to_bytes()))
        self.assertEqual(len(self.store.capture().commits), count)
        with self.assertRaisesRegex(C.HistoryError, 'stale_baseline'):
            self.commit(stale)

    def test_structured_argument_pairs_remain_two_and_equal_seen_can_collapse(self):
        rule = fixture.claim('p.sum', operation='sum', body={'rule': {
            'op': 'add', 'args': [{'ref': 'p.input'}, {'ref': 'p.other'}]}})
        decision = fixture.claim('d.equal', kind='judgment', operation='equal', body={
            'verdict': 'ready', 'rests_on': ['p.input', 'p.other'], 'wrong_if': {'expr': 'p.input + p.other > 5'},
            'seen': {'p.input': 1, 'p.other': 1}}, pins={'p.input': self.fixture.source['id'], 'p.other': self.other['id']})
        self.publish(rule, decision)
        self.commit(I.prepare_same(self.entry, 'p.input', 'p.other'))
        document = self.store.capture().document
        self.assertEqual(document['readings']['p.sum']['rule']['args'], [{'ref': 'p.input'}, {'ref': 'p.input'}])
        self.assertEqual(document['decisions']['d.equal']['rests_on'], ['p.input'])
        self.assertEqual(document['decisions']['d.equal']['seen'], {'p.input': 1})
        from scripts.reasoning.evaluate import Evaluator
        result = Evaluator(Snapshot.capture(self.entry, read_mode='frozen')).evaluate({'ref': 'p.sum'}, declared=['p.sum'])
        self.assertEqual(result['value']['numerator'], '2')

    def test_pin_context_collapse_refuses_unequal_original_bodies_even_when_values_agree(self):
        newer = fixture.claim('p.dated', operation='dated', body={'v': 1, 'of': '2026-09-17'})
        decision = fixture.claim('d.both', kind='judgment', operation='both', body={
            'verdict': 'ready', 'rests_on': ['p.input', 'p.dated'], 'wrong_if': {'expr': 'p.input > 5'}},
            pins={'p.input': self.fixture.source['id'], 'p.dated': newer['id']})
        self.publish(newer, decision)
        with self.assertRaisesRegex(C.HistoryError, 'identity_pin_collision'):
            I.prepare_same(self.entry, 'p.input', 'p.dated')

    def test_late_physical_source_edit_is_not_overwritten(self):
        mutation = I.prepare_same(self.entry, 'p.input', 'p.other')
        before = self.entry.read_bytes()
        def change(data):
            Path(self.store.layout['view']).write_text('sections: [{text: "{{p.other}}"}]\n')
        with self.assertRaisesRegex(C.HistoryError, 'identity_source_changed'):
            I.commit(self.entry, mutation, verify=change)
        self.assertEqual(self.entry.read_bytes(), before)
        self.assertEqual(len(self.store.capture().commits), 2)

    def test_group_judgment_survivor_keeps_its_exact_pin_context(self):
        first = fixture.claim('d.first', kind='judgment', operation='first', body={
            'verdict': 'yes', 'rests_on': ['p.input'], 'wrong_if': {'expr': 'p.input > 5'}},
            pins={'p.input': self.fixture.source['id']})
        second = fixture.claim('d.second', kind='judgment', operation='second', body={
            'verdict': 'yes', 'rests_on': ['p.other'], 'wrong_if': {'expr': 'p.other > 5'}},
            pins={'p.other': self.other['id']})
        self.publish(first, second)
        proposal = HH.prepare(self.entry, 'alternative', {'kind': 'add', 'id': 'd.second', 'body': {
            'verdict': 'yes', 'rests_on': ['p.other'], 'wrong_if': {'expr': 'p.other > 8'}}})
        HH.commit(self.entry, proposal, verify=lambda data: None)
        self.commit(I.prepare_same(self.entry, 'd.first', 'd.second'))
        captured = self.store.capture()
        projection = I.D.from_store_capture(captured)
        groups, index = HH.layers(projection.projection, projection.document)
        chosen = captured.objects[index['groups']['alternative']['d.first'][0]]
        self.assertEqual(chosen['body']['rests_on'], ['p.input'])
        self.assertEqual(chosen['pins'], {'p.input': self.fixture.source['id']})

    def test_optional_as_of_keeps_default_receipt_bytes_and_actual_recording_clock(self):
        clock = '2026-09-17T12:34:56+00:00'
        for kind in ('same', 'distinct'):
            args = (self.entry, 'p.input', 'p.other') + (('Different readings',) if kind == 'distinct' else ())
            prepare = I.prepare_same if kind == 'same' else I.prepare_distinct
            before = prepare(*args, operation='identity-' + kind, recorded_at=clock)
            explicit_none = prepare(*args, operation='identity-' + kind, recorded_at=clock, as_of=None)
            self.assertEqual(before.to_bytes(), explicit_none.to_bytes())
            self.assertNotIn('as_of', before.to_data()['receipt']['before']['identity_authoring'])
            I.verify_prepared(self.entry, T.PreparedMutation.from_bytes(before.to_bytes()))
            with mock.patch.object(I.S, '_merge', wraps=I.S._merge) as merge:
                past = prepare(*args, operation='past-' + kind, recorded_at=clock, as_of='2026-09-01')
            if kind == 'same':
                self.assertEqual(merge.call_args.args[-1], '2026-09-01')
            self.assertEqual(past.to_data()['receipt']['before']['identity_authoring']['as_of'], '2026-09-01')
            for item in past.files:
                if item['role'] == 'history_object':
                    self.assertEqual(C.decode_document(item['after'])['on'], clock)
            I.verify_prepared(self.entry, T.PreparedMutation.from_bytes(past.to_bytes()))
            if kind == 'distinct':
                body = next(C.decode_document(item['after'])['body'] for item in past.files
                            if item['role'] == 'history_object' and C.decode_document(item['after'])['kind'] != 'act')
                self.assertNotIn('of', body)

    def test_as_of_is_canonical_iso_date_and_never_future(self):
        import datetime
        with mock.patch.object(I.P, 'latest_today', return_value=datetime.date(2026, 9, 17)):
            for value in ('2026-09-18', '2026-02-30', '20260901', '2026-9-1', '2026-09-01T00:00:00Z', 20260901):
                for kind in ('same', 'distinct'):
                    with self.subTest(kind=kind, value=value), self.assertRaisesRegex(C.HistoryError, 'invalid_identity_as_of'):
                        if kind == 'same':
                            I.prepare_same(self.entry, 'p.input', 'p.other', as_of=value)
                        else:
                            I.prepare_distinct(self.entry, 'p.input', 'p.other', 'reason', as_of=value)

    def test_exact_imported_physical_original_is_not_active_hypothesis_authority(self):
        physical = Path(self.store.layout['hypotheses']) / 'archived.yaml'
        physical.parent.mkdir(parents=True)
        original = b'hypothesis: {claim: retained empty layer}\nreadings: {}\n'
        physical.write_bytes(original)
        captured = self.store.capture()
        template = self.store._template(captured.commits)
        template['meta']['history_hypothesis_import'] = {'version': 1, 'physical': [
            {'name': 'archived', 'path': physical.relative_to(self.entry.parent).as_posix(), 'sha256': C.sha256(original)}]}
        receipt = T.semantic_receipt(profile='core/v1', capabilities={}, before={}, after={})
        arguments = dict(marker=captured.marker, operation='retain-physical', parents=C.commit_frontier(captured.commits),
                         baseline=captured.baseline, objects=[], receipt=receipt, view_template=template)
        draft = C.make_commit(**arguments, view=b'')
        rendered = self.store.render(captured, commits={**captured.commits, 'retain-physical': C.encode_document(draft)})
        manifest = C.make_commit(**arguments, view=rendered)
        mutation = T.PreparedMutation(operation='retain-physical', authority=captured.marker, baseline=captured.baseline,
            entry=self.entry.name, receipt=receipt, files=[
                {'role': 'record', 'path': self.entry.name, 'before': captured.entry_bytes, 'after': rendered},
                {'role': 'history_commit', 'path': (Path(self.store.layout['history_commits']) / 'retain-physical.yaml').relative_to(self.entry.parent).as_posix(),
                 'before': None, 'after': C.encode_document(manifest)}])
        self.store.commit(mutation, verify=lambda data: None)
        self.commit(I.prepare_distinct(self.entry, 'p.input', 'p.other', 'Distinct'))
        self.assertEqual(physical.read_bytes(), original)
        physical.write_bytes(original + b'# concurrent change\n')
        with self.assertRaisesRegex(C.HistoryError, 'imported_hypothesis_changed'):
            I.prepare_same(self.entry, 'p.input', 'p.other')


    def brief_mutation(self):
        path = Path(self.store.layout['view'])
        path.parent.mkdir(parents=True, exist_ok=True)
        raw = b'# original brief comment\ntabs:\n  - title: Now\n    serves: [p.other, p.input]\n    sections:\n      - title: Status\n        text: "{{p.other}} and {{p.input}}"\n        seen:\n          p.input: 1\n          p.other: 2\n        pick: [p.other, p.input]\n        wrong_if: {expr: "p.other > 5"}\nlabels:\n  p.input: kept label\n  p.other: retired label\ngroups:\n  chosen: [p.other, p.input]\nrule: {text: p.other}\n'
        path.write_bytes(raw)
        mutation = I.prepare_same(self.entry, 'p.input', 'p.other')
        return path, raw, mutation

    def test_brief_references_join_same_manifest_and_keep_original_evidence(self):
        original = self.store.capture()
        path, raw, mutation = self.brief_mutation()
        self.assertEqual(path.read_bytes(), raw)
        receipt = mutation.to_data()['receipt']['before']['identity_authoring']
        self.assertEqual(receipt['version'], 2)
        self.assertEqual(receipt['brief']['before_utf8'].encode(), raw)
        self.commit(mutation)
        brief = C.decode_document(path.read_bytes())
        self.assertEqual(brief['tabs'][0]['serves'], ['p.input'])
        section = brief['tabs'][0]['sections'][0]
        self.assertEqual(section['seen'], {'p.input': 1})
        self.assertEqual(section['pick'], ['p.input'])
        self.assertEqual(section['wrong_if'], {'expr': 'p.input > 5'})
        self.assertEqual(brief['labels'], {'p.input': 'kept label'})
        self.assertEqual(brief['groups']['chosen'], ['p.input'])
        self.assertEqual(brief['rule'], {'text': 'p.other'})
        self.assertTrue(path.read_bytes().startswith(b'# original brief comment\n'))
        current = self.store.capture()
        for version, obj in original.objects.items():
            self.assertEqual(current.objects[version], obj)
        self.assertFalse(T._target(self.entry.parent, T.journal_for(self.entry)).exists())
        self.commit(T.PreparedMutation.from_bytes(mutation.to_bytes()))
        self.assertEqual(len(self.store.capture().commits), len(original.commits) + 1)

    def test_distinct_does_not_rewrite_brief_or_upgrade_old_receipt(self):
        path, raw, _ = self.brief_mutation()
        mutation = I.prepare_distinct(self.entry, 'p.input', 'p.other', 'Different records')
        self.assertEqual(mutation.to_data()['receipt']['before']['identity_authoring']['version'], 1)
        self.assertFalse(any(item['role'] == 'view' for item in mutation.files))
        self.commit(mutation)
        self.assertEqual(path.read_bytes(), raw)

    def test_auxiliary_publication_crashes_block_readers_and_replay_exact_operation(self):
        for boundary in ('record', 'brief'):
            with self.subTest(boundary=boundary):
                fixture_case = Identity()
                fixture_case.setUp()
                self.addCleanup(fixture_case.doCleanups)
                path, raw, mutation = fixture_case.brief_mutation()
                target = fixture_case.entry if boundary == 'record' else path
                replace = T._replace
                def fail(current, after):
                    if Path(current) == target:
                        raise OSError('auxiliary image crash')
                    return replace(current, after)
                with mock.patch.object(T, '_replace', side_effect=fail):
                    with self.assertRaisesRegex(OSError, 'auxiliary image crash'):
                        fixture_case.commit(mutation)
                with self.assertRaisesRegex(ValueError, 'recovery_required'):
                    Snapshot.capture(fixture_case.entry, read_mode='frozen')
                with self.assertRaisesRegex(C.HistoryError, 'history_already_committed'):
                    fixture_case.store.cancel_auxiliary(mutation, verify=lambda data: None)
                fixture_case.commit(T.PreparedMutation.from_bytes(mutation.to_bytes()))
                self.assertEqual(path.read_bytes(), T.auxiliary_view(mutation)['after'])
                self.assertIn(mutation.to_data()['operation'], fixture_case.store.capture().commits)

    def test_auxiliary_completion_callback_failure_keeps_guard_for_exact_retry(self):
        path, raw, mutation = self.brief_mutation()
        calls = []
        def check(data):
            calls.append(data['operation'])
            if len(calls) == 2:
                raise ValueError('completion callback interrupted')
        with self.assertRaisesRegex(ValueError, 'completion callback interrupted'):
            I.commit(self.entry, mutation, verify=check)
        self.assertEqual(path.read_bytes(), T.auxiliary_view(mutation)['after'])
        with self.assertRaisesRegex(C.HistoryError, 'recovery_required'):
            self.store.capture()
        self.commit(mutation)
        self.assertEqual(len(self.store.capture().commits), 3)

    def test_pre_manifest_cancel_checks_brief_and_generic_recovery_refuses_owner(self):
        path, raw, mutation = self.brief_mutation()
        publish = T.publish_immutable
        def fail(current, *args, **kwargs):
            if Path(current).parent == Path(self.store.layout['history_commits']):
                raise OSError('before manifest')
            return publish(current, *args, **kwargs)
        with mock.patch.object(T, 'publish_immutable', side_effect=fail):
            with self.assertRaisesRegex(OSError, 'before manifest'):
                self.commit(mutation)
        for recover in (T.recover_legacy, T.recover_transition):
            with self.assertRaisesRegex(C.HistoryError, 'history_auxiliary_recovery_required'):
                recover(self.entry.parent, T.journal_for(self.entry), verify=lambda data: None)
        path.write_bytes(raw + b'# unrelated later edit\n')
        with self.assertRaisesRegex(C.HistoryError, 'concurrent_edit'):
            self.store.cancel_auxiliary(mutation, verify=lambda data: None)
        with self.assertRaisesRegex(C.HistoryError, 'concurrent_brief_edit|identity_source_changed'):
            self.commit(mutation)
        self.assertTrue(T._target(self.entry.parent, T.journal_for(self.entry)).exists())
        path.write_bytes(raw)
        self.store.cancel_auxiliary(mutation, verify=lambda data: None)
        self.assertEqual(len(self.store.capture().commits), 2)
        self.assertEqual(path.read_bytes(), raw)

    def test_forged_auxiliary_rewrite_refuses_even_with_noop_caller_verifier(self):
        path, raw, mutation = self.brief_mutation()
        data = mutation.to_data()
        files = copy.deepcopy(mutation.files)
        item = next(item for item in files if item['role'] == 'view')
        item['after'] += b'unrequested: changed\n'
        before = copy.deepcopy(data['receipt']['before'])
        after = copy.deepcopy(data['receipt']['after'])
        before['identity_authoring']['brief']['after_sha256'] = C.sha256(item['after'])
        after['identity_authoring']['view_sha256'] = C.sha256(item['after'])
        receipt = T.semantic_receipt(profile=data['receipt']['profile'], capabilities=data['receipt']['capabilities'],
                                     before=before, after=after)
        manifest_item = next(item for item in files if item['role'] == 'history_commit')
        manifest = C.decode_document(manifest_item['after'])
        manifest['receipt'] = receipt
        manifest_item['after'] = C.encode_document(manifest)
        forged = T.PreparedMutation(operation=data['operation'], authority=data['authority'],
            baseline=data['baseline'], files=files, receipt=receipt, entry=data['entry'])
        with self.assertRaisesRegex(C.HistoryError, 'identity_receipt_mismatch'):
            self.store.commit(forged, verify=lambda data: None)
        self.assertEqual(path.read_bytes(), raw)
        self.assertEqual(len(self.store.capture().commits), 2)

    def test_auxiliary_owner_requires_actual_lock_and_exact_journal(self):
        path, raw, mutation = self.brief_mutation()
        with self.assertRaisesRegex(C.HistoryError, 'auxiliary_writer_lock_required'):
            with T.auxiliary_owner(self.entry.parent, T.journal_for(self.entry), mutation):
                pass
        journal = T._target(self.entry.parent, T.journal_for(self.entry))
        journal.parent.mkdir(parents=True, exist_ok=True)
        journal.write_bytes(b'foreign operation')
        with self.assertRaisesRegex(C.HistoryError, 'recovery_required'):
            self.commit(mutation)
        self.assertEqual(journal.read_bytes(), b'foreign operation')
        self.assertEqual(path.read_bytes(), raw)

    def test_ambiguous_brief_rewrite_refuses_without_any_source_changes(self):
        path = Path(self.store.layout['view'])
        path.write_bytes(b'labels: {p.input: first, p.other: second}\n')
        original = self.entry.read_bytes()
        raw = path.read_bytes()
        with self.assertRaises(C.HistoryError):
            I.prepare_same(self.entry, 'p.input', 'p.other')
        self.assertEqual(path.read_bytes(), raw)
        self.assertEqual(self.entry.read_bytes(), original)

    def test_final_callback_history_edit_cannot_clear_auxiliary_guard(self):
        path, raw, mutation = self.brief_mutation()
        item = next(item for item in mutation.files if item['role'] == 'history_object')
        target = self.entry.parent / item['path']
        calls = []
        def change(data):
            calls.append(True)
            if len(calls) == 2:
                target.write_bytes(item['after'] + b'# concurrent immutable corruption\n')
        with self.assertRaises(C.HistoryError):
            I.commit(self.entry, mutation, verify=change)
        self.assertTrue(T._target(self.entry.parent, T.journal_for(self.entry)).exists())
        with self.assertRaisesRegex(C.HistoryError, 'recovery_required'):
            self.store.capture()
        target.write_bytes(item['after'])
        self.commit(mutation)
        self.assertEqual(path.read_bytes(), T.auxiliary_view(mutation)['after'])

if __name__ == '__main__':
    unittest.main()
