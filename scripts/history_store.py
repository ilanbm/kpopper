"""Bounded committed history storage. Reads observe evidence and never publish it.

Activation belongs to migration. Acceptance is reduced here independently of
falsifier computation, dependency assessment, and review sufficiency.
"""
import copy
import re
from dataclasses import dataclass
from pathlib import Path

from . import history_contract as C, history_transaction as T, provenance as P, versions as V
from .pending_grounding import identity

MAX_CAPTURE_BYTES = T.MAX_TRANSACTION_BYTES
MAX_REDUCTION_WORK = 4_000_000
MAX_RECONCILIATION_WORK = 4_000_000


def _view_alternatives(raw):
    """Split complete, column-zero Git markers; YAML text is never a marker.

    Parse valid YAML first: quoted/block strings containing marker-like text
    remain ordinary authored content. Invalid/nested/truncated conflicts refuse.
    Each side includes all common lines, preserving edits outside the hunks.
    """
    try:
        return (('view', raw, C.decode_document(raw)),)
    except C.HistoryError:
        pass
    marker = re.compile(rb'^(<{7,}|\|{7,}|={7,}|>{7,})(?:[ \t].*)?$')
    sides = {'ours': [], 'theirs': [], 'base': []}
    active, width, count, bases = None, None, 0, 0
    for line in raw.splitlines(keepends=True):
        match = marker.fullmatch(line.rstrip(b'\r\n'))
        if match is None:
            if active is None:
                for output in sides.values():
                    output.append(line)
            else:
                sides[active].append(line)
            continue
        token = match[1]
        kind = token[:1]
        if kind == b'<':
            C._require(active is None, 'invalid_view_conflict', 'nested marker')
            active, width = 'ours', len(token)
            count += 1
        else:
            C._require(active is not None and len(token) == width,
                       'invalid_view_conflict', 'unmatched marker')
            if kind == b'|':
                C._require(active == 'ours', 'invalid_view_conflict')
                active = 'base'
                bases += 1
            elif kind == b'=':
                C._require(active in ('ours', 'base') and
                           line.rstrip(b'\r\n') == token, 'invalid_view_conflict')
                active = 'theirs'
            else:
                C._require(active == 'theirs', 'invalid_view_conflict')
                active, width = None, None
    C._require(count and active is None, 'invalid_view_conflict', 'missing complete conflict')
    # A mixture of ordinary and diff3 hunks has no complete reconstructed base.
    C._require(bases in (0, count), 'invalid_view_conflict', 'incomplete base alternative')
    names = ('ours', 'theirs', 'base') if count == bases else ('ours', 'theirs')
    return tuple((name, b''.join(sides[name]), C.decode_document(b''.join(sides[name])))
                 for name in names)


def _view_changes(before, after):
    """Typed leaf differences, including presence, without inventing act intent."""
    changes = []

    def walk(left, right, path):
        if identity(left) == identity(right):
            return
        if isinstance(left, dict) and isinstance(right, dict):
            for key in sorted(set(left) | set(right)):
                if key in left and key in right:
                    walk(left[key], right[key], path + [key])
                else:
                    changes.append({'path': path + [key], 'before_present': key in left,
                                    'after_present': key in right,
                                    **({'before': copy.deepcopy(left[key])} if key in left else {}),
                                    **({'after': copy.deepcopy(right[key])} if key in right else {})})
        else:
            changes.append({'path': path, 'before_present': True, 'after_present': True,
                            'before': copy.deepcopy(left), 'after': copy.deepcopy(right)})

    walk(before, after, [])
    return changes


def _claim_key(obj):
    # Operation, writer and time do not turn agreement into disagreement. Pins
    # and authored interpretation do: equal printed bodies need not mean equality.
    return identity(C.claim_meaning(obj))


def reduce(objects, rules=None, ancestry=None):
    """Pure acceptance/act reduction; never evaluates a predicate or a review."""
    objects = C.validate_closure(objects)
    rules = C.detached(dict(V.RULES, **(rules or {})))
    grouped = {}
    for vid, obj in sorted(objects.items()):
        grouped.setdefault(obj['subject'], {})[vid] = obj
    C._require(sum(len(v) ** 2 for v in grouped.values()) <= MAX_REDUCTION_WORK,
               'history_limit', 'acceptance reduction work')
    subjects = {}
    for subject, held in sorted(grouped.items()):
        C._require(len({v['kind'] for v in held.values() if v['kind'] != 'act'}) <= 1,
                   'mixed_subject_kind', subject)
        try:
            entry = V._subject_entry(subject, held, rules, ancestry, claim_key=_claim_key)
        except (TypeError, ValueError) as error:
            raise C.HistoryError('invalid_source_clock', subject) from error
        if entry is None:
            continue
        if entry['status'] in ('accepted', 'pending'):
            acceptance = 'accepted'
        elif entry['status'] in ('contested', 'divergent'):
            acceptance = 'contested'
        elif entry['proposals']:
            acceptance = 'proposed'
        elif 'corrected' in entry['marks'].values():
            acceptance = 'corrected'
        elif 'refuted' in entry['marks'].values():
            acceptance = 'refuted'
        elif 'retired' in entry['marks'].values():
            acceptance = 'retired'
        else:
            acceptance = 'unavailable'
        entry['acceptance'] = acceptance
        entry['status'] = acceptance
        entry['review_evidence'] = entry.pop('reviews')
        subjects[subject] = entry
    return {'subjects': subjects, 'rules': rules,
            'implied': sorted((item for e in subjects.values() for item in e['implied']),
                              key=lambda i: (i['subject'], i['superseded'], i['by']))}


def baseline(marker, commits, state):
    return C.validate_baseline({
        'version': 1, 'record_id': marker['record_id'],
        'authority_generation': marker['generation'],
        'committed_set_digest': C.committed_set_digest(commits),
        'heads': {s: e['heads'] for s, e in state['subjects'].items()},
        'open_acts': {s: sorted({a for ids in e['open_acts'].values() for a in ids})
                      for s, e in state['subjects'].items()}})


@dataclass(frozen=True)
class Capture:
    """Detached raw evidence. Mutating copies never changes files or authority."""
    entry_bytes: bytes
    document: dict
    marker: dict
    commits: dict
    object_bytes: dict
    objects: dict
    state: dict
    baseline: dict
    inventory: dict
    authority_bytes: bytes = b''
    view_alternatives: tuple = ()


class Store:
    def __init__(self, entry):
        self.entry = Path(entry).absolute()
        self.root = self.entry.parent
        self.layout = P.layout(self.entry)

    def _path(self, path):
        try:
            relative = Path(path).relative_to(self.root)
        except ValueError:
            try:
                relative = Path(path).relative_to(self.root.resolve(strict=True))
            except ValueError:
                raise C.HistoryError('invalid_path', str(path)) from None
        return T._target(self.root, relative.as_posix())

    def capture(self, *, rules=None, ancestry=None):
        """Capture exact membership and bytes, including inactive staging residue."""
        with T.reader_guard(self.root, T.journal_for(self.entry)):
            return self._capture(rules=rules, ancestry=ancestry)

    def capture_reconciliation(self, *, allow_conflicts=False):
        """Read-only opt-in capture retaining exact Git conflict alternatives.

        Normal readers keep using ``capture`` and reject invalid generated YAML.
        This loader does not certify an alternative as an authentic baseline.
        ``prepare_reconciliation`` performs that complete-closure check.
        """
        with T.reader_guard(self.root, T.journal_for(self.entry)):
            return self._capture(allow_conflicts=allow_conflicts)

    def _capture(self, *, rules=None, ancestry=None, allow_conflicts=False):
        from .reasoning.snapshot import _Inventory
        inventory = _Inventory(retain_bytes=True)
        total = 0

        def event(kind, path, value):
            inventory(kind, str(path), value)
            P._capture_event(kind, str(path), value)

        def read(path, maximum):
            nonlocal total
            path = self._path(path)
            exists = path.exists()
            event('exists', path, exists)
            C._require(exists and path.is_file(), 'missing_history_file', str(path))
            C._require(path.stat().st_size <= maximum, 'history_limit')
            with path.open('rb') as stream:
                raw = stream.read(maximum + 1)
            total += len(raw)
            C._require(len(raw) <= maximum and total <= MAX_CAPTURE_BYTES, 'history_limit')
            event('bytes', path, raw)
            return raw

        def listing(path):
            path = self._path(path)
            event('directory', path, path.is_dir())
            event('exists', path, path.exists())
            C._require(not path.exists() or path.is_dir(), 'invalid_history_path')
            import glob
            pattern = glob.escape(str(path)) + '/*'
            found = sorted(glob.glob(pattern))
            C._require(len(found) <= C.MAX_OBJECTS, 'history_limit')
            event('glob', pattern, found)
            return [self._path(p) for p in found]

        entry_bytes = read(self.entry, C.MAX_REQUEST_BYTES)
        marker_path = self._path(self.layout['history_authority'])
        event('exists', marker_path, marker_path.exists())
        C._require(marker_path.exists(), 'history_not_active')
        authority_bytes = read(marker_path, C.MAX_OBJECT_BYTES)
        marker = C.validate_authority(C.decode_document(authority_bytes))
        C._require(marker['authority'] == 'history', 'history_not_active')
        alternatives = _view_alternatives(entry_bytes) if allow_conflicts else (
            ('view', entry_bytes, C.decode_document(entry_bytes)),)
        document = alternatives[0][2]
        for _, _, alternative in alternatives:
            meta = alternative.get('meta', {})
            C._require(isinstance(meta, dict) and 'history' in meta, 'baseline_mismatch')
            C.bind_authority(marker, meta['history'])
        commits, objects = {}, {}
        for path in listing(self.layout['history_commits']):
            C._require(path.suffix == '.yaml' and path.is_file(), 'invalid_history_path')
            C._text(path.stem)
            commits[path.stem] = read(path, C.MAX_REQUEST_BYTES)
        for directory in listing(self.layout['history']):
            C._require(directory.is_dir(), 'invalid_history_path')
            C._text(directory.name, C.SUBJECT)
            for path in listing(directory):
                C._require(path.suffix == '.yaml' and path.is_file(), 'invalid_history_path')
                C._text(path.stem, C.OBJECT_ID)
                C._require(len(objects) < C.MAX_OBJECTS, 'history_limit')
                objects[(directory.name, path.stem)] = read(path, C.MAX_OBJECT_BYTES)
        selected = C.committed_objects(marker, commits, objects)
        # A stale view may name older heads, which remain in complete history.
        # Missing manifest membership cannot turn that evidence into empty state.
        for _, _, alternative in alternatives:
            for role, kind in (('heads', 'claim'), ('open_acts', 'act')):
                for subject, ids in alternative['meta']['history'][role].items():
                    for vid in ids:
                        obj = selected.get(vid)
                        C._require(obj is not None, 'incomplete_view_baseline', vid)
                        C._require(obj['subject'] == subject and
                                   (obj['kind'] == 'act') == (kind == 'act'),
                                   'baseline_reference_mismatch', vid)
        state = reduce(selected, rules, ancestry)
        current = baseline(marker, commits, state)
        inventory.verify()
        return Capture(entry_bytes, document, marker, commits, objects, selected,
                       state, current, copy.deepcopy(inventory.events), authority_bytes,
                       tuple(copy.deepcopy(alternatives)))

    def state(self, capture=None, *, rules=None, ancestry=None):
        captured = capture or self.capture(rules=rules, ancestry=ancestry)
        return copy.deepcopy(captured.state)

    @staticmethod
    def _template(commits):
        manifests = {op: C.validate_commit(C.decode_document(raw)) for op, raw in commits.items()}
        parents = {parent for m in manifests.values() for parent in m['parents']}
        frontier = [m for op, m in manifests.items() if op not in parents]
        C._require(frontier and all('view_template' in m for m in frontier), 'unresolved_template')
        templates = {identity(m['view_template']): m['view_template'] for m in frontier}
        C._require(len(templates) == 1, 'contested_template')
        return copy.deepcopy(next(iter(templates.values())))

    def render(self, captured, *, objects=None, commits=None):
        """Render original bodies into immutable frontier templates.

        Ordinary-reader mapping collections retain their names and shape. A
        Missing authored mappings refuse the entire render. Nonaccepted subjects
        have no scalar projection; their alternatives and acts remain in captured
        evidence. Original `seen` is copied verbatim; pins never replace bodies.
        """
        objects = captured.objects if objects is None else C.validate_closure(objects)
        commits = captured.commits if commits is None else commits
        state = reduce(objects, captured.state['rules'])
        document = self._template(commits)
        roles = copy.deepcopy(document.get('schema', {}))
        C._require(isinstance(roles, dict), 'unresolved_mapping', 'schema')
        selected_profile = None
        for obj in objects.values():
            if obj['kind'] == 'act':
                continue
            authored = obj.get('authored')
            C._require(authored is not None, 'unresolved_mapping', obj['subject'])
            collection = authored['collection']
            C._require(collection not in ('meta', 'schema', 'record', 'also')
                       and collection in document and isinstance(document[collection], dict),
                       'unresolved_mapping', collection)
        for subject, entry in state['subjects'].items():
            if entry['acceptance'] != 'accepted':
                continue
            C._require('head' in entry, 'unresolved_projection', subject)
            obj = objects[entry['head']]
            authored = obj['authored']
            C._require(selected_profile in (None, authored['profile']),
                       'incompatible_authored_profiles', subject)
            selected_profile = authored['profile']
            for role, field in authored['fields'].items():
                C._require(role not in roles or roles[role] == field,
                           'incompatible_field_roles', subject)
                roles[role] = field
            collection = authored['collection']
            C._require(not document[collection].get(subject), 'duplicate_projection', subject)
            document[collection][subject] = copy.deepcopy(obj['body'])
        declaration = document.get('meta', {}).get('reasoning')
        if declaration is not None:
            C._require(isinstance(declaration, dict) and
                       selected_profile in (None, declaration.get('profile')),
                       'incompatible_authored_profiles')
        if selected_profile == 'core/v1':
            from .reasoning.contract import capabilities
            C._require(declaration is not None, 'missing_reasoning_declaration')
            C._require(capabilities(document)['profile'] == selected_profile,
                       'incompatible_authored_profiles')
        document.setdefault('meta', {})['history'] = baseline(captured.marker, commits, state)
        return C.encode_document(document)

    def _known_view(self, captured):
        """A manifest digest certifies rendered bytes, never a hand-edited scalar."""
        manifests = {op: C.decode_document(raw) for op, raw in captured.commits.items()}
        if any(manifest['view_sha256'] == C.sha256(captured.entry_bytes) for manifest in manifests.values()):
            return True
        # A canonical union rebuild has no knowledge commit of its own. A later
        # commit's bound before baseline and parent closure can attest that view.
        expected = identity(captured.document['meta']['history'])
        examined = set()
        for manifest in manifests.values():
            if manifest['baseline_digest'] != expected:
                continue
            parents = tuple(sorted(manifest['parents']))
            if parents in examined:
                continue
            examined.add(parents)
            known, pending = set(), list(parents)
            while pending:
                parent = pending.pop()
                if parent not in known:
                    known.add(parent)
                    pending.extend(manifests[parent]['parents'])
            subset = {op: captured.commits[op] for op in known}
            if not subset:
                continue
            objects = C.committed_objects(captured.marker, subset, captured.object_bytes)
            state = reduce(objects, captured.state['rules'])
            if identity(baseline(captured.marker, subset, state)) == expected \
                    and self.render(captured, objects=objects, commits=subset) == captured.entry_bytes:
                return True
        return False

    def _baseline_views(self, captured, wanted, retained_baselines):
        """Find exact historical closures without guessing arbitrary subsets."""
        manifests = {op: C.validate_commit(C.decode_document(raw))
                     for op, raw in captured.commits.items()}
        work = 0
        examined, found = set(), {}

        def closure(frontier):
            nonlocal work
            result, pending = set(), list(frontier)
            while pending:
                op = pending.pop()
                if op not in result:
                    work += 1
                    C._require(work <= MAX_RECONCILIATION_WORK, 'history_limit', 'baseline search')
                    C._require(op in manifests, 'incomplete_view_baseline', op)
                    result.add(op)
                    pending.extend(manifests[op]['parents'])
            return tuple(sorted(result))

        def candidates():
            yield tuple(sorted(manifests))
            for retained in retained_baselines:
                C._require(isinstance(retained, Capture), 'invalid_retained_baseline')
                C._require(identity(retained.marker) == identity(captured.marker), 'authority_mismatch')
                C._require(all(captured.commits.get(op) == raw
                               for op, raw in retained.commits.items()), 'incomplete_view_baseline')
                yield closure(retained.commits)
            for op, manifest in sorted(manifests.items()):
                yield closure([op])
                yield closure(manifest['parents'])

        for operations in candidates():
            if operations in examined:
                continue
            examined.add(operations)
            work += len(operations)
            C._require(work <= MAX_RECONCILIATION_WORK, 'history_limit', 'baseline search')
            commits = {op: captured.commits[op] for op in operations}
            digest = C.committed_set_digest(commits)
            if digest not in wanted:
                continue
            objects = C.committed_objects(captured.marker, commits, captured.object_bytes)
            state = reduce(objects, captured.state['rules'])
            bound = baseline(captured.marker, commits, state)
            if commits:
                document = C.decode_document(self.render(captured, objects=objects, commits=commits))
            else:
                # An initial import retains its original empty render template.
                templates = {identity(m['view_template']): m['view_template']
                             for m in manifests.values() if not m['parents']
                             and m['baseline_digest'] == identity(bound) and 'view_template' in m}
                C._require(len(templates) == 1, 'unresolved_template')
                document = copy.deepcopy(next(iter(templates.values())))
                document.setdefault('meta', {})['history'] = bound
            found[digest] = (bound, document, objects, state)
            if set(found) == wanted:
                break
        C._require(set(found) == wanted, 'incomplete_view_baseline', 'retained committed closure required')
        return found

    def prepare_reconciliation(self, capture=None, *, allow_conflicts=False, retained_baselines=()):
        """Describe manual edits against complete immutable original evidence.

        This is a read-only preparation description, not a prepared writer
        mutation. It infers no acceptance, correction, deletion or review acts.
        ``changes`` retain exact typed before/after values and missing keys.
        Body edits are proposal candidates; recorded claim matches and recorded
        acts are evidence only. Headers need an explicit template-authoring path.
        Historical union views can supply retained Captures when their frontier
        is not discoverable from current manifest ancestry. Their manifest bytes
        must already exist identically in the live committed closure.
        """
        live = self.capture_reconciliation(allow_conflicts=allow_conflicts)
        if capture is not None:
            C._require(live.inventory == capture.inventory, 'stale_baseline')
        alternatives = live.view_alternatives
        wanted = {doc['meta']['history']['committed_set_digest'] for _, _, doc in alternatives}
        views = self._baseline_views(live, wanted, retained_baselines)
        descriptions = []
        for name, raw, document in alternatives:
            observed = document['meta']['history']
            bound, original, objects, state = views[observed['committed_set_digest']]
            C._require(identity(observed) == identity(bound), 'baseline_mismatch')
            changes = _view_changes(original, document)
            if not changes and raw != C.encode_document(original):
                # Comments and formatting have no typed representation. Without
                # exact retained generated bytes they are still authored edits.
                changes.append({'path': [], 'kind': 'text_edit',
                                'before_sha256': C.sha256(C.encode_document(original)),
                                'after_sha256': C.sha256(raw)})
            collections = set(P.collections_of(original)) - {'meta'}
            recorded_bodies, edited_matches = {}, {}
            for vid, obj in objects.items():
                if obj['kind'] != 'act':
                    key = (obj['subject'], obj.get('authored', {}).get('collection'),
                           identity(obj['body']))
                    recorded_bodies.setdefault(key, []).append(vid)
            for change in changes:
                if change.get('kind') == 'text_edit':
                    continue
                path = change['path']
                if len(path) >= 2 and path[0] in collections:
                    subject = path[1]
                    key = (subject, path[0])
                    if key not in edited_matches:
                        body = document.get(path[0], {}).get(subject)
                        edited_matches[key] = sorted(recorded_bodies.get((*key, identity(body)), []))
                    change.update(kind='proposal_candidate', subject=subject,
                                  baseline_heads=bound['heads'].get(subject, []),
                                  recorded_claim_matches=edited_matches[key])
                else:
                    change['kind'] = 'header_edit'
            descriptions.append({'name': name, 'entry_sha256': C.sha256(raw),
                                 'baseline': bound, 'changes': changes,
                                 'original_view': original, 'edited_view': document,
                                 'original_versions': {
                                     subject: sorted(set(entry['heads']) | set(entry['proposals']))
                                     for subject, entry in state['subjects'].items()},
                                 'recorded_acts': sorted(vid for vid, obj in objects.items()
                                                         if obj['kind'] == 'act')})
        result = C.detached({'version': 1, 'authority': live.marker,
                             'entry_sha256': C.sha256(live.entry_bytes),
                             'current_baseline': live.baseline,
                             'conflicted': alternatives[0][0] != 'view',
                             'alternatives': descriptions,
                             'rebuild_safe': not any(d['changes'] for d in descriptions)},
                            C.MAX_PROJECTION_BYTES)
        C._require(self.capture_reconciliation(allow_conflicts=allow_conflicts).inventory == live.inventory,
                   'stale_baseline')
        return result

    def rebuild(self, capture=None, *, write=False, allow_conflicts=False, retained_baselines=()):
        """Regenerate a known view; stale or edited unknown baselines require reconciliation."""
        with T.writer_guard(self.root):
            live = self.capture_reconciliation(allow_conflicts=allow_conflicts)
            if capture is not None:
                C._require(live.inventory == capture.inventory, 'stale_baseline')
            rendered = self.render(live)
            if allow_conflicts:
                description = self.prepare_reconciliation(live, allow_conflicts=True,
                                                          retained_baselines=retained_baselines)
                C._require(description['rebuild_safe'], 'unresolved_view_edit')
            else:
                C._require(self._known_view(live) or identity(live.document) ==
                           identity(C.decode_document(rendered)), 'unresolved_view_edit')
            if write:
                C._require(self.capture_reconciliation(allow_conflicts=allow_conflicts).inventory ==
                           live.inventory, 'stale_baseline')
                T._replace(self._path(self.entry), rendered)
            return rendered

    def commit(self, mutation, *, verify):
        """Verify live baseline, stage objects, publish manifest, refresh exact view.

        `verify(prepared_data)` rechecks caller capability/evaluator evidence. It is
        mandatory even on retries. Exact manifest replay repairs only matching
        before/after entry bytes; it never overwrites an unrelated edit.
        """
        C._require(isinstance(mutation, T.PreparedMutation) and callable(verify), 'missing_verifier')
        data = mutation.to_data()
        C._require(data['entry'] == self.entry.name, 'entry_mismatch')
        C._require(data['authority']['authority'] == 'history', 'history_not_active')
        with T.writer_guard(self.root):
            live = self.capture()
            C._require(identity(data['authority']) == identity(live.marker), 'authority_mismatch')
            files = mutation.files
            C._require(all(i['role'] in ('record', 'history_object', 'history_commit', 'history_evidence') for i in files),
                       'unsupported_file_role')
            manifest_item = next(i for i in files if i['role'] == 'history_commit')
            record = next(i for i in files if i['role'] == 'record')
            operation = data['operation']
            prior = live.commits.get(operation)
            edit_receipt = data['receipt']['before'].get('history_edit')
            if edit_receipt is not None:
                C._require(edit_receipt == {'version': 1, 'kind': 'view-edit-proposals'}, 'invalid_edit_receipt')
                from . import history_edits
                history_edits.verify_prepared(self.entry, mutation)
            if prior is not None:
                C._require(prior == manifest_item['after'], 'operation_collision')
            else:
                C._require(identity(data['baseline']) == identity(live.baseline), 'stale_baseline')
                C._require(identity(live.document['meta']['history']) == identity(live.baseline),
                           'stale_view')
                C._require(live.entry_bytes == record['before'], 'concurrent_edit')
                if live.commits and edit_receipt is None:
                    C._require(identity(live.document) == identity(C.decode_document(self.render(live))),
                               'unresolved_view_edit')
            C._require(live.entry_bytes in (record['before'], record['after']), 'concurrent_edit')
            combined = dict(live.commits, **{operation: manifest_item['after']})
            staged = dict(live.object_bytes)
            for item in files:
                self._path(self.root / item['path'])
                if item['role'] == 'history_evidence':
                    C._require(T._read(self._path(self.root / item['path'])) in (None, item['after']),
                               'immutable_collision', item['path'])
                if item['role'] == 'history_object':
                    obj = C.validate_object(C.decode_document(item['after']))
                    key = (obj['subject'], obj['id'])
                    C._require(key not in staged or staged[key] == item['after'], 'immutable_collision')
                    staged[key] = item['after']
            selected = C.committed_objects(live.marker, combined, staged)
            C._require(sum(map(len, combined.values())) + sum(map(len, staged.values()))
                       <= MAX_CAPTURE_BYTES, 'history_limit')
            expected_view = self.render(live, objects=selected, commits=combined)
            C._require(record['after'] == expected_view, 'view_projection_mismatch')
            commit = C.decode_document(manifest_item['after'])
            expected_parents = C.commit_frontier({op: raw for op, raw in live.commits.items() if op != operation})
            if prior is None:
                C._require(commit['parents'] == expected_parents, 'parent_baseline_mismatch')
            verify(data)
            # The callback may consult external evidence or accidentally change
            # local files. Its return never waives optimistic source checks.
            C._require(self.capture().inventory == live.inventory, 'stale_baseline')
            for item in files:
                if item['role'] in ('history_object', 'history_evidence'):
                    T.publish_immutable(self.root / item['path'], item['after'], root=self.root)
            T.publish_immutable(self.root / manifest_item['path'], manifest_item['after'], root=self.root)
            current_entry = T._read(self._path(self.entry))
            C._require(current_entry in (record['before'], record['after']), 'concurrent_edit')
            if current_entry != record['after']:
                T._replace(self._path(self.entry), record['after'])
            return C.validate_commit(commit)
