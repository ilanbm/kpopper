"""Explicit, frozen full-closure conversion; never publication or registration.

The capture owns discovery. This module edits captured physical bytes, retains the
originals, and validates an absent-destination tree before its atomic publication.
"""
import copy
import hashlib
import glob
import json
import os
from pathlib import Path
import tempfile

try:
    from . import provenance as P, knowledge_views as V, pending_grounding as G
    from .reasoning.snapshot import Snapshot, capture_source
    from .reasoning.conversion import convert_snapshot, compare_snapshots, report_to_json, TRANSFORMATION
    from .reasoning.authoring import pending_compatible
    from .reasoning.contract import CapabilityError
except ImportError:
    import provenance as _reader
    P = _reader._peer('provenance')
    V, G = P._peer('knowledge_views'), P._peer('pending_grounding')
    _snapshots, _conversion = P._peer('reasoning.snapshot'), P._peer('reasoning.conversion')
    Snapshot, capture_source = _snapshots.Snapshot, _snapshots.capture_source
    convert_snapshot, compare_snapshots = _conversion.convert_snapshot, _conversion.compare_snapshots
    report_to_json, TRANSFORMATION = _conversion.report_to_json, _conversion.TRANSFORMATION
    pending_compatible = P._peer('reasoning.authoring').pending_compatible
    CapabilityError = P._peer('reasoning.contract').CapabilityError

ARTIFACTS = '.kpopper-migration'


def _hash(data):
    return hashlib.sha256(data).hexdigest()


def _json(value):
    return G.json_bytes(G._encode(value))


def _origin(record, path):
    path = Path(path).absolute()
    try:
        return 'origin:' + path.relative_to(record.parent).as_posix()
    except ValueError:
        return 'external:' + _hash(str(path).encode())[:24] + '/' + path.name


def _portable(record, path):
    origin = _origin(record, path)
    return origin[7:] if origin.startswith('origin:') else '_external/' + origin[9:]


def _extend_inventory(source, record):
    """Observe layout adjuncts using reader-owned names, never rediscover records."""
    inventory = source.inventory
    for (kind, path), exists in list(inventory.events.items()):
        if kind == 'exists' and not exists:
            raise ValueError('missing referenced migration source: ' + path)
    layout = P.layout(record)
    other = P.layout(record.parent / (P.ENTRY if layout['legacy'] else P.LEGACY_ENTRY))
    for role in ('view', 'measure', 'session', 'hypotheses'):
        alternate = other[role]
        inventory('exists', alternate, os.path.exists(alternate))
        if role == 'hypotheses':
            found = []
            for suffix in ('*.yaml', '*.yml'):
                pattern = os.path.join(alternate, suffix)
                matches = sorted(map(os.path.abspath, glob.glob(pattern)))
                inventory('glob', pattern, matches)
                found.extend(matches)
            if found:
                raise ValueError('half-moved record layout: leftover hypotheses')
        elif os.path.exists(alternate):
            raise ValueError('half-moved record layout: leftover ' + role)
        path = layout[role]
        if role == 'hypotheses':
            # Capture absent directory globs too, so a new layer invalidates preview.
            for suffix in ('*.yaml', '*.yml'):
                pattern = os.path.join(path, suffix)
                inventory('glob', pattern, sorted(map(os.path.abspath, glob.glob(pattern))))
            continue
        inventory('exists', path, os.path.exists(path))
        if os.path.exists(path):
            try:
                inventory('bytes', path, Path(path).read_bytes())
            except OSError as error:
                raise ValueError('unreadable migration sidecar: ' + path) from error
    inventory.verify()


def _capture_evidence(source, record):
    """Copy explicit record-relative evidence; external locators remain locators."""
    def references(value):
        if isinstance(value, dict):
            if isinstance(value.get('file'), str):
                yield value['file']
            for child in value.values():
                yield from references(child)
        elif isinstance(value, list):
            for child in value:
                yield from references(child)
    documents = [source.snapshot.to_data()['document']]
    documents.extend(hyp['document'] for hyp in source.snapshot.to_data()['hypotheses'].values()
                     if hyp.get('kind') != 'contribution')
    for name in sorted(set(path for document in documents for path in references(document))):
        # Absolute, URL and parent-relative locators confer no extra file access.
        if os.path.isabs(name) or '://' in name or '..' in Path(name).parts:
            continue
        # A leading ./ is the same local locator; retain its authored spelling.
        G.M.relative_path(Path(name).as_posix())
        if '.git' in Path(name).parts or name.startswith(ARTIFACTS + '/'):
            raise ValueError('evidence cannot name administration or migration artifacts')
        path = record.parent / name
        if not path.resolve().is_relative_to(record.parent.resolve()):
            raise ValueError('evidence locator escapes the captured record directory')
        absolute = str(path.absolute())
        source.inventory('exists', absolute, path.exists())
        if path.is_dir():
            raise ValueError('directory evidence locator requires an explicit file: ' + name)
        if not path.is_file():
            raise ValueError('missing referenced evidence: ' + name)
        try:
            source.inventory('bytes', absolute, path.read_bytes())
        except OSError as error:
            raise ValueError('unreadable referenced evidence: ' + name) from error
    source.verify()


def _observe_admin(source, project):
    # Administration is verified privately, never copied beside the destination.
    paths = [project.config_path, project.state / 'publication.json']
    if project.git:
        paths.append(project.common / 'kpopper-record')
    for path in paths:
        path = str(path.absolute())
        source.inventory('exists', path, os.path.exists(path))
        if os.path.exists(path):
            try:
                data = Path(path).read_bytes()
            except OSError as error:
                raise ValueError('unreadable project observation') from error
            source.inventory.events[('bytes', path)] = _hash(data)
    source.verify()
    return set(str(path.absolute()) for path in paths)


def _parse(data):
    try:
        doc = P.parse(text=data.decode('utf-8')) or {}
    except (P.yaml.YAMLError, UnicodeDecodeError) as error:
        raise ValueError('invalid migration record: ' + str(error)) from error
    if not isinstance(doc, dict):
        raise ValueError('migration requires a mapping of collections')
    return doc


def _merge(base, document):
    result = copy.deepcopy(base)
    for key, value in document.items():
        if isinstance(value, dict) and isinstance(result.get(key), dict):
            if key == 'meta' and isinstance(value.get('prefixes'), dict) and isinstance(result[key].get('prefixes'), dict):
                value = dict(value, prefixes={**result[key]['prefixes'], **value['prefixes']})
            result[key].update(copy.deepcopy(value))
        else:
            result[key] = copy.deepcopy(value)
    return result


def _layer(base, document):
    record = P.Record(copy.deepcopy(base))
    return dict(P.layered(record, {'doc': copy.deepcopy(document)}))


def _world(document, template, *, hypotheses=None, context=None):
    data = template.to_data()
    return Snapshot.from_data(document, context=data['context'] if context is None else context,
        hypotheses=data['hypotheses'] if hypotheses is None else hypotheses, as_of=data['as_of'])


def _flow(value):
    return P.yaml.safe_dump(value, allow_unicode=True, default_flow_style=True, width=1000000).strip().removesuffix('\n...')


def _patch(data, before, after):
    """Patch only changed active values/keys; historical and unknown spans survive."""
    text = data.decode('utf-8')
    tree = P.yaml.compose(text)
    edits = []
    def visit(node, old, new):
        if old == new:
            return
        if isinstance(old, dict) and isinstance(new, dict) and hasattr(node, 'value') and isinstance(node.value, list):
            pairs = {key.value: (key, value) for key, value in node.value}
            # Implicit v -> rule is the only generated entry-key rename.
            removed, added = set(old) - set(new), set(new) - set(old)
            if removed == {'v'} and added == {'rule'}:
                key, value = pairs['v']
                edits.append((key.start_mark.index, key.end_mark.index, 'rule'))
                edits.append((value.start_mark.index, value.end_mark.index, _flow(new['rule'])))
                removed, added = set(), set()
            if not removed:
                if added:
                    extra = {key: new[key] for key in new if key in added}
                    if node.flow_style:
                        at = node.start_mark.index
                        if text[at:at + 1] != '{':
                            raise ValueError('migration cannot add metadata to an aliased mapping')
                        edits.append((at + 1, at + 1, _flow(extra)[1:-1] + (', ' if old else '')))
                    else:
                        first = node.value[0][0]
                        at = first.start_mark.index - first.start_mark.column
                        indent = ' ' * first.start_mark.column
                        added_text = ''.join(indent + line + '\n' for line in
                            P.yaml.safe_dump(extra, sort_keys=False, allow_unicode=True).splitlines())
                        edits.append((at, at, added_text))
                for key in old.keys() & new.keys():
                    visit(pairs[key][1], old[key], new[key])
                return
        end = node.end_mark.index
        while end > node.start_mark.index and text[end - 1] in '\r\n':
            end -= 1
        edits.append((node.start_mark.index, end, _flow(new)))
    if tree is None:
        return P.yaml.safe_dump(after, sort_keys=False, allow_unicode=True).encode()
    visit(tree, before, after)
    for start, end, value in sorted(edits, reverse=True):
        text = text[:start] + value + text[end:]
    result = text.encode('utf-8')
    if _parse(result) != after:
        raise ValueError('migration byte patch did not preserve the document')
    return result


def _pointers(document, source, mapping):
    result = copy.deepcopy(document)
    def rewrite(value):
        if isinstance(value, str) and value.endswith(('.yaml', '.yml')):
            target = os.path.abspath(os.path.join(os.path.dirname(source), value))
            if target not in mapping:
                raise ValueError('missing copied record pointer: ' + value)
            return os.path.relpath(mapping[target], os.path.dirname(mapping[source]) or '.').replace(os.sep, '/')
        if isinstance(value, list):
            return [rewrite(item) for item in value]
        if isinstance(value, dict):
            return {key: rewrite(item) for key, item in value.items()}
        return value
    for key in ('record', 'also'):
        if key in result:
            result[key] = rewrite(result[key])
    return result


def _conflicts(document, hypotheses, context):
    holders, meanings, active = {}, {}, set()
    def add(label, body, meaning_document=None):
        roles = G.semantic_roles(meaning_document or body)
        for nid, (collection, value) in G.entries(body).items():
            holders.setdefault(nid, []).append([label, copy.deepcopy(value)])
            meanings.setdefault(nid, set()).add(G.identity({'collection': collection, 'body': value,
                'reasoning': G.meaning_capabilities(meaning_document or body),
                'roles': {'judgment': nid in roles[0], 'fields': roles[1] if nid in roles[0] else {}} if roles else None}))
    add('checkout', document)
    for name, hyp in hypotheses.items():
        if hyp.get('error'):
            continue
        if hyp.get('kind') == 'contribution':
            active.update(G.entries(hyp['document']))
            add(name, hyp['document'])
        else:
            add('hypothesis:' + name, hyp['document'], _layer(document, hyp['document']))
    # Captured target documents are observations, never authoritative overlays.
    target = context.get('target', {})
    captured_target = target.get('snapshot', {})
    if captured_target:
        label = 'target:' + str(target.get('ref'))
        add(label, captured_target['doc'])
        for hyp in captured_target.get('hypotheses', []):
            add(label + ':hypothesis:' + hyp['name'], _layer(captured_target['doc'], hyp['doc']))
    # Older captures expose only already conflicting target witnesses.
    for nid, variants in context.get('conflicts', {}).items():
        for label, body in variants:
            if not captured_target and label.startswith('target:'):
                holders.setdefault(nid, []).append([label, copy.deepcopy(body)])
                meanings.setdefault(nid, set()).add(G.identity({'target': body}))
    return {nid: holders[nid] for nid in sorted(active) if len(meanings.get(nid, ())) > 1}


class Plan:
    def __init__(self, record, *, route=True, read_mode='live'):
        if record is None:
            record = P._peer('ingestion')._routed_record(None)[0]
        record = Path(record).expanduser().resolve()
        if route:
            record = V.write_paths([str(record)])[0]
        self.record = Path(record).expanduser().resolve()
        self.project = V.project_for([str(self.record)])
        self.source = capture_source([str(self.record)], read_mode=read_mode)
        self.record_files = [path for path in self.source.files if Path(path).parent != Path(P.layout(self.record)['hypotheses'])]
        _extend_inventory(self.source, self.record)
        _capture_evidence(self.source, self.record)
        self._admin_paths = _observe_admin(self.source, self.project)
        self.original = self.source.snapshot
        self.source_files = self.source.files
        self.mapping = {path: _portable(self.record, path) for path in self.source_files}
        for path in self.mapping.values():
            G._portable_files({path: b''})
            if path == ARTIFACTS or path.startswith(ARTIFACTS + '/'):
                raise ValueError('source conflicts with migration artifacts')
        if len(set(self.mapping.values())) != len(self.mapping):
            raise ValueError('migration path collision')
        self.files = {self.mapping[path]: data for path, data in self.source_files.items()}
        self.problems, self.reports = [], []
        self._prepare()

    def _prepare(self):
        source = self.original.to_data()
        for hyp in source['hypotheses'].values():
            if hyp.get('error'):
                raise ValueError('unreadable hypothesis: ' + str(hyp['error']))
        for (kind, path), value in self.source.inventory.events.items():
            if kind == 'unreadable':
                raise ValueError('unreadable migration source: ' + path)
        layout = P.layout(self.record)
        hypotheses = {path: Path(path).stem for path in self.source_files
            if Path(path).parent == Path(layout['hypotheses']) and path.endswith(('.yaml', '.yml'))}
        # The byte inventory comes from the actual reader, including its order.
        # Sidecars have reader-owned roles; only parsed record reads are semantic files.
        records = self.record_files
        if not records:
            sidecars = {layout[key] for key in ('view', 'measure', 'session')}
            records = [path for path in self.source_files if path not in hypotheses and path not in sidecars]
        self.record_files = records
        if str(self.record) not in records:
            raise ValueError('record routing changed or entry is not in captured closure')
        docs = {path: _parse(self.source_files[path]) for path in [*records, *hypotheses]}
        transformed = {}
        for path in records:
            physical = docs[path]
            # Shadowed physical bodies are checked under their actual surrounding schema.
            world = _merge(source['document'], physical)
            report = convert_snapshot(_world(world, self.original, hypotheses={}), reader=P)
            self._report(path, report)
            edited = copy.deepcopy(physical)
            for change in report['changes']:
                collection, nid = change['collection'], change['id']
                if nid in physical.get(collection, {}) and isinstance(physical[collection][nid], dict) and change['field'] in physical[collection][nid]:
                    body = edited[collection][nid]
                    if change['target'] != change['field']:
                        del body[change['field']]
                    body[change['target']] = copy.deepcopy(change['after'])
            edited.setdefault('meta', {})['reasoning'] = copy.deepcopy(report['document'].get('meta', {}).get('reasoning'))
            transformed[path] = edited
        document = {}
        for path in records:
            document = _merge(document, transformed[path])
        candidate_hyps = copy.deepcopy(source['hypotheses'])
        for path, name in hypotheses.items():
            physical = copy.deepcopy(docs[path])
            head = physical.pop('hypothesis', None)
            world = _layer(source['document'], physical)
            report = convert_snapshot(_world(world, self.original, hypotheses={}), reader=P)
            self._report(path, report)
            edited = copy.deepcopy(physical)
            for change in report['changes']:
                collection, nid = change['collection'], change['id']
                if nid in physical.get(collection, {}) and isinstance(physical[collection][nid], dict) and change['field'] in physical[collection][nid]:
                    body = edited[collection][nid]
                    if change['target'] != change['field']:
                        del body[change['field']]
                    body[change['target']] = copy.deepcopy(change['after'])
            edited.setdefault('meta', {})['reasoning'] = copy.deepcopy(report['document'].get('meta', {}).get('reasoning'))
            candidate_hyps[name]['document'] = edited
            before = _world(world, self.original, hypotheses={})
            after = _world(_layer(document, edited), self.original, hypotheses={})
            self._report(path, compare_snapshots(before, after, reader=P))
            transformed[path] = dict(edited, hypothesis=head) if head is not None else edited
        context = copy.deepcopy(source['context'])
        context['conflicts'] = _conflicts(document, candidate_hyps, context)
        context['migration'] = {'transformation': TRANSFORMATION, 'source_snapshot_id': self.original.snapshot_id,
            'frozen_candidate': True, 'publication_authority': False}
        pending_incompatibilities = []
        candidate_capabilities = G.meaning_capabilities(document)
        for revision, bundle in context.get('pending', {}).get('bundles', {}).items():
            manifest = bundle['manifest']
            try:
                compatible = G.meaning_capabilities(manifest['document']) == candidate_capabilities
            except CapabilityError as error:
                if error.code != 'unsupported_capability':
                    raise
                # Capture rejects active unsupported overlays. Retired archives
                # remain immutable evidence, without authorizing interpretation.
                compatible = False
            if not compatible:
                pending_incompatibilities.append(revision)
        context['migration']['incompatible_original_pending'] = sorted(pending_incompatibilities)
        self.candidate = _world(document, self.original, hypotheses=candidate_hyps, context=context)
        self._report(str(self.record), compare_snapshots(self.original, self.candidate, reader=P))
        for path, edited in transformed.items():
            self.files[self.mapping[path]] = _patch(self.source_files[path], docs[path], _pointers(edited, path, self.mapping))
        self._build_artifacts()
        self.source.verify()

    def _report(self, path, report):
        report = copy.deepcopy(report)
        report.pop('document', None)
        report['origin'] = _origin(self.record, path)
        self.reports.append(report)
        self.problems.extend(item for item in report['problems'] if item not in self.problems)

    def _build_artifacts(self):
        original_path, candidate_path = ARTIFACTS + '/original.json', ARTIFACTS + '/candidate.json'
        self.files[original_path] = self.original.to_json().encode()
        self.files[candidate_path] = self.candidate.to_json().encode()
        for path, data in self.source_files.items():
            self.files[ARTIFACTS + '/originals/' + self.mapping[path]] = data
        pending = self.original.to_data()['context'].get('pending', {})
        self.files[ARTIFACTS + '/pending.json'] = _json(pending)
        for revision, bundle in pending.get('bundles', {}).items():
            for path, value in bundle.get('files', {}).items():
                G._portable_files({path: b''})
                if not revision or any(c not in '0123456789abcdef' for c in revision):
                    raise ValueError('invalid pending revision')
                data = bytes.fromhex(value['data']) if isinstance(value, dict) and value.get('encoding') == 'hex' else value
                if not isinstance(data, bytes):
                    raise ValueError('invalid captured pending evidence')
                self.files[ARTIFACTS + '/pending/' + revision + '/' + path] = data
        self.manifest = {'version': 1, 'transformation': TRANSFORMATION, 'record': self.mapping[str(self.record)],
            'state': 'frozen_candidate', 'publication_authority': False,
            'source_snapshot_id': self.original.snapshot_id, 'candidate_snapshot_id': self.candidate.snapshot_id,
            'observation_sha256': _hash(_json({'context': self.original.to_data()['context'], 'config_exists': self.source.observation['config_exists']})),
            'config_sha256': _hash(_json(self.source.observation['config'])),
            'inventory_sha256': _hash(_json(sorted([[kind, _origin(self.record, path), [_origin(self.record, p) for p in value] if kind == 'glob' else value] for (kind, path), value in self.source.inventory.events.items() if path not in self._admin_paths]))),
            'source': [{'origin': _origin(self.record, path), 'path': self.mapping[path], 'sha256': _hash(data)} for path, data in sorted(self.source_files.items())],
            'destination': {path: _hash(data) for path, data in sorted(self.files.items())},
            'reports': G._encode(self.reports), 'problems': self.problems, 'complete': not self.problems}
        self.files[ARTIFACTS + '/receipt.json'] = G.json_bytes(self.manifest)

    def summary(self):
        return {'profile': 'core/v1', 'state': 'blocked' if self.problems else 'preview',
            'record': str(self.record), 'complete': not self.problems, 'problems': list(self.problems),
            'changes': json.loads(json.dumps([change for report in self.reports for change in report['changes']], default=lambda value: {'typed': G._encode(value)})),
            'fired': sorted({nid for report in self.reports for nid in report['fired']}),
            'reports': json.loads(json.dumps(self.reports, default=lambda value: {'typed': G._encode(value)})), 'manifest': copy.deepcopy(self.manifest)}

    def validate_destination(self, destination_directory, receipt_path=None):
        destination = Path(destination_directory).absolute()
        self.source.verify()
        if self.problems:
            raise ValueError('incomplete core conversion: ' + '; '.join(self.problems))
        receipt = Path(receipt_path) if receipt_path is not None else destination / ARTIFACTS / 'receipt.json'
        if receipt.read_bytes() != self.files[ARTIFACTS + '/receipt.json']:
            raise ValueError('migration receipt does not match recomputed source conversion')
        actual = {path.relative_to(destination).as_posix(): path for path in destination.rglob('*') if path.is_file() or path.is_symlink()}
        if set(actual) != set(self.files):
            raise ValueError('migration destination inventory changed')
        for name, data in self.files.items():
            path = actual[name]
            if path.is_symlink() or path.read_bytes() != data:
                raise ValueError('migration destination bytes changed: ' + name)
        for kind, expected in (('original', self.original), ('candidate', self.candidate)):
            replay = Snapshot.from_json((destination / ARTIFACTS / (kind + '.json')).read_bytes())
            if replay.snapshot_id != expected.snapshot_id:
                raise ValueError('migration snapshot changed')
        # Actual copied reader discovery must stay inside the mapped tree.
        _Inventory = P._peer('reasoning.snapshot')._Inventory
        inventory = _Inventory(retain_bytes=True)
        token, core = P._CAPTURE_READS.set(inventory), P._CORE_READS.set(True)
        try:
            copied = P.load([str(destination / self.mapping[str(self.record)])], read_mode='frozen')
        finally:
            P._CORE_READS.reset(core)
            P._CAPTURE_READS.reset(token)
        expected_reads = {str(destination / self.mapping[path]) for path in self.record_files}
        expected_reads.update(str(destination / self.mapping[path]) for path in self.source_files if Path(path).parent == Path(P.layout(self.record)['hypotheses']))
        if set(inventory.contents) != expected_reads:
            raise ValueError('copied reader closure differs from mapped inventory')
        # Pointers are storage locators; the candidate retains origin-context locators.
        body, expected = dict(copied), copy.deepcopy(self.candidate.to_data()['document'])
        for key in ('record', 'also'):
            body.pop(key, None)
            expected.pop(key, None)
        if G.identity(body) != G.identity(expected):
            raise ValueError('copied document differs from transformed candidate')
        for name, hypothesis in copied.hypotheses.items():
            expected_hypothesis = self.candidate.to_data()['hypotheses'].get(name)
            if expected_hypothesis is None or G.identity(hypothesis['head']) != G.identity(expected_hypothesis['head']):
                raise ValueError('copied hypothesis head differs from captured candidate')
            actual_doc, expected_doc = copy.deepcopy(hypothesis['doc']), copy.deepcopy(expected_hypothesis['document'])
            for key in ('record', 'also'):
                actual_doc.pop(key, None)
                expected_doc.pop(key, None)
            if G.identity(actual_doc) != G.identity(expected_doc):
                raise ValueError('copied hypothesis differs from transformed candidate')
        self.source.verify()
        return {'valid': True, 'record': str(destination / self.mapping[str(self.record)]), 'manifest': copy.deepcopy(self.manifest)}

    def publish(self, destination):
        if self.problems:
            raise ValueError('incomplete core conversion: ' + '; '.join(self.problems))
        self.source.verify()
        def populate(root):
            for name, data in self.files.items():
                G._portable_files({name: data})
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(data)
        result = V.publish_tree(destination, populate, self.validate_destination)
        return dict(self.summary(), state='materialized', record=str(result / self.mapping[str(self.record)]),
            receipt=str(result / ARTIFACTS / 'receipt.json'), read_mode='frozen')

    def apply(self):
        changed = [path for path, data in self.source_files.items() if self.files[self.mapping[path]] != data]
        if changed != [str(self.record)] or len(self.record_files) != 1:
            raise ValueError('requires_copy_migration: in-place conversion needs one changed canonical file')
        if self.problems:
            raise ValueError('incomplete core conversion: ' + '; '.join(self.problems))
        with P._locked(str(self.record), project=self.project):
            self.source.verify()
            pending_compatible(P, [str(self.record)], self.candidate.to_data()['document'])
            fd, backup = tempfile.mkstemp(prefix='.' + self.record.name + '.pre-core-', suffix='.bak', dir=self.record.parent)
            with os.fdopen(fd, 'wb') as handle:
                handle.write(self.source_files[str(self.record)])
            self.source.verify()
            P._write_text(str(self.record), self.files[self.mapping[str(self.record)]].decode())
        return dict(self.summary(), state='applied', applied=True, backup=backup)


def prepare(record=None, *, route=True, read_mode='live'):
    return Plan(record, route=route, read_mode=read_mode)
