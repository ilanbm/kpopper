"""Committed visibility, independent acceptance, and guarded failure/retry behavior."""
import copy
import datetime
from pathlib import Path
import tempfile
import unittest
from unittest import mock

from scripts import history_contract as C, history_transaction as T, history_store as H
from scripts import provenance as P, versions as V
from scripts.pending_grounding import identity


def claim(subject='p.input', value=1, *, op='claim-1', saw=(), kind='reading', body=None, pins=None, at=None):
    return C.make_object(subject=subject, kind=kind, by='writer', on='2026-09-17',
                         operation=op, body={'v': value} if body is None else body,
                         saw=saw, pins=pins, at=at,
                         authored={'collection': 'readings' if kind == 'reading' else 'judgments',
                                   'profile': 'core/v1', 'fields': {'value': 'v'}})


def act(target, what='accept', *, op='act-1', over=(), saw=(), read=None):
    body = {'act': what, 'of': target['id'], 'over': sorted(over), 'because': 'explicit decision'}
    if read is not None:
        body['read'] = read
    return C.make_object(subject=target['subject'], kind='act', by='reviewer', on='2026-09-17',
                         operation=op, body=body, saw=saw)


def held(*objects):
    return {o['id']: o for o in objects}


class Reduction(unittest.TestCase):
    def entry(self, *objects):
        return H.reduce(held(*objects))['subjects']['p.input']

    def test_no_falsifier_or_review_assessment_is_called(self):
        judgment = claim(kind='judgment', body={'verdict': 'ready', 'wrong_if': 'x > 0',
                                                'seen': {'x': 7}})
        with mock.patch.object(V, 'state', side_effect=AssertionError), \
             mock.patch.object(V, '_cross', side_effect=AssertionError), \
             mock.patch.object(V, '_evaluate', side_effect=AssertionError):
            entry = self.entry(judgment)
        self.assertEqual(entry['acceptance'], 'accepted')
        self.assertNotIn('fired', entry)
        self.assertNotIn('reviewed_by', entry)
        self.assertEqual(entry['body']['seen'], {'x': 7})

    def test_typed_agreement_distinguishes_values_and_pins(self):
        for a, b in [(True, 1), (1, 1.0), (datetime.date(2026, 9, 17), '2026-09-17')]:
            self.assertEqual(self.entry(claim(value=a), claim(value=b, op='other'))['acceptance'], 'contested')
        self.assertEqual(self.entry(claim(), claim(op='other'))['agreed'], 2)
        p, q = claim('p.dep', op='p'), claim('p.dep', op='q')
        a = claim(pins={'p.dep': p['id']})
        b = claim(op='b', pins={'p.dep': q['id']})
        self.assertEqual(H.reduce(held(p, q, a, b))['subjects']['p.input']['acceptance'], 'contested')

    def test_correction_refutation_and_explicit_return(self):
        a = claim()
        b = claim(value=2, op='b', saw=[a['id']])
        self.assertEqual(self.entry(a, b)['heads'], [a['id']])
        correction = act(b, 'correct', over=[a['id']])
        e = self.entry(a, b, correction)
        self.assertEqual(e['heads'], [b['id']])
        self.assertEqual(e['marks'][a['id']], 'corrected')
        refutation = act(b, 'refute', op='refute', saw=[correction['id']])
        self.assertEqual(self.entry(a, b, correction, refutation)['heads'], [])
        back = act(a, op='return', saw=[correction['id']], over=[b['id']])
        self.assertEqual(self.entry(a, b, correction, refutation, back)['heads'], [a['id']])

    def test_concurrent_acts_and_order_permutations(self):
        root = claim()
        accept, refute = act(root), act(root, 'refute', op='refute')
        objects = [root, accept, refute]
        import itertools
        states = [H.reduce(held(*order)) for order in itertools.permutations(objects)]
        self.assertTrue(all(s == states[0] for s in states))
        self.assertEqual(states[0]['subjects']['p.input']['acceptance'], 'contested')

    def test_review_is_evidence_separate_from_acceptance(self):
        root = claim(kind='judgment')
        review = act(root, 'review', read={})
        e = self.entry(root, review)
        self.assertEqual(e['acceptance'], 'accepted')
        self.assertEqual(len(e['review_evidence']), 1)
        self.assertFalse(e['open_acts'])

    def test_existing_source_clock_is_preserved(self):
        a = claim(value=1, at={'day': '2026-09-16'})
        b = claim(value=2, op='later', at={'day': '2026-09-17'})
        self.assertEqual(self.entry(a, b)['heads'], [b['id']])
        self.assertEqual(self.entry(a, b)['implied'][0]['rule'], 'source_clock')

    def test_missing_reference_and_mixed_kind_refuse(self):
        a = claim()
        with self.assertRaisesRegex(C.HistoryError, 'incomplete_closure'):
            self.entry(claim(saw=[a['id']], op='missing'))
        with self.assertRaisesRegex(C.HistoryError, 'mixed_subject_kind'):
            self.entry(a, claim(kind='judgment', op='judgment'))


class Storage(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.entry = Path(self.temp.name) / 'GROUNDING.yaml'
        self.store = H.Store(self.entry)
        self.marker = C.authority(record_id='fixture', authority='history', generation=1)
        marker = Path(self.store.layout['history_authority'])
        marker.parent.mkdir(parents=True)
        marker.write_bytes(C.encode_document(self.marker))
        self.entry.write_bytes(C.encode_document({
            'meta': {'schema': 'original', 'history': H.baseline(self.marker, {}, H.reduce({}))},
            'readings': {}, 'judgments': {}, 'record': {'keep': True}}))

    def mutation(self, objects, op='operation'):
        capture = self.store.capture()
        pairs = [(o, C.encode_document(o)) for o in objects]
        receipt = T.semantic_receipt(profile='core/v1', capabilities={}, before={}, after={})
        # Baselines bind committed operation/object evidence independently of the
        # manifest's generated-view digest, avoiding a self-referential hash.
        draft = C.make_commit(marker=self.marker, operation=op,
                              parents=C.commit_frontier(capture.commits),
                              baseline=capture.baseline, objects=pairs, receipt=receipt, view=b'',
                              view_template=C.document_template(capture.document))
        commits = dict(capture.commits, **{op: C.encode_document(draft)})
        after = self.store.render(capture, objects={**capture.objects, **held(*objects)}, commits=commits)
        manifest = C.make_commit(marker=self.marker, operation=op, parents=draft['parents'],
                                 baseline=capture.baseline, objects=pairs, receipt=receipt, view=after,
                                 view_template=C.document_template(capture.document))
        files = [{'path': self.entry.name, 'role': 'record', 'before': capture.entry_bytes, 'after': after}]
        for obj, raw in pairs:
            files.append({'path': str(Path(self.store.layout['history']).relative_to(self.entry.parent) /
                                      obj['subject'] / (obj['id'] + '.yaml')),
                          'role': 'history_object', 'before': None, 'after': raw})
        files.append({'path': str(Path(self.store.layout['history_commits']).relative_to(self.entry.parent) /
                                  (op + '.yaml')), 'role': 'history_commit', 'before': None,
                      'after': C.encode_document(manifest)})
        return T.PreparedMutation(operation=op, authority=self.marker, baseline=capture.baseline,
                                  files=files, receipt=receipt)

    def publish(self, objects, op='operation'):
        mutation = self.mutation(objects, op)
        self.store.commit(mutation, verify=lambda data: None)
        return mutation

    def test_marker_selects_authority_and_mismatch_never_imports_view(self):
        Path(self.store.layout['history_authority']).unlink()
        with self.assertRaisesRegex(C.HistoryError, 'history_not_active'):
            self.store.capture()
        other = C.authority(record_id='other', authority='history', generation=1)
        Path(self.store.layout['history_authority']).write_bytes(C.encode_document(other))
        with self.assertRaisesRegex(C.HistoryError, 'authority_mismatch'):
            self.store.capture()

    def test_reads_do_not_write_and_observe_bytes_and_membership(self):
        self.publish([claim()])
        before = {p: p.read_bytes() for p in self.entry.parent.rglob('*') if p.is_file()}
        events = []
        token = P._CAPTURE_READS.set(lambda kind, path, value: events.append((kind, path)))
        try:
            self.store.capture()
            self.store.state()
        finally:
            P._CAPTURE_READS.reset(token)
        self.assertEqual(before, {p: p.read_bytes() for p in self.entry.parent.rglob('*') if p.is_file()})
        self.assertTrue(any(kind == 'glob' for kind, path in events))
        self.assertTrue(any(kind == 'bytes' for kind, path in events))
        self.assertFalse(any(p.name == '.index.json' for p in before))

    def test_orphan_staging_is_inactive_even_when_malformed(self):
        orphan = claim()
        path = Path(self.store.layout['history']) / orphan['subject'] / (orphan['id'] + '.yaml')
        path.parent.mkdir(parents=True)
        path.write_bytes(b'broken yaml : [')
        capture = self.store.capture()
        self.assertEqual(capture.objects, {})
        self.assertEqual(capture.state['subjects'], {})
        self.assertEqual(capture.object_bytes[(orphan['subject'], orphan['id'])], b'broken yaml : [')

    def test_committed_missing_and_corrupt_object_refuse_complete_result(self):
        obj = claim()
        self.publish([obj])
        path = Path(self.store.layout['history']) / obj['subject'] / (obj['id'] + '.yaml')
        original = path.read_bytes()
        path.unlink()
        with self.assertRaisesRegex(C.HistoryError, 'incomplete_commit'):
            self.store.capture()
        path.write_bytes(original + b'\n')
        with self.assertRaisesRegex(C.HistoryError, 'object_bytes_mismatch'):
            self.store.capture()

    def test_stale_baseline_and_mandatory_verifier(self):
        prepared = self.mutation([claim()])
        with self.assertRaisesRegex(C.HistoryError, 'missing_verifier'):
            self.store.commit(prepared, verify=None)
        self.publish([claim('p.other', op='other')], 'other')
        with self.assertRaisesRegex(C.HistoryError, 'stale_baseline'):
            self.store.commit(prepared, verify=lambda d: None)

    def test_exact_retry_and_operation_collision(self):
        prepared = self.publish([claim()])
        self.store.commit(prepared, verify=lambda d: None)
        # A distinct manifest with the same operation is rejected before writing.
        changed = self.mutation([claim('p.other', op='other')], op='another')
        files = changed.files
        cm = next(f for f in files if f['role'] == 'history_commit')
        doc = C.decode_document(cm['after'])
        doc['operation'] = 'operation'
        doc['parents'].pop('operation')
        cm['path'] = cm['path'].replace('another.yaml', 'operation.yaml')
        cm['after'] = C.encode_document(doc)
        bad = T.PreparedMutation(operation='operation', authority=self.marker,
                                 baseline=changed.to_data()['baseline'], files=files,
                                 receipt=changed.to_data()['receipt'])
        with self.assertRaisesRegex(C.HistoryError, 'operation_collision'):
            self.store.commit(bad, verify=lambda d: None)

    def test_faults_before_manifest_are_inactive_and_exact_retry_completes(self):
        for fail_call in (1, 2):
            with self.subTest(fail_call=fail_call):
                prepared = self.mutation([claim(op='o' + str(fail_call))], op='op' + str(fail_call))
                original = T.publish_immutable
                calls = []
                def fail(path, raw, **kwargs):
                    calls.append(path)
                    if len(calls) == fail_call:
                        raise OSError('injected')
                    return original(path, raw, **kwargs)
                before = self.store.capture().objects
                with mock.patch.object(T, 'publish_immutable', side_effect=fail):
                    with self.assertRaisesRegex(OSError, 'injected'):
                        self.store.commit(prepared, verify=lambda d: None)
                self.assertEqual(self.store.capture().objects, before)
                self.store.commit(prepared, verify=lambda d: None)

    def test_view_failure_commits_evidence_and_retry_repairs_exact_before(self):
        prepared = self.mutation([claim()])
        with mock.patch.object(T, '_replace', side_effect=OSError('view failure')):
            with self.assertRaisesRegex(OSError, 'view failure'):
                self.store.commit(prepared, verify=lambda d: None)
        self.assertEqual(len(self.store.capture().objects), 1)
        self.store.commit(prepared, verify=lambda d: None)
        self.assertEqual(self.store.capture().document['readings']['p.input']['v'], 1)

    def test_retry_and_rebuild_refuse_unresolved_hand_edit(self):
        prepared = self.publish([claim()])
        document = C.decode_document(self.entry.read_bytes())
        document['readings']['p.input']['v'] = 900
        self.entry.write_bytes(C.encode_document(document))
        with self.assertRaisesRegex(C.HistoryError, 'concurrent_edit'):
            self.store.commit(prepared, verify=lambda d: None)
        with self.assertRaisesRegex(C.HistoryError, 'unresolved_view_edit'):
            self.store.rebuild(write=True)
        self.assertEqual(C.decode_document(self.entry.read_bytes())['readings']['p.input']['v'], 900)

    def test_rebuild_preserves_collections_metadata_and_seen(self):
        document = C.decode_document(self.entry.read_bytes())
        self.entry.write_bytes(C.encode_document(document))
        body = {'verdict': 'go', 'seen': {'old': {'v': 7}}, 'wrong_if': 'unknown > 1'}
        obj = claim('d.ready', kind='judgment', body=body)
        self.publish([obj])
        rebuilt = C.decode_document(self.store.rebuild())
        self.assertIsInstance(rebuilt['judgments'], dict)
        self.assertEqual(rebuilt['judgments'], {'d.ready': body})
        self.assertEqual(rebuilt['meta']['schema'], 'original')
        self.assertEqual(rebuilt['record'], {'keep': True})

    def test_dispute_never_projects_arbitrary_scalar(self):
        self.publish([claim()])
        live = self.store.capture()
        other = claim(value=2, op='other')
        rendered = C.decode_document(self.store.render(live, objects={**live.objects, **held(other)}))
        self.assertEqual(rendered['readings'], {})
        self.assertEqual(len(rendered['meta']['history']['heads']['p.input']), 2)

    def test_refute_publishes_no_scalar_and_return_requires_explicit_act(self):
        root = claim()
        self.publish([root])
        refutation = act(root, 'refute', op='refute-act')
        self.publish([refutation], op='refute-operation')
        live = self.store.capture()
        self.assertEqual(live.document['readings'], {})
        self.assertEqual(live.state['subjects']['p.input']['acceptance'], 'refuted')
        self.assertEqual(live.baseline['heads']['p.input'], [])
        self.assertEqual(live.baseline['open_acts']['p.input'], [refutation['id']])
        self.assertEqual(C.decode_document(self.store.rebuild())['readings'], {})
        back = act(root, op='return-act', saw=[refutation['id']])
        self.publish([back], op='return-operation')
        self.assertEqual(self.store.capture().document['readings']['p.input']['v'], 1)

    def test_concurrent_decisions_publish_contested_state_without_scalar(self):
        root = claim()
        self.publish([root])
        yes = act(root, op='yes')
        no = act(root, 'refute', op='no')
        self.publish([yes, no], op='concurrent-decisions')
        live = self.store.capture()
        self.assertEqual(live.document['readings'], {})
        self.assertEqual(live.state['subjects']['p.input']['acceptance'], 'contested')
        self.assertEqual(live.baseline['heads']['p.input'], [root['id']])
        self.assertEqual(live.baseline['open_acts']['p.input'], sorted([yes['id'], no['id']]))
        self.assertEqual(self.store.rebuild(write=True), self.store.rebuild())

    def test_contested_branch_union_rebuild_retains_both_alternatives(self):
        left = self.mutation([claim()], op='left')
        right_claim = claim(value=2, op='other')
        right = self.mutation([right_claim], op='right')
        self.store.commit(left, verify=lambda d: None)
        self.merge_fixture(right)
        rendered = C.decode_document(self.store.rebuild(write=True))
        self.assertEqual(rendered['readings'], {})
        self.assertEqual(len(rendered['meta']['history']['heads']['p.input']), 2)
        state = self.store.capture().state['subjects']['p.input']
        self.assertEqual(state['acceptance'], 'contested')
        self.assertEqual(sorted(body['v'] for body in state['bodies'].values()), [1, 2])

    def test_missing_parent_refuses(self):
        self.publish([claim()])
        self.publish([claim('p.other', op='other')], op='other')
        (Path(self.store.layout['history_commits']) / 'operation.yaml').unlink()
        with self.assertRaisesRegex(C.HistoryError, 'incomplete_commit'):
            self.store.capture()

    def test_custom_entry_uses_reader_layout(self):
        custom = H.Store(self.entry.parent / 'CUSTOM.yaml')
        self.assertEqual(Path(custom.layout['history']).name, 'PROVENANCE.history')

    def test_verifier_changes_are_rechecked_before_object_publication(self):
        prepared = self.mutation([claim()])
        def changed(_):
            document = C.decode_document(self.entry.read_bytes())
            document['record']['keep'] = False
            self.entry.write_bytes(C.encode_document(document))
        with self.assertRaisesRegex(C.HistoryError, 'stale_baseline'):
            self.store.commit(prepared, verify=changed)
        self.assertFalse(Path(self.store.layout['history_commits']).exists())
        self.assertFalse(Path(self.store.layout['history']).exists())
        self.assertFalse(C.decode_document(self.entry.read_bytes())['record']['keep'])

    def test_view_edit_during_publication_is_not_overwritten(self):
        prepared = self.mutation([claim()])
        original = T.publish_immutable
        edited = None
        def interleave(path, raw, **kwargs):
            nonlocal edited
            result = original(path, raw, **kwargs)
            if path.parent.name == 'history-commits':
                document = C.decode_document(self.entry.read_bytes())
                document['record']['keep'] = False
                edited = C.encode_document(document)
                self.entry.write_bytes(edited)
            return result
        with mock.patch.object(T, 'publish_immutable', side_effect=interleave):
            with self.assertRaisesRegex(C.HistoryError, 'concurrent_edit'):
                self.store.commit(prepared, verify=lambda d: None)
        self.assertEqual(self.entry.read_bytes(), edited)
        self.assertEqual(len(self.store.capture().objects), 1)

    def test_new_commit_refuses_hand_edit_even_with_unchanged_baseline(self):
        self.publish([claim()])
        document = C.decode_document(self.entry.read_bytes())
        document['readings']['p.input']['v'] = 999
        self.entry.write_bytes(C.encode_document(document))
        prepared = self.mutation([claim('p.other', op='other')], op='other')
        with self.assertRaisesRegex(C.HistoryError, 'unresolved_view_edit'):
            self.store.commit(prepared, verify=lambda d: None)
        self.assertEqual(C.decode_document(self.entry.read_bytes())['readings']['p.input']['v'], 999)
        self.assertEqual(len(self.store.capture().commits), 1)

    def test_after_image_scalar_must_match_committed_evidence(self):
        prepared = self.mutation([claim()])
        files = prepared.files
        record = next(f for f in files if f['role'] == 'record')
        document = C.decode_document(record['after'])
        document['readings']['p.input']['v'] = 999
        record['after'] = C.encode_document(document)
        manifest = next(f for f in files if f['role'] == 'history_commit')
        commit = C.decode_document(manifest['after'])
        commit['view_sha256'] = C.sha256(record['after'])
        manifest['after'] = C.encode_document(commit)
        bad = T.PreparedMutation(operation=prepared.to_data()['operation'], authority=self.marker,
                                 baseline=prepared.to_data()['baseline'], files=files,
                                 receipt=prepared.to_data()['receipt'])
        with self.assertRaisesRegex(C.HistoryError, 'view_projection_mismatch'):
            self.store.commit(bad, verify=lambda d: None)
        self.assertFalse(Path(self.store.layout['history_commits']).exists())

    def test_rebuild_repairs_stale_previous_manifest_view(self):
        self.publish([claim()])
        before = self.entry.read_bytes()
        prepared = self.mutation([claim('p.other', op='other')], op='other')
        with mock.patch.object(T, '_replace', side_effect=OSError('view failure')):
            with self.assertRaises(OSError):
                self.store.commit(prepared, verify=lambda d: None)
        self.assertEqual(self.entry.read_bytes(), before)
        rebuilt = self.store.rebuild(write=True)
        self.assertEqual(set(C.decode_document(rebuilt)['readings']), {'p.input', 'p.other'})
        self.assertEqual(self.store.rebuild(write=True), rebuilt)

    def merge_fixture(self, mutation):
        # A Git union brings immutable evidence from a separately prepared
        # branch; the generated entry intentionally remains from this branch.
        for item in mutation.files:
            if item['role'] != 'record':
                T.publish_immutable(self.entry.parent / item['path'], item['after'],
                                    root=self.entry.parent)

    def test_union_and_parent_order_independence_with_repeated_rebuild(self):
        left = self.mutation([claim()], op='left')
        right = self.mutation([claim('p.other', op='other')], op='right')
        self.store.commit(left, verify=lambda d: None)
        self.merge_fixture(right)
        live = self.store.capture()
        rendered = self.store.render(live)
        self.assertEqual(rendered, self.store.render(live, commits=dict(reversed(list(live.commits.items())))))
        self.assertEqual(self.store.rebuild(write=True), rendered)
        # The union view is not the intended view of either original manifest.
        self.assertFalse(self.store._known_view(self.store.capture()))
        self.assertEqual(self.store.rebuild(write=True), rendered)
        next_write = self.mutation([claim('p.third', op='third')], op='third')
        self.store.commit(next_write, verify=lambda d: None)
        manifest = C.decode_document(self.store.capture().commits['third'])
        self.assertEqual(set(manifest['parents']), {'left', 'right'})

    def test_frontier_template_disagreement_requires_reconciliation(self):
        left = self.mutation([claim()], op='left')
        right = self.mutation([claim('p.other', op='other')], op='right')
        files = right.files
        manifest = next(f for f in files if f['role'] == 'history_commit')
        commit = C.decode_document(manifest['after'])
        commit['view_template']['record']['keep'] = False
        manifest['after'] = C.encode_document(commit)
        # Publish fixture bytes directly: this is a union of two valid effect
        # templates, not permission for local writers to invent a resolution.
        self.store.commit(left, verify=lambda d: None)
        before = self.entry.read_bytes()
        for item in files:
            if item['role'] != 'record':
                T.publish_immutable(self.entry.parent / item['path'], item['after'], root=self.entry.parent)
        with self.assertRaisesRegex(C.HistoryError, 'contested_template'):
            self.store.rebuild(write=True)
        self.assertEqual(self.entry.read_bytes(), before)

    def test_corrupt_parent_hash_is_not_a_partial_generation(self):
        self.publish([claim()])
        self.publish([claim('p.other', op='other')], op='other')
        path = Path(self.store.layout['history_commits']) / 'operation.yaml'
        path.write_bytes(path.read_bytes() + b'\n')
        with self.assertRaisesRegex(C.HistoryError, 'parent_bytes_mismatch'):
            self.store.capture()

    def test_capture_and_reducer_limits_refuse_instead_of_truncating(self):
        with mock.patch.object(H, 'MAX_CAPTURE_BYTES', 1):
            with self.assertRaisesRegex(C.HistoryError, 'history_limit'):
                self.store.capture()
        with mock.patch.object(H, 'MAX_REDUCTION_WORK', 1):
            with self.assertRaisesRegex(C.HistoryError, 'history_limit'):
                H.reduce(held(claim(), claim(op='second')))

    def test_unknown_legacy_mapping_is_not_guessed(self):
        self.publish([claim()])
        old = V.version('p.legacy', 'reading', 'writer', {'v': 2}, op='old', on='2026-09-17')
        live = self.store.capture()
        with self.assertRaisesRegex(C.HistoryError, 'unresolved_mapping'):
            self.store.render(live, objects={**live.objects, old['id']: old})

    def test_accepted_profile_and_field_role_conflicts_are_not_flattened(self):
        self.publish([claim()])
        live = self.store.capture()
        for change, code in [('profile', 'incompatible_authored_profiles'),
                             ('fields', 'incompatible_field_roles')]:
            with self.subTest(change=change):
                other = claim('p.other', op='other')
                other['authored'][change] = ('ordinary-reader/v1' if change == 'profile'
                                             else {'value': 'amount'})
                other['id'] = C.object_identity(other)
                with self.assertRaisesRegex(C.HistoryError, code):
                    self.store.render(live, objects={**live.objects, other['id']: other})


if __name__ == '__main__':
    unittest.main()
