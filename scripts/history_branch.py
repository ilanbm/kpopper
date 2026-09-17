"""Bounded local Git history observations and explicitly prepared target adoption.

The commit association is an observation made by the local Git reader, not a
standalone Merkle inclusion proof. Replay verifies exact captured blob identities,
complete committed history and explicit authored evidence. Version 2 names prior
branch capsules whose raw bytes are not transferred. Replay never resolves a ref,
reads the source checkout, fetches a locator or grants a sharing scope.
"""
import copy
import hashlib
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import signal
import threading
import time

from . import history_contract as C, history_bundle as B, history_adapter as A
from . import history_hypotheses as HH, history_hypothesis_import as HI
from . import provenance as P, pending_grounding as G, project_modes as M
from .reasoning.snapshot import Snapshot

KIND = 'committed-branch-history/v1'
SCOPED_KIND = 'committed-branch-history/v2'
MAX_BYTES = B.MAX_BYTES
MAX_FILES = 3 * C.MAX_OBJECTS + 1024
HEX = re.compile(r'(?:[0-9a-f]{40}|[0-9a-f]{64})\Z')


def _require(condition, code, detail=''):
    C._require(condition, code, detail)


GIT_TIMEOUT = 10


def _git(root, *arguments, maximum=C.MAX_REQUEST_BYTES, missing=False):
    """Bound execution and pipe draining, including a child that never closes stdout."""
    deadline = time.monotonic() + GIT_TIMEOUT
    environment = os.environ.copy()
    for key in ('GIT_DIR', 'GIT_WORK_TREE', 'GIT_COMMON_DIR', 'GIT_OBJECT_DIRECTORY', 'GIT_ALTERNATE_OBJECT_DIRECTORIES'):
        environment.pop(key, None)
    environment.update(GIT_NO_LAZY_FETCH='1', GIT_NO_REPLACE_OBJECTS='1', GIT_TERMINAL_PROMPT='0',
                       GIT_OPTIONAL_LOCKS='0')
    command = ['git', '--no-pager', '--no-replace-objects', '--no-lazy-fetch', '-C', str(root), *arguments]
    try:
        process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, env=environment, start_new_session=os.name != 'nt')
    except OSError as error:
        raise C.HistoryError('branch_git_unavailable') from error
    output, failures = bytearray(), []
    exceeded = threading.Event()
    def remaining():
        return max(0, deadline - time.monotonic())
    def terminate():
        try:
            if os.name == 'nt':
                process.kill()
            else:
                os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        except PermissionError:
            try:
                process.kill()
            except ProcessLookupError:
                pass
    def drain():
        try:
            while True:
                chunk = process.stdout.read(8192)
                if not chunk:
                    return
                if len(output) + len(chunk) > maximum:
                    exceeded.set()
                    terminate()
                    return
                output.extend(chunk)
        except OSError as error:
            failures.append(error)
        finally:
            process.stdout.close()
    reader = threading.Thread(target=drain, daemon=True)
    reader.start()
    try:
        process.wait(timeout=remaining())
        reader.join(timeout=remaining())
        if reader.is_alive():
            raise subprocess.TimeoutExpired(command, GIT_TIMEOUT)
    except subprocess.TimeoutExpired as error:
        terminate()
        raise C.HistoryError('branch_git_timeout') from error
    finally:
        if process.poll() is None:
            terminate()
        process.wait()
        reader.join(timeout=remaining())
    _require(not exceeded.is_set(), 'branch_capture_limit')
    if failures or process.returncode:
        if missing and not failures:
            return None
        raise C.HistoryError('branch_git_unavailable')
    return bytes(output)


def _path(value):
    return M.relative_path(value)


def _roles(entry):
    return {role: str(PurePosixPath(path).relative_to('/'))
            for role, path in P.layout('/' + entry).items()
            if role in ('history', 'history_commits', 'history_authority', 'history_cancellations', 'hypotheses')}


def _joined(entry, relative):
    _path(relative)
    return _path((PurePosixPath(entry).parent / relative).as_posix())


def _blob_identity(raw, algorithm):
    return hashlib.new(algorithm, b'blob ' + str(len(raw)).encode('ascii') + b'\0' + raw).hexdigest()


def _core(files, entry):
    roles = _roles(entry)
    _require(entry in files and roles['history_authority'] in files, 'branch_history_not_active')
    marker = C.validate_authority(C.decode_document(files[roles['history_authority']]))
    _require(marker['authority'] == 'history', 'branch_history_not_active')
    core = {'entry.yaml': files[entry], 'authority.yaml': files[roles['history_authority']]}
    paths = {entry, roles['history_authority']}
    for role, prefix in (('history_commits', 'commits'), ('history', 'objects'), ('history_cancellations', 'cancellations')):
        root = roles[role] + '/'
        for path, raw in files.items():
            if path.startswith(root):
                core[prefix + '/' + path[len(root):]] = raw
                paths.add(path)
    return B._capture(core, None), paths


def _required(captured, entry, files=None, *, include_prior_audit=True):
    """Only explicit record-relative paths; external citations confer no reads."""
    required, authored_files = {}, set()
    documents = [A.from_store_capture(captured).document]
    hypothesis_dir = _roles(entry)['hypotheses']
    documents.extend(C.decode_document(raw) for path, raw in (files or {}).items()
        if PurePosixPath(path).parent.as_posix() == hypothesis_dir and PurePosixPath(path).suffix in ('.yaml', '.yml'))
    documents.extend(obj for obj in captured.objects.values())
    documents.extend(C.decode_document(raw) for raw in captured.commits.values())
    for generation in captured.inactive_generations.values():
        documents.extend(generation['objects'].values())
        documents.extend(C.decode_document(raw) for raw in generation['commits'].values())
    for document in documents:
        G._privacy(document)
        try:
            for relative in G._files(document):
                path = _joined(entry, relative)
                authored_files.add(path)
                required.setdefault(path, None)
        except ValueError as error:
            raise C.HistoryError('external_branch_evidence_unavailable') from error
    # Commit-bound immutable report/view/branch audit evidence is part of the
    # source closure even though evidence-map keys are not authored `file` refs.
    generations = [captured.commits, *(item['commits'] for item in captured.inactive_generations.values())]
    for commits in generations:
        for raw in commits.values():
            manifest = C.validate_commit(C.decode_document(raw))
            evidence = manifest['receipt'].get('before', {}).get('authoring', {}).get('evidence', {})
            _require(isinstance(evidence, dict), 'invalid_branch_evidence_inventory')
            for relative, digest in evidence.items():
                C._text(digest, C.HEX)
                path = _joined(entry, relative)
                _require(required.get(path, digest) in (None, digest), 'branch_evidence_mismatch')
                required[path] = digest
    if not include_prior_audit:
        for item in _audit_coverage(captured, entry)['prior_branch_capsules']:
            _require(item['path'] not in authored_files, 'branch_recursive_audit_authored_evidence', item['path'])
            required.pop(item['path'], None)
    document = documents[0]
    imported = document.get('meta', {}).get('history_import')
    members = {}
    if imported is not None:
        C._mapping(imported, ('version', 'operation', 'recorded_at', 'members'))
        _require(type(imported['version']) is int and imported['version'] == 1 and
                 isinstance(imported['members'], list), 'invalid_history_import')
        for item in imported['members']:
            C._mapping(item, ('path', 'sha256', 'role'))
            _require(item['role'] in ('retained_original', 'replaced') and item['path'] not in members,
                     'invalid_history_import')
            C._text(item['sha256'], C.HEX)
            members[item['path']] = item
            required[_joined(entry, item['path'])] = item['sha256']
    import fnmatch
    for key in ('record', 'also'):
        value = document.get(key)
        values = [value] if isinstance(value, str) else value if isinstance(value, list) else \
            list(value.values()) if isinstance(value, dict) else []
        for pointer in values:
            if isinstance(pointer, str) and pointer.endswith(('.yaml', '.yml')):
                _path(pointer)
                _require(any(fnmatch.fnmatchcase(name, pointer) and item['role'] == 'retained_original'
                             for name, item in members.items()), 'unbound_branch_record_pointer')
    mapping = document.get('meta', {}).get(HI.MAPPING_KEY)
    if mapping is not None:
        mapping = HI.validate_mapping(mapping, entry=PurePosixPath(entry).name)
        for item in mapping['physical']:
            path = _joined(entry, item['path'])
            _require(required.get(path, item['sha256']) in (None, item['sha256']), 'branch_evidence_mismatch')
            required[path] = item['sha256']
    return required


def _audit_coverage(captured, entry):
    """Prior adoption capsules are hash observations, not recursive raw imports."""
    prior = {}
    generations = [captured.commits, *(item['commits'] for item in captured.inactive_generations.values())]
    for commits in generations:
        for raw in commits.values():
            manifest = C.validate_commit(C.decode_document(raw))
            adoption = manifest['receipt'].get('after', {}).get('history_branch_adoption', {})
            bindings = [adoption.get('source', {})] if adoption.get('version') == 1 else adoption.get('sources', [])
            for binding in bindings:
                audit = binding.get('audit')
                if audit is None:
                    continue
                C._mapping(audit, ('path', 'sha256', 'revision'))
                C._text(audit['revision'], C.HEX)
                C._text(audit['sha256'], C.HEX)
                expected = (Path(P.layout('/' + PurePosixPath(entry).name)['home']) / 'evidence' / 'branches' /
                            (audit['revision'] + '.json')).relative_to('/').as_posix()
                _require(audit['path'] == expected and manifest['receipt']['before'].get('authoring', {}).get(
                    'evidence', {}).get(expected) == audit['sha256'], 'branch_adoption_audit_mismatch')
                path = _joined(entry, expected)
                item = {'path': path, 'sha256': audit['sha256'], 'revision': audit['revision'],
                        'availability': 'not_transferred'}
                _require(path not in prior or prior[path] == item, 'branch_evidence_mismatch')
                prior[path] = item
    return {'history': 'current_and_inactive_generations',
            'prior_branch_capsules': [prior[path] for path in sorted(prior)]}


def _snapshot(captured, files, source, as_of):
    adapted = A.from_store_capture(captured)
    named, _ = HH.layers(adapted.projection, adapted.document)
    roles = _roles(source['entry'])
    physical, paths = {}, set()
    for path, raw in files.items():
        candidate = PurePosixPath(path)
        if candidate.parent.as_posix() != roles['hypotheses'] or candidate.suffix not in ('.yaml', '.yml'):
            continue
        name = C.hypothesis_name(candidate.stem)
        _require(name not in physical, 'ambiguous_branch_hypothesis', name)
        document = C.decode_document(raw)
        G._privacy(document)
        head = document.pop('hypothesis', {})
        _require(isinstance(head, dict), 'invalid_branch_hypothesis')
        G.document_capabilities(document)
        physical[name] = {'document': document, 'head': head, 'error': None}
        paths.add(path)
    mapping = adapted.document.get('meta', {}).get(HI.MAPPING_KEY)
    if mapping:
        for item in HI.validate_mapping(mapping, entry=PurePosixPath(source['entry']).name)['physical']:
            _require(item['name'] in physical and _joined(source['entry'], item['path']) in paths and
                     C.sha256(files[_joined(source['entry'], item['path'])]) == item['sha256'],
                     'missing_imported_hypothesis')
            physical.pop(item['name'])
    _require(not set(physical) & set(named), 'hypothesis_authority_collision')
    snapshot = adapted.snapshot(context={'read_mode': 'frozen', 'branch_source': source},
                                hypotheses={**physical, **named}, as_of=as_of)
    return snapshot, paths


def validate(envelope):
    """Return the detached history capture using only envelope bytes, with no I/O."""
    C._mapping(envelope, ('revision', 'manifest', 'files'))
    manifest = C.detached(envelope['manifest'], C.MAX_REQUEST_BYTES)
    C._mapping(manifest, ('version', 'kind', 'source', 'files', 'snapshot'), ('audit_coverage',))
    _require(type(manifest['version']) is int and
             ((manifest['version'] == 1 and manifest['kind'] == KIND and 'audit_coverage' not in manifest) or
              (manifest['version'] == 2 and manifest['kind'] == SCOPED_KIND and 'audit_coverage' in manifest)),
             'unsupported_branch_capture')
    _require(G.identity(manifest) == envelope['revision'], 'branch_capture_identity')
    _require(isinstance(manifest['snapshot'], dict) and 'as_of' in manifest['snapshot'], 'invalid_branch_snapshot')
    source = manifest['source']
    C._mapping(source, ('commit', 'entry', 'object_format', 'association'))
    _require(source['object_format'] in ('sha1', 'sha256') and source['association'] == 'local_git_capture',
             'invalid_branch_source')
    _path(source['entry'])
    _require(isinstance(source['commit'], str) and HEX.fullmatch(source['commit']) and
             len(source['commit']) == (40 if source['object_format'] == 'sha1' else 64), 'invalid_branch_source')
    files = envelope['files']
    _require(isinstance(files, dict) and len(files) <= MAX_FILES and set(files) == set(manifest['files']),
             'branch_capture_membership')
    G._portable_files(files)
    total = 0
    for path, raw in files.items():
        _path(path)
        _require(type(raw) is bytes and len(raw) <= C.MAX_REQUEST_BYTES, 'branch_capture_limit')
        total += len(raw)
        _require(total <= MAX_BYTES, 'branch_capture_limit')
        item = manifest['files'][path]
        C._mapping(item, ('sha256', 'git_oid', 'mode'))
        _require(item['mode'] in ('100644', '100755') and C.sha256(raw) == item['sha256'] and
                 _blob_identity(raw, source['object_format']) == item['git_oid'], 'branch_blob_mismatch')
    captured, core_paths = _core(files, source['entry'])
    if manifest['version'] == 2:
        _require(manifest['audit_coverage'] == _audit_coverage(captured, source['entry']) and
                 bool(manifest['audit_coverage']['prior_branch_capsules']), 'branch_audit_coverage_mismatch')
    required = _required(captured, source['entry'], files, include_prior_audit=manifest['version'] == 1)
    for path, digest in required.items():
        _require(path in files and (digest is None or C.sha256(files[path]) == digest),
                 'branch_evidence_unavailable', path)
    expected, hypothesis_paths = _snapshot(captured, files, source, manifest['snapshot']['as_of'])
    _require(set(files) == core_paths | set(required) | hypothesis_paths, 'branch_capture_membership')
    recorded = Snapshot.from_snapshot(manifest['snapshot'])
    _require(recorded.to_data() == expected.to_data(), 'branch_snapshot_mismatch')
    return captured


def snapshot(envelope):
    validate(envelope)
    return Snapshot.from_snapshot(envelope['manifest']['snapshot'])


def capture(repo, ref, *, entry='GROUNDING.yaml', as_of=None):
    """Capture one locally known commit once. No fetch, index, ref or ledger writes."""
    _require(isinstance(ref, str) and ref and '\0' not in ref, 'invalid_branch_ref')
    entry = _path(entry)
    root = Path(repo).expanduser().resolve()
    oid = _git(root, 'rev-parse', '--verify', '--end-of-options', ref + '^{commit}', maximum=128).decode().strip()
    _require(HEX.fullmatch(oid) is not None, 'invalid_branch_ref')
    algorithm = _git(root, 'rev-parse', '--show-object-format', maximum=128).decode().strip()
    _require(algorithm in ('sha1', 'sha256'), 'unsupported_git_object_format')
    inventory = {}
    def listing(path):
        raw = _git(root, '--literal-pathspecs', 'ls-tree', '-r', '-z', oid, '--', path)
        found = {}
        for row in raw.split(b'\0'):
            if not row:
                continue
            info, name = row.split(b'\t', 1)
            mode, kind, blob = info.decode('ascii').split()
            try:
                name = _path(name.decode('utf-8'))
            except (UnicodeError, ValueError) as error:
                raise C.HistoryError('invalid_branch_path') from error
            _require(name not in found, 'ambiguous_branch_path')
            _require(mode in ('100644', '100755') and kind == 'blob', 'unsupported_branch_file')
            found[name] = {'mode': mode, 'git_oid': blob}
            _require(len(found) <= MAX_FILES, 'branch_capture_limit')
        return found
    selected = listing(entry)
    if entry not in selected and PurePosixPath(entry).name in P.ENTRY_NAMES:
        for name in P.ENTRY_NAMES:
            alternative = str(PurePosixPath(entry).parent / name)
            candidate = listing(alternative)
            if alternative in candidate:
                entry, selected = alternative, candidate
                break
    _require(entry in selected, 'branch_record_unavailable')
    roles = _roles(entry)
    inventory.update(selected)
    for role in ('history_authority', 'history', 'history_commits', 'history_cancellations'):
        inventory.update(listing(roles[role]))
        _require(len(inventory) <= MAX_FILES, 'branch_capture_limit')
    _require(roles['history_authority'] in inventory, 'branch_history_not_active')
    files, total = {}, 0
    def read(path):
        nonlocal total
        if path in files:
            return
        if path not in inventory:
            inventory.update(listing(path))
            _require(len(inventory) <= MAX_FILES, 'branch_capture_limit')
        _require(path in inventory, 'branch_evidence_unavailable', path)
        _require(len(files) < MAX_FILES, 'branch_capture_limit')
        blob = inventory[path]['git_oid']
        _require(HEX.fullmatch(blob) is not None, 'invalid_branch_blob')
        size = int(_git(root, 'cat-file', '-s', blob, maximum=64))
        _require(0 <= size <= C.MAX_REQUEST_BYTES and total + size <= MAX_BYTES, 'branch_capture_limit')
        raw = _git(root, 'cat-file', 'blob', blob, maximum=size)
        _require(len(raw) == size and _blob_identity(raw, algorithm) == blob, 'branch_blob_mismatch')
        files[path] = raw
        total += size
    for path in sorted(inventory):
        read(path)
    # Store capture deliberately ignores orphan staging files. Do not transport
    # them or disclose their unrelated bodies as authoritative branch evidence.
    commit_prefix, object_prefix = roles['history_commits'] + '/', roles['history'] + '/'
    commits = {path[len(commit_prefix):-5]: raw for path, raw in files.items() if path.startswith(commit_prefix)}
    storage = {path[len(object_prefix):]: raw for path, raw in files.items() if path.startswith(object_prefix)}
    _, object_paths = C.objects_from_storage(commits, storage)
    held = set(object_paths.values())
    files = {path: raw for path, raw in files.items()
             if not path.startswith(object_prefix) or path[len(object_prefix):] in held}
    captured, _ = _core(files, entry)
    for path in _required(captured, entry, include_prior_audit=False):
        read(path)
    for path, item in listing(roles['hypotheses']).items():
        if PurePosixPath(path).parent.as_posix() == roles['hypotheses'] and PurePosixPath(path).suffix in ('.yaml', '.yml'):
            inventory[path] = item
            _require(len(inventory) <= MAX_FILES, 'branch_capture_limit')
            read(path)
    # Validate private/unsupported hypothesis declarations before reading their
    # evidence. Only the bounded explicit closure can request additional blobs.
    for path in _required(captured, entry, files, include_prior_audit=False):
        read(path)
    source = {'commit': oid, 'entry': entry, 'object_format': algorithm, 'association': 'local_git_capture'}
    observed, _ = _snapshot(captured, files, source, as_of)
    manifest = {'version': 1, 'kind': KIND, 'source': source,
                'files': {path: {**inventory[path], 'sha256': C.sha256(raw)} for path, raw in sorted(files.items())},
                'snapshot': observed.to_data()}
    coverage = _audit_coverage(captured, entry)
    if coverage['prior_branch_capsules']:
        manifest.update(version=2, kind=SCOPED_KIND, audit_coverage=coverage)
    envelope = {'revision': G.identity(manifest), 'manifest': manifest, 'files': files}
    validate(envelope)
    return envelope


MAX_WIRE_BYTES = 2 * MAX_BYTES + C.MAX_REQUEST_BYTES


def to_bytes(envelope):
    """Canonical typed transport; bytes and authored date/scalar types survive."""
    from . import history_transaction as T
    validate(envelope)
    value = {**envelope, 'files': {path: T._blob(raw) for path, raw in envelope['files'].items()}}
    raw = G.json_bytes(G._encode(value))
    _require(len(raw) <= MAX_WIRE_BYTES, 'branch_capture_limit')
    return raw


def from_bytes(raw):
    from . import history_transaction as T
    from .reasoning.snapshot import _json_object, _json_constant, _check_typed_json
    import json
    _require(type(raw) is bytes and len(raw) <= MAX_WIRE_BYTES, 'branch_capture_limit')
    try:
        encoded = json.loads(raw.decode('utf-8'), object_pairs_hook=_json_object, parse_constant=_json_constant)
        _check_typed_json(encoded)
        value = G._decode(encoded)
        _require(G._encode(value) == encoded and isinstance(value, dict) and isinstance(value.get('files'), dict),
                 'invalid_branch_capture')
        total = 0
        for blob in value['files'].values():
            _require(isinstance(blob, dict) and isinstance(blob.get('data'), str) and
                     len(blob['data']) <= 4 * ((C.MAX_REQUEST_BYTES + 2) // 3), 'branch_capture_limit')
            total += (len(blob['data']) // 4) * 3
            _require(total <= MAX_BYTES + 2 * len(value['files']), 'branch_capture_limit')
        value['files'] = {path: T._unblob(blob) for path, blob in value['files'].items()}
        validate(value)
        return value
    except (ValueError, TypeError, KeyError, IndexError, UnicodeError, RecursionError) as error:
        if isinstance(error, C.HistoryError):
            raise
        raise C.HistoryError('invalid_branch_capture') from error


def _binding(envelope, source):
    result = {'kind': envelope['manifest']['kind'], 'revision': envelope['revision'],
            'git': copy.deepcopy(envelope['manifest']['source']),
            'authority': copy.deepcopy(source.marker), 'baseline': copy.deepcopy(source.baseline),
            'closure_digest': A.from_store_capture(source).projection['closure_digest'],
            'snapshot_id': envelope['manifest']['snapshot']['snapshot_id'],
            'files': copy.deepcopy(envelope['manifest']['files'])}
    if 'audit_coverage' in envelope['manifest']:
        result['audit_coverage'] = copy.deepcopy(envelope['manifest']['audit_coverage'])
    return result


def _physical(envelope, source):
    immutable, _ = HH.layers(A.from_store_capture(source).projection, A.from_store_capture(source).document)
    entry = envelope['manifest']['source']['entry']
    directory = _roles(entry)['hypotheses']
    items = []
    for name, hypothesis in envelope['manifest']['snapshot']['hypotheses'].items():
        if name in immutable:
            continue
        paths = [path for path in envelope['files'] if PurePosixPath(path).parent.as_posix() == directory and
                 PurePosixPath(path).suffix in ('.yaml', '.yml') and PurePosixPath(path).stem == name]
        _require(len(paths) == 1, 'ambiguous_branch_hypothesis', name)
        path = paths[0]
        items.append({'name': name, 'document': hypothesis['document'], 'head': hypothesis['head'],
                      'path': PurePosixPath(path).relative_to(PurePosixPath(entry).parent).as_posix(),
                      'bytes': envelope['files'][path], 'error': hypothesis.get('error')})
    return items


def _evidence_requirements(envelope, source):
    entry = envelope['manifest']['source']['entry']
    documents = list(source.objects.values())
    documents.extend(item['document'] for item in envelope['manifest']['snapshot']['hypotheses'].values())
    result = {}
    for document in documents:
        for relative in G._files(document):
            path = _joined(entry, relative)
            _require(path in envelope['files'], 'branch_evidence_unavailable', path)
            result[relative] = {'source_commit': envelope['manifest']['source']['commit'],
                                'source_path': path, 'target_relative': relative,
                                'sha256': C.sha256(envelope['files'][path])}
    return result


def _evidence_status(entry, required, target_evidence=None):
    from . import history_transaction as T
    if target_evidence is not None:
        _require(isinstance(target_evidence, dict) and all(type(raw) is bytes for raw in target_evidence.values()),
                 'invalid_target_evidence')
    result, total = [], 0
    for relative, expected in sorted(required.items()):
        raw = None
        status = 'unassessed'
        if target_evidence is not None:
            raw = target_evidence.get(relative)
            status = 'missing'
        elif entry is not None:
            path = T._target(Path(entry).resolve().parent, relative)
            status = 'missing'
            if path.exists():
                _require(path.is_file() and path.stat().st_size <= C.MAX_REQUEST_BYTES, 'branch_target_evidence_limit')
                raw = path.read_bytes()
        if raw is not None:
            total += len(raw)
            _require(len(raw) <= C.MAX_REQUEST_BYTES and total <= MAX_BYTES, 'branch_target_evidence_limit')
            status = 'matching' if C.sha256(raw) == expected['sha256'] else 'conflicting'
        result.append({**expected, 'status': status})
    return result


def _require_target_evidence(entry, required, target_evidence=None):
    rows = _evidence_status(entry, required, target_evidence)
    for row in rows:
        _require(row['status'] == 'matching', 'branch_target_evidence_' + row['status'], row['target_relative'])
    return rows


def preview_adoption(target, envelope, *, entry=None, target_evidence=None):
    """Explicit immutable choices, named proposals, and target evidence availability."""
    source = validate(envelope)
    result = B._preview_capture_adoption(target, source, envelope['revision'])
    result['source_revision'] = result.pop('artifact_revision')
    result['source'] = copy.deepcopy(envelope['manifest']['source'])
    projection = A.from_store_capture(source).projection
    named, _ = HH.layers(projection, A.from_store_capture(source).document)
    physical = _physical(envelope, source)
    result['named_proposals'] = {name: {'kind': 'immutable', 'head': item['head'],
        'subjects': sorted(G.entries(item['doc']))} for name, item in named.items()}
    for item in physical:
        result['named_proposals'][item['name']] = {'kind': 'physical_observation', 'head': item['head'],
            'subjects': sorted(G.entries(item['document'])), 'admission': 'proposed_only'}
        for subject in G.entries(item['document']):
            if subject not in result['subjects']:
                existing = target.state['subjects'].get(subject, {})
                result['subjects'][subject] = {'requires_choice': subject in target.state['subjects'],
                    'target_heads': existing.get('heads', []), 'incoming_heads': [],
                    'combined_heads': existing.get('heads', []),
                    'claims': sorted(version for version, obj in target.objects.items()
                                     if obj['subject'] == subject and obj['kind'] != 'act')}
    for subject, item in result['subjects'].items():
        item['source_acceptance'] = source.state['subjects'].get(subject, {}).get('acceptance', 'proposed')
        item['source_proposals'] = source.state['subjects'].get(subject, {}).get('proposals', [])
    result['evidence'] = _evidence_status(entry, _evidence_requirements(envelope, source), target_evidence)
    return result


def _prepare_adoption(entry, envelope, *, choices, by, operation, recorded_at, capture, target_evidence=None):
    from . import history_store as H
    source = validate(envelope)
    target = capture or H.Store(entry).capture()
    _require(target.entry_bytes == H.Store(entry).render(target), 'unresolved_view_edit')
    required = _evidence_requirements(envelope, source)
    _require_target_evidence(entry, required, target_evidence)
    preview = preview_adoption(target, envelope, entry=entry, target_evidence=target_evidence)
    _require(isinstance(choices, dict), 'invalid_adoption_choices')
    for subject, chosen in choices.items():
        _require(subject in preview['subjects'] and chosen in preview['subjects'][subject]['claims'],
                 'invalid_adoption_choice')
    physical = _physical(envelope, source)
    observed = (HI.prepare(source.objects, physical, base_document=A.from_store_capture(source).document,
                           entry=PurePosixPath(envelope['manifest']['source']['entry']).name,
                           operation=operation, recorded_at=recorded_at) if physical else
                {'objects': {}, 'physical': {'version': 1, 'physical': []}})
    binding = _binding(envelope, source)
    audit_raw = to_bytes(envelope)
    audit_path = (Path(P.layout('/' + Path(entry).name)['home']) / 'evidence' / 'branches' /
                  (envelope['revision'] + '.json')).relative_to('/').as_posix()
    binding['audit'] = {'path': audit_path, 'sha256': C.sha256(audit_raw), 'revision': envelope['revision']}
    binding['required_evidence'] = required
    binding['physical_observation'] = {'operation': operation, 'recorded_at': recorded_at,
        'original_writer': None, 'original_operation': None, 'original_recorded_at': None,
        'files': observed['physical']}
    return B._prepare_capture_adoption(entry, source, envelope['revision'], choices=choices, by=by,
        operation=operation, recorded_at=recorded_at, capture=target, source_binding=binding,
        observations=observed['objects'], audit_files={audit_path: audit_raw})


def audit_evidence(mutation):
    """Validate and return the exact audit envelope using only retained mutation bytes.

    Constructor callers may supply their decoded data dictionary; this helper
    never reconstructs PreparedMutation and therefore cannot recurse through its
    validation. Encoded blob fields and raw file bytes are both accepted.
    """
    from . import history_transaction as T
    data = mutation if isinstance(mutation, dict) else mutation.to_data()
    adoption = data.get('receipt', {}).get('after', {}).get('history_branch_adoption', {})
    _require(adoption.get('version') == 1, 'invalid_branch_adoption_audit')
    binding = adoption.get('source', {})
    audit = binding.get('audit', {})
    C._mapping(audit, ('path', 'sha256', 'revision'))
    revision = adoption.get('source_revision')
    C._text(revision, C.HEX)
    expected_path = (Path(P.layout('/' + data['entry'])['home']) / 'evidence' / 'branches' /
                     (revision + '.json')).relative_to('/').as_posix()
    _require(audit['path'] == expected_path and audit['revision'] == revision, 'branch_adoption_audit_mismatch')
    files = [item for item in data['files'] if item['role'] == 'history_evidence']
    _require(len(files) == 1 and files[0]['path'] == expected_path, 'branch_adoption_audit_mismatch')
    item = files[0]
    raw = item['after'] if type(item['after']) is bytes else T._unblob(item['after'])
    before = item['before'] if item['before'] is None or type(item['before']) is bytes else T._unblob(item['before'])
    _require(before is None and type(raw) is bytes and C.sha256(raw) == audit['sha256'],
             'branch_adoption_audit_mismatch')
    _require(data['receipt']['before'].get('authoring', {}).get('evidence') == {expected_path: audit['sha256']},
             'branch_adoption_audit_mismatch')
    envelope = from_bytes(raw)
    source = validate(envelope)
    _require(envelope['revision'] == revision and
             all(G.identity(binding.get(key)) == G.identity(value)
                 for key, value in _binding(envelope, source).items()) and
             G.identity(binding.get('required_evidence')) == G.identity(_evidence_requirements(envelope, source)),
             'branch_adoption_audit_mismatch')
    return envelope


def prepare_adoption(entry, envelope, *, choices, by, operation, recorded_at, capture=None):
    """Prepare one local branch adoption; never install evidence or enter a ledger.

    The mutation retains the complete source envelope as immutable audit evidence.
    The target commit binds that envelope and imports current-generation objects;
    source inactive generations and original physical files remain audit only.
    """
    return _prepare_adoption(entry, envelope, choices=choices, by=by, operation=operation,
                             recorded_at=recorded_at, capture=capture)


@B.HP.replay_mutation
def verify_adoption(entry, mutation, envelope, *, capture=None, target_evidence=None):
    """Recompute exact retained bytes, optionally with explicit offline target evidence.

    Offline bytes are proof inputs only; commit_adoption always reads and rechecks
    actual target evidence. No source Git repository or locator is read here.
    """
    from . import history_transaction as T, history_store as H
    mutation = T.PreparedMutation.from_bytes(mutation.to_bytes())
    live = capture or H.Store(entry).capture()
    before = B._adoption_before(live, mutation)
    data = mutation.to_data()
    adoption = data['receipt']['after'].get('history_branch_adoption', {})
    _require(adoption.get('source_revision') == envelope['revision'], 'branch_adoption_source_mismatch')
    expected = _prepare_adoption(entry, envelope, choices=adoption.get('choices'), by=adoption.get('by'),
        operation=data['operation'], recorded_at=adoption.get('recorded_at'), capture=before,
        target_evidence=target_evidence)
    _require(expected.to_bytes() == mutation.to_bytes(), 'branch_adoption_mutation_mismatch')
    return mutation


def commit_adoption(entry, mutation, envelope, *, verify):
    """Commit under the caller's routing guard and recheck real evidence after it."""
    from . import history_store as H
    _require(callable(verify), 'missing_verifier')
    source = validate(envelope)
    required = _evidence_requirements(envelope, source)
    def checked(data):
        verify_adoption(entry, mutation, envelope)
        verify(data)
        _require_target_evidence(entry, required)
    return H.Store(entry).commit(mutation, verify=checked)


def _source_set(envelopes):
    _require(isinstance(envelopes, (list, tuple)) and 2 <= len(envelopes) <= 16, 'invalid_branch_source_set')
    total = 0
    for envelope in envelopes:
        _require(isinstance(envelope, dict) and isinstance(envelope.get('files'), dict) and
                 len(envelope['files']) <= MAX_FILES, 'invalid_branch_source_set')
        for raw in envelope['files'].values():
            _require(type(raw) is bytes, 'invalid_branch_source_set')
            total += len(raw)
            _require(total <= MAX_BYTES, 'branch_capture_limit')
    validated = [(item, validate(item)) for item in envelopes]
    validated.sort(key=lambda item: item[0]['revision'])
    ordered, captures = [item[0] for item in validated], [item[1] for item in validated]
    _require(len({item['revision'] for item in ordered}) == len(ordered), 'duplicate_branch_source')
    revisions = [item['revision'] for item in ordered]
    revision = G.identity({'kind': 'committed-branch-history-set/v1', 'revisions': revisions})
    return ordered, captures, revision


def preview_adoption_set(target, envelopes, *, entry=None, target_evidence=None):
    """Preview every source and one global choice set, without synthetic authority."""
    ordered, captures, revision = _source_set(envelopes)
    result = B._preview_capture_adoption(target, captures[0], revision, additional_sources=captures[1:])
    result['source_set_revision'] = result.pop('artifact_revision')
    result['source_revisions'] = [item['revision'] for item in ordered]
    result['sources'] = [{'revision': item['revision'], **copy.deepcopy(item['manifest']['source'])} for item in ordered]
    result['named_proposals'], result['evidence'], counts = {}, [], {}
    for envelope in ordered:
        part = preview_adoption(target, envelope, entry=entry, target_evidence=target_evidence)
        for subject, item in part['subjects'].items():
            counts[subject] = counts.get(subject, 0) + 1
            result['subjects'].setdefault(subject, copy.deepcopy(item))
            result['subjects'][subject].setdefault('source_dispositions', []).append({
                'revision': envelope['revision'], 'acceptance': item['source_acceptance'],
                'proposals': item['source_proposals']})
        for name, item in part['named_proposals'].items():
            result['named_proposals'].setdefault(name, []).append({'revision': envelope['revision'], **item})
        result['evidence'].extend({'source_revision': envelope['revision'], **item} for item in part['evidence'])
    for subject, count in counts.items():
        if count > 1:
            result['subjects'][subject]['requires_choice'] = True
    hashes = {}
    for item in result['evidence']:
        hashes.setdefault(item['target_relative'], set()).add(item['sha256'])
    result['evidence_conflicts'] = sorted(path for path, values in hashes.items() if len(values) > 1)
    return result


def _prepare_adoption_set(entry, envelopes, *, choices, by, operation, recorded_at, capture,
                          target_evidence=None):
    from . import history_store as H
    ordered, sources, revision = _source_set(envelopes)
    target = capture or H.Store(entry).capture()
    _require(target.entry_bytes == H.Store(entry).render(target), 'unresolved_view_edit')
    preview = preview_adoption_set(target, ordered, entry=entry, target_evidence=target_evidence)
    _require(not preview['evidence_conflicts'], 'branch_evidence_sources_conflict', ', '.join(preview['evidence_conflicts']))
    _require(isinstance(choices, dict), 'invalid_adoption_choices')
    for subject, chosen in choices.items():
        _require(subject in preview['subjects'] and chosen in preview['subjects'][subject]['claims'],
                 'invalid_adoption_choice')
    bindings, observations, audit_files = [], {}, {}
    for envelope, source in zip(ordered, sources):
        required = _evidence_requirements(envelope, source)
        _require_target_evidence(entry, required, target_evidence)
        physical = _physical(envelope, source)
        observed = (HI.prepare(source.objects, physical, base_document=A.from_store_capture(source).document,
                    entry=PurePosixPath(envelope['manifest']['source']['entry']).name,
                    operation=operation, recorded_at=recorded_at) if physical else
                    {'objects': {}, 'physical': {'version': 1, 'physical': []}})
        for version, obj in observed['objects'].items():
            _require(version not in observations or observations[version] == obj, 'identity_mismatch')
            observations[version] = obj
        raw = to_bytes(envelope)
        path = (Path(P.layout('/' + Path(entry).name)['home']) / 'evidence' / 'branches' /
                (envelope['revision'] + '.json')).relative_to('/').as_posix()
        audit_files[path] = raw
        _require(sum(len(item) for item in audit_files.values()) <= MAX_BYTES, 'branch_capture_limit')
        binding = _binding(envelope, source)
        binding.update(audit={'path': path, 'sha256': C.sha256(raw), 'revision': envelope['revision']},
            required_evidence=required, physical_observation={'operation': operation, 'recorded_at': recorded_at,
                'original_writer': None, 'original_operation': None, 'original_recorded_at': None,
                'files': observed['physical']})
        bindings.append(binding)
    return B._prepare_capture_adoption(entry, sources[0], revision, choices=choices, by=by,
        operation=operation, recorded_at=recorded_at, capture=target,
        source_binding={'version': 2, 'sources': bindings, 'source_revisions': preview['source_revisions']},
        observations=observations, audit_files=audit_files, additional_sources=sources[1:],
        required_choices={subject for subject, item in preview['subjects'].items() if item['requires_choice']})


def prepare_adoption_set(entry, envelopes, *, choices, by, operation, recorded_at, capture=None):
    """Prepare one commit for all selected branch sources, never a commit loop."""
    return _prepare_adoption_set(entry, envelopes, choices=choices, by=by, operation=operation,
                                 recorded_at=recorded_at, capture=capture)


def audit_evidences(mutation):
    """Return all exact audit envelopes, accepting existing single-source receipts."""
    data = mutation if isinstance(mutation, dict) else mutation.to_data()
    adoption = data.get('receipt', {}).get('after', {}).get('history_branch_adoption', {})
    if adoption.get('version') == 1:
        return [audit_evidence(data)]
    _require(adoption.get('version') == 2, 'invalid_branch_adoption_audit')
    bindings, revisions = adoption.get('sources'), adoption.get('source_revisions')
    _require(isinstance(bindings, list) and isinstance(revisions, list) and 2 <= len(bindings) <= 16 and
             all(isinstance(item, str) and C.HEX.fullmatch(item) for item in revisions) and
             all(isinstance(item, dict) for item in bindings) and
             revisions == sorted(set(revisions)) and len(bindings) == len(revisions), 'invalid_branch_source_set')
    evidence = [item for item in data['files'] if item['role'] == 'history_evidence']
    capsule_total = 0
    for item in evidence:
        blob = item.get('after')
        if type(blob) is bytes:
            capsule_total += len(blob)
        else:
            _require(isinstance(blob, dict) and isinstance(blob.get('data'), str), 'invalid_branch_adoption_audit')
            capsule_total += len(blob['data']) * 3 // 4
        _require(capsule_total <= MAX_BYTES, 'branch_capture_limit')
    expected = {item.get('audit', {}).get('path'): item.get('audit', {}).get('sha256') for item in bindings}
    _require(len(evidence) == len(bindings) == len(expected) and
             data['receipt']['before'].get('authoring', {}).get('evidence') == expected,
             'branch_adoption_audit_mismatch')
    envelopes = []
    for revision, binding in zip(revisions, bindings):
        files = [item for item in evidence if item['path'] == binding.get('audit', {}).get('path')]
        fragment = {'entry': data['entry'], 'files': files, 'receipt': {
            'before': {'authoring': {'evidence': {binding.get('audit', {}).get('path'): binding.get('audit', {}).get('sha256')}}},
            'after': {'history_branch_adoption': {'version': 1, 'source_revision': revision, 'source': binding}}}}
        envelopes.append(audit_evidence(fragment))
    ordered, _, revision = _source_set(envelopes)
    _require(adoption.get('source_set_revision') == revision and envelopes == ordered,
             'branch_adoption_audit_mismatch')
    return envelopes


@B.HP.replay_mutation
def verify_adoption_set(entry, mutation, envelopes, *, capture=None, target_evidence=None):
    """Replay an atomic retained source set using only its exact envelopes."""
    from . import history_transaction as T, history_store as H
    mutation = T.PreparedMutation.from_bytes(mutation.to_bytes())
    live = capture or H.Store(entry).capture()
    before = B._adoption_before(live, mutation)
    data = mutation.to_data()
    adoption = data['receipt']['after'].get('history_branch_adoption', {})
    _require(adoption.get('version') == 2, 'invalid_branch_source_set')
    expected = _prepare_adoption_set(entry, envelopes, choices=adoption.get('choices'), by=adoption.get('by'),
        operation=data['operation'], recorded_at=adoption.get('recorded_at'), capture=before,
        target_evidence=target_evidence)
    _require(expected.to_bytes() == mutation.to_bytes(), 'branch_adoption_mutation_mismatch')
    return mutation


def commit_adoption_set(entry, mutation, envelopes, *, verify):
    """Publish one target commit; verify every source's actual target evidence."""
    from . import history_store as H
    _require(callable(verify), 'missing_verifier')
    ordered, captures, _ = _source_set(envelopes)
    def checked(data):
        verify_adoption_set(entry, mutation, ordered)
        verify(data)
        for envelope, source in zip(ordered, captures):
            _require_target_evidence(entry, _evidence_requirements(envelope, source))
    return H.Store(entry).commit(mutation, verify=checked)
