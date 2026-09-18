"""Pure prospective snapshots for an already prepared history mutation.

The adapter consumes detached evidence only.  It never resolves a path, reads a
store, allocates time, or treats the candidate as committed authority.
"""
import copy
from dataclasses import replace

from . import history_adapter as A
from . import history_contract as C
from . import history_store as H
from . import history_transaction as T
from .reasoning.snapshot import Snapshot
from .pending_grounding import identity


_SUPPORTED = frozenset(('record', 'history_object', 'history_commit'))


def _fail(condition, code, detail=''):
    C._require(condition, code, detail)


def _prospective_capture(captured, mutation):
    _fail(isinstance(captured, H.Capture), 'invalid_capture')
    _fail(isinstance(mutation, T.PreparedMutation), 'invalid_mutation')
    data = mutation.to_data()
    _fail(identity(data['authority']) == identity(captured.marker), 'authority_mismatch')
    _fail(identity(data['baseline']) == identity(captured.baseline), 'baseline_mismatch')

    files = list(mutation.files)
    _fail(files, 'empty_mutation')
    by_role = {}
    for item in files:
        role = item['role']
        _fail(role in _SUPPORTED or role == 'history_evidence', 'unsupported_mutation_role', role)
        if role in ('history_object', 'history_evidence'):
            by_role.setdefault(role, []).append(item)
        else:
            _fail(role not in by_role, 'duplicate_mutation_role', role)
            by_role[role] = item
        _fail(item['before'] is None if role != 'record' else
              item['before'] == captured.entry_bytes, 'before_image_mismatch', role)
        if role not in ('record', 'history_evidence'):
            _fail(item['after'] is not None, 'invalid_mutation')
    _fail('record' in by_role and 'history_object' in by_role and 'history_commit' in by_role,
          'unsupported_mutation_shape')
    if 'history_evidence' in by_role:
        from . import history_branch
        _fail(data['receipt'].get('after', {}).get('history_branch_adoption') is not None,
              'invalid_branch_adoption_audit')
        # audit_evidences validates the retained envelope and all bindings from
        # mutation bytes alone; it performs no source Git/store reads.
        history_branch.audit_evidences(data)

    record = by_role['record']
    _fail(record['path'] == data['entry'], 'role_path_mismatch', record['path'])
    _fail(isinstance(record['after'], bytes), 'invalid_bytes')
    document = C.decode_document(record['after'])

    raw_objects = dict(captured.object_bytes)
    objects = dict(captured.objects)
    for item in by_role['history_object']:
        obj = C.validate_object(C.decode_document(item['after']))
        _fail(item['path'].endswith('.yaml'), 'role_path_mismatch', item['path'])
        key = (obj['subject'], obj['id'])
        # Adoption receipts repeat immutable objects already held by the target
        # to prove the adopted inventory. Identical bytes are not a rewrite.
        _fail(key not in raw_objects or key in captured.object_bytes
              and captured.object_bytes[key] == item['after'],
              'object_rewrite', obj['id'])
        raw_objects[key] = item['after']
        objects[obj['id']] = obj

    manifest_item = by_role['history_commit']
    manifest = C.validate_commit(C.decode_document(manifest_item['after']))
    _fail(manifest['operation'] == data['operation'], 'operation_mismatch')
    _fail(manifest['record_id'] == captured.marker['record_id'] and
          manifest['authority_generation'] == captured.marker['generation'], 'authority_mismatch')
    _fail(manifest.get('baseline_digest') == identity(captured.baseline), 'baseline_mismatch')
    _fail(manifest['parents'] == C.commit_frontier(captured.commits), 'parent_baseline_mismatch')
    commits = dict(captured.commits)
    _fail(data['operation'] not in commits, 'operation_already_prepared')
    commits[data['operation']] = manifest_item['after']
    _fail(not any(data['operation'] in generation['commits']
                  for generation in captured.inactive_generations.values()),
          'operation_collision')

    # This is the same all-or-refuse boundary used by publication.  In
    # particular, parent and object bytes are checked before any projection.
    selected = C.committed_objects(captured.marker, commits, raw_objects)
    _fail(sum(map(len, commits.values())) + sum(map(len, raw_objects.values()))
          <= H.MAX_CAPTURE_BYTES, 'history_limit')
    state = H.reduce(selected, captured.state['rules'])
    prospective_baseline = H.baseline(captured.marker, commits, state)
    _fail(C.validate_baseline(prospective_baseline) == prospective_baseline, 'invalid_baseline')
    meta = document.get('meta', {})
    _fail(isinstance(meta, dict) and identity(meta.get('history')) == identity(prospective_baseline),
          'baseline_mismatch')
    _fail(manifest.get('view_sha256') == C.sha256(record['after']), 'view_mismatch')
    virtual = replace(captured, entry_bytes=record['after'], document=document,
                   commits=commits, objects=selected, object_bytes=raw_objects,
                   state=state, baseline=prospective_baseline,
                   storage_bytes=dict(captured.storage_bytes),
                   object_paths=dict(captured.object_paths))
    # Use the canonical renderer itself so a caller cannot rehash a forged
    # record view and have it mistaken for the prepared operation's result.
    rendered = H.Store.__new__(H.Store).render(virtual, objects=selected, commits=commits)
    _fail(rendered == record['after'], 'view_mismatch')
    return virtual


def snapshot_after(captured, mutation, *, context=None, hypotheses=None, as_of=None):
    """Return an immutable supplied snapshot for ``mutation`` applied in memory."""
    for key in ('history', 'history_hypotheses', 'history_view', 'operation'):
        _fail(context is None or key not in context, 'duplicate_prospective_context', key)
    virtual = _prospective_capture(captured, mutation)
    adapted = A.from_store_capture(virtual)
    supplied = copy.deepcopy(context) if context is not None else {}
    supplied['read_mode'] = 'supplied'
    supplied['operation'] = {
        'version': 1,
        'phase': 'prospective',
        'kind': 'prepared_history',
        'original_baseline': copy.deepcopy(captured.baseline),
        'mutation_operation': mutation.to_data()['operation'],
    }
    return adapted.snapshot(context=supplied, hypotheses=hypotheses, as_of=as_of)


def assess(captured, mutation, *, hypotheses=None, as_of=None):
    """Compare committed authority with its prepared candidate, never a live overlay.

    Physical hypotheses may be supplied from the caller's verified capture. Named
    history hypotheses are reconstructed independently on each side. Pending
    contributions and publication policy remain the caller's separate boundary.
    """
    from .reasoning.context import CapturedAssessment
    from .reasoning.operations import findings
    context = {'read_mode': 'supplied', 'operation_scope': 'committed_history'}
    before = A.from_store_capture(captured).snapshot(context=context, hypotheses=hypotheses, as_of=as_of)
    after = snapshot_after(captured, mutation, context=context, hypotheses=hypotheses, as_of=before.to_data()['as_of'])
    before_context = CapturedAssessment.from_snapshot(before)
    after_context = CapturedAssessment.from_snapshot(after)
    old, new = findings(before_context), findings(after_context)
    introduced = {key: sorted(set(new[key]) - set(old[key]))
                  for key in ('falsified', 'holes', 'moved', 'notes')}
    return {'before': before_context, 'after': after_context,
            'before_findings': old, 'after_findings': new, 'introduced': introduced}
