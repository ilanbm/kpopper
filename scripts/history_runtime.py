"""Read-only deployment declarations and bounded probes of managed launchers.

The source manifest identifies resolved files, not executing bytecode. A nonce
correlates a fresh invocation; it is not cryptographic attestation. Callers own
launcher inventory, expected digests and immutable deployment during transitions.
"""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import secrets
import stat
import sys
import zipfile

from .reasoning import runtime as R

ENDPOINT = 'history/capabilities'
INCLUSION = ['*.py', 'reasoning/*.py', 'session/*.py']
REQUIRED = sorted(['__init__.py', 'cli.py', 'history_cli.py', 'history_runtime.py',
    'history_contract.py', 'history_store.py', 'history_adapter.py', 'history_transaction.py', 'history_paths.py',
    'history_authoring.py', 'history_identity.py', 'history_edits.py', 'history_direct.py', 'history_bundle.py', 'history_migration.py', 'history_activation.py',
    'history_group_activation.py', 'history_hypotheses.py', 'history_hypothesis_import.py', 'history_branch.py',
    'provenance.py', 'pending_grounding.py', 'knowledge_views.py', 'reasoning/__init__.py',
    'reasoning/contract.py', 'reasoning/snapshot.py', 'reasoning/evaluate.py',
    'reasoning/runtime.py', 'session/__init__.py'])
MAX_SOURCE_BYTES = 32 * 1024 * 1024
MAX_OUTPUT_BYTES = 2 * 1024 * 1024
MAX_ARCHIVE_BYTES = 32 * 1024 * 1024
MAX_EXPANDED_BYTES = 256 * 1024 * 1024
HEX = re.compile(r'^[0-9a-f]{64}$')
NONCE = re.compile(r'^[A-Za-z0-9_-]{16,128}$')


class RuntimeDeclarationError(ValueError):
    def __init__(self, code, detail=''):
        self.code = code
        super().__init__(code + (': ' + detail if detail else ''))


def _require(value, code, detail=''):
    if not value:
        raise RuntimeDeclarationError(code, detail)


def _json(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=False,
                      allow_nan=False).encode('utf-8')


def _digest(value):
    return hashlib.sha256(_json(value)).hexdigest()


def _seal(value):
    return {**value, 'digest': _digest(value)}


def _strict_json(raw):
    def pairs(items):
        result = {}
        for key, value in items:
            _require(key not in result, 'invalid_runtime_json', 'duplicate key')
            result[key] = value
        return result
    def constant(value):
        raise RuntimeDeclarationError('invalid_runtime_json', 'nonfinite number')
    try:
        return json.loads(raw, object_pairs_hook=pairs, parse_constant=constant)
    except (ValueError, UnicodeError, RecursionError) as error:
        if isinstance(error, RuntimeDeclarationError):
            raise
        raise RuntimeDeclarationError('invalid_runtime_json') from error


def _path(root, relative):
    candidate = root / relative
    _require(not Path(relative).is_absolute() and '..' not in Path(relative).parts,
             'invalid_runtime_path')
    current = root
    for part in Path(relative).parts:
        current = current / part
        _require(not current.is_symlink(), 'runtime_symlink', relative)
    _require(candidate.resolve().is_relative_to(root), 'mixed_runtime_roots', relative)
    _require(candidate.is_file(), 'missing_runtime_source', relative)
    return candidate


def _read(root, relative, maximum=MAX_SOURCE_BYTES):
    path = _path(root, relative)
    _require(path.stat().st_size <= maximum, 'runtime_size_limit', relative)
    with path.open('rb') as stream:
        raw = stream.read(maximum + 1)
    _require(len(raw) <= maximum, 'runtime_size_limit', relative)
    return raw


def _source_manifest(root):
    paths = sorted({path.relative_to(root).as_posix() for pattern in INCLUSION for path in root.glob(pattern)})
    _require(set(REQUIRED) <= set(paths), 'missing_runtime_source')
    _require(len(paths) <= 1024, 'runtime_size_limit')
    files, total = [], 0
    for relative in paths:
        raw = _read(root, relative)
        total += len(raw)
        _require(total <= MAX_SOURCE_BYTES, 'runtime_size_limit')
        files.append({'path': relative, 'sha256': hashlib.sha256(raw).hexdigest()})
    return _seal({'version': 1, 'scheme': 'product-python/v1', 'inclusion': INCLUSION,
                  'required': REQUIRED, 'files': files})


def _resolved(root, manifest):
    """Bind loaded product modules and the resolved CLI to one source package."""
    package = __package__
    expected = {item['path'] for item in manifest['files']}
    aliases = {'cli', 'provenance', 'history_cli'}
    packages = {package, 'scripts', 'kpopper', '_kpopper_runtime'}
    for name, module in tuple(sys.modules.items()):
        if module is None or not (name in aliases or any(
                name == prefix or name.startswith(prefix + '.') for prefix in packages)):
            continue
        source = getattr(module, '__file__', None)
        spec = getattr(module, '__spec__', None)
        origin = getattr(spec, 'origin', None)
        _require(isinstance(source, str) and source.endswith('.py') and
                 (origin is None or origin.endswith('.py')), 'bytecode_only_runtime', name)
        path = Path(source).absolute()
        _require(path.resolve().is_relative_to(root), 'mixed_runtime_roots', name)
        relative = path.resolve().relative_to(root).as_posix()
        _require(relative in expected, 'mixed_runtime_roots', name)
        _require(_path(root, relative) == path.resolve(), 'mixed_runtime_roots', name)
        if origin is not None:
            _require(Path(origin).resolve() == path.resolve(), 'mixed_runtime_roots', name)
    spec = importlib.util.find_spec(package + '.cli')
    _require(spec is not None and isinstance(spec.origin, str) and spec.origin.endswith('.py'),
             'bytecode_only_runtime', 'CLI')
    cli = Path(spec.origin).resolve()
    _require(cli == _path(root, 'cli.py'), 'mixed_runtime_roots', 'CLI')
    executable = Path(sys.executable).resolve()
    _require(executable.is_file(), 'missing_runtime_executable')
    return {'executable': str(executable), 'cli': str(cli), 'package_root': str(root),
            'argv': list(sys.argv), 'bytecode_write_disabled': bool(sys.dont_write_bytecode)}


def _history_schemas():
    return {'authority': [1, 2], 'baseline': [1], 'commit': [1],
            'typed_object': [2], 'prepared_mutation': [1, 2], 'projection': [1], 'import': [1, 2],
            'authoring_receipt': [1, 2, 3, 4, 5, 6], 'identity_receipt': [1, 2], 'history_auxiliary': [1],
            'group_transition': [1], 'named_hypotheses': [1], 'physical_hypothesis_import': [1],
            'branch_capture': [1, 2], 'branch_adoption': [1, 2],
            'bundle': [1, 2, 3], 'contribution': [1, 2, 3], 'retained_generations': [1], 'cancellation': [1],
            'commit_capabilities': ['explicit-root-disposition/v1', 'subject-paths/v2',
                                    'temporal-applicability/v1'],
            'bundle_capabilities': ['generation-cancellation/v1', 'history-closure/v1', 'history-generations/v1', 'history-subset/v1', 'subject-paths/v2'],
            'act_kinds': ['accept', 'correct', 'propose', 'refute', 'retire', 'review']}


def _schemas(root):
    resources = []
    for name in ('assessment.schema.json', 'reasoning/assessment.schema.json',
                 'reasoning/history_assessment.schema.json'):
        raw = _read(root, name)
        _strict_json(raw)
        resources.append({'path': name, 'sha256': hashlib.sha256(raw).hexdigest()})
    return _seal({'version': 1, 'history': _history_schemas(),
        'identity_schemes': ['prototype/v1', 'typed-history/v2'], 'resources': resources})


def _native(root):
    """Validate the packaged archive using the existing manifest contract, without extraction."""
    try:
        target = R.target_name()
    except R.RuntimeUnavailable:
        return _seal({'version': 1, 'status': 'unavailable', 'reason': 'unsupported_target',
                      'target': None, 'readiness': 'not_tested'})
    relative = 'reasoning/native/' + target + '.zip'
    if not (root / relative).exists():
        return _seal({'version': 1, 'status': 'unavailable', 'reason': 'archive_missing',
                      'target': target, 'readiness': 'not_tested'})
    raw = _read(root, relative, MAX_ARCHIVE_BYTES)
    import io
    try:
        with zipfile.ZipFile(io.BytesIO(raw)) as archive:
            infos = archive.infolist()
            names = [info.filename for info in infos]
            _require(len(names) == len(set(names)) and 'manifest.json' in names,
                     'invalid_runtime_archive')
            _require(len(names) <= 1024 and sum(info.file_size for info in infos) <= MAX_EXPANDED_BYTES,
                     'runtime_size_limit')
            for info in infos:
                R._relative(info.filename)
                _require(not info.is_dir() and not stat.S_ISLNK(info.external_attr >> 16)
                         and info.file_size <= 128 * 1024 * 1024, 'invalid_runtime_archive')
            manifest = _strict_json(archive.read('manifest.json'))
            for source in (root / 'reasoning/lean').glob('*.lean'):
                _path(root, source.relative_to(root).as_posix())
            validator = R.Runtime.__new__(R.Runtime)
            validator.manifest = manifest
            validator._validate_manifest(target, root / 'reasoning')
            _require(set(names) == {'manifest.json', *manifest['files']}, 'invalid_runtime_archive')
            for name, expected in manifest['files'].items():
                _require(hashlib.sha256(archive.read(name)).hexdigest() == expected,
                         'runtime_archive_checksum_mismatch', name)
    except (OSError, ValueError, KeyError, TypeError, zipfile.BadZipFile) as error:
        if isinstance(error, RuntimeDeclarationError):
            raise
        raise RuntimeDeclarationError('invalid_runtime_archive') from error
    return _seal({'version': 1, 'status': 'archive_validated', 'target': target,
                  'archive': relative, 'archive_sha256': hashlib.sha256(raw).hexdigest(),
                  'manifest': manifest, 'readiness': 'not_tested'})


def describe(nonce):
    """Return a strict v1 declaration from resolved sources; create no caches/files."""
    _require(isinstance(nonce, str) and NONCE.fullmatch(nonce), 'invalid_runtime_nonce')
    root = Path(__file__).resolve().parent
    sources = _source_manifest(root)
    resolved = _resolved(root, sources)
    schemas = _schemas(root)
    native = _native(root)
    # Recheck byte membership across the whole observation, including the native
    # archive and schema resources. A changed source is not an attested generation.
    _require(_source_manifest(root) == sources and _schemas(root) == schemas and _native(root) == native,
             'runtime_sources_changed')
    _require(_resolved(root, sources) == resolved, 'mixed_runtime_roots')
    return {'version': 1, 'endpoint': ENDPOINT, 'nonce': nonce, 'resolved': resolved,
            'sources': sources, 'schemas': schemas, 'native': native,
            'assurance': {'source': 'resolved_files_only', 'native': 'archive_validation_only',
                          'freshness': 'nonce_correlation_only'}}


def _validate_declaration(value, nonce):
    _require(isinstance(value, dict) and set(value) ==
        {'version', 'endpoint', 'nonce', 'resolved', 'sources', 'schemas', 'native', 'assurance'},
        'unsupported_runtime_endpoint')
    _require(type(value['version']) is int and value['version'] == 1 and value['endpoint'] == ENDPOINT,
             'unsupported_runtime_endpoint')
    _require(value['nonce'] == nonce, 'runtime_nonce_mismatch')
    _require(value['assurance'] == {'source': 'resolved_files_only', 'native': 'archive_validation_only',
                                  'freshness': 'nonce_correlation_only'}, 'unsupported_runtime_assurance')
    resolved = value['resolved']
    _require(isinstance(resolved, dict) and set(resolved) ==
             {'executable', 'cli', 'package_root', 'argv', 'bytecode_write_disabled'}, 'invalid_runtime_resolution')
    _require(type(resolved['bytecode_write_disabled']) is bool and isinstance(resolved['argv'], list)
             and all(isinstance(arg, str) for arg in resolved['argv']), 'invalid_runtime_resolution')
    for key in ('executable', 'cli', 'package_root'):
        _require(isinstance(resolved[key], str) and Path(resolved[key]).is_absolute()
                 and str(Path(resolved[key]).resolve()) == resolved[key], 'invalid_runtime_resolution')
    for key in ('sources', 'schemas', 'native'):
        item = value[key]
        _require(isinstance(item, dict) and isinstance(item.get('digest'), str)
                 and HEX.fullmatch(item['digest']) and type(item.get('version')) is int and item['version'] == 1,
                 'invalid_runtime_manifest', key)
        _require(_digest({k: v for k, v in item.items() if k != 'digest'}) == item['digest'],
                 'runtime_digest_mismatch', key)
    source = value['sources']
    _require(set(source) == {'version', 'scheme', 'inclusion', 'required', 'files', 'digest'}
             and source['scheme'] == 'product-python/v1' and source['inclusion'] == INCLUSION
             and source['required'] == REQUIRED and isinstance(source['files'], list), 'unsupported_source_manifest')
    paths = []
    for item in source['files']:
        _require(isinstance(item, dict) and set(item) == {'path', 'sha256'}
                 and isinstance(item['path'], str) and isinstance(item['sha256'], str)
                 and HEX.fullmatch(item['sha256']), 'invalid_source_manifest')
        path = Path(item['path'])
        _require(not path.is_absolute() and '..' not in path.parts and '\\' not in item['path']
                 and ':' not in item['path'] and path.suffix == '.py'
                 and str(path) == item['path'], 'invalid_source_manifest')
        paths.append(item['path'])
    _require(len(paths) <= 1024 and paths == sorted(set(paths)) and set(REQUIRED) <= set(paths),
             'invalid_source_manifest')
    schemas = value['schemas']
    _require(set(schemas) == {'version', 'history', 'identity_schemes', 'resources', 'digest'}
             and schemas['history'] == _history_schemas()
             and schemas['identity_schemes'] == ['prototype/v1', 'typed-history/v2']
             and isinstance(schemas['resources'], list), 'unsupported_runtime_schema')
    resources = schemas['resources']
    schema_paths = ('assessment.schema.json', 'reasoning/assessment.schema.json',
                    'reasoning/history_assessment.schema.json')
    _require(len(resources) == len(schema_paths), 'unsupported_runtime_schema')
    for item, path in zip(resources, schema_paths):
        _require(isinstance(item, dict) and set(item) == {'path', 'sha256'} and item['path'] == path
                 and isinstance(item['sha256'], str) and HEX.fullmatch(item['sha256']),
                 'unsupported_runtime_schema')
    native = value['native']
    _require(native.get('readiness') == 'not_tested', 'unsupported_native_assurance')
    if native.get('status') == 'unavailable':
        _require(set(native) == {'version', 'status', 'reason', 'target', 'readiness', 'digest'}
                 and native['reason'] in ('unsupported_target', 'archive_missing'), 'unsupported_native_manifest')
    else:
        _require(set(native) == {'version', 'status', 'target', 'archive', 'archive_sha256', 'manifest',
                                'readiness', 'digest'} and native['status'] == 'archive_validated'
                 and isinstance(native['target'], str) and
                 native['archive'] == 'reasoning/native/' + native['target'] + '.zip'
                 and isinstance(native['archive_sha256'], str) and HEX.fullmatch(native['archive_sha256'])
                 and isinstance(native['manifest'], dict), 'unsupported_native_manifest')
    return value


def probe_launchers(inventory, expected_digests, nonce=None, *, timeout=10):
    """Probe only explicitly configured managed launchers, with no shell or PATH search.

    Inventory entries: {id, argv, package_root, executable}. argv is the launch
    prefix; the capabilities endpoint and challenge are appended. executable is
    the expected actual Python interpreter (possibly different from argv[0] for
    a script launcher). Expected digests: {id: {sources, schemas, native}}.
    Neither argument may be supplied from record/report content by a caller.
    """
    _require(isinstance(inventory, list) and 0 < len(inventory) <= 16, 'invalid_launcher_inventory')
    _require(isinstance(expected_digests, dict), 'invalid_expected_digests')
    _require(type(timeout) in (int, float) and 0 < timeout <= 30, 'invalid_probe_timeout')
    nonce = nonce or secrets.token_hex(24)
    _require(isinstance(nonce, str) and NONCE.fullmatch(nonce), 'invalid_runtime_nonce')
    selected, ids = [], []
    for item in inventory:
        _require(isinstance(item, dict) and set(item) == {'id', 'argv', 'package_root', 'executable'},
                 'invalid_launcher_inventory')
        name, argv = item['id'], item['argv']
        _require(isinstance(name, str) and re.fullmatch(r'[A-Za-z0-9_.-]{1,80}', name), 'invalid_launcher_inventory')
        _require(isinstance(argv, list) and argv and len(argv) <= 64 and
                 all(isinstance(arg, str) and '\0' not in arg and len(arg) <= 8192 for arg in argv),
                 'invalid_launcher_inventory')
        _require(Path(argv[0]).is_absolute() and Path(argv[0]).is_file(), 'absolute_launcher_required', name)
        for key in ('package_root', 'executable'):
            _require(isinstance(item[key], str) and Path(item[key]).is_absolute()
                     and str(Path(item[key]).resolve()) == item[key], 'invalid_launcher_inventory', key)
        _require(Path(item['package_root']).is_dir() and Path(item['executable']).is_file(),
                 'invalid_launcher_inventory', name)
        expected = expected_digests.get(name)
        _require(isinstance(expected, dict) and set(expected) == {'sources', 'schemas', 'native'}
                 and all(isinstance(value, str) and HEX.fullmatch(value) for value in expected.values()),
                 'invalid_expected_digests', name)
        selected.append(item)
        ids.append(name)
    _require(len(ids) == len(set(ids)) and set(expected_digests) == set(ids), 'invalid_launcher_inventory')
    proof = []
    for item in selected:
        command = [*item['argv'], 'history', 'capabilities', '--nonce', nonce, '--json']
        try:
            raw = R._run_bounded(command, b'', timeout=timeout, output_bytes=MAX_OUTPUT_BYTES)
        except Exception as error:
            raise RuntimeDeclarationError('launcher_probe_failed', item['id']) from error
        value = _validate_declaration(_strict_json(raw), nonce)
        resolved = value['resolved']
        _require(resolved['package_root'] == item['package_root'] and
                 resolved['cli'] == str(Path(item['package_root']) / 'cli.py') and
                 resolved['executable'] == item['executable'], 'runtime_root_mismatch', item['id'])
        _require({key: value[key]['digest'] for key in ('sources', 'schemas', 'native')} ==
                 expected_digests[item['id']], 'runtime_digest_mismatch', item['id'])
        proof.append({'id': item['id'], 'argv': command, 'declaration': value,
                      'declaration_digest': _digest(value)})
    return {'version': 1, 'kind': 'managed-launcher-probe/v1', 'nonce': nonce,
            'scope': 'selected_managed_launchers', 'complete': True, 'launchers': proof,
            'assurance': 'fresh_nonce_correlation_not_attestation',
            'deployment_stability': 'caller_responsibility', 'unlisted_launchers': 'not_covered'}
