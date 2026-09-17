"""Bounded local Git observations of committed history, without adoption or publication.

The commit association is an observation made by the local Git reader, not a
standalone Merkle inclusion proof. Replay verifies exact captured blob identities,
complete committed history and all declared local evidence; it never resolves a
ref, reads the source checkout, fetches a locator or grants a sharing scope.
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


def _required(captured, entry, files=None):
    """Only explicit record-relative paths; external citations confer no reads."""
    required = {}
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
                required.setdefault(_joined(entry, relative), None)
        except ValueError as error:
            raise C.HistoryError('external_branch_evidence_unavailable') from error
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
    C._mapping(manifest, ('version', 'kind', 'source', 'files', 'snapshot'))
    _require(type(manifest['version']) is int and manifest['version'] == 1 and manifest['kind'] == KIND,
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
    required = _required(captured, source['entry'], files)
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
    for path in _required(captured, entry):
        read(path)
    for path, item in listing(roles['hypotheses']).items():
        if PurePosixPath(path).parent.as_posix() == roles['hypotheses'] and PurePosixPath(path).suffix in ('.yaml', '.yml'):
            inventory[path] = item
            _require(len(inventory) <= MAX_FILES, 'branch_capture_limit')
            read(path)
    # Validate private/unsupported hypothesis declarations before reading their
    # evidence. Only the bounded explicit closure can request additional blobs.
    for path in _required(captured, entry, files):
        read(path)
    source = {'commit': oid, 'entry': entry, 'object_format': algorithm, 'association': 'local_git_capture'}
    observed, _ = _snapshot(captured, files, source, as_of)
    manifest = {'version': 1, 'kind': KIND, 'source': source,
                'files': {path: {**inventory[path], 'sha256': C.sha256(raw)} for path, raw in sorted(files.items())},
                'snapshot': observed.to_data()}
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
