"""Named immutable proposal groups; explicit folds/refutations use one manifest.

A named layer is computational context, never base acceptance. Physical hypothesis
files remain separate retained sources and names cannot silently change authority.
"""
import copy
import datetime
from dataclasses import replace
from pathlib import Path
import re
import uuid

from . import history_paths as HP
from . import history_contract as C, history_store as H, history_transaction as T
from . import history_adapter as D, history_authoring as A, provenance as P, recording
from .pending_grounding import identity, entries
from .reasoning.contract import capabilities

KIND = 'named-history-hypothesis/v1'


def _name(name):
    return C.hypothesis_name(name)


def layers(projection, document):
    """Derive sparse named layers exclusively from retained immutable witnesses."""
    projection = C.validate_projection(projection)
    groups = {}
    for subject, disposition in sorted(projection.get('dispositions', {}).items()):
        for version in disposition['proposals']:
            witness = projection['pins'].get(version)
            C._require(witness is not None and witness['status'] == 'recorded', 'missing_hypothesis_witness', version)
            obj = witness['object']
            group = obj.get('authored', {}).get('hypothesis')
            if group is None:
                continue
            name = _name(group['name'])
            item = groups.setdefault(name, {'versions': {}, 'heads': {}, 'claims': {}})
            item['versions'].setdefault(subject, []).append(version)
            item['heads'][identity(group['head'])] = group['head']
            item['claims'][version] = obj
    result, index = {}, {}
    for name, item in sorted(groups.items()):
        errors = []
        headers = {identity(obj['authored'].get('locator', {}).get('document_headers')):
                   obj['authored'].get('locator', {}).get('document_headers')
                   for obj in item['claims'].values()
                   if 'document_headers' in obj['authored'].get('locator', {})}
        profiles = {obj['authored']['profile'] for obj in item['claims'].values()}
        fields, conflicting_fields = {}, False
        for obj in item['claims'].values():
            for role, field in obj['authored']['fields'].items():
                if role in fields and fields[role] != field:
                    conflicting_fields = True
                fields[role] = field
        if len(headers) > 1 or len(profiles) != 1 or conflicting_fields:
            errors.append('contested hypothesis interpretation')
        if headers:
            body = copy.deepcopy(next(iter(headers.values())))
            C._require(isinstance(body, dict), 'invalid_hypothesis_headers')
        else:
            body = {'schema': copy.deepcopy(document.get('schema', {}))}
            declaration = document.get('meta', {}).get('reasoning')
            if declaration is not None:
                body['meta'] = {'reasoning': copy.deepcopy(declaration)}
        if len(item['heads']) != 1:
            errors.append('contested hypothesis head')
        for subject, versions in sorted(item['versions'].items()):
            meanings = {identity(C.claim_meaning(item['claims'][version])) for version in versions}
            if len(meanings) != 1:
                errors.append('contested hypothesis subject: ' + subject)
                continue
            obj = item['claims'][min(versions)]
            body.setdefault(obj['authored']['collection'], {})[subject] = D._adapt_body(obj)
        head = copy.deepcopy(next(iter(item['heads'].values()))) if len(item['heads']) == 1 else {}
        result[name] = {'kind': KIND, 'doc': body, 'head': head, 'error': '; '.join(errors) or None,
                        'ids': set(entries(body)), 'raw': P.bodies(body), 'name': name,
                        'profile': next(iter(profiles)) if len(profiles) == 1 else None,
                        'fields': fields if not conflicting_fields else None}
        index[name] = {subject: sorted(versions) for subject, versions in sorted(item['versions'].items())}
    return result, {'version': 1, 'groups': index}


def active_physical(document, entry, physical):
    """Exact imported files remain original evidence, not a second group authority."""
    mapping = document.get('meta', {}).get('history_hypothesis_import')
    if mapping is None:
        return physical
    from . import history_hypothesis_import as I
    entry = Path(entry).resolve()
    mapping = I.validate_mapping(mapping, entry=entry.name)
    result = dict(physical)
    for item in mapping['physical']:
        hypothesis = result.get(item['name'])
        path = T._target(entry.parent, item['path'])
        C._require(hypothesis is not None and Path(hypothesis['path']).resolve() == path,
                   'missing_imported_hypothesis', item['name'])
        raw = path.read_bytes()
        P._capture_event('bytes', str(path), raw)
        C._require(C.sha256(raw) == item['sha256'], 'imported_hypothesis_changed', item['name'])
        del result[item['name']]
    return result


def _capture(entry, captured=None):
    store = H.Store(entry)
    captured = captured or store.capture()
    C._require(captured.commits and captured.entry_bytes == store.render(captured), 'unresolved_view_edit')
    adapted = D.from_store_capture(captured)
    groups, index = layers(adapted.projection, adapted.document)
    physical = active_physical(adapted.document, store.entry, P.load_hypotheses([str(store.entry)]))
    C._require(not set(groups) & set(physical), 'hypothesis_authority_collision')
    # Any physical context is retained; new groups may not claim its name.
    return store, captured, A._document(adapted.document), groups, index, physical


def _guard_names(names, groups, physical, *, required=True):
    C._require(isinstance(names, (list, tuple)) and names and len(set(names)) == len(names), 'invalid_hypothesis_names')
    for name in names:
        _name(name)
        C._require(name not in physical, 'hypothesis_authority_collision', name)
        C._require(not required or name in groups, 'missing_history_hypothesis', name)


def _physical_evidence(store):
    """Keep physical-name authority and bytes in the prepared retry baseline."""
    import glob
    directory = Path(store.layout['hypotheses'])
    evidence, size = {}, 0
    for extension in ('*.yaml', '*.yml'):
        for path in sorted(P._capture_glob(glob.escape(str(directory)) + '/' + extension)):
            path = store._path(path)
            C._require(path.is_file() and path.stat().st_size <= C.MAX_REQUEST_BYTES, 'history_limit')
            raw = path.read_bytes()
            size += len(raw)
            C._require(size <= C.MAX_REQUEST_BYTES, 'history_limit')
            evidence[path.relative_to(store.root.resolve()).as_posix()] = C.sha256(raw)
    return evidence


def _selected(index, names):
    return {subject: versions for name in names for subject, versions in index['groups'].get(name, {}).items()}


def _layer(base, groups, names):
    document = P.Record(copy.deepcopy(base))
    document.hypotheses = copy.deepcopy(groups)
    for name in names:
        C._require(not groups[name]['error'], 'unresolved_history_hypothesis', name)
        document = P.layered(document, groups[name])
    return document


def _pins(captured, index, name, deps, *, blocked=False):
    pins, gaps = {}, {}
    for dependency in deps:
        versions = index['groups'].get(name, {}).get(dependency)
        if versions:
            meanings = {identity(C.claim_meaning(captured.objects[version])) for version in versions}
            C._require(len(meanings) == 1, 'unresolved_history_hypothesis', dependency)
            pins[dependency] = min(versions)
        elif dependency in captured.state['subjects']:
            pins[dependency] = A._head(captured, dependency)['id']
        else:
            C._require(blocked, 'unresolved_history_subject', dependency)
            gaps[dependency] = 'unavailable'
    return pins, gaps


def _act(captured, target, kind, operation, recorded_at, by, because, *, over=(), extra=(), read=None):
    body = {'act': kind, 'of': target['id'], 'over': sorted(over), 'because': because}
    if read is not None:
        body['read'] = read
    return C.make_object(subject=target['subject'], kind='act', by=by, on=recorded_at,
        operation=operation, saw=sorted({vid for vid, obj in captured.objects.items()
                                      if obj['subject'] == target['subject']} | set(extra)), body=body)


def _mutation(store, captured, objects, intent, before_document, after_document):
    operation = intent['operation']
    C._require(operation not in captured.commits, 'operation_already_prepared')
    cap = capabilities(before_document)
    archive = A._archive(store)
    physical = _physical_evidence(store)
    _guard_names([intent['name']] if intent['kind'] == 'edit' else intent['names'], {},
                 P.load_hypotheses([str(store.entry)]), required=False)
    intent = {**intent, 'archive': archive, 'physical': physical, 'baseline': captured.baseline}
    before = A._evidence(before_document, A._world(before_document))
    before['hypothesis_authoring'] = intent
    after = A._evidence(after_document, A._world(after_document))
    after['hypothesis_authoring'] = {'objects': sorted(obj['id'] for obj in objects)}
    receipt = T.semantic_receipt(profile=cap['profile'], capabilities=cap, before=before, after=after)
    template = store._template(captured.commits)
    for obj in objects:
        if obj['kind'] != 'act':
            template.setdefault(obj['authored']['collection'], {})
    pairs = [(obj, C.encode_document(obj)) for obj in objects]
    arguments = dict(marker=captured.marker, operation=operation, parents=C.commit_frontier(captured.commits),
        baseline=captured.baseline, objects=pairs, receipt=receipt, view_template=template,
        requires=HP.commit_requires([C.EXPLICIT_ROOT_DISPOSITION]))
    draft = C.make_commit(**arguments, view=b'')
    selected = {**captured.objects, **{obj['id']: obj for obj in objects}}
    commits = {**captured.commits, operation: C.encode_document(draft)}
    rendered = store.render(captured, objects=selected, commits=commits)
    manifest = C.make_commit(**arguments, view=rendered)
    files = [{'path': store.entry.name, 'role': 'record', 'before': captured.entry_bytes, 'after': rendered}]
    files.extend({'path': (Path(store.layout['history']) / HP.path_for_object(obj, captured)).relative_to(store.root).as_posix(),
                  'role': 'history_object', 'before': None, 'after': raw} for obj, raw in pairs)
    files.append({'path': (Path(store.layout['history_commits']) / (operation + '.yaml')).relative_to(store.root).as_posix(),
                  'role': 'history_commit', 'before': None, 'after': C.encode_document(manifest)})
    C._require(A._archive(store) == archive, 'concurrent_archive_edit')
    C._require(_physical_evidence(store) == physical, 'concurrent_hypothesis_edit')
    return T.PreparedMutation(operation=operation, authority=captured.marker, baseline=captured.baseline,
                              files=files, receipt=receipt, entry=store.entry.name)


def prepare(entry, name, action, *, head=None, by=None, operation=None, recorded_at=None, capture=None,
            _receipt_version=2):
    """Prepare add/set/review in one named layer; nothing is accepted into base."""
    store, captured, base, groups, index, physical = _capture(entry, capture)
    _guard_names([name], groups, physical, required=False)
    operation = operation or 'hypothesis-' + uuid.uuid4().hex
    recorded_at = recorded_at or datetime.datetime.now(datetime.timezone.utc).isoformat()
    action = C.detached(action)
    C._require(action.get('kind') in ('add', 'set', 'review') and not action.get('section'), 'invalid_hypothesis_action')
    C._require(action.get('hypothesis') in (None, name), 'hypothesis_name_mismatch')
    action['hypothesis'] = name
    action['as_of'] = action.get('as_of') or datetime.date.today().isoformat()
    if name in groups:
        C._require(not groups[name]['error'], 'unresolved_history_hypothesis', name)
        C._require(head is None or identity(head) == identity(groups[name]['head']), 'hypothesis_head_change_requires_group_edit')
        head = groups[name]['head']
    else:
        head = C.detached(head if head is not None else {'born': action['as_of']})
        C._require(isinstance(head, dict), 'invalid_history_hypothesis')
        groups[name] = {'name': name, 'kind': KIND, 'doc': {}, 'head': head, 'ids': set(), 'raw': {}, 'error': None}
    document = _layer(base, groups, [name])
    C._require(groups[name].get('profile') in (None, capabilities(document)['profile']),
               'history_hypothesis_profile_migration_required')
    C._require(action.get('profile') in (None, capabilities(document)['profile']),
               'history_profile_migration_required')
    before_document = copy.deepcopy(document)
    ids, judgments, fields = A.READER.infer(document)
    world = A._world(document)
    fields = {**P.authored_fields(action, fields), **(world.fields if world else {})}
    raw = world.raw if world else P.with_builtins(document, ids, judgments, fields)
    normalized, notes = P.normalize_authored(action, ids, fields, raw)
    validation_document = document
    if normalized['kind'] == 'add' and normalized['id'] in groups[name]['ids']:
        # A complete replacement inside this explicitly named proposal group is
        # another proposal version. Keep all structural/condition checks while
        # allowing its existing name; retirement below preserves the old body.
        validation_document = copy.deepcopy(document)
        validation_document.hypotheses[name]['ids'].discard(normalized['id'])
    refusals = P.validate(normalized, validation_document, ids, judgments, fields, raw)
    if refusals:
        raise P.Refused('refused - ' + '; '.join(refusals))
    C._require(_receipt_version in (1, 2), 'invalid_hypothesis_receipt')
    receipt_version = 2 if _receipt_version == 2 and world is not None else 1
    subject = normalized['id']
    previous = index['groups'].get(name, {}).get(subject, [])
    known = entries(document)
    objects = []
    if normalized['kind'] == 'review':
        C._require(previous, 'review_requires_group_proposal', subject)
        target = captured.objects[min(previous)]
        C._require(target['kind'] == 'judgment', 'invalid_review')
        C._require('_record_scope' not in normalized or
                   identity(normalized['_record_scope']) == identity(target['body'].get('scope')),
                   'history_review_scope_change_requires_claim')
        pins, gaps = _pins(captured, index, name, judgments[subject]['deps'], blocked=bool(P._blocked_text(target['body'])))
        C._require(not gaps, 'unavailable_review_pin', subject)
        objects = [_act(captured, captured.objects[version], 'review', operation, recorded_at, by,
                        normalized.get('why') or 'explicit hypothesis review', read=pins) for version in previous]
    else:
        if normalized['kind'] == 'set':
            collection, old = known[subject]
            body = recording.set_body(old, normalized)
            target = captured.objects[min(previous)] if previous else A._head(captured, subject)
            authored = copy.deepcopy(target['authored'])
        else:
            body = copy.deepcopy(normalized['body'])
            collection = known[subject][0] if subject in known else P._collection_for(
                document, ids, judgments, fields, subject, body, normalized.get('into'))
            authored = {'collection': collection, 'profile': capabilities(document)['profile'],
                        'fields': {role: field for role, field in fields.items() if field}}
            group_headers = {key: copy.deepcopy(value) for key, value in groups[name]['doc'].items()
                             if key in ('meta', 'schema', 'record', 'also')}
            if any('document_headers' in captured.objects[version].get('authored', {}).get('locator', {})
                   for versions in index['groups'].get(name, {}).values() for version in versions):
                authored['locator'] = {'document_headers': group_headers}
        if receipt_version == 2 and isinstance(body, dict) and fields['deps'] in body:
            body[fields['snapshot']] = P._snapshot(
                list(body[fields['deps']]), raw, ids, judgments, [], None)
            normalized['body'] = copy.deepcopy(body)
        authored['hypothesis'] = {'version': 1, 'name': name, 'head': copy.deepcopy(head)}
        deps = body.get(fields['deps'], []) if isinstance(body, dict) else []
        pins, gaps = _pins(captured, index, name, deps, blocked=bool(P._blocked_text(body)))
        claim = C.make_object(subject=subject, kind='judgment' if isinstance(body, dict) and fields['deps'] in body else 'reading',
            by=by, on=recorded_at, operation=operation, body=body, authored=authored, pins=pins,
            pin_gaps=gaps or None, saw=sorted(vid for vid, obj in captured.objects.items() if obj['subject'] == subject))
        objects = [claim, _act(captured, claim, 'propose', operation, recorded_at, by,
                               normalized.get('why') or 'explicit named hypothesis', extra=[claim['id']])]
        for version in previous:
            objects.append(_act(captured, captured.objects[version], 'retire', operation, recorded_at, by,
                                 'superseded within hypothesis ' + name, extra=[claim['id']]))
        document.setdefault(collection, {})[subject] = copy.deepcopy(body)
    intent = {'version': receipt_version, 'kind': 'edit', 'name': name, 'head': head, 'action': action,
              'operation': operation, 'recorded_at': recorded_at, 'by': by}
    return _mutation(store, captured, objects, intent, before_document, document)


def _finish(entry, names, because, *, kind, take=(), drops=None, by=None, operation=None,
            recorded_at=None, capture=None, _assessment_version=1):
    C._require(type(_assessment_version) is int and _assessment_version in (0, 1),
               'unsupported_hypothesis_assessment')
    store, captured, base, groups, index, physical = _capture(entry, capture)
    _guard_names(names, groups, physical)
    C._require(isinstance(because, str) and because.strip(), 'act_reason_required')
    C._require(isinstance(take, (list, tuple)) and all(isinstance(name, str) for name in take), 'invalid_hypothesis_take')
    operation = operation or 'hypothesis-' + kind + '-' + uuid.uuid4().hex
    recorded_at = recorded_at or datetime.datetime.now(datetime.timezone.utc).isoformat()
    names = sorted(names)
    chosen = {}
    for name in names:
        for subject, versions in index['groups'][name].items():
            C._require(subject not in chosen, 'overlapping_hypothesis_selection', subject)
            chosen[subject] = (name, versions)
    C._require(set(take) <= set(chosen), 'invalid_hypothesis_take')
    if kind == 'fold':
        prospective = _layer(base, groups, names)
        ids, judgments, fields = A.READER.infer(prospective)
        world = A._world(prospective)
        raw = world.raw if world else P.with_builtins(prospective, ids, judgments, fields)
        for name in names:
            head = groups[name]['head']
            C._require(head.get('folds') != 'never', 'hypothesis_never_folds', name)
            condition = head.get('wrong_if')
            C._require(not condition or P.evaluate(condition, raw, ids) is not True,
                       'hypothesis_head_falsified', name)
        base_ids, base_judgments, base_fields = A.READER.infer(base)
        before_world = A._world(base)
        base_raw = before_world.raw if before_world else P.with_builtins(base, base_ids, base_judgments, base_fields)
        existing = entries(base)
        for subject, (name, versions) in chosen.items():
            target = captured.objects[min(versions)]
            if target['kind'] == 'judgment':
                pred = P.predicate_of(target['body'], fields)
                C._require(not pred or P.evaluate(pred, raw, ids) is not True, 'hypothesis_falsified', subject)
            if subject not in captured.state['subjects'] or not captured.state['subjects'][subject]['heads']:
                continue
            C._require(subject in existing, 'unresolved_history_subject', subject)
            old = existing[subject][1]
            if identity(old) == identity(target['body']):
                continue
            allowed, reason = P.may_supersede(subject, old, target['body'], base_raw,
                base_ids, base_judgments, base_fields, recorded_at[:10], by_hand=subject in take)
            C._require(allowed, 'hypothesis_fold_requires_resolution', subject + ': ' + reason)
            if subject in base_judgments:
                gone = P.dropped_deps(old, target['body'], base_fields)
                C._require(all(isinstance((drops or {}).get(dep), str) and (drops or {})[dep].strip()
                               for dep in gone), 'hypothesis_drop_reason_required', subject)
        after_document = prospective
    else:
        # Explicit refutation can close every named conflicting alternative. It
        # touches only proposals, never the base heads or a different group.
        after_document = base
    objects = []
    for subject, (name, versions) in sorted(chosen.items()):
        for version in versions:
            target = captured.objects[version]
            objects.append(_act(captured, target, 'accept' if kind == 'fold' else 'refute',
                operation, recorded_at, by, because,
                over=captured.state['subjects'][subject]['heads'] if kind == 'fold' else []))
    intent = {'version': 1, 'kind': kind, 'names': names, 'because': because, 'take': sorted(take),
              'drops': C.detached(drops or {}), 'operation': operation, 'recorded_at': recorded_at, 'by': by}
    if kind == 'fold' and _assessment_version:
        intent['assessment_version'] = _assessment_version
    mutation = _mutation(store, captured, objects, intent, base, after_document)
    if kind == 'fold' and _assessment_version and capabilities(base)['profile'] == 'core/v1':
        from . import history_prospective
        checked = history_prospective.assess(captured, mutation, hypotheses=physical,
                                             as_of=recorded_at[:10])
        introduced = checked['introduced']
        C._require(not introduced['falsified'] and not introduced['holes'],
                   'hypothesis_candidate_not_clean',
                   '; '.join(introduced['falsified'] + introduced['holes']))
    return mutation


def prepare_fold(entry, names, *, because, take=(), drops=None, by=None, operation=None,
                 recorded_at=None, capture=None, _assessment_version=1):
    return _finish(entry, names, because, kind='fold', take=take, drops=drops, by=by,
                   operation=operation, recorded_at=recorded_at, capture=capture,
                   _assessment_version=_assessment_version)


def prepare_refute(entry, names, *, because, by=None, operation=None, recorded_at=None, capture=None):
    return _finish(entry, names, because, kind='refute', by=by, operation=operation,
                   recorded_at=recorded_at, capture=capture)


@HP.replay_mutation
def verify_prepared(entry, mutation):
    """Replay exact original intent against its complete committed parent closure."""
    C._require(isinstance(mutation, T.PreparedMutation), 'invalid_mutation')
    store = H.Store(entry)
    live = store.capture()
    data = mutation.to_data()
    intent = data['receipt']['before'].get('hypothesis_authoring')
    C._require(isinstance(intent, dict) and intent.get('version') in (1, 2),
               'invalid_hypothesis_receipt')
    C._require(A._archive(store) == intent['archive'], 'concurrent_archive_edit')
    C._require(_physical_evidence(store) == intent['physical'], 'concurrent_hypothesis_edit')
    manifest = C.decode_document(next(item['after'] for item in mutation.files if item['role'] == 'history_commit'))
    commits, pending = {}, list(manifest['parents'])
    while pending:
        operation = pending.pop()
        if operation not in commits:
            C._require(operation in live.commits, 'incomplete_commit', operation)
            commits[operation] = live.commits[operation]
            pending.extend(C.decode_document(commits[operation])['parents'])
    objects = C.committed_objects(live.marker, commits, live.object_bytes)
    state = H.reduce(objects, live.state['rules'])
    before = next(item['before'] for item in mutation.files if item['role'] == 'record')
    captured = replace(live, entry_bytes=before, document=C.decode_document(before), commits=commits,
                       objects=objects, state=state, baseline=H.baseline(live.marker, commits, state))
    kwargs = {'capture': captured, 'operation': intent['operation'], 'recorded_at': intent['recorded_at'], 'by': intent['by']}
    if intent['kind'] == 'edit':
        expected = prepare(entry, intent['name'], intent['action'], head=intent['head'],
                           _receipt_version=intent['version'], **kwargs)
    elif intent['kind'] == 'fold':
        expected = prepare_fold(entry, intent['names'], because=intent['because'], take=intent['take'], drops=intent['drops'],
                                _assessment_version=intent.get('assessment_version', 0), **kwargs)
    elif intent['kind'] == 'refute':
        expected = prepare_refute(entry, intent['names'], because=intent['because'], **kwargs)
    else:
        raise C.HistoryError('invalid_hypothesis_receipt')
    C._require(expected.to_bytes() == mutation.to_bytes(), 'hypothesis_receipt_mismatch')


def commit(entry, mutation, *, verify):
    """Routing/privacy authorization remains mandatory at the shared writer boundary."""
    C._require(callable(verify), 'missing_verifier')
    def checked(data):
        verify_prepared(entry, mutation)
        verify(data)
        C._require(A._archive(H.Store(entry)) == data['receipt']['before']['hypothesis_authoring']['archive'],
                   'concurrent_archive_edit')
        C._require(_physical_evidence(H.Store(entry)) == data['receipt']['before']['hypothesis_authoring']['physical'],
                   'concurrent_hypothesis_edit')
    return H.Store(entry).commit(mutation, verify=checked)
