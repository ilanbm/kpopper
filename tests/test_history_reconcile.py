"""Explicit generated-view repair never invents acts or consumes manual edits."""
import unittest
from unittest import mock

from scripts import history_contract as C, history_store as H, history_transaction as T
from tests import test_history_store as fixtures


def conflict(ours, theirs, base=None, width=7):
    return (b'<' * width + b' HEAD\n' + ours +
            (b'|' * width + b' base\n' + base if base is not None else b'') +
            b'=' * width + b'\n' + theirs + b'>' * width + b' branch\n')


class Reconciliation(unittest.TestCase):
    setUp = fixtures.Storage.setUp
    mutation = fixtures.Storage.mutation
    publish = fixtures.Storage.publish
    merge_fixture = fixtures.Storage.merge_fixture

    def branches(self, same_subject=False):
        base = self.entry.read_bytes()
        left = self.mutation([fixtures.claim()], op='left')
        right = self.mutation([fixtures.claim('p.input' if same_subject else 'p.other',
                                             value=2, op='right-claim')], op='right')
        self.store.commit(left, verify=lambda data: None)
        self.merge_fixture(right)
        ours = next(f['after'] for f in left.files if f['role'] == 'record')
        theirs = next(f['after'] for f in right.files if f['role'] == 'record')
        return base, ours, theirs

    def files(self):
        return {p: p.read_bytes() for p in self.entry.parent.rglob('*') if p.is_file()}

    def test_conflicted_union_requires_opt_in_and_creates_no_acts(self):
        base, ours, theirs = self.branches(same_subject=True)
        self.entry.write_bytes(conflict(ours, theirs, base))
        before = self.files()
        with self.assertRaises(C.HistoryError):
            self.store.capture()
        with self.assertRaises(C.HistoryError):
            self.store.rebuild(write=True)
        prepared = self.store.prepare_reconciliation(allow_conflicts=True)
        self.assertTrue(prepared['rebuild_safe'])
        self.assertEqual([a['name'] for a in prepared['alternatives']], ['ours', 'theirs', 'base'])
        self.assertEqual(before, self.files())
        after = self.store.rebuild(write=True, allow_conflicts=True)
        self.assertEqual(C.decode_document(after)['readings'], {})
        current = self.store.capture()
        self.assertEqual(len(current.objects), 2)
        self.assertEqual(current.state['subjects']['p.input']['acceptance'], 'contested')
        self.assertEqual({p: b for p, b in before.items() if p != self.entry},
                         {p: b for p, b in self.files().items() if p != self.entry})

    def test_conflict_order_and_marker_width_do_not_select_winner(self):
        _, ours, theirs = self.branches()
        self.entry.write_bytes(conflict(ours, theirs, width=9))
        a = self.store.rebuild(allow_conflicts=True)
        self.entry.write_bytes(conflict(theirs, ours))
        self.assertEqual(a, self.store.rebuild(allow_conflicts=True))

    def test_multiple_hunks_reconstruct_original_sides(self):
        _, ours, theirs = self.branches()
        left, right = [raw.split(b'record:', 1) for raw in (ours, theirs)]
        raw = conflict(left[0], right[0]) + conflict(b'record:' + left[1], b'record:' + right[1])
        self.entry.write_bytes(raw)
        report = self.store.prepare_reconciliation(allow_conflicts=True)
        self.assertTrue(report['rebuild_safe'])
        self.assertEqual([a['entry_sha256'] for a in report['alternatives']],
                         [C.sha256(ours), C.sha256(theirs)])

    def test_marker_like_yaml_values_are_not_conflicts(self):
        obj = fixtures.claim(body={'v': '<< text', 'note': '<<<<<<< HEAD\n=======\n>>>>>>> branch'})
        self.publish([obj])
        report = self.store.prepare_reconciliation(allow_conflicts=True)
        self.assertFalse(report['conflicted'])
        self.assertTrue(report['rebuild_safe'])
        self.assertEqual(self.store.rebuild(allow_conflicts=True), self.entry.read_bytes())

    def test_invalid_marker_structure_refuses_without_writes(self):
        _, ours, theirs = self.branches()
        for raw in [conflict(ours, theirs).replace(b'=======\n', b'======\n'),
                    conflict(ours, theirs).replace(b'>>>>>>> branch\n', b''),
                    conflict(conflict(ours, theirs), theirs),
                    b'<<<<<<< HEAD\n' + ours]:
            with self.subTest(raw=raw[:30]):
                self.entry.write_bytes(raw)
                before = self.files()
                with self.assertRaises(C.HistoryError):
                    self.store.rebuild(write=True, allow_conflicts=True)
                self.assertEqual(before, self.files())

    def test_pending_body_and_header_edits_are_described_and_preserved(self):
        _, ours, theirs = self.branches()
        for path in [('readings', 'p.input', 'v'), ('record', 'keep'), ('meta', 'schema')]:
            doc = C.decode_document(ours)
            target = doc
            for key in path[:-1]:
                target = target[key]
            target[path[-1]] = 'manual pending value'
            raw = conflict(C.encode_document(doc), theirs)
            self.entry.write_bytes(raw)
            before = self.files()
            result = self.store.prepare_reconciliation(allow_conflicts=True)
            self.assertFalse(result['rebuild_safe'])
            change = result['alternatives'][0]['changes'][0]
            self.assertEqual(change['path'], list(path))
            self.assertEqual(change['after'], 'manual pending value')
            with self.assertRaisesRegex(C.HistoryError, 'unresolved_view_edit'):
                self.store.rebuild(write=True, allow_conflicts=True)
            self.assertEqual(before, self.files())

    def test_valid_manual_edits_are_baseline_bound_not_acts(self):
        root = fixtures.claim()
        self.publish([root])
        proposal = fixtures.claim(value=2, op='proposal', saw=[root['id']])
        self.publish([proposal], op='proposal')
        accepted = fixtures.act(proposal, over=[root['id']], op='accept')
        self.publish([accepted], op='accept')
        doc = C.decode_document(self.entry.read_bytes())
        doc['readings']['p.input']['v'] = 3
        doc['readings']['p.new'] = {'v': None}
        del doc['record']['keep']
        self.entry.write_bytes(C.encode_document(doc))
        before = self.files()
        result = self.store.prepare_reconciliation()
        alt = result['alternatives'][0]
        self.assertEqual(alt['recorded_acts'], [accepted['id']])
        self.assertEqual(alt['original_view']['readings']['p.input']['v'], 2)
        self.assertEqual(alt['original_versions']['p.input'], [proposal['id']])
        kinds = {tuple(c['path']): c['kind'] for c in alt['changes']}
        self.assertEqual(kinds[('readings', 'p.input', 'v')], 'proposal_candidate')
        self.assertEqual(kinds[('readings', 'p.new')], 'proposal_candidate')
        self.assertEqual(kinds[('record', 'keep')], 'header_edit')
        self.assertEqual(before, self.files())

    def test_comment_only_edits_are_not_discarded(self):
        _, ours, theirs = self.branches()
        self.entry.write_bytes(conflict(ours + b'# pending annotation\n', theirs))
        before = self.entry.read_bytes()
        report = self.store.prepare_reconciliation(allow_conflicts=True)
        self.assertEqual(report['alternatives'][0]['changes'][0]['kind'], 'text_edit')
        with self.assertRaisesRegex(C.HistoryError, 'unresolved_view_edit'):
            self.store.rebuild(write=True, allow_conflicts=True)
        self.assertEqual(self.entry.read_bytes(), before)

    def test_edit_matching_recorded_proposal_is_evidence_not_acceptance(self):
        root = fixtures.claim()
        self.publish([root])
        proposal = fixtures.claim(value=2, op='proposal', saw=[root['id']])
        self.publish([proposal], op='proposal')
        doc = C.decode_document(self.entry.read_bytes())
        doc['readings']['p.input']['v'] = 2
        self.entry.write_bytes(C.encode_document(doc))
        report = self.store.prepare_reconciliation()
        change = report['alternatives'][0]['changes'][0]
        self.assertEqual(change['recorded_claim_matches'], [proposal['id']])
        self.assertEqual(report['alternatives'][0]['recorded_acts'], [])
        self.assertEqual(self.store.capture().state['subjects']['p.input']['head'], root['id'])

    def test_common_lines_outside_conflict_remain_accounted_edits(self):
        _, ours, theirs = self.branches()
        documents = [C.decode_document(raw) for raw in (ours, theirs)]
        for doc in documents:
            del doc['record']
        raw = conflict(*(C.encode_document(d) for d in documents)) + b'record:\n  keep: edited\n'
        self.entry.write_bytes(raw)
        report = self.store.prepare_reconciliation(allow_conflicts=True)
        self.assertTrue(all(a['changes'][0]['after'] == 'edited' for a in report['alternatives']))
        with self.assertRaisesRegex(C.HistoryError, 'unresolved_view_edit'):
            self.store.rebuild(write=True, allow_conflicts=True)
        self.assertEqual(self.entry.read_bytes(), raw)

    def test_retained_capture_supplies_original_historical_union_closure(self):
        third = self.mutation([fixtures.claim('p.third', op='third-claim')], op='third')
        _, _, _ = self.branches()
        union = self.store.rebuild(write=True)
        retained = self.store.capture()
        self.merge_fixture(third)
        third_view = next(f['after'] for f in third.files if f['role'] == 'record')
        self.entry.write_bytes(conflict(union, third_view))
        with self.assertRaisesRegex(C.HistoryError, 'incomplete_view_baseline'):
            self.store.rebuild(allow_conflicts=True)
        rendered = self.store.rebuild(allow_conflicts=True, retained_baselines=[retained])
        self.assertEqual(set(C.decode_document(rendered)['readings']), {'p.input', 'p.other', 'p.third'})
        retained.commits['left'] += b'# tampered\n'
        with self.assertRaisesRegex(C.HistoryError, 'incomplete_view_baseline'):
            self.store.rebuild(allow_conflicts=True, retained_baselines=[retained])

    def test_reconciliation_search_has_explicit_resource_refusal(self):
        _, ours, theirs = self.branches()
        self.entry.write_bytes(conflict(ours, theirs))
        with mock.patch.object(H, 'MAX_RECONCILIATION_WORK', 0):
            with self.assertRaisesRegex(C.HistoryError, 'history_limit'):
                self.store.prepare_reconciliation(allow_conflicts=True)

    def test_removed_forged_or_incomplete_baseline_never_imports_roots(self):
        _, ours, theirs = self.branches()
        for alteration in ('remove', 'digest', 'heads'):
            doc = C.decode_document(ours)
            if alteration == 'remove':
                del doc['meta']['history']
            elif alteration == 'digest':
                doc['meta']['history']['committed_set_digest'] = '0' * 64
            else:
                doc['meta']['history']['heads'] = {}
            self.entry.write_bytes(conflict(C.encode_document(doc), theirs))
            before = self.files()
            with self.assertRaises(C.HistoryError):
                self.store.rebuild(write=True, allow_conflicts=True)
            self.assertEqual(before, self.files())

    def test_missing_committed_body_refuses_before_repair(self):
        _, ours, theirs = self.branches()
        self.entry.write_bytes(conflict(ours, theirs))
        next(p for p in self.entry.parent.rglob('*.yaml') if p.parent.name == 'p.input').unlink()
        with self.assertRaisesRegex(C.HistoryError, 'incomplete_commit'):
            self.store.rebuild(write=True, allow_conflicts=True)

    def test_capture_binds_exact_authority_bytes(self):
        marker = self.entry.parent / '.kpopper/history.yaml'
        marker.write_bytes(marker.read_bytes() + b'# retained original marker comment\n')
        self.assertEqual(self.store.capture().authority_bytes, marker.read_bytes())

    def test_stale_capture_and_edit_during_render_preserve_concurrent_bytes(self):
        _, ours, theirs = self.branches()
        self.entry.write_bytes(conflict(ours, theirs))
        captured = self.store.capture_reconciliation(allow_conflicts=True)
        concurrent = conflict(theirs, ours)
        self.entry.write_bytes(concurrent)
        with self.assertRaisesRegex(C.HistoryError, 'stale_baseline'):
            self.store.rebuild(captured, write=True, allow_conflicts=True)
        render = self.store.render
        def edit_during_render(*args, **kwargs):
            output = render(*args, **kwargs)
            self.entry.write_bytes(concurrent + b'# concurrent manual comment\n')
            return output
        with mock.patch.object(self.store, 'render', side_effect=edit_during_render):
            with self.assertRaisesRegex(C.HistoryError, 'stale_baseline'):
                self.store.rebuild(write=True, allow_conflicts=True)
        self.assertTrue(self.entry.read_bytes().endswith(b'# concurrent manual comment\n'))

    def test_replace_failure_preserves_conflict_and_retry_is_idempotent(self):
        _, ours, theirs = self.branches()
        raw = conflict(ours, theirs)
        self.entry.write_bytes(raw)
        with mock.patch.object(T, '_replace', side_effect=OSError('injected')):
            with self.assertRaisesRegex(OSError, 'injected'):
                self.store.rebuild(write=True, allow_conflicts=True)
        self.assertEqual(self.entry.read_bytes(), raw)
        first = self.store.rebuild(write=True, allow_conflicts=True)
        self.assertEqual(first, self.store.rebuild(write=True, allow_conflicts=True))


if __name__ == '__main__':
    unittest.main()
