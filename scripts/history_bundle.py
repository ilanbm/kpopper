"""Explicit, bounded, source-free transport of complete committed history.

Commit manifests are indivisible: an export refuses when their complete ancestry
carries a subject outside the explicitly authorized roots. Staging
objects and the private capture inventory never travel. Digests establish
consistency of captured evidence, not authenticity or publication permission.
"""
import copy

from . import history_contract as C, history_store as H, history_adapter as A
from . import pending_grounding as G

CAPABILITY = 'history-closure/v1'
MAX_BYTES = H.MAX_CAPTURE_BYTES
PREFIX = 'history-closure/'


def _files(files):
    C._require(isinstance(files, dict) and len(files) <= 2 * C.MAX_OBJECTS + 2,
               'history_limit')
    total = 0
    for path, raw in files.items():
        C.relative_path(path)
        C._require(type(raw) is bytes, 'invalid_bytes')
        total += len(raw)
        C._require(len(raw) <= C.MAX_REQUEST_BYTES and total <= MAX_BYTES, 'history_limit')
    G._portable_files(files)


def _authorization(captured, roots, scope, shareability, *, subset=False):
    C._require(shareability == 'project', 'history_not_shareable')
    C._require(isinstance(roots, list) and roots and roots == sorted(set(roots)),
               'invalid_history_roots')
    for subject in roots:
        C._text(subject, C.SUBJECT)
    C._require(isinstance(scope, dict) and scope.get('kind') in ('project', 'external', 'code')
               and isinstance(scope.get('environment'), str) and scope['environment'].strip(),
               'invalid_history_scope')
    if scope['kind'] == 'code':
        import re
        C._require(re.fullmatch(r'[0-9a-f]{40}|[0-9a-f]{64}', str(scope.get('commit', ''))),
                   'invalid_history_scope')
    G._privacy(scope)
    subjects = {obj['subject'] for obj in captured.objects.values()}
    C._require(set(roots) <= subjects, 'invalid_history_roots')
    C._require(subset or set(roots) == subjects, 'history_export_requires_full_authorization',
               ', '.join(sorted(subjects - set(roots))))
    document = A.from_store_capture(captured).document
    for subject, (_, body) in G.entries(document).items():
        if subset and subject not in roots:
            continue
        C._require(isinstance(body, dict) and G.identity(body.get('scope')) == G.identity(scope),
                   'history_scope_mismatch', subject)
    if subset:
        C._require(set(roots) <= set(G.entries(document)), 'unavailable_subset_root')
    return document


def _capture(files, rules):
    marker = C.validate_authority(C.decode_document(files['authority.yaml']))
    document = C.decode_document(files['entry.yaml'])
    commits, objects = {}, {}
    for path, raw in files.items():
        parts = path.split('/')
        if len(parts) == 2 and parts[0] == 'commits' and parts[1].endswith('.yaml'):
            op = parts[1][:-5]
            C._text(op)
            commits[op] = raw
        elif len(parts) == 3 and parts[0] == 'objects' and parts[2].endswith('.yaml'):
            subject, version = parts[1], parts[2][:-5]
            C._text(subject, C.SUBJECT)
            C._text(version, C.OBJECT_ID)
            objects[(subject, version)] = raw
        else:
            C._require(path in ('authority.yaml', 'entry.yaml'), 'invalid_history_bundle_path')
    selected = C.committed_objects(marker, commits, objects)
    C._require(set(objects) == {(o['subject'], vid) for vid, o in selected.items()},
               'history_bundle_membership', 'uncommitted objects are not portable authority')
    state = H.reduce(selected, rules=rules)
    baseline = H.baseline(marker, commits, state)
    C.bind_authority(marker, document.get('meta', {}).get('history'))
    captured = H.Capture(files['entry.yaml'], document, marker, commits, objects,
                         selected, state, baseline, {}, authority_bytes=files['authority.yaml'])
    # These methods consume only the supplied evidence; never open a store.
    store = object.__new__(H.Store)
    rendered = store.render(captured)
    C._require(store._known_view(captured) or
               G.identity(C.decode_document(rendered)) == G.identity(document),
               'unresolved_view_edit')
    for role, kind in (('heads', 'claim'), ('open_acts', 'act')):
        for subject, versions in document['meta']['history'][role].items():
            for version in versions:
                obj = selected.get(version)
                C._require(obj is not None and obj['subject'] == subject and
                           (obj['kind'] == 'act') == (kind == 'act'),
                           'incomplete_view_baseline')
    return captured


def validate(artifact):
    """Validate complete bytes/membership and return detached committed capture."""
    C._require(isinstance(artifact, dict) and set(artifact) == {'revision', 'manifest', 'files'},
               'invalid_history_bundle')
    manifest = C.detached(artifact['manifest'], C.MAX_REQUEST_BYTES)
    C._mapping(manifest, ('version', 'requires', 'roots', 'scope', 'shareability', 'rules',
                          'baseline', 'files'))
    C._require(type(manifest['version']) is int and manifest['version'] in (1, 2)
               and manifest['requires'] == ([CAPABILITY] if manifest['version'] == 1 else
                                            [CAPABILITY, SUBSET_CAPABILITY]), 'unsupported_history_bundle')
    C._require(G.identity(manifest) == artifact['revision'], 'history_bundle_identity')
    files = artifact['files']
    _files(files)
    C._require(isinstance(manifest['files'], dict) and set(files) == set(manifest['files'])
               and {'authority.yaml', 'entry.yaml'} <= set(files), 'history_bundle_membership')
    for path, raw in files.items():
        C._require(C.sha256(raw) == manifest['files'][path], 'history_bundle_checksum', path)
        G._privacy(C.decode_document(raw))
    captured = _capture(files, manifest['rules'])
    C._require(G.identity(captured.state['rules']) == G.identity(manifest['rules']), 'rules_mismatch')
    C._require(G.identity(captured.baseline) == G.identity(manifest['baseline']), 'baseline_mismatch')
    if manifest['version'] == 2:
        _validate_subset(captured, manifest)
    else:
        C._require('history_subset' not in captured.document.get('meta', {}), 'subset_capability_required')
    _authorization(captured, manifest['roots'], manifest['scope'], manifest['shareability'],
                   subset=manifest['version'] == 2)
    return captured


def export(captured, *, roots, scope, shareability, authority_bytes=None):
    """Export an already captured complete closure, with explicit sharing consent.

    The exact authority bytes must be retained by the reader or explicitly
    supplied, and must match its private captured inventory. No live reads occur.
    """
    C._require(isinstance(roots, (list, tuple)) and roots and all(isinstance(s, str) for s in roots),
               'invalid_history_roots')
    raw = authority_bytes if authority_bytes is not None else getattr(captured, 'authority_bytes', None)
    C._require(type(raw) is bytes, 'missing_captured_authority_bytes')
    C._require(G.identity(C.decode_document(raw)) == G.identity(captured.marker), 'authority_mismatch')
    digests = {digest for (kind, _), digest in captured.inventory.items() if kind == 'bytes'}
    C._require(C.sha256(raw) in digests, 'uncaptured_authority_bytes')
    files = {'authority.yaml': raw, 'entry.yaml': captured.entry_bytes}
    files.update({'commits/' + op + '.yaml': data for op, data in captured.commits.items()})
    files.update({'objects/' + obj['subject'] + '/' + vid + '.yaml':
                  captured.object_bytes[(obj['subject'], vid)] for vid, obj in captured.objects.items()})
    _files(files)
    manifest = {'version': 1, 'requires': [CAPABILITY], 'roots': sorted(set(roots)),
                'scope': copy.deepcopy(scope), 'shareability': shareability,
                'rules': copy.deepcopy(captured.state['rules']), 'baseline': copy.deepcopy(captured.baseline),
                'files': {path: C.sha256(data) for path, data in sorted(files.items())}}
    artifact = {'revision': G.identity(manifest), 'manifest': manifest, 'files': files}
    validate(artifact)
    return artifact


def adapt(artifact):
    """Replay using only captured portable bytes, preserving original profiles."""
    return A.from_store_capture(validate(artifact))


def from_contribution(bundle):
    """Extract the v3 manifest-bound artifact; validation remains explicit."""
    binding = bundle['manifest']['history']
    return {'revision': binding['revision'], 'manifest': binding['manifest'],
            'files': {path: bundle['files'][PREFIX + path]
                      for path in binding['manifest']['files'] if PREFIX + path in bundle['files']}}


def matches_document(captured, document):
    """Match an observed, regenerated or explicitly adapted view of this capture."""
    rendered = C.decode_document(object.__new__(H.Store).render(captured))
    adapted = A.from_store_capture(captured).document
    return G.identity(document) in {G.identity(captured.document), G.identity(rendered), G.identity(adapted)}


SUBSET_CAPABILITY = 'history-subset/v1'


def _subject_inventory(captured, subject):
    return {vid: C.sha256(captured.object_bytes[(subject, vid)])
            for vid, obj in sorted(captured.objects.items()) if obj['subject'] == subject}


def _locator_disclosures(objects, source_entry, disclosed):
    """Consent covers exact path/hash metadata, never permission to read files."""
    C.relative_path(source_entry)
    C._require(isinstance(disclosed, list), 'invalid_locator_disclosures')
    allowed = set()
    for item in disclosed:
        C._mapping(item, ('path', 'sha256'))
        C.relative_path(item['path'])
        C._require(item['sha256'] is None or C.HEX.fullmatch(str(item['sha256'])), 'invalid_locator_disclosures')
        allowed.add((item['path'], item['sha256']))
    used = set()
    def walk(value):
        if isinstance(value, dict):
            if 'path' in value:
                path = C.relative_path(value['path'])
                digest = value.get('sha256')
                if path != source_entry:
                    C._require((path, digest) in allowed, 'undisclosed_history_locator')
                    used.add((path, digest))
            for item in value.values():
                walk(item)
        elif isinstance(value, list):
            for item in value:
                walk(item)
    for obj in objects.values():
        walk(obj.get('authored', {}).get('locator'))
        walk(obj.get('at'))
    C._require(used == allowed, 'unused_locator_disclosure')


def _subset_subjects(captured, roots):
    """All versions of each subject, then positive-reference fixed point."""
    C._require(isinstance(roots, (list, tuple)) and roots and all(isinstance(x, str) for x in roots),
               'invalid_history_roots')
    document = A.from_store_capture(captured).document
    plain = copy.deepcopy(document)
    plain['meta'].pop('history')
    by_subject = {}
    for obj in captured.objects.values():
        by_subject.setdefault(obj['subject'], []).append(obj)
    selected, pending = set(), list(roots)
    _, language, _, _ = G._reasoning_modules()
    while pending:
        subject = pending.pop()
        if subject in selected:
            continue
        C._require(subject in by_subject, 'incomplete_subset_dependency', subject)
        selected.add(subject)
        deps = set()
        for obj in by_subject[subject]:
            deps.update(name for name, _, _ in C.references(obj))
            body = obj['body']
            if not isinstance(body, dict) or obj['kind'] == 'act':
                continue
            fields = obj['authored']['fields']
            declared = body.get(fields.get('deps', 'rests_on'), [])
            C._require(isinstance(declared, (dict, list)), 'invalid_subset_dependencies')
            deps.update(declared)
            seen = body.get(fields.get('snapshot', 'seen'), {})
            C._require(isinstance(seen, dict), 'invalid_subset_dependencies')
            deps.update(seen)
            deps.update(obj.get('pin_gaps', {}))
            for text in G._strings(body):
                deps.update(G.P.refs_in(text))
            deps.update(x for x in G.P._mentioned(body) if x in by_subject)
            for field in ('rule', fields.get('predicate', 'wrong_if')):
                if isinstance(body.get(field), dict):
                    deps.update(language.references(language.lower(body[field])))
            definition = body.get('collection_scope')
            if definition is not None:
                C._require(isinstance(definition, dict) and isinstance(definition.get('collection'), str),
                           'invalid_subset_scope')
                deps.update(plain.get(definition['collection'], {}))
        for dep in deps:
            C._require(isinstance(dep, str) and not G.P.is_builtin(dep), 'unsupported_subset_dependency')
        pending.extend(sorted(deps - selected))
    return selected


def _candidate(captured, prepared):
    from dataclasses import replace
    from . import history_transaction as T
    if prepared is None:
        return captured, None
    mutation = T.PreparedMutation.from_bytes(prepared.to_bytes())
    data = mutation.to_data()
    C._require(G.identity(data['authority']) == G.identity(captured.marker) and
               G.identity(data['baseline']) == G.identity(captured.baseline), 'stale_baseline')
    commits, raw_objects = dict(captured.commits), dict(captured.object_bytes)
    entry = None
    for item in mutation.files:
        if item['role'] == 'history_object':
            obj = C.validate_object(C.decode_document(item['after']))
            key = obj['subject'], obj['id']
            C._require(key not in raw_objects or raw_objects[key] == item['after'], 'object_bytes_mismatch')
            raw_objects[key] = item['after']
        elif item['role'] == 'history_commit':
            manifest = C.validate_commit(C.decode_document(item['after']))
            C._require(manifest['operation'] not in commits, 'operation_already_prepared')
            commits[manifest['operation']] = item['after']
        elif item['role'] == 'record':
            C._require(item['before'] == captured.entry_bytes, 'stale_baseline')
            entry = item['after']
        elif item['role'] == 'history_evidence':
            # PreparedMutation validates confined paths and receipt byte hashes.
            # Portable source files travel in G.prepare's explicit evidence map,
            # never as knowledge objects or as the generated record.
            continue
        else:
            raise C.HistoryError('unsupported_subset_mutation')
    C._require(entry is not None, 'invalid_subset_mutation')
    objects = C.committed_objects(captured.marker, commits, raw_objects)
    state = H.reduce(objects, captured.state['rules'])
    candidate = replace(captured, commits=commits, object_bytes=raw_objects, objects=objects,
                        state=state, baseline=H.baseline(captured.marker, commits, state),
                        entry_bytes=entry, document=C.decode_document(entry))
    C._require(object.__new__(H.Store).render(candidate) == entry, 'unresolved_view_edit')
    return candidate, C.sha256(mutation.to_bytes())


def prepare_subset(captured, roots, *, scope, shareability, operation, recorded_at,
                   source_entry, disclosed_locators=(), prepared=None):
    """Prepare an independent transport observation, never source/target adoption.

    Source membership is a capture attestation. Original commit manifests and
    mutation receipts remain private; only opaque digests cross this boundary.
    """
    from dataclasses import replace
    from . import history_transaction as T
    C._text(operation)
    C._require(isinstance(recorded_at, str) and recorded_at, 'invalid_recorded_time')
    G._privacy({key: captured.document[key] for key in
                ('meta', 'privacy', 'visibility', 'private', 'shareability') if key in captured.document})
    # Re-establish committed object identity even for caller-supplied captures.
    C.relative_path(source_entry)
    C._require(any(kind == 'bytes' and path.replace('\\', '/').endswith('/' + source_entry) and
                   digest == C.sha256(captured.entry_bytes)
                   for (kind, path), digest in captured.inventory.items()), 'uncaptured_source_entry')
    verified = C.committed_objects(captured.marker, captured.commits, captured.object_bytes)
    C._require(G.identity(verified) == G.identity(captured.objects), 'invalid_source_capture')
    C._require(G.identity(H.reduce(verified, captured.state['rules'])) == G.identity(captured.state),
               'source_reduction_mismatch')
    candidate, mutation_digest = _candidate(captured, prepared)
    subjects = _subset_subjects(candidate, roots)
    objects = {vid: obj for vid, obj in candidate.objects.items() if obj['subject'] in subjects}
    raw_objects = {(obj['subject'], vid): candidate.object_bytes[(obj['subject'], vid)]
                   for vid, obj in objects.items()}
    disclosures = sorted(copy.deepcopy(list(disclosed_locators)), key=lambda item: (item['path'], str(item['sha256'])))
    _locator_disclosures(objects, source_entry, disclosures)
    state = H.reduce(objects, candidate.state['rules'])
    for subject in subjects:
        C._require(G.identity(state['subjects'][subject]) == G.identity(candidate.state['subjects'][subject]),
                   'subset_reduction_mismatch')
    source_projection = A.from_store_capture(captured).projection
    origin = {'version': 1, 'kind': 'selected-subject-observation', 'operation': operation,
              'recorded_at': recorded_at, 'source_authority': captured.marker,
              'source_capture_digest': source_projection['closure_digest'], 'source_entry': source_entry,
              'roots': sorted(set(roots)), 'disclosed_locators': disclosures,
              'prepared_digest': mutation_digest,
              'subjects': {subject: {'source_state': 'prepared_candidate' if any(
                  obj['subject'] == subject and vid not in captured.objects for vid, obj in objects.items()) else 'committed',
                  'objects_digest': G.identity(_subject_inventory(candidate, subject)),
                  'reduction_digest': G.identity(state['subjects'][subject])} for subject in sorted(subjects)}}
    C.validate_subset_origin(origin)
    marker = C.authority(record_id='subset-' + G.identity({'source': captured.marker, 'operation': operation}),
                         authority='history', generation=1)
    template = {'schema': copy.deepcopy(candidate.document.get('schema', {})),
                'meta': {'history_subset': origin}}
    if 'reasoning' in candidate.document.get('meta', {}):
        template['meta']['reasoning'] = copy.deepcopy(candidate.document['meta']['reasoning'])
    for obj in objects.values():
        if obj['kind'] != 'act':
            template.setdefault(obj['authored']['collection'], {})
            body = obj['body']
            if isinstance(body, dict) and isinstance(body.get('collection_scope'), dict):
                template.setdefault(body['collection_scope']['collection'], {})
    cap = G.document_capabilities({**template, 'meta': {k: v for k, v in template['meta'].items() if k != 'history_subset'}})
    receipt = T.semantic_receipt(profile=cap['profile'], capabilities=cap, before={},
                                  after={'history_subset': origin})
    pairs = [(obj, raw_objects[(obj['subject'], vid)]) for vid, obj in objects.items()]
    before = H.baseline(marker, {}, H.reduce({}))
    draft = C.make_commit(marker=marker, operation=operation, parents={}, baseline=before,
                          objects=pairs, receipt=receipt, view=b'', view_template=template)
    commits = {operation: C.encode_document(draft)}
    empty = replace(candidate, marker=marker, objects=objects, object_bytes=raw_objects, commits=commits,
                    state=state, baseline=H.baseline(marker, commits, state))
    rendered = object.__new__(H.Store).render(empty)
    manifest = C.make_commit(marker=marker, operation=operation, parents={}, baseline=before,
                             objects=pairs, receipt=receipt, view=rendered, view_template=template)
    files = {'authority.yaml': C.encode_document(marker), 'entry.yaml': rendered,
             'commits/' + operation + '.yaml': C.encode_document(manifest)}
    files.update({'objects/' + subject + '/' + vid + '.yaml': raw for (subject, vid), raw in raw_objects.items()})
    binding = {'version': 2, 'requires': [CAPABILITY, SUBSET_CAPABILITY], 'roots': sorted(set(roots)),
               'scope': copy.deepcopy(scope), 'shareability': shareability, 'rules': state['rules'],
               'baseline': empty.baseline, 'files': {path: C.sha256(raw) for path, raw in sorted(files.items())}}
    artifact = {'revision': G.identity(binding), 'manifest': binding, 'files': files}
    validate(artifact)
    return artifact


def _validate_subset(captured, manifest):
    from . import history_transaction as T
    C._require(len(captured.commits) == 1, 'invalid_subset_commits')
    commit = C.decode_document(next(iter(captured.commits.values())))
    T.validate_receipt(commit['receipt'])
    origin = C.validate_subset_origin(captured.document.get('meta', {}).get('history_subset'))
    C._require(commit['operation'] == origin['operation'] and not commit['parents'] and
               G.identity(commit['receipt'].get('after')) == G.identity({'history_subset': origin}),
               'invalid_subset_receipt')
    C._require(origin['roots'] == manifest['roots'] and set(origin['subjects']) == set(captured.state['subjects']),
               'invalid_subset_coverage')
    _locator_disclosures(captured.objects, origin['source_entry'], origin['disclosed_locators'])
    C._require(_subset_subjects(captured, origin['roots']) == set(origin['subjects']), 'invalid_subset_coverage')
    for subject, evidence in origin['subjects'].items():
        C._require(evidence['objects_digest'] == G.identity(_subject_inventory(captured, subject)) and
                   evidence['reduction_digest'] == G.identity(captured.state['subjects'][subject]),
                   'invalid_subset_receipt')
    projection = A.from_store_capture(captured).projection
    C._require(projection['coverage']['scope'] == 'selected' and
               G.identity(projection.get('origin')) == G.identity(origin), 'invalid_subset_coverage')


def preview_adoption(target, artifact):
    """Read-only overlapping subjects and explicit immutable choices available."""
    source = validate(artifact)
    actual = C.committed_objects(target.marker, target.commits, target.object_bytes)
    C._require(G.identity(actual) == G.identity(target.objects) and
               G.identity(H.reduce(actual, target.state['rules'])) == G.identity(target.state),
               'invalid_target_capture')
    C._require(artifact['manifest']['version'] == 2, 'subset_adoption_required')
    C._require(target.marker['record_id'] != source.marker['record_id'], 'independent_adoption_required')
    combined = dict(target.objects)
    for version, obj in source.objects.items():
        C._require(version not in combined or G.identity(combined[version]) == G.identity(obj), 'identity_mismatch')
        combined[version] = obj
    state = H.reduce(combined, target.state['rules'])
    overlap = set(target.state['subjects']) & set(source.state['subjects'])
    return {'artifact_revision': artifact['revision'], 'target_authority': copy.deepcopy(target.marker),
            'target_baseline': copy.deepcopy(target.baseline),
            'subjects': {subject: {'requires_choice': subject in overlap,
                'target_heads': target.state['subjects'].get(subject, {}).get('heads', []),
                'incoming_heads': source.state['subjects'][subject]['heads'],
                'combined_heads': state['subjects'][subject]['heads'],
                'claims': sorted(version for version, obj in combined.items()
                                 if obj['subject'] == subject and obj['kind'] != 'act')}
                for subject in sorted(source.state['subjects'])}}


def prepare_adoption(entry, artifact, *, choices, by, operation, recorded_at, capture=None):
    """Prepare explicit target adoption; the caller still owns publication authority.

    Every overlapping subject (including dependencies) requires one chosen claim.
    No original claim, pin, review or recorded time is rewritten. Retain this
    PreparedMutation for retries; do not prepare again against changed evidence.
    """
    from pathlib import Path
    from dataclasses import replace
    from . import history_transaction as T
    store = H.Store(entry)
    target = capture or store.capture()
    source = validate(artifact)
    preview = preview_adoption(target, artifact)
    C._text(operation)
    C._require(operation not in target.commits, 'operation_already_prepared')
    C._require(isinstance(by, str) and by and isinstance(recorded_at, str) and recorded_at, 'invalid_adopter')
    C._require(isinstance(choices, dict), 'invalid_adoption_choices')
    overlap = {s for s, item in preview['subjects'].items() if item['requires_choice']}
    C._require(overlap <= set(choices) <= set(preview['subjects']), 'adoption_choice_required')
    C._require(G.identity(source.state['rules']) == G.identity(target.state['rules']), 'rules_mismatch')
    combined = {**target.objects, **source.objects}
    raw_objects = dict(target.object_bytes)
    for key, raw in source.object_bytes.items():
        C._require(key not in raw_objects or raw_objects[key] == raw, 'object_bytes_mismatch')
        raw_objects[key] = raw
    before_state = H.reduce(combined, target.state['rules'])
    resolutions = []
    for subject, chosen in sorted(choices.items()):
        C._require(chosen in preview['subjects'][subject]['claims'], 'invalid_adoption_choice')
        # Observe all subject acts so a past refutation is not falsely concurrent.
        saw = sorted(vid for vid, obj in combined.items() if obj['subject'] == subject)
        competing = set(before_state['subjects'][subject]['heads']) | set(
            before_state['subjects'][subject]['disputed_acts'])
        obj = C.make_object(subject=subject, kind='act', by=by, on=recorded_at,
            operation=operation, saw=saw,
            body={'act': 'accept', 'of': chosen, 'over': sorted(competing - {chosen}),
                  'because': 'explicit adoption of ' + artifact['revision']})
        resolutions.append(obj)
        combined[obj['id']] = obj
        raw_objects[(subject, obj['id'])] = C.encode_document(obj)
    state = H.reduce(combined, target.state['rules'])
    for subject, chosen in choices.items():
        actual = state['subjects'][subject]
        C._require(actual['acceptance'] == 'accepted' and chosen in actual['heads'] and all(
            G.identity(C.claim_meaning(combined[vid])) == G.identity(C.claim_meaning(combined[chosen]))
            for vid in actual['heads']), 'unresolved_adoption_choice', subject)
    template = H.Store._template(target.commits)
    C._require('history_subset' not in template.get('meta', {}), 'cannot_adopt_into_transport_authority')
    incoming_template = H.Store._template(source.commits)
    # Adopter cannot silently promote/change profiles or reinterpret field roles.
    C._require(G.identity(template.get('schema', {})) == G.identity(incoming_template.get('schema', {})) and
               G.identity(template.get('meta', {}).get('reasoning')) ==
               G.identity(incoming_template.get('meta', {}).get('reasoning')), 'adoption_profile_mismatch')
    for obj in source.objects.values():
        if obj['kind'] != 'act':
            template.setdefault(obj['authored']['collection'], {})
    inventory = {vid: {'subject': obj['subject'], 'sha256': C.sha256(source.object_bytes[(obj['subject'], vid)])}
                 for vid, obj in sorted(source.objects.items())}
    adoption = {'version': 1, 'artifact_revision': artifact['revision'], 'objects': inventory,
                'choices': copy.deepcopy(choices), 'by': by, 'recorded_at': recorded_at,
                'resolutions': sorted(obj['id'] for obj in resolutions)}
    cap_doc = copy.deepcopy(template)
    cap_doc.get('meta', {}).pop('history', None)
    cap = G.document_capabilities(cap_doc)
    receipt = T.semantic_receipt(profile=cap['profile'], capabilities=cap,
                                  before={'baseline': target.baseline}, after={'history_adoption': adoption})
    # Inventory repeats already-held contribution objects to prove exact adoption
    # within this particular committed receipt; publication need not rewrite them.
    adopted = {**source.objects, **{obj['id']: obj for obj in resolutions}}
    pairs = [(obj, raw_objects[(obj['subject'], vid)]) for vid, obj in adopted.items()]
    args = dict(marker=target.marker, operation=operation, parents=C.commit_frontier(target.commits),
                baseline=target.baseline, objects=pairs, receipt=receipt, view_template=template)
    draft = C.make_commit(**args, view=b'')
    commits = {**target.commits, operation: C.encode_document(draft)}
    candidate = replace(target, objects=combined, object_bytes=raw_objects, commits=commits, state=state,
                        baseline=H.baseline(target.marker, commits, state))
    rendered = store.render(candidate)
    A.from_store_capture(candidate)
    commit = C.make_commit(**args, view=rendered)
    files = [{'path': store.entry.name, 'role': 'record', 'before': target.entry_bytes, 'after': rendered}]
    for obj, raw in pairs:
        key = obj['subject'], obj['id']
        path = Path(store.layout['history']) / key[0] / (key[1] + '.yaml')
        files.append({'path': path.relative_to(store.root).as_posix(), 'role': 'history_object',
                      'before': None, 'after': raw})
    path = Path(store.layout['history_commits']) / (operation + '.yaml')
    files.append({'path': path.relative_to(store.root).as_posix(), 'role': 'history_commit',
                  'before': None, 'after': C.encode_document(commit)})
    return T.PreparedMutation(operation=operation, authority=target.marker, baseline=target.baseline,
                              files=files, receipt=receipt, entry=store.entry.name)


def adopted_by(target, artifact):
    """Acceptance requires a target committed adoption receipt and exact inventory."""
    from . import history_transaction as T
    source = validate(artifact)
    C.committed_objects(target.marker, target.commits, target.object_bytes)
    if artifact['manifest']['version'] != 2:
        return False
    expected = {vid: {'subject': obj['subject'], 'sha256': C.sha256(source.object_bytes[(obj['subject'], vid)])}
                for vid, obj in source.objects.items()}
    for raw in target.commits.values():
        manifest = C.validate_commit(C.decode_document(raw))
        T.validate_receipt(manifest['receipt'])
        adoption = manifest['receipt'].get('after', {}).get('history_adoption')
        if not isinstance(adoption, dict) or adoption.get('version') != 1 or adoption.get('artifact_revision') != artifact['revision']:
            continue
        if G.identity(adoption.get('objects')) != G.identity(expected):
            continue
        inventory = {obj['id']: {'subject': obj['subject'], 'sha256': obj['sha256']} for obj in manifest['objects']}
        if all(inventory.get(vid) == item and target.object_bytes.get((item['subject'], vid)) ==
               source.object_bytes[(item['subject'], vid)] for vid, item in expected.items()):
            return True
    return False


def verify_adoption(entry, mutation, artifact, *, capture=None):
    """Verify an exact retained adoption against its committed parent closure."""
    from dataclasses import replace
    from . import history_transaction as T
    mutation = T.PreparedMutation.from_bytes(mutation.to_bytes())
    data = mutation.to_data()
    live = capture or H.Store(entry).capture()
    manifest = C.decode_document(next(item['after'] for item in mutation.files if item['role'] == 'history_commit'))
    known, pending = set(), list(manifest['parents'])
    while pending:
        operation = pending.pop()
        if operation in known:
            continue
        C._require(operation in live.commits, 'incomplete_commit')
        known.add(operation)
        pending.extend(C.decode_document(live.commits[operation])['parents'])
    commits = {operation: live.commits[operation] for operation in known}
    objects = C.committed_objects(live.marker, commits, live.object_bytes)
    state = H.reduce(objects, live.state['rules'])
    record = next(item for item in mutation.files if item['role'] == 'record')
    before = replace(live, entry_bytes=record['before'], document=C.decode_document(record['before']),
                     commits=commits, objects=objects,
                     object_bytes={(obj['subject'], version): live.object_bytes[(obj['subject'], version)]
                                   for version, obj in objects.items()},
                     state=state, baseline=H.baseline(live.marker, commits, state))
    adoption = data['receipt']['after'].get('history_adoption', {})
    expected = prepare_adoption(entry, artifact, choices=adoption.get('choices'), by=adoption.get('by'),
                                 operation=data['operation'], recorded_at=adoption.get('recorded_at'), capture=before)
    C._require(expected.to_bytes() == mutation.to_bytes(), 'adoption_mutation_mismatch')
    return mutation


def commit_adoption(entry, mutation, artifact, *, verify):
    """Publish only an explicitly prepared adoption after verifier/routing checks."""
    C._require(callable(verify), 'missing_verifier')
    def checked(data):
        verify_adoption(entry, mutation, artifact)
        verify(data)
    return H.Store(entry).commit(mutation, verify=checked)
