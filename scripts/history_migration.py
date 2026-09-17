"""Lossless, copy-only import from an ordinary record into immutable history.

Import records a new capture event, never an invented historical observation.
Original claim provenance and unrecorded dependency pins remain explicitly unknown.
The sole authority is the generated entry; retained original member paths are
bound evidence, not additional writable authorities. Live cutover is separate.
"""
import copy
import datetime
import glob
from pathlib import Path
import uuid

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


def _capture_history(source, record):
    """Retain inactive stores verbatim, without adopting their old acceptance."""
    layout = P.layout(record)
    inventory = source.inventory
    other = P.layout(record.parent / (P.ENTRY if layout['legacy'] else P.LEGACY_ENTRY))
    for role in ('history', 'history_commits', 'history_authority'):
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
                    pending.append(child)
                else:
                    C._require(child.is_file() and child.stat().st_size <= C.MAX_REQUEST_BYTES,
                               'history_limit')
                    inventory('bytes', member, child.read_bytes())
    source.verify()


def _original_provenance():
    return {'writer': None, 'operation': None, 'recorded_at': None,
            'source_clock': None, 'condition_profile': None}


class Plan:
    def __init__(self, record, *, route=False, read_mode='frozen', operation=None, recorded_at=None):
        C._require(read_mode == 'frozen', 'history_import_requires_frozen_source')
        record = Path(record).expanduser().resolve()
        if route:
            record = Path(V.write_paths([str(record)])[0]).resolve()
        self.record = record
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
        self.source_files = self.source.files
        self.mapping = {}
        for path in self.source_files:
            C._require(Path(path).is_relative_to(record.parent), 'external_history_import_member', path)
            relative = Path(path).relative_to(record.parent).as_posix()
            C.relative_path(relative)
            C._require(relative != ARTIFACTS and not relative.startswith(ARTIFACTS + '/'),
                       'migration_artifact_collision')
            self.mapping[path] = relative
        # Capture the reader's own origin map while retaining the same byte inventory.
        token, core = P._CAPTURE_READS.set(self.source.inventory), P._CORE_READS.set(True)
        try:
            self.loaded = P.load([str(record)], read_mode='frozen')
        finally:
            P._CORE_READS.reset(core)
            P._CAPTURE_READS.reset(token)
        self.source.verify()
        self.problems = []
        self.files = {self.mapping[path]: raw for path, raw in self.source_files.items()}
        self.objects, self.locators = {}, []
        self._prepare()

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
            if subject not in known:
                self.problems.append('archive_successor_unknown: ' + subject)
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
                seen = historical.get(fields['snapshot'], {})
                has_core_history = isinstance(seen, dict) and any(
                    isinstance(value, dict) and isinstance(value.get('computed'), dict)
                    and value['computed'].get('version') == 2 for value in seen.values())
                if has_core_history or any(isinstance(historical.get(field), dict)
                                           for field in ('rule', fields['predicate'])):
                    # The archive has no original record declaration. Retention
                    # is lossless, but ordinary interpretation cannot establish
                    # the original executable condition's meaning.
                    self.problems.append('archive_condition_profile_unknown: ' + subject + ':' + str(index))
                old = self._claim(subject, collection, historical, fields, 'ordinary-reader/v1', location,
                                  saw=saw, judgment=True)
                if retired is not None:
                    act = self._replace(subject, retired, old, saw=[*saw, old['id']], locator=location)
                    saw.append(act['id'])
                retired = old
                saw.append(old['id'])
            current = self._claim(subject, collection, body, fields, profile, locator, saw=saw,
                                  judgment=isinstance(body, dict) and fields['deps'] in body)
            if retired is not None:
                self._replace(subject, retired, current, saw=[*saw, current['id']], locator=locator)
        C.validate_closure(self.objects)
        marker_source = self.source_files.get(P.layout(self.record)['history_authority'])
        if marker_source is not None:
            previous = C.validate_authority(C.decode_document(marker_source))
            record_id, generation = previous['record_id'], previous['generation'] + 1
        else:
            record_id, generation = 'record-' + uuid.uuid4().hex, 1
        self.marker = C.authority(record_id=record_id, authority='history', generation=generation)
        self.files[ARTIFACTS + '/original.json'] = self.original.to_json().encode()
        for path, raw in self.source_files.items():
            self.files[ARTIFACTS + '/originals/' + self.mapping[path]] = raw
        members = {ARTIFACTS + '/original.json': 'retained_original'}
        members.update({ARTIFACTS + '/originals/' + relative: 'retained_original'
                        for relative in self.mapping.values()})
        members.update({self.mapping[path]: 'retained_original' for path in self.record_files
                        if path != str(self.record)})
        if archive_path in self.source_files:
            members[self.mapping[archive_path]] = 'replaced'
        template = C.document_template(document)
        physical_entry = _parse(self.source_files[str(self.record)])
        for key in ('record', 'also'):
            if key in physical_entry:
                template[key] = copy.deepcopy(physical_entry[key])
            else:
                template.pop(key, None)
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
                    'source': {self.mapping[path]: C.sha256(raw) for path, raw in self.source_files.items()}},
            after={'kind': 'preserved-authored-import/v1', 'operation': self.operation,
                   'recorded_at': self.recorded_at, 'original_provenance': _original_provenance(),
                   'locators': self.locators, 'unresolved': self.problems})
        draft = C.make_commit(marker=self.marker, operation=self.operation, parents={}, baseline=baseline,
                              objects=pairs, receipt=receipt, view=b'', view_template=template)
        commits = {self.operation: C.encode_document(draft)}
        store = H.Store(self.record)
        rendered = store.render(capture, objects=self.objects, commits=commits)
        manifest = C.make_commit(marker=self.marker, operation=self.operation, parents={}, baseline=baseline,
                                 objects=pairs, receipt=receipt, view=rendered, view_template=template)
        commits = {self.operation: C.encode_document(manifest)}
        object_bytes = {(obj['subject'], obj['id']): raw for obj, raw in pairs}
        state = H.reduce(self.objects)
        capture = H.Capture(rendered, C.decode_document(rendered), self.marker, commits, object_bytes,
                            self.objects, state, H.baseline(self.marker, commits, state), {})
        adapted = A.from_store_capture(capture)
        context = copy.deepcopy(data['context'])
        context['migration'] = {'kind': 'history-import/v1', 'source_snapshot_id': self.original.snapshot_id,
                                'original_provenance': 'unknown unless retained in original body'}
        self.candidate = adapted.snapshot(context=context, hypotheses=data['hypotheses'], as_of=data['as_of'])
        self.files[self.record.name] = rendered
        layout = P.layout(self.record)
        self.files[Path(layout['history_authority']).relative_to(self.record.parent).as_posix()] = C.encode_document(self.marker)
        commit_path = Path(layout['history_commits']) / (self.operation + '.yaml')
        C._require(str(commit_path) not in self.source_files, 'operation_collision')
        self.files[commit_path.relative_to(self.record.parent).as_posix()] = C.encode_document(manifest)
        for obj, raw in pairs:
            path = Path(layout['history']) / obj['subject'] / (obj['id'] + '.yaml')
            relative = path.relative_to(self.record.parent).as_posix()
            C._require(relative not in self.files or self.files[relative] == raw, 'immutable_collision')
            self.files[relative] = raw
        self.files[ARTIFACTS + '/candidate.json'] = self.candidate.to_json().encode()
        self.manifest = {'version': 1, 'kind': 'history-import/v1', 'operation': self.operation,
            'record': self.record.name, 'record_id': self.marker['record_id'], 'complete': not self.problems,
            'problems': self.problems, 'source_snapshot_id': self.original.snapshot_id,
            'candidate_snapshot_id': self.candidate.snapshot_id, 'locators': self.locators,
            'originals': {self.mapping[path]: {'path': ARTIFACTS + '/originals/' + self.mapping[path],
                                             'sha256': C.sha256(raw)} for path, raw in self.source_files.items()},
            'destination': {path: C.sha256(raw) for path, raw in sorted(self.files.items())}}
        self.files[ARTIFACTS + '/receipt.json'] = _json(self.manifest)
        self.source.verify()

    def summary(self):
        return {'state': 'blocked' if self.problems else 'preview', 'complete': not self.problems,
                'historical_support_complete': self.candidate.to_data()['context']['history']['coverage']['complete'],
                'record': str(self.record), 'operation': self.operation, 'problems': list(self.problems),
                'original_provenance': _original_provenance(), 'manifest': copy.deepcopy(self.manifest)}

    def _populate(self, root):
        for name, raw in self.files.items():
            path = T._target(root, name)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(raw)

    def validate_destination(self, destination):
        self.source.verify()
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
        C._require(G.identity(copied.to_data()['hypotheses']) == G.identity(self.original.to_data()['hypotheses']),
                   'migration_hypothesis_mismatch')
        for kind, expected in (('original', self.original), ('candidate', self.candidate)):
            artifact = Snapshot.from_json((destination / ARTIFACTS / (kind + '.json')).read_bytes())
            C._require(artifact.snapshot_id == expected.snapshot_id, 'migration_replay_mismatch')
        self.source.verify()
        return {'valid': True, 'record': str(destination / self.record.name),
                'snapshot_id': copied.snapshot_id, 'manifest': copy.deepcopy(self.manifest)}

    def publish(self, destination):
        C._require(not self.problems, 'incomplete_history_import', '; '.join(self.problems))
        self.source.verify()
        root = V.publish_tree(destination, self._populate, self.validate_destination)
        return {**self.summary(), 'state': 'materialized', 'record': str(root / self.record.name),
                'receipt': str(root / ARTIFACTS / 'receipt.json'), 'read_mode': 'frozen'}

    def restore_copy(self, copied, destination):
        """Export exact pre-import bytes into absence, only from an unchanged copy.

        New committed knowledge makes the copy differ and refuses this rehearsal
        inverse. This is not an in-place authority rollback or live cutover.
        """
        self.validate_destination(copied)
        originals = {self.mapping[path]: raw for path, raw in self.source_files.items()}
        def populate(root):
            for name, raw in originals.items():
                path = T._target(root, name)
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(raw)
        def verify(root):
            self.source.verify()
            for name, raw in originals.items():
                C._require((root / name).read_bytes() == raw, 'migration_bytes_changed', name)
            replay = Snapshot.capture([str(root / self.record.name)], read_mode='frozen')
            C._require(_entry_identity(replay.to_data()['document']) ==
                       _entry_identity(self.original.to_data()['document']), 'migration_body_mismatch')
        root = V.publish_tree(destination, populate, verify)
        return {'state': 'restored_copy', 'record': str(root / self.record.name)}


def prepare(record, *, route=False, read_mode='frozen', operation=None, recorded_at=None):
    return Plan(record, route=route, read_mode=read_mode, operation=operation, recorded_at=recorded_at)
