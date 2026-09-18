"""Lossless, copy-only import from an ordinary record into immutable history.

Import records a new capture event, never an invented historical observation.
Original claim provenance and unrecorded dependency pins remain explicitly unknown.
The sole authority is the generated entry; retained original member paths are
bound evidence, not additional writable authorities. Live cutover is separate.
"""
import copy
import datetime
import glob
import os
from pathlib import Path
import uuid

from . import history_paths as HP
from . import core_migration as M, history_contract as C, history_store as H
from . import history_adapter as A, history_transaction as T, knowledge_views as V
from . import pending_grounding as G, provenance as P
from .reasoning.contract import capabilities
from .reasoning.snapshot import Snapshot, capture_source, _fields

ARTIFACTS = '.kpopper-history-migration'


def _json(value):
    return G.json_bytes(G._encode(value))


def _parse(raw):
    return M._parse(raw)

def _entry_identity(document):
    return G.identity({subject: list(pair) for subject, pair in G.entries(document).items()})


def _hypothesis_identity(hypotheses):
    return G.identity({name: {'document': item.get('document', item.get('doc', {})),
                              'head': item.get('head', {}), 'error': item.get('error')}
                       for name, item in hypotheses.items()})


def _capture_history(source, record):
    """Retain inactive stores verbatim, without adopting their old acceptance."""
    layout = P.layout(record)
    inventory = source.inventory
    other = P.layout(record.parent / (P.ENTRY if layout['legacy'] else P.LEGACY_ENTRY))
    for role in ('history', 'history_commits', 'history_cancellations', 'history_authority'):
        alternate = other[role]
        inventory('exists', alternate, Path(alternate).exists())
        C._require(not Path(alternate).exists(), 'half_moved_history_layout', alternate)
        path = Path(layout[role])
        inventory('exists', str(path), path.exists())
        if not path.exists():
            continue
        if role == 'history_authority':
            raw = path.read_bytes()
            inventory('bytes', str(path), raw)
            marker = C.validate_authority(C.decode_document(raw))
            C._require(marker['authority'] == 'legacy', 'already_active_history')
            continue
        C._require(path.is_dir() and not path.is_symlink(), 'invalid_history_path')
        if role == 'history_cancellations':
            hidden_pattern = glob.escape(str(path)) + '/.*'
            hidden = sorted(glob.glob(hidden_pattern))
            inventory('glob', hidden_pattern, hidden)
            C._require(not hidden, 'cancellation_membership_mismatch')
        pending = [path]
        while pending:
            directory = pending.pop()
            pattern = glob.escape(str(directory)) + '/*'
            members = sorted(glob.glob(pattern))
            inventory('glob', pattern, members)
            for member in members:
                child = Path(member)
                C._require(not child.is_symlink(), 'invalid_history_path')
                if child.is_dir():
                    C._require(role != 'history_cancellations', 'cancellation_membership_mismatch')
                    pending.append(child)
                else:
                    C._require(child.is_file() and child.stat().st_size <= C.MAX_REQUEST_BYTES,
                               'history_limit')
                    inventory('bytes', member, child.read_bytes())
    marker_raw = source.files.get(str(layout['history_authority']))
    prefix = str(layout['history_commits']) + os.sep
    commits = {Path(path).stem: raw for path, raw in source.files.items() if path.startswith(prefix)}
    if commits:
        C._require(marker_raw is not None, 'missing_retained_history_authority')
        marker = C.validate_authority(C.decode_document(marker_raw))
        prefix = str(layout['history']) + os.sep
        storage = {path[len(prefix):].replace(os.sep, '/'): raw for path, raw in source.files.items()
                   if path.startswith(prefix)}
        objects, _ = C.objects_from_storage(commits, storage)
        cancelled = {Path(path).name: raw for path, raw in source.files.items()
                     if path.startswith(str(layout['history_cancellations']) + os.sep)}
        C.committed_generations(marker, commits, objects, cancelled)
    else:
        C._require(not any(path.startswith(str(layout['history_cancellations']) + os.sep) for path in source.files),
                   'cancellation_membership_mismatch')
    source.verify()


def _capture_retained_artifacts(source, record):
    root = record.parent / ARTIFACTS
    source.inventory('exists', str(root), root.exists())
    if not root.exists():
        return
    marker = source.files.get(str(P.layout(record)['history_authority']))
    C._require(marker is not None and C.validate_authority(C.decode_document(marker))['authority'] == 'legacy',
               'migration_artifact_collision')
    pending, count, total = [root], 0, 0
    while pending:
        directory = pending.pop()
        C._require(directory.is_dir() and not directory.is_symlink(), 'migration_symlink')
        found = []
        for suffix in ('/*', '/.*'):
            pattern = glob.escape(str(directory)) + suffix
            matches = sorted(glob.glob(pattern))
            source.inventory('glob', pattern, matches)
            found.extend(matches)
        found = sorted(set(found))
        for name in found:
            path = Path(name)
            C._require(not path.is_symlink(), 'migration_symlink')
            if path.is_dir():
                pending.append(path)
            else:
                count += 1
                C._require(path.is_file() and count <= C.MAX_OBJECTS and path.stat().st_size <= C.MAX_REQUEST_BYTES,
                           'history_limit')
                raw = path.read_bytes()
                total += len(raw)
                C._require(total <= T.MAX_TRANSACTION_BYTES, 'history_limit')
                source.inventory('bytes', str(path), raw)
    retained = {Path(path).relative_to(record.parent).as_posix(): raw for path, raw in source.files.items()
                if path.startswith(str(root) + os.sep)}
    known = set()
    for name, raw in retained.items():
        if name != ARTIFACTS + '/receipt.json' and not (name.startswith(ARTIFACTS + '/generations/') and
                name.count('/') == 3 and name.endswith('/receipt.json')):
            continue
        receipt = _copy_receipt(raw)
        known.add(name)
        for member, expected in receipt['destination'].items():
            if member.startswith(ARTIFACTS + '/'):
                C._require(member in retained and C.sha256(retained[member]) == expected,
                           'retained_artifact_mismatch', member)
                known.add(member)
    C._require(set(retained) <= known, 'unknown_retained_artifact')
    source.verify()


def _immutable_original(name, entry, artifacts):
    """Only already-retained audit/immutable history paths may be storage aliases."""
    layout = P.layout('/' + entry)
    prefixes = [str(Path(layout[role]).relative_to('/')) + '/' for role in ('history', 'history_commits', 'history_cancellations')]
    return (name.startswith(ARTIFACTS + '/') and not name.startswith(artifacts + '/')) or any(
        name.startswith(prefix) for prefix in prefixes)


def _pointer_values(document):
    def values(value):
        if isinstance(value, str) and value.endswith(('.yaml', '.yml')):
            yield value
        elif isinstance(value, list):
            for child in value:
                yield from values(child)
        elif isinstance(value, dict):
            for child in value.values():
                yield from values(child)
    for key in ('record', 'also'):
        yield from values(document.get(key))


def _original_provenance():
    return {'writer': None, 'operation': None, 'recorded_at': None,
            'source_clock': None, 'condition_profile': None}


class Plan:
    def __init__(self, record, *, route=False, read_mode='frozen', operation=None, recorded_at=None, record_id=None):
        C._require(read_mode in ('live', 'frozen'), 'invalid_history_import_read_mode')
        self.read_mode = read_mode
        record = Path(record).expanduser().resolve()
        if route:
            record = Path(V.write_paths([str(record)])[0]).resolve()
        self.record = record
        self.record_id = record_id
        self.operation = operation or 'import-' + uuid.uuid4().hex
        self.recorded_at = recorded_at or datetime.datetime.now(datetime.timezone.utc).isoformat()
        self.source = capture_source([str(record)], read_mode='frozen')
        self.original = self.source.snapshot
        self.project = V.project_for([str(record)])
        self.record_files = [path for path in self.source.files
            if Path(path).parent != Path(P.layout(record)['hypotheses'])
            and path != P.layout(record)['history_authority']]
        M._extend_inventory(self.source, record)
        M._capture_evidence(self.source, record)
        self._admin_paths = M._observe_admin(self.source, self.project)
        _capture_history(self.source, record)
        _capture_retained_artifacts(self.source, record)
        previous_raw = self.source.files.get(str(P.layout(record)['history_authority']))
        previous = C.validate_authority(C.decode_document(previous_raw)) if previous_raw else None
        generation = previous['generation'] + 1 if previous else 1
        self.artifacts = ARTIFACTS if generation == 1 else ARTIFACTS + '/generations/' + str(generation)
        C._require(not any(path == str(record.parent / self.artifacts) or
                           path.startswith(str(record.parent / self.artifacts) + os.sep)
                           for path in self.source.files), 'existing_generation_artifacts')
        self.source_files = self.source.files
        self.mapping = {}
        for path in self.source_files:
            relative = M._portable(record, path)
            C.relative_path(relative)
            C._require(relative != self.artifacts and not relative.startswith(self.artifacts + '/'),
                       'migration_artifact_collision')
            self.mapping[path] = relative
        C._require(len(set(self.mapping.values())) == len(self.mapping), 'migration_path_collision')
        common = Path(os.path.commonpath([str(record.parent), *self.source_files]))
        self.topology = None
        if common != record.parent:
            self.topology = {'version': 1, 'entry': record.relative_to(common).as_posix(),
                'originals': {self.mapping[path]: Path(path).relative_to(common).as_posix()
                              for path in self.source_files}}
        self.observed = capture_source([str(record)], read_mode='live') if read_mode == 'live' else None
        if self.observed is not None:
            _validate_pending(self.observed.snapshot)
            C._require(_entry_identity(self.observed.snapshot.to_data()['document']) ==
                       _entry_identity(self.original.to_data()['document']), 'migration_source_changed')
        # Capture the reader's own origin map while retaining the same byte inventory.
        token, core = P._CAPTURE_READS.set(self.source.inventory), P._CORE_READS.set(True)
        try:
            self.loaded = P.load([str(record)], read_mode='frozen')
        finally:
            P._CORE_READS.reset(core)
            P._CAPTURE_READS.reset(token)
        self.verify_source()
        self.problems = []
        absolute_members = []
        for path in self.record_files:
            for pointer in _pointer_values(_parse(self.source_files[path])):
                if Path(pointer).is_absolute():
                    absolute_members.append(self.mapping[path])
                target = os.path.abspath(os.path.join(os.path.dirname(path), pointer))
                C._require(target in self.mapping, 'unresolved_original_pointer')
        self.inverse = ({'representable': False, 'reason': 'absolute_original_pointer',
                         'members': sorted(set(absolute_members))} if absolute_members else None)
        self.files = {self.mapping[path]: raw for path, raw in self.source_files.items()}
        self.objects, self.locators = {}, []
        self._prepare()

    def verify_source(self):
        self.source.verify()
        if self.observed is not None:
            self.observed.verify()

    def _locator(self, path, collection, subject, **extra):
        return {'version': 1, 'kind': 'legacy_import', 'path': self.mapping[str(path)],
                'sha256': C.sha256(self.source_files[str(path)]), 'collection': collection,
                'subject': subject, 'original': _original_provenance(),
                'import': {'operation': self.operation, 'recorded_at': self.recorded_at}, **extra}

    def _claim(self, subject, collection, body, fields, profile, locator, *, saw=(), judgment=False):
        deps = body.get(fields['deps'], []) if isinstance(body, dict) else []
        C._require(isinstance(deps, list) and all(isinstance(dep, str) for dep in deps),
                   'unsupported_legacy_dependency_mapping', subject)
        obj = C.make_object(subject=subject, kind='judgment' if judgment else 'reading',
            by=None, on=self.recorded_at, operation=self.operation, body=body, saw=saw,
            authored={'collection': collection, 'fields': fields, 'profile': profile, 'locator': locator},
            pins={}, pin_gaps={dep: 'not_recorded' for dep in deps})
        self.objects[obj['id']] = obj
        self.locators.append({'subject': subject, 'version': obj['id'], 'locator': locator})
        return obj

    def _replace(self, subject, old, new, *, saw, locator):
        act = C.make_object(subject=subject, kind='act', by=None, on=self.recorded_at,
            operation=self.operation, saw=saw, at={'imported_replacement': locator},
            body={'act': 'accept', 'of': new['id'], 'over': [old['id']],
                  'because': 'imported recorded replacement; original actor and operation are unknown'})
        self.objects[act['id']] = act
        self.locators.append({'subject': subject, 'version': act['id'], 'locator': locator})
        return act

    def _prepare(self):
        data = self.original.to_data()
        document = copy.deepcopy(data['document'])
        C._require(isinstance(document.get('meta', {}), dict), 'unsupported_history_metadata')
        C._require(not document.get('meta', {}).get('history'), 'already_active_history')
        C._require(not document.get('meta', {}).get('history_import'), 'existing_import_mapping')
        for hypothesis in data['hypotheses'].values():
            C._require(not hypothesis.get('error'), 'unreadable_hypothesis')
        fields = _fields(document)
        profile = capabilities(document)['profile']
        known = G.entries(document)
        archive_path = P.layout(self.record)['replaced']
        archive = _parse(self.source_files[archive_path]) if archive_path in self.source_files else {}
        C._require(isinstance(archive, dict), 'invalid_replaced_archive')
        for subject, versions in archive.items():
            C._require(isinstance(versions, list) and all(isinstance(v, dict) for v in versions),
                       'invalid_replaced_archive', subject)
        for subject, (collection, body) in sorted(known.items()):
            origin = self.loaded.origins.get(collection, {}).get(subject)
            C._require(origin in self.source_files, 'unresolved_import_origin', subject)
            locator = self._locator(origin, collection, subject)
            retired, saw = None, []
            for index, raw in enumerate(archive.get(subject, []), 1):
                historical = P.version_at(archive[subject], index)
                if 'same_as' in historical:
                    self.problems.append('unresolved_archive_alias: ' + subject + ':' + str(index))
                    continue
                historical = copy.deepcopy(historical)
                historical.update({key: raw[key] for key in ('day', 'ended', 'dropped') if key in raw})
                location = self._locator(archive_path, collection, subject, archive_index=index,
                    archive_metadata=copy.deepcopy(raw), original_dependency_versions='not_recorded',
                    interpretation={'profile': 'ordinary-reader/v1', 'scope': 'retained_archive_only',
                                    'original_condition_profile': 'unknown'})
                old = self._claim(subject, collection, historical, fields, 'ordinary-reader/v1', location,
                                  saw=saw, judgment=fields['deps'] in historical)
                if retired is not None:
                    act = self._replace(subject, retired, old, saw=[*saw, old['id']], locator=location)
                    saw.append(act['id'])
                retired = old
                saw.append(old['id'])
            current = self._claim(subject, collection, body, fields, profile, locator, saw=saw,
                                  judgment=isinstance(body, dict) and fields['deps'] in body)
            if retired is not None:
                self._replace(subject, retired, current, saw=[*saw, current['id']], locator=locator)
        # An archive-only subject has no known current successor. Keep its
        # actual archive container and original membership uncertainty explicit.
        for subject in sorted(set(archive) - set(known)):
            saw, previous = [], None
            for index, raw in enumerate(archive[subject], 1):
                body = copy.deepcopy(P.version_at(archive[subject], index))
                if 'same_as' in body:
                    self.problems.append('unresolved_archive_alias: ' + subject + ':' + str(index))
                    continue
                body.update({key: raw[key] for key in ('day', 'ended', 'dropped') if key in raw})
                locator = self._locator(archive_path, 'replaced', subject, archive_index=index,
                    archive_metadata=copy.deepcopy(raw), original_collection=None,
                    mapping='retained_archive_container', original_dependency_versions='not_recorded',
                    interpretation={'profile': 'ordinary-reader/v1', 'scope': 'retained_archive_only',
                                    'original_condition_profile': 'unknown'})
                claim = self._claim(subject, 'replaced', body, fields, 'ordinary-reader/v1', locator,
                                    saw=saw, judgment=fields['deps'] in body)
                if previous is not None:
                    replacement = self._replace(subject, previous, claim, saw=[*saw, claim['id']], locator=locator)
                    saw.append(replacement['id'])
                saw.append(claim['id'])
                previous = claim
            if previous is not None:
                reason = previous['body'].get('ended') or previous['body'].get('dropped') or 'recorded archive without a current successor'
                retired = C.make_object(subject=subject, kind='act', by=None, on=self.recorded_at,
                    operation=self.operation, saw=sorted(saw), at={'imported_retirement': locator},
                    body={'act': 'retire', 'of': previous['id'], 'over': [], 'because': str(reason)})
                self.objects[retired['id']] = retired
                self.locators.append({'subject': subject, 'version': retired['id'], 'locator': locator})
        from . import history_hypothesis_import as HI
        physical = []
        for name, hypothesis in sorted(data['hypotheses'].items()):
            original = self.loaded.hypotheses[name]
            path = original['path']
            C._require(path in self.source_files, 'missing_imported_hypothesis', name)
            physical.append({'name': name, 'document': hypothesis['document'], 'head': hypothesis['head'],
                             'path': self.mapping[path], 'bytes': self.source_files[path],
                             'error': hypothesis.get('error')})
        self.hypothesis_import = HI.prepare(self.objects, physical, base_document=document,
            entry=self.record.name, operation=self.operation, recorded_at=self.recorded_at)
        self.objects.update(self.hypothesis_import['objects'])
        C.validate_closure(self.objects)
        marker_source = self.source_files.get(P.layout(self.record)['history_authority'])
        if marker_source is not None:
            previous = C.validate_authority(C.decode_document(marker_source))
            C._require(self.record_id in (None, previous['record_id']), 'authority_mismatch')
            record_id, generation = previous['record_id'], previous['generation'] + 1
        else:
            record_id, generation = self.record_id or 'record-' + uuid.uuid4().hex, 1
        self.marker = C.authority(record_id=record_id, authority='history', generation=generation,
            cancellations=previous.get('cancellations') if marker_source is not None else None)
        self.files[self.artifacts + '/original.json'] = self.original.to_json().encode()
        self.observation = None
        if self.observed is not None:
            raw = self.observed.snapshot.to_json().encode()
            self.files[self.artifacts + '/live.json'] = raw
            self.observation = {'read_mode': 'live', 'path': self.artifacts + '/live.json',
                                'snapshot_id': self.observed.snapshot.snapshot_id, 'sha256': C.sha256(raw)}
        self.originals = {}
        for path, raw in self.source_files.items():
            name = self.mapping[path]
            retained = _immutable_original(name, self.record.name, self.artifacts)
            storage = name if retained else self.artifacts + '/originals/' + name
            if not retained:
                self.files[storage] = raw
            self.originals[name] = {'path': storage, 'sha256': C.sha256(raw),
                                    'storage': 'retained' if retained else 'copy'}
        self.receipt_version = 2 if any(item['storage'] == 'retained' for item in self.originals.values()) else 1
        if self.receipt_version == 1:
            self.originals = {name: {key: item[key] for key in ('path', 'sha256')}
                              for name, item in self.originals.items()}
        members = {self.artifacts + '/original.json': 'retained_original'}
        if self.observation:
            members[self.observation['path']] = 'retained_original'
        members.update({item['path']: 'retained_original' for item in self.originals.values()})
        members.update({self.mapping[path]: 'retained_original' for path in self.source_files
                        if self.mapping[path].startswith(ARTIFACTS + '/')})
        members.update({item['path']: 'retained_original' for item in self.hypothesis_import['physical']['physical']})
        members.update({self.mapping[path]: 'retained_original' for path in self.record_files
                        if path != str(self.record)})
        if archive_path in self.source_files:
            members[self.mapping[archive_path]] = 'replaced'
        template = C.document_template(document)
        for obj in self.objects.values():
            if obj['kind'] != 'act':
                template.setdefault(obj['authored']['collection'], {})
        physical_entry = M._pointers(_parse(self.source_files[str(self.record)]), str(self.record), self.mapping)
        for key in ('record', 'also'):
            if key in physical_entry:
                template[key] = copy.deepcopy(physical_entry[key])
            else:
                template.pop(key, None)
        if physical:
            template.setdefault('meta', {})['history_hypothesis_import'] = self.hypothesis_import['physical']
        template.setdefault('meta', {})['history_import'] = {
            'version': 1, 'operation': self.operation, 'recorded_at': self.recorded_at,
            'members': [{'path': path, 'sha256': C.sha256(self.files[path]), 'role': role}
                        for path, role in sorted(members.items())]}
        self.template = template
        empty = H.reduce({})
        baseline = H.baseline(self.marker, {}, empty)
        capture = H.Capture(b'', {}, self.marker, {}, {}, {}, empty, baseline, {})
        pairs = [(obj, C.encode_document(obj)) for obj in self.objects.values()]
        receipt = T.semantic_receipt(profile=profile, capabilities=capabilities(document),
            before={'kind': 'legacy-import/v1', 'snapshot_id': self.original.snapshot_id,
                    **({'artifact_root': self.artifacts} if self.artifacts != ARTIFACTS else {}),
                    **({'originals_storage': self.originals} if self.receipt_version == 2 else {}),
                    'source': {self.mapping[path]: C.sha256(raw) for path, raw in self.source_files.items()},
                    **({'topology': self.topology} if self.topology else {}),
                    **({'observation': self.observation} if self.observation else {}),
                    **({'inverse': self.inverse} if self.inverse else {})},
            after={'kind': 'preserved-authored-import/v1', 'operation': self.operation,
                   'recorded_at': self.recorded_at, 'original_provenance': _original_provenance(),
                   'locators': self.locators, 'unresolved': self.problems})
        draft = C.make_commit(marker=self.marker, operation=self.operation, parents={}, baseline=baseline,
                              objects=pairs, receipt=receipt, view=b'', view_template=template, requires=HP.commit_requires())
        commits = {self.operation: C.encode_document(draft)}
        store = H.Store(self.record)
        rendered = store.render(capture, objects=self.objects, commits=commits)
        manifest = C.make_commit(marker=self.marker, operation=self.operation, parents={}, baseline=baseline,
                                 objects=pairs, receipt=receipt, view=rendered, view_template=template, requires=HP.commit_requires())
        commits = {self.operation: C.encode_document(manifest)}
        object_bytes = {(obj['subject'], obj['id']): raw for obj, raw in pairs}
        state = H.reduce(self.objects)
        capture = H.Capture(rendered, C.decode_document(rendered), self.marker, commits, object_bytes,
                            self.objects, state, H.baseline(self.marker, commits, state), {})
        prefix = str(P.layout(self.record)['history_commits']) + os.sep
        prior_commits = {Path(path).stem: raw for path, raw in self.source_files.items() if path.startswith(prefix)}
        prefix = str(P.layout(self.record)['history']) + os.sep
        prior_storage = {path[len(prefix):].replace(os.sep, '/'): raw for path, raw in self.source_files.items()
                         if path.startswith(prefix)}
        prior_objects, prior_paths = C.objects_from_storage(prior_commits, prior_storage)
        cancelled = {Path(path).name: raw for path, raw in self.source_files.items()
                     if path.startswith(str(P.layout(self.record)['history_cancellations']) + os.sep)}
        retained = C.committed_generations(self.marker, prior_commits, prior_objects, cancelled)
        from dataclasses import replace
        capture = replace(capture, object_bytes={**prior_objects, **object_bytes}, inactive_generations=retained,
                          cancellation_bytes=cancelled,
                          object_paths={**prior_paths, **{(obj['subject'], obj['id']): HP.object_path(obj['subject'], obj['id'])
                                                         for obj, raw in pairs}})
        adapted = A.from_store_capture(capture)
        observation_data = self.observed.snapshot.to_data() if self.observed else data
        context = copy.deepcopy(observation_data['context'])
        context['read_mode'] = 'frozen'
        context['migration'] = {'kind': 'history-import/v1', 'source_snapshot_id': self.original.snapshot_id,
                                'original_provenance': 'unknown unless retained in original body',
                                'source_read_mode': self.read_mode, 'copied_state': 'frozen',
                                'observation_snapshot_id': observation_data['snapshot_id']}
        from . import history_hypotheses as HH
        named, named_index = HH.layers(adapted.projection, adapted.document)
        if named:
            context['history_hypotheses'] = named_index
        candidate_hypotheses = copy.deepcopy(observation_data['hypotheses'])
        candidate_hypotheses.update(named)
        self.candidate = adapted.snapshot(context=context, hypotheses=candidate_hypotheses, as_of=data['as_of'])
        self.files[self.record.name] = rendered
        layout = P.layout(self.record)
        self.files[Path(layout['history_authority']).relative_to(self.record.parent).as_posix()] = C.encode_document(self.marker)
        commit_path = Path(layout['history_commits']) / (self.operation + '.yaml')
        C._require(str(commit_path) not in self.source_files, 'operation_collision')
        self.files[commit_path.relative_to(self.record.parent).as_posix()] = C.encode_document(manifest)
        for obj, raw in pairs:
            path = Path(layout['history']) / HP.object_path(obj['subject'], obj['id'])
            relative = path.relative_to(self.record.parent).as_posix()
            C._require(relative not in self.files or self.files[relative] == raw, 'immutable_collision')
            self.files[relative] = raw
        self.files[self.artifacts + '/candidate.json'] = self.candidate.to_json().encode()
        self.manifest = {'version': self.receipt_version, 'kind': 'history-import/v' + str(self.receipt_version), 'operation': self.operation,
            'record': self.record.name, 'record_id': self.marker['record_id'],
            **({'artifact_root': self.artifacts} if self.artifacts != ARTIFACTS else {}), 'complete': not self.problems,
            'problems': self.problems, 'source_snapshot_id': self.original.snapshot_id,
            'candidate_snapshot_id': self.candidate.snapshot_id, 'locators': self.locators,
            'originals': copy.deepcopy(self.originals),
            'destination': {path: C.sha256(raw) for path, raw in sorted(self.files.items())},
            **({'topology': self.topology} if self.topology else {}),
            **({'observation': self.observation} if self.observation else {}),
            **({'inverse': self.inverse} if self.inverse else {})}
        self.files[self.artifacts + '/receipt.json'] = _json(self.manifest)
        self.verify_source()

    def summary(self):
        return {'state': 'blocked' if self.problems else 'preview', 'complete': not self.problems,
                'historical_support_complete': self.candidate.to_data()['context']['history']['coverage']['complete'],
                'record': str(self.record), 'operation': self.operation, 'problems': list(self.problems),
                'source_read_mode': self.read_mode, 'copied_read_mode': 'frozen',
                'captured_pending_context': self.observed is not None,
                'inverse': copy.deepcopy(self.inverse) if self.inverse else {'representable': True},
                'original_provenance': _original_provenance(), 'manifest': copy.deepcopy(self.manifest)}

    def _populate(self, root):
        for name, raw in self.files.items():
            path = T._target(root, name)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(raw)

    def validate_destination(self, destination):
        self.verify_source()
        C._require(not self.problems, 'incomplete_history_import', '; '.join(self.problems))
        destination = Path(destination).resolve()
        actual = {p.relative_to(destination).as_posix(): p for p in destination.rglob('*')
                  if p.is_file() or p.is_symlink()}
        C._require(set(actual) == set(self.files), 'migration_inventory_changed')
        for name, raw in self.files.items():
            C._require(not actual[name].is_symlink() and actual[name].read_bytes() == raw,
                       'migration_bytes_changed', name)
        copied = Snapshot.capture([str(destination / self.record.name)], read_mode='frozen')
        replay = Snapshot.from_json(copied.to_json())
        C._require(replay.snapshot_id == copied.snapshot_id, 'migration_replay_mismatch')
        C._require(_entry_identity(copied.to_data()['document']) ==
                   _entry_identity(self.original.to_data()['document']), 'migration_body_mismatch')
        C._require(_hypothesis_identity(copied.to_data()['hypotheses']) == _hypothesis_identity(self.original.to_data()['hypotheses']),
                   'migration_hypothesis_mismatch')
        for kind, expected in (('original', self.original), ('candidate', self.candidate)):
            artifact = Snapshot.from_json((destination / self.artifacts / (kind + '.json')).read_bytes())
            C._require(artifact.snapshot_id == expected.snapshot_id, 'migration_replay_mismatch')
        self.verify_source()
        return {'valid': True, 'record': str(destination / self.record.name),
                'snapshot_id': copied.snapshot_id, 'manifest': copy.deepcopy(self.manifest)}

    def publish(self, destination):
        C._require(not self.problems, 'incomplete_history_import', '; '.join(self.problems))
        self.verify_source()
        root = V.publish_tree(destination, self._populate, self.validate_destination)
        return {**self.summary(), 'state': 'materialized', 'record': str(root / self.record.name),
                'receipt': str(root / self.artifacts / 'receipt.json'), 'read_mode': 'frozen'}

    def restore_copy(self, copied, destination):
        """Restore from the copy's own verified evidence, independent of this plan's source."""
        return restore_from_copy(copied, destination)


def _copy_receipt(raw):
    """Decode the canonical typed receipt, refusing duplicate keys and aliases."""
    import json
    from .reasoning.snapshot import _json_object, _json_constant, _check_typed_json
    C._require(type(raw) is bytes and len(raw) <= T.MAX_TRANSACTION_BYTES, 'history_limit')
    try:
        encoded = json.loads(raw, object_pairs_hook=_json_object, parse_constant=_json_constant)
        _check_typed_json(encoded)
        value = G._decode(encoded)
        C._require(_json(value) == raw, 'invalid_migration_receipt')
        C._mapping(value, ('version', 'kind', 'operation', 'record', 'record_id', 'complete',
            'problems', 'source_snapshot_id', 'candidate_snapshot_id', 'locators', 'originals', 'destination'),
            ('topology', 'observation', 'inverse', 'artifact_root'))
        C._require(type(value['version']) is int and value['version'] in (1, 2) and
                   value['kind'] == 'history-import/v' + str(value['version']) and value['complete'] is True and
                   value['problems'] == [], 'incomplete_history_import')
        C._text(value['operation'])
        C._text(value['record_id'])
        C._require(isinstance(value['record'], str) and Path(value['record']).name == value['record'],
                   'invalid_migration_receipt')
        C.relative_path(value['record'])
        C._require(isinstance(value['destination'], dict) and isinstance(value['originals'], dict)
                   and value['record'] in value['originals'], 'invalid_migration_receipt')
        for name, digest in value['destination'].items():
            C.relative_path(name)
            C._text(digest, C.HEX)
        artifacts = value.get('artifact_root', ARTIFACTS)
        C.relative_path(artifacts)
        C._require(artifacts == ARTIFACTS or (artifacts.startswith(ARTIFACTS + '/generations/') and
                   artifacts[len(ARTIFACTS + '/generations/'):].isdigit() and
                   int(artifacts[len(ARTIFACTS + '/generations/'):]) > 1), 'invalid_migration_receipt')
        C._require(artifacts + '/receipt.json' not in value['destination'], 'invalid_migration_receipt')
        for name, original in value['originals'].items():
            C.relative_path(name)
            C._mapping(original, ('path', 'sha256') if value['version'] == 1 else ('path', 'sha256', 'storage'))
            storage = original.get('storage', 'copy')
            C._require((storage == 'copy' and original['path'] == artifacts + '/originals/' + name) or
                       (storage == 'retained' and value['version'] == 2 and original['path'] == name and
                        _immutable_original(name, value['record'], artifacts)), 'invalid_original_mapping', name)
            C._text(original['sha256'], C.HEX)
            C._require(value['destination'].get(original['path']) == original['sha256'],
                       'invalid_original_mapping', name)
        topology = value.get('topology')
        if topology is not None:
            C._mapping(topology, ('version', 'entry', 'originals'))
            C._require(type(topology['version']) is int and topology['version'] == 1 and
                       isinstance(topology['originals'], dict) and
                       set(topology['originals']) == set(value['originals']), 'invalid_original_topology')
            C.relative_path(topology['entry'])
            G._portable_files({name: b'' for name in topology['originals'].values()})
            C._require(len(set(topology['originals'].values())) == len(topology['originals']) and
                       topology['originals'][value['record']] == topology['entry'], 'invalid_original_topology')
        inverse = value.get('inverse')
        if inverse is not None:
            C._mapping(inverse, ('representable', 'reason', 'members'))
            C._require(inverse['representable'] is False and inverse['reason'] == 'absolute_original_pointer' and
                       isinstance(inverse['members'], list) and inverse['members'] and
                       inverse['members'] == sorted(set(inverse['members'])) and
                       set(inverse['members']) <= set(value['originals']), 'invalid_inverse_constraint')
        observation = value.get('observation')
        if observation is not None:
            C._mapping(observation, ('read_mode', 'path', 'snapshot_id', 'sha256'))
            C._require(observation['read_mode'] == 'live' and observation['path'] == artifacts + '/live.json',
                       'invalid_migration_observation')
            C._text(observation['snapshot_id'], C.HEX)
            C._text(observation['sha256'], C.HEX)
            C._require(value['destination'].get(observation['path']) == observation['sha256'],
                       'invalid_migration_observation')
        return value
    except (ValueError, TypeError, KeyError, IndexError, RecursionError) as error:
        if isinstance(error, C.HistoryError):
            raise
        raise C.HistoryError('invalid_migration_receipt') from error


def _copy_inventory(root):
    """Capture complete membership and bytes, including refusal of directory symlinks."""
    result, total = {}, 0
    for path in root.rglob('*'):
        C._require(not path.is_symlink(), 'migration_symlink', str(path))
        if path.is_dir():
            continue
        C._require(path.is_file() and len(result) < C.MAX_OBJECTS, 'history_limit')
        relative = path.relative_to(root).as_posix()
        C._require(T._target(root, relative) == path, 'invalid_migration_path')
        C._require(path.stat().st_size <= T.MAX_TRANSACTION_BYTES, 'history_limit')
        with path.open('rb') as stream:
            raw = stream.read(T.MAX_TRANSACTION_BYTES + 1)
        total += len(raw)
        C._require(total <= T.MAX_TRANSACTION_BYTES, 'history_limit')
        result[relative] = raw
    return result


def _copy_artifacts(files):
    """Choose the receipt bound to the explicit authority marker, never max directory."""
    candidates = []
    for path, raw in files.items():
        if path == ARTIFACTS + '/receipt.json' or (path.startswith(ARTIFACTS + '/generations/') and
                path.count('/') == 3 and path.endswith('/receipt.json')):
            receipt = _copy_receipt(raw)
            entry = receipt['record']
            marker_path = Path(P.layout('/' + entry)['history_authority']).relative_to('/').as_posix()
            if marker_path not in files:
                continue
            marker = C.validate_authority(C.decode_document(files[marker_path]))
            expected = ARTIFACTS if marker['generation'] == 1 else ARTIFACTS + '/generations/' + str(marker['generation'])
            if (marker['authority'] == 'history' and path == expected + '/receipt.json' and
                    receipt.get('artifact_root', ARTIFACTS) == expected):
                candidates.append(expected)
    C._require(len(candidates) == 1, 'missing_migration_receipt')
    return candidates[0]


def restore_from_copy(copied, destination):
    """Export verified pre-import bytes to a NEW directory, without live-source I/O.

    This copy-only inverse accepts the exact initial copied generation. Any newer
    commit, inactive retained event addition, edit, deletion or unknown extra file
    refuses. It does not deactivate an authority or restore over an existing tree.
    Receipt hashes establish internal consistency, not external authenticity.
    """
    copied = Path(copied).expanduser().resolve()
    destination = Path(destination).expanduser().absolute()
    C._require(copied.is_dir(), 'migration_copy_unavailable')
    C._require(not destination.exists() and not destination.is_symlink(), 'migration_destination_exists')
    with T.directory_guards([copied], exclusive=False):
        files = _copy_inventory(copied)
        artifacts = _copy_artifacts(files)
        receipt_name = artifacts + '/receipt.json'
        C._require(receipt_name in files, 'missing_migration_receipt')
        receipt = _copy_receipt(files[receipt_name])
        expected = receipt['destination']
        C._require(set(files) == {*expected, receipt_name}, 'migration_inventory_changed')
        for name, digest in expected.items():
            C._require(C.sha256(files[name]) == digest, 'migration_bytes_changed', name)
        entry = copied / receipt['record']
        # The manifest is verified through the actual authority/closure loader,
        # not trusted as an arbitrary original-file allowlist.
        store = H.Store(entry)
        capture = store.capture()
        C._require(set(capture.commits) == {receipt['operation']} and
                   capture.marker['record_id'] == receipt['record_id'], 'newer_history_not_representable')
        commit = C.validate_commit(C.decode_document(capture.commits[receipt['operation']]))
        C._require(commit['parents'] == {} and store.render(capture) == files[receipt['record']],
                   'migration_view_mismatch')
        import_receipt = T.validate_receipt(commit['receipt'])
        source = import_receipt['before']
        C._require(source.get('kind') == 'legacy-import/v1' and
                   source.get('source') == {name: item['sha256'] for name, item in receipt['originals'].items()},
                   'invalid_original_mapping')
        if receipt['version'] == 2:
            C._require(source.get('originals_storage') == receipt['originals'], 'invalid_original_mapping')
        C._require(import_receipt['after'].get('locators') == receipt['locators'] and
                   import_receipt['after'].get('operation') == receipt['operation'], 'invalid_migration_receipt')
        C._require(source.get('topology') == receipt.get('topology') and
                   source.get('observation') == receipt.get('observation') and
                   source.get('inverse') == receipt.get('inverse') and
                   source.get('artifact_root', ARTIFACTS) == artifacts, 'invalid_original_mapping')
        C._require(receipt.get('inverse', {}).get('representable', True), 'nonrepresentable_inverse',
                   receipt.get('inverse', {}).get('reason', ''))
        topology = receipt.get('topology', {'entry': receipt['record'],
                    'originals': {name: name for name in receipt['originals']}})
        originals = {topology['originals'][name]: files[item['path']] for name, item in receipt['originals'].items()}
        restored_entry = topology['entry']
        original = Snapshot.from_json(files[artifacts + '/original.json'])
        candidate = Snapshot.from_json(files[artifacts + '/candidate.json'])
        C._require(original.snapshot_id == receipt['source_snapshot_id'] == source.get('snapshot_id')
                   and candidate.snapshot_id == receipt['candidate_snapshot_id'], 'migration_replay_mismatch')
        adapted = A.from_store_capture(capture)
        C._require(G.identity(adapted.document) == G.identity(candidate.to_data()['document']) and
                   _entry_identity(adapted.document) == _entry_identity(original.to_data()['document']),
                   'migration_body_mismatch')
        members = commit['view_template'].get('meta', {}).get('history_import', {}).get('members', [])
        original_binding = {item['path']: item['sha256'] for item in members
                            if item.get('role') == 'retained_original'}
        C._require(original_binding.get(artifacts + '/original.json') == C.sha256(files[artifacts + '/original.json'])
                   and all(original_binding.get(item['path']) == item['sha256']
                           for item in receipt['originals'].values()), 'invalid_original_mapping')
        C._require(_copy_inventory(copied) == files, 'migration_copy_changed')

        def populate(root):
            for name, raw in originals.items():
                path = T._target(root, name)
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(raw)

        def verify(root):
            C._require(all(Path(path).resolve().is_relative_to(root.resolve()) for path in
                           P._files_of([str(root / restored_entry)])), 'nonrelocatable_original_pointer')
            C._require(_copy_inventory(copied) == files, 'migration_copy_changed')
            C._require(_copy_inventory(root) == originals, 'migration_original_mismatch')
            replay = Snapshot.capture([str(root / restored_entry)], read_mode='frozen')
            C._require(_entry_identity(replay.to_data()['document']) == _entry_identity(original.to_data()['document'])
                       and _hypothesis_identity(replay.to_data()['hypotheses']) == _hypothesis_identity(original.to_data()['hypotheses']),
                       'migration_replay_mismatch')
            C._require(_copy_inventory(copied) == files, 'migration_copy_changed')

        # Staging uses a disjoint lock namespace while the copy remains guarded.
        from concurrent.futures import ThreadPoolExecutor
        with ThreadPoolExecutor(max_workers=1) as executor:
            root = executor.submit(V.publish_tree, destination, populate, verify).result()
    return {'state': 'restored_copy', 'record': str(root / restored_entry),
            'source_snapshot_id': original.snapshot_id, 'copy_receipt_sha256': C.sha256(files[receipt_name])}


def prepare(record, *, route=False, read_mode='frozen', operation=None, recorded_at=None, record_id=None):
    return Plan(record, route=route, read_mode=read_mode, operation=operation, recorded_at=recorded_at,
                record_id=record_id)


def _validate_pending(snapshot):
    from .reasoning.snapshot import _history_bytes
    data = snapshot.to_data()
    pending = data['context'].get('pending', {})
    bundles, events = pending.get('bundles', {}), pending.get('events', [])
    C._require(isinstance(bundles, dict) and isinstance(events, list) and all(
        isinstance(event, dict) and event.get('revision') in bundles and isinstance(event.get('event_id'), str)
        for event in events), 'invalid_pending_observation')
    C._require({event['revision'] for event in events} == set(bundles) and
               len({event['event_id'] for event in events}) == len(events), 'invalid_pending_observation')
    for revision, portable in bundles.items():
        bundle = {'revision': portable['revision'], 'manifest': portable['manifest'],
                  'files': _history_bytes(portable['files'])}
        C._require(revision == bundle['revision'], 'invalid_pending_revision')
        G.validate_bundle(bundle)
    return snapshot


def replay_from_copy(copied):
    """Replay the sealed import observation, including captured pending context.

    Ordinary Snapshot.capture reads the copy's present frozen local state. This
    explicit API preserves the source observation without configuring a ledger,
    routing, publication permission, or automatically accepting a contribution.
    """
    copied = Path(copied).expanduser().resolve()
    with T.directory_guards([copied], exclusive=False):
        files = _copy_inventory(copied)
        artifacts = _copy_artifacts(files)
        receipt_name = artifacts + '/receipt.json'
        C._require(receipt_name in files, 'missing_migration_receipt')
        receipt = _copy_receipt(files[receipt_name])
        C._require(set(files) == {*receipt['destination'], receipt_name} and all(
            C.sha256(files[path]) == digest for path, digest in receipt['destination'].items()),
            'migration_bytes_changed')
        captured = H.Store(copied / receipt['record']).capture()
        C._require(set(captured.commits) == {receipt['operation']}, 'newer_history_not_representable')
        commit = C.decode_document(captured.commits[receipt['operation']])
        source = T.validate_receipt(commit['receipt'])['before']
        C._require(source.get('observation') == receipt.get('observation') and
                   source.get('topology') == receipt.get('topology') and
                   source.get('inverse') == receipt.get('inverse') and
                   source.get('artifact_root', ARTIFACTS) == artifacts, 'invalid_migration_observation')
        if receipt['version'] == 2:
            C._require(source.get('originals_storage') == receipt['originals'], 'invalid_original_mapping')
        candidate = _validate_pending(Snapshot.from_json(files[artifacts + '/candidate.json']))
        C._require(candidate.snapshot_id == receipt['candidate_snapshot_id'] and
                   G.identity(candidate.to_data()['document']) == G.identity(A.from_store_capture(captured).document),
                   'migration_replay_mismatch')
        if receipt.get('observation'):
            observed = _validate_pending(Snapshot.from_json(files[receipt['observation']['path']]))
            C._require(observed.snapshot_id == receipt['observation']['snapshot_id'], 'migration_replay_mismatch')
            expected = observed.to_data()
            actual = candidate.to_data()
            C._require(_hypothesis_identity(actual['hypotheses']) == _hypothesis_identity(expected['hypotheses']) and
                       all(G.identity(actual['context'].get(key)) == G.identity(expected['context'].get(key))
                           for key in ('pending', 'target', 'project', 'history_contributions', 'conflicts')),
                       'migration_context_mismatch')
        C._require(_copy_inventory(copied) == files, 'migration_copy_changed')
        return candidate
