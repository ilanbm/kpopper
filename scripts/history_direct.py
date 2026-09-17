"""Routed direct history writes retain one private exact retry envelope."""
import copy
import json
import os
from pathlib import Path

from . import history_authoring as A, history_adapter, history_contract as C
from . import history_store as H, history_transaction as T, provenance as P, recording
from .pending_grounding import identity


def active(paths):
    if len(paths) != 1:
        return False
    path = Path(P.layout(paths[0])['history_authority'])
    if not path.exists():
        return False
    raw = path.read_bytes()
    try:
        return C.validate_authority(C.decode_document(raw))['authority'] == 'history'
    except C.HistoryError as error:
        raise P.Refused(error.code + ': ' + str(error)) from None


def journal(entry):
    # The committed manifest is authority while a view lags. A prepared history
    # operation must not trigger the legacy multi-image incomplete-state guard.
    return T.journal_for(entry) + '.history'


def _routing(original_paths, paths, project):
    return {'paths': list(map(os.path.realpath, original_paths)),
            'destination': list(map(os.path.realpath, paths)), 'policy': project.config()}


def _verify_routing(routing, original_paths, paths, project):
    current = P._peer('knowledge_views').write_paths(original_paths)
    C._require(identity(routing) == identity(_routing(original_paths, current, project))
               and list(map(os.path.realpath, paths)) == list(map(os.path.realpath, current)),
               'history_routing_changed')


def _envelope(mutation, routing):
    value = {'version': 1, 'kind': 'direct-history/v1', 'routing': routing,
             'mutation': T._blob(mutation.to_bytes())}
    return {**value, 'digest': identity(value)}


def _read_envelope(raw):
    value = C.decode_document(raw)
    C._mapping(value, ('version', 'kind', 'routing', 'mutation', 'digest'))
    C._require(type(value['version']) is int and value['version'] == 1
               and value['kind'] == 'direct-history/v1', 'invalid_history_journal')
    mutation = T.PreparedMutation.from_bytes(T._unblob(value['mutation']))
    C._require(identity(value) == identity(_envelope(mutation, value['routing'])),
               'invalid_history_journal')
    return mutation, value['routing']


def recover_report(paths, *, direction='after'):
    """Dispatch before taking a direct lock; report recovery owns its event lock."""
    if len(paths) != 1:
        return None
    entry = Path(paths[0]).resolve()
    pending = T._target(entry.parent, journal(entry))
    if not pending.is_file():
        return None
    value = C.decode_document(pending.read_bytes())
    if value.get('kind') != 'report-history/v1':
        return None
    C._mapping(value, ('version', 'kind', 'record', 'state_dir', 'event_id', 'mutation', 'digest'))
    C._require(type(value['version']) is int and value['version'] == 1
               and value['record'] == str(entry)
               and isinstance(value['state_dir'], str) and Path(value['state_dir']).is_absolute(),
               'invalid_history_journal')
    C._text(value['event_id'])
    C._require(value['digest'] == identity({key: val for key, val in value.items() if key != 'digest'}),
               'invalid_history_journal')
    T.PreparedMutation.from_bytes(T._unblob(value['mutation']))
    return P._peer('ingestion').recover_history(entry, value['state_dir'], value['event_id'], direction=direction)


def apply(paths, action, *, project, original_paths):
    """Caller holds policy plus complete record locks; no caller action is inferred."""
    entry = Path(paths[0]).absolute()
    pending = T._target(entry.parent, journal(entry))
    C._require(not pending.exists(), 'recovery_required', 'run recover for the retained history operation')
    routing = _routing(original_paths, paths, project)
    captured = H.Store(entry).capture()
    document = P.Record(history_adapter.from_store_capture(captured).document)
    document.get('meta', {}).pop('history', None)
    document.hypotheses = P.load_hypotheses(paths)
    action = copy.deepcopy(action)
    explicit = recording.ROUTING.intersection(action)
    scope = action.get('scope', 'unclear')
    scope_kind = scope.get('kind') if isinstance(scope, dict) else scope
    # The legacy router cannot publish a history operation as a YAML-only bundle.
    # Local code/feature actions and Simple writes retain its privacy policy.
    if explicit and project.config()['mode'] == 'advanced' and scope_kind in ('project', 'external'):
        raise P.Refused('history_contribution_write_pending: use an explicit complete history contribution')
    routed = recording.route(paths, action, A.READER, project=project,
                             expected_policy=routing['policy'], document=document)
    if routed is not None:
        print(json.dumps(routed, ensure_ascii=False))
        return 0
    mutation = A.prepare(entry, action, capture=captured)
    _verify_routing(routing, original_paths, paths, project)
    T.publish_immutable(pending.parent / '.gitignore', b'*\n', root=entry.parent)
    T.publish_immutable(pending, C.encode_document(_envelope(mutation, routing)), root=entry.parent)
    A.commit(entry, mutation, verify=lambda data: _verify_routing(routing, original_paths, paths, project))
    pending.unlink()
    T._sync(pending.parent)
    P.forget(entry)
    print('history committed: ' + mutation.to_data()['operation'] + ' (' + action['kind'] + ' ' + action['id'] + ')')
    return 0


def recover(paths, *, project, original_paths, direction='after'):
    entry = Path(paths[0]).absolute()
    pending = T._target(entry.parent, journal(entry))
    C._require(pending.is_file(), 'no_recovery_pending')
    raw = pending.read_bytes()
    mutation, routing = _read_envelope(raw)
    _verify_routing(routing, original_paths, paths, project)
    C._require(pending.read_bytes() == raw, 'concurrent_edit')
    if direction == 'before':
        live = H.Store(entry).capture()
        C._require(mutation.to_data()['operation'] not in live.commits,
                   'history_already_committed', 'committed evidence requires an explicit new act')
        A.verify_prepared(entry, mutation)
    else:
        A.commit(entry, mutation, verify=lambda data: _verify_routing(routing, original_paths, paths, project))
    pending.unlink()
    T._sync(pending.parent)
    P.forget(entry)
    return mutation
