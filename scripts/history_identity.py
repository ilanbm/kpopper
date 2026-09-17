"""Explicit same/distinct intents over immutable current claims and named proposals.

Old claims, seen, and review acts are never rewritten. A complete current identity
migration is one manifest; unsupported physical/ambiguous contexts refuse whole.
"""
import copy
import datetime
from dataclasses import replace
from pathlib import Path
import uuid

from . import history_contract as C, history_store as H, history_transaction as T
from . import history_adapter as D, history_authoring as A, history_hypotheses as HH
from . import provenance as P, sameness as S
from .pending_grounding import entries, identity
from . import history_paths as HP


_UNSET = object()


def _brief_text(raw):
    C._require(type(raw) is bytes, 'invalid_brief_encoding')
    try:
        return raw.decode('utf-8')
    except UnicodeDecodeError:
        raise C.HistoryError('invalid_brief_encoding', 'brief must be valid UTF-8') from None


def _rewrite_brief(raw, retired, survivor, fields):
    """Apply the established brief migration without manufacturing review evidence."""
    if raw is None:
        return None
    C._require(type(raw) is bytes and len(raw) <= C.MAX_REQUEST_BYTES, 'history_limit')
    text = _brief_text(raw)
    if not S._token(retired).search(text):
        return raw
    document = C.decode_document(raw)
    lines = text.split('\n')
    sections = [value for value in document.get('sections', []) or [] if isinstance(value, dict)]
    for tab in document.get('tabs', []) or []:
        if isinstance(tab, dict):
            sections += [value for value in tab.get('sections', []) or [] if isinstance(value, dict)]
    for section in sections:
        seen = section.get('seen')
        if isinstance(seen, dict) and retired in seen and survivor in seen and section.get('text'):
            span = P._section_span(lines, str(section.get('title')), having='text')
            C._require(span is not None, 'identity_ambiguous_brief_section')
            _, start, end = span
            S._drop_key(lines, start, end, 'seen', retired)
    labels = document.get('labels') or {}
    if isinstance(labels, dict) and retired in labels and survivor in labels:
        collections = {name: (start, end) for name, start, end in P._collections_in(lines)}
        C._require('labels' in collections, 'identity_ambiguous_brief_labels')
        start, end = collections['labels']
        indent, members = P._members_of(lines, start, end)
        for name, index in members:
            if name == retired:
                del lines[index:P._block_end(lines, index, indent, end)]
                break
    rewritten, _ = S._rewrite_text('\n'.join(lines), retired, survivor, fields['predicate'], fields['snapshot'])
    rewritten = S._dedupe_distinct(S._dedupe_flow_lists(rewritten, survivor), survivor).encode('utf-8')
    C.decode_document(rewritten)  # Never publish duplicate keys or an ambiguous rewrite.
    return rewritten


def _as_of(value):
    if value is None:
        return None
    try:
        day = datetime.date.fromisoformat(value) if type(value) is str else None
    except ValueError:
        day = None
    C._require(day is not None and day.isoformat() == value and day <= P.latest_today(), 'invalid_identity_as_of')
    return value


def _rewrite(body, retired, survivor, fields):
    """Rename identity references without manufacturing historical seen payloads."""
    if not isinstance(body, dict):
        raw, _ = S._rewrite_text(C.encode_document({'value': body}).decode(), retired, survivor)
        return C.decode_document(raw.encode())['value']
    body = copy.deepcopy(body)
    snapshot = fields.get('snapshot', 'seen')
    had_seen = snapshot in body
    seen = body.pop(snapshot, None) if snapshot in body else None
    dep_field = fields.get('deps', 'rests_on')
    dep_mapping = body.pop(dep_field) if isinstance(body.get(dep_field), dict) else None
    # Source paths and URI strings are provenance, not entry identifiers.
    protected = {key: body.pop(key) for key in ('file', 'path', 'url', 'uri') if key in body}
    raw, _ = S._rewrite_text(C.encode_document({'body': body}).decode(), retired, survivor,
                             fields.get('predicate', 'wrong_if'), snapshot)
    rewritten = C.decode_document(raw.encode())['body']
    rewritten.update(protected)
    if dep_mapping is not None:
        renamed = {}
        for dependency, version in dep_mapping.items():
            renamed.setdefault(survivor if dependency == retired else dependency, version)
        rewritten[dep_field] = renamed
    if seen is not None:
        C._require(isinstance(seen, dict), 'identity_invalid_seen')
        renamed = {}
        for dependency, evidence in seen.items():
            key = survivor if dependency == retired else dependency
            C._require(key not in renamed or identity(renamed[key]) == identity(evidence),
                       'identity_seen_collision', key)
            renamed[key] = copy.deepcopy(evidence)
        rewritten[snapshot] = renamed
    elif had_seen:
        rewritten[snapshot] = None
    deps = rewritten.get(dep_field)
    if isinstance(deps, list):
        rewritten[dep_field] = list(dict.fromkeys(deps))
    return rewritten


def _mentions(body, retired, survivor, fields):
    return identity(body) != identity(_rewrite(body, retired, survivor, fields))


def _capture(entry, capture=None):
    # HH._capture verifies HH.active_physical's exact import map and returns
    # only still-active physical files. Retained imported originals are allowed.
    store, captured, base, groups, index, physical = HH._capture(entry, capture)
    C._require(not physical, 'identity_physical_hypotheses_require_import')
    for name, group in groups.items():
        C._require(not group['error'], 'unresolved_history_hypothesis', name)
        for versions in index['groups'][name].values():
            for version in versions:
                authored = captured.objects[version]['authored']
                C._require(authored['profile'] == A.capabilities(base)['profile'], 'identity_mixed_group_profile')
                fields = A.READER.infer(base)[2]
                C._require(all(authored['fields'].get(role) == fields[role]
                               for role in ('deps', 'snapshot', 'predicate')), 'identity_mixed_group_fields')
    return store, captured, base, groups, index


def _world(document):
    ids, judgments, fields = A.READER.infer(document)
    world = A._world(document)
    raw = world.raw if world else P.with_builtins(document, ids, judgments, fields)
    return ids, judgments, fields, raw, world


def _merge(survivor, retired, kept, removed, document, as_of):
    ids, judgments, fields, raw, world = _world(document)
    C._require(isinstance(kept, dict) and isinstance(removed, dict), 'identity_requires_mapping_body')
    C._require(P._judgment_shaped(kept, fields) == P._judgment_shaped(removed, fields) or not kept,
               'identity_mixed_kind')
    if world is not None and kept:
        left, right = S._claim(kept, ids, raw), S._claim(removed, ids, raw)
        if not isinstance(left, dict) and not isinstance(right, dict):
            C._require(not P._same(left, right) or world.same(left, right), 'identity_typed_value_conflict')
    result, notes = S._merge(survivor, retired, copy.deepcopy(kept), copy.deepcopy(removed),
                              ids, judgments, fields, raw, as_of)
    if P._judgment_shaped(kept, fields):
        taking_removed = not P._same(P._verdict_of(result), P._verdict_of(kept))
    else:
        source_fields = set(S.READING) - {'unit'}
        taking_removed = not kept or identity({key: result[key] for key in source_fields if key in result}) != \
            identity({key: kept[key] for key in source_fields if key in kept})
    return result, notes, taking_removed


def _errors(document):
    world = A._world(document)
    if world is None:
        return set()
    report = world.assessment()
    return {(name, issue['code']) for name, node in report['nodes'].items()
            for issue in node['state']['integrity']['issues'] if issue['code'] != 'missing_snapshot'}


def _finish(store, captured, objects, template, intent, before, after, *, brief_before=_UNSET, receipt_version=None):
    archive, physical = A._archive(store), HH._physical_evidence(store)
    observed_view = T._read(store._path(store.layout['view']))
    view = observed_view if brief_before is _UNSET else brief_before
    view_after = view
    if intent['kind'] == 'same':
        survivor, retired = (intent['a'], intent['b']) if intent['keep'] in (None, 'a', intent['a']) else (intent['b'], intent['a'])
        view_after = _rewrite_brief(view, retired, survivor, A.READER.infer(before)[2])
    changed_view = view_after != view
    if receipt_version == 1:
        C._require(not changed_view, 'identity_brief_requires_migration')
    C._require(receipt_version in (None, 1, 2) and (receipt_version != 2 or changed_view), 'invalid_identity_receipt')
    intent = {**intent, 'version': 2 if changed_view else 1}
    if changed_view:
        intent['brief'] = {'path': Path(store.layout['view']).relative_to(store.root).as_posix(),
            'before_utf8': _brief_text(view), 'before_sha256': C.sha256(view), 'after_sha256': C.sha256(view_after)}
    view_hash = C.sha256(view) if view is not None else None
    before_evidence = A._evidence(before, A._world(before))
    before_evidence['identity_authoring'] = {**intent, 'baseline': captured.baseline,
                                             'archive': archive, 'physical': physical, 'view_sha256': view_hash}
    after_evidence = A._evidence(after, A._world(after))
    after_evidence['identity_authoring'] = {'objects': sorted(obj['id'] for obj in objects)}
    if changed_view:
        after_evidence['identity_authoring']['view_sha256'] = C.sha256(view_after)
    cap = A.capabilities(before)
    receipt = T.semantic_receipt(profile=cap['profile'], capabilities=cap,
                                  before=before_evidence, after=after_evidence)
    for obj in objects:
        if obj['kind'] != 'act':
            template.setdefault(obj['authored']['collection'], {})
    pairs = [(obj, C.encode_document(obj)) for obj in objects]
    args = dict(marker=captured.marker, operation=intent['operation'], parents=C.commit_frontier(captured.commits),
        baseline=captured.baseline, objects=pairs, receipt=receipt, view_template=template,
        requires=HP.commit_requires([C.EXPLICIT_ROOT_DISPOSITION]))
    C._require(intent['operation'] not in captured.commits, 'operation_already_prepared')
    draft = C.make_commit(**args, view=b'')
    combined = {**captured.objects, **{obj['id']: obj for obj in objects}}
    rendered = store.render(captured, objects=combined,
                            commits={**captured.commits, intent['operation']: C.encode_document(draft)})
    commits = {**captured.commits, intent['operation']: C.encode_document(draft)}
    state = H.reduce(combined, captured.state['rules'])
    candidate = replace(captured, entry_bytes=rendered, document=C.decode_document(rendered), objects=combined,
        commits=commits, state=state, baseline=H.baseline(captured.marker, commits, state),
        object_bytes={**captured.object_bytes, **{(obj['subject'], obj['id']): raw for obj, raw in pairs}})
    adapted = D.from_store_capture(candidate)
    HH.layers(adapted.projection, adapted.document)
    manifest = C.make_commit(**args, view=rendered)
    files = [{'role': 'record', 'path': store.entry.name, 'before': captured.entry_bytes, 'after': rendered}]
    files += [{'role': 'history_object', 'path': (Path(store.layout['history']) / HP.path_for_object(obj, captured)).relative_to(store.root).as_posix(), 'before': None, 'after': raw}
              for obj, raw in pairs]
    files += [{'role': 'history_commit', 'path': (Path(store.layout['history_commits']) /
                (intent['operation'] + '.yaml')).relative_to(store.root).as_posix(),
               'before': None, 'after': C.encode_document(manifest)}]
    if changed_view:
        files.append({'role': 'view', 'path': intent['brief']['path'], 'before': view, 'after': view_after})
    C._require(A._archive(store) == archive and HH._physical_evidence(store) == physical and
               T._read(store._path(store.layout['view'])) == observed_view, 'identity_source_changed')
    return T.PreparedMutation(operation=intent['operation'], authority=captured.marker, baseline=captured.baseline,
                              files=files, receipt=receipt, entry=store.entry.name)


def prepare_same(entry, a, b, *, keep=None, as_of=None, by=None, operation=None, recorded_at=None, capture=None,
                 _brief_before=_UNSET, _receipt_version=None):
    as_of = _as_of(as_of)
    store, captured, base, groups, index = _capture(entry, capture)
    C._require(a != b and not P.is_builtin(a) and not P.is_builtin(b), 'invalid_identity_subjects')
    survivor, retired = (a, b) if keep in (None, 'a', a) else (b, a) if keep in ('b', b) else (None, None)
    C._require(survivor is not None, 'invalid_identity_keep')
    operation = operation or 'history-same-' + uuid.uuid4().hex
    recorded_at = recorded_at or datetime.datetime.now(datetime.timezone.utc).isoformat()
    own = {'': {subject: captured.state['subjects'][subject]['heads'] for subject in entries(base)}}
    own.update(index['groups'])
    live = {subject for members in own.values() for subject in members}
    C._require(a in live and b in live, 'identity_subject_unavailable')
    raws = [P.bodies(base), *(group['raw'] for group in groups.values())]
    C._require(frozenset((a, b)) not in S.distinct_pairs(raws), 'identities_declared_distinct')
    fields = A.READER.infer(base)[2]
    template = store._template(captured.commits)
    # Header/routing changes require their own migration, not an identity guess.
    C._require(not _mentions(template, retired, survivor, fields), 'identity_template_requires_migration')
    specs, old_to_key, worlds = {}, {}, {}
    for name, versions in own.items():
        worlds[name] = base if not name else HH._layer(base, groups, [name])
        docs = entries(worlds[name])
        changed_head = None
        if name:
            changed_head = _rewrite(groups[name]['head'], retired, survivor, fields)
        for subject, ids in versions.items():
            prior = captured.objects[min(ids)]
            target = survivor if subject == retired else subject
            if subject == survivor and retired in versions:
                continue
            body = copy.deepcopy(prior['body'])
            authored = copy.deepcopy(prior['authored'])
            if subject == retired:
                kept = docs.get(survivor, (None, {}))[1]
                body, _, taking_removed = _merge(survivor, retired, kept, body, worlds[name], as_of or recorded_at[:10])
                kept_ids = versions.get(survivor) or own[''].get(survivor)
                if kept_ids:
                    kept_obj = captured.objects[min(kept_ids)]
                    if not taking_removed:
                        prior = kept_obj
                    authored = copy.deepcopy(prior['authored'])
                    authored['collection'] = kept_obj['authored']['collection']
            aliases = copy.deepcopy(body.get('also', [])) if isinstance(body, dict) and target == survivor else None
            rewritten = _rewrite(body, retired, survivor, fields)
            if target == survivor:
                C._require(isinstance(rewritten, dict), 'identity_requires_mapping_body')
                aliases = [aliases] if isinstance(aliases, str) else list(aliases or [])
                rewritten['also'] = list(dict.fromkeys([item for item in aliases if item != survivor] + [retired]))
            if name:
                authored['hypothesis'] = {'version': 1, 'name': name, 'head': changed_head}
            else:
                authored.pop('hypothesis', None)
            if target != subject or identity(rewritten) != identity(prior['body']) or identity(authored) != identity(prior['authored']):
                key = name, target
                old_ids = sorted(set(ids) | set(versions.get(survivor, []))) if subject == retired else list(ids)
                specs[key] = {'subject': target, 'body': rewritten, 'authored': authored, 'prior': prior,
                              'olds': old_ids, 'kind': prior['kind'], 'group': name}
                for old in old_ids:
                    old_to_key[name, old] = key
    # Every current pin to a changing version must become an explicitly authored
    # dependent version as well, even when its printed body did not change.
    changed = True
    while changed:
        changed = False
        for name, versions in own.items():
            for subject, ids in versions.items():
                if subject == retired or (name, subject) in specs:
                    continue
                prior = captured.objects[min(ids)]
                if any((name, version) in old_to_key or ('', version) in old_to_key for version in prior['pins'].values()):
                    key = name, subject
                    specs[key] = {'subject': subject, 'body': copy.deepcopy(prior['body']),
                                  'authored': copy.deepcopy(prior['authored']), 'prior': prior,
                                  'olds': list(ids), 'kind': prior['kind'], 'group': name}
                    for old in ids:
                        old_to_key[name, old] = key
                    changed = True
    created, visiting = {}, set()
    def build(key):
        if key in created:
            return created[key]
        C._require(key not in visiting, 'identity_pin_cycle')
        visiting.add(key)
        spec = specs[key]
        name, subject = key
        pins = {}
        for dep, version in spec['prior']['pins'].items():
            target = survivor if dep == retired else dep
            if dep == retired and survivor in spec['prior']['pins']:
                other = captured.objects[spec['prior']['pins'][survivor]]
                C._require(identity(C.claim_meaning(other)) == identity(C.claim_meaning(captured.objects[version])),
                           'identity_pin_collision')
            mapped = old_to_key.get((name, version), old_to_key.get(('', version)))
            if dep == retired:
                C._require(mapped is not None, 'historical_alias_pin_requires_resolution')
            result = build(mapped)['id'] if mapped is not None else version
            C._require(target not in pins or pins[target] == result, 'identity_pin_collision')
            pins[target] = result
        gaps = {}
        for dep, reason in spec['prior'].get('pin_gaps', {}).items():
            target = survivor if dep == retired else dep
            C._require(target not in pins and (target not in gaps or gaps[target] == reason), 'identity_pin_collision')
            gaps[target] = reason
        body = copy.deepcopy(spec['body'])
        deps_field = spec['authored']['fields'].get('deps', 'rests_on')
        deps = body.get(deps_field, []) if isinstance(body, dict) else []
        C._require(subject not in deps, 'identity_self_dependency', subject)
        if isinstance(deps, dict):
            body[deps_field] = {dep: pins.get(dep, value) for dep, value in deps.items()}
        obj = C.make_object(subject=subject, kind=spec['kind'], by=by, on=recorded_at, operation=operation,
            body=body, authored=spec['authored'], pins=pins, pin_gaps=gaps or None,
            saw=sorted(vid for vid, held in captured.objects.items() if held['subject'] == subject))
        created[key] = obj
        visiting.remove(key)
        return obj
    for key in sorted(specs):
        build(key)
    objects = list(created.values())
    for key, obj in created.items():
        spec = specs[key]
        olds = [vid for vid in spec['olds'] if captured.objects[vid]['subject'] == obj['subject']]
        objects.append(HH._act(captured, obj, 'propose' if spec['group'] else 'accept', operation,
            recorded_at, by, 'explicit same ' + retired + ' as ' + survivor,
            over=[] if spec['group'] else olds, extra=[obj['id']]))
        if spec['group']:
            for old in olds:
                objects.append(HH._act(captured, captured.objects[old], 'retire', operation, recorded_at, by,
                                       'identity migration within named hypothesis', extra=[obj['id']]))
    for name, versions in own.items():
        for old in versions.get(retired, []):
            objects.append(HH._act(captured, captured.objects[old], 'retire', operation, recorded_at, by,
                                   'explicit alias of ' + survivor))
    objects = list({obj['id']: obj for obj in objects}.values())
    after = copy.deepcopy(base)
    for members in P.collections_of(after).values():
        members.pop(retired, None)
    for (name, subject), obj in created.items():
        if not name:
            after.setdefault(obj['authored']['collection'], {})[subject] = copy.deepcopy(obj['body'])
    before_errors = _errors(base)
    C._require(_errors(after) <= before_errors, 'identity_computation_regression')
    for name in groups:
        candidate = copy.deepcopy(worlds[name])
        for members in P.collections_of(candidate).values():
            members.pop(retired, None)
        for (group, subject), obj in created.items():
            if not group or group == name:
                candidate.setdefault(obj['authored']['collection'], {})[subject] = copy.deepcopy(obj['body'])
        C._require(_errors(candidate) <= _errors(worlds[name]), 'identity_computation_regression', name)
    intent = {'version': 1, 'kind': 'same', 'a': a, 'b': b, 'keep': keep, 'by': by,
              'operation': operation, 'recorded_at': recorded_at}
    if as_of is not None:
        intent['as_of'] = as_of
    return _finish(store, captured, objects, template, intent, base, after,
                   brief_before=_brief_before, receipt_version=_receipt_version)


def prepare_distinct(entry, a, b, because, *, as_of=None, by=None, operation=None, recorded_at=None, capture=None):
    as_of = _as_of(as_of)
    store, captured, base, groups, index = _capture(entry, capture)
    C._require(a != b and not P.is_builtin(a) and not P.is_builtin(b), 'invalid_identity_subjects')
    C._require(isinstance(because, str) and because.strip(), 'identity_reason_required')
    own = {'': {subject: captured.state['subjects'][subject]['heads'] for subject in entries(base)}}
    own.update(index['groups'])
    live = {subject for versions in own.values() for subject in versions}
    C._require(a in live and b in live, 'identity_subject_unavailable')
    raws = [P.bodies(base), *(group['raw'] for group in groups.values())]
    C._require(frozenset((a, b)) not in S.distinct_pairs(raws), 'identities_already_distinct')
    operation = operation or 'history-distinct-' + uuid.uuid4().hex
    recorded_at = recorded_at or datetime.datetime.now(datetime.timezone.utc).isoformat()
    objects = []
    after = copy.deepcopy(base)
    for name, versions in own.items():
        if a not in versions:
            continue
        old_ids = versions[a]
        prior = captured.objects[min(old_ids)]
        body = copy.deepcopy(prior['body'])
        C._require(isinstance(body, dict), 'identity_requires_mapping_body')
        body['distinct_from'] = ', '.join(list(dict.fromkeys([*S._ids_in(body.get('distinct_from')), b])))
        obj = C.make_object(subject=a, kind=prior['kind'], by=by, on=recorded_at, operation=operation,
            body=body, authored=prior['authored'], pins=prior['pins'], pin_gaps=prior.get('pin_gaps'),
            saw=sorted(vid for vid, value in captured.objects.items() if value['subject'] == a))
        objects.extend([obj, HH._act(captured, obj, 'propose' if name else 'accept', operation, recorded_at,
            by, because, over=[] if name else old_ids, extra=[obj['id']])])
        if name:
            for old in old_ids:
                objects.append(HH._act(captured, captured.objects[old], 'retire', operation, recorded_at,
                                       by, 'explicit distinct metadata update', extra=[obj['id']]))
        else:
            after[obj['authored']['collection']][a] = copy.deepcopy(body)
    C._require(_errors(after) <= _errors(base), 'identity_computation_regression')
    intent = {'version': 1, 'kind': 'distinct', 'a': a, 'b': b, 'because': because,
              'operation': operation, 'recorded_at': recorded_at, 'by': by}
    if as_of is not None:
        intent['as_of'] = as_of
    return _finish(store, captured, objects, store._template(captured.commits), intent, base, after)


@HP.replay_mutation
def verify_prepared(entry, mutation):
    C._require(isinstance(mutation, T.PreparedMutation), 'invalid_mutation')
    store = H.Store(entry)
    live = store.capture()
    data = mutation.to_data()
    intent = data['receipt']['before'].get('identity_authoring')
    C._require(isinstance(intent, dict) and intent.get('version') in (1, 2), 'invalid_identity_receipt')
    C._require(A._archive(store) == intent['archive'] and HH._physical_evidence(store) == intent['physical'],
               'identity_source_changed')
    view = T._read(store._path(store.layout['view']))
    auxiliary = T.auxiliary_view(mutation)
    if intent['version'] == 2:
        C._require(auxiliary is not None and view in (auxiliary['before'], auxiliary['after']), 'identity_source_changed')
    else:
        C._require(auxiliary is None and (C.sha256(view) if view is not None else None) == intent['view_sha256'], 'identity_source_changed')
    manifest = C.decode_document(next(item['after'] for item in mutation.files if item['role'] == 'history_commit'))
    commits, pending = {}, list(manifest['parents'])
    while pending:
        operation = pending.pop()
        if operation not in commits:
            C._require(operation in live.commits, 'incomplete_commit')
            commits[operation] = live.commits[operation]
            pending.extend(C.decode_document(commits[operation])['parents'])
    selected = C.committed_objects(live.marker, commits, live.object_bytes)
    state = H.reduce(selected, live.state['rules'])
    record = next(item for item in mutation.files if item['role'] == 'record')
    captured = replace(live, entry_bytes=record['before'], document=C.decode_document(record['before']),
                       commits=commits, objects=selected, state=state, baseline=H.baseline(live.marker, commits, state))
    args = {'by': intent['by'], 'operation': intent['operation'], 'recorded_at': intent['recorded_at'], 'capture': captured}
    if 'as_of' in intent:
        args['as_of'] = intent['as_of']
    if intent['kind'] == 'same':
        expected = prepare_same(entry, intent['a'], intent['b'], keep=intent['keep'],
            _receipt_version=intent['version'],
            _brief_before=auxiliary['before'] if auxiliary is not None else _UNSET, **args)
    elif intent['kind'] == 'distinct':
        expected = prepare_distinct(entry, intent['a'], intent['b'], intent['because'], **args)
    else:
        raise C.HistoryError('invalid_identity_receipt')
    C._require(expected.to_bytes() == mutation.to_bytes(), 'identity_receipt_mismatch')


def commit(entry, mutation, *, verify):
    C._require(callable(verify), 'missing_verifier')
    def checked(data):
        verify_prepared(entry, mutation)
        verify(data)
        before = data['receipt']['before']['identity_authoring']
        C._require(A._archive(H.Store(entry)) == before['archive'] and
                   HH._physical_evidence(H.Store(entry)) == before['physical'], 'identity_source_changed')
        view = T._read(H.Store(entry)._path(H.Store(entry).layout['view']))
        auxiliary = T.auxiliary_view(mutation)
        if auxiliary is not None:
            C._require(view in (auxiliary['before'], auxiliary['after']), 'identity_source_changed')
        else:
            C._require((C.sha256(view) if view is not None else None) == before['view_sha256'], 'identity_source_changed')
    return H.Store(entry).commit(mutation, verify=checked)
