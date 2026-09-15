"""Offline, versioned native runtime supplied as ordinary package data."""
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import platform
import shutil
import stat
import subprocess
import tempfile
import zipfile

from .contract import MAX_REQUEST_BYTES, validate_value
from . import adapter_identity


class RuntimeUnavailable(ValueError):
    pass


def source_hash(lean_dir):
    root = Path(lean_dir)
    files = {p.relative_to(root).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest()
             for p in sorted(root.glob('*.lean'))
             if not p.name.startswith(('Proof', 'Audit'))}
    if not files:
        raise RuntimeUnavailable('runtime source identity is unavailable')
    return hashlib.sha256(json.dumps(files, sort_keys=True, separators=(',', ':')).encode()).hexdigest()


def target_name():
    system, machine = platform.system().lower(), platform.machine().lower()
    machine = {'amd64': 'x86_64', 'aarch64': 'arm64'}.get(machine, machine)
    if system == 'linux' and machine == 'arm64':
        machine = 'aarch64'
    target = system + '-' + machine
    if target not in {'darwin-arm64', 'darwin-x86_64', 'linux-x86_64',
                      'linux-aarch64', 'windows-x86_64'}:
        raise RuntimeUnavailable('unsupported runtime platform: ' + target)
    return target


def _relative(name):
    if not isinstance(name, str) or not name or '\\' in name:
        raise RuntimeUnavailable('invalid runtime archive path')
    path = PurePosixPath(name)
    if path.is_absolute() or str(path) != name or any(p in ('.', '..') for p in path.parts):
        raise RuntimeUnavailable('runtime archive path escapes its directory')
    return path


def _sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


class Runtime:
    def __init__(self, archive=None):
        source = Path(__file__).resolve().parent
        target = target_name()
        self.archive = Path(archive) if archive is not None else source / 'native' / (target + '.zip')
        if not self.archive.is_file():
            raise RuntimeUnavailable('packaged reasoning runtime is unavailable for ' + target)
        archive_digest = _sha(self.archive)
        try:
            with zipfile.ZipFile(self.archive) as bundle:
                names = bundle.namelist()
                if len(names) != len(set(names)) or 'manifest.json' not in names:
                    raise RuntimeUnavailable('invalid runtime archive inventory')
                for info in bundle.infolist():
                    _relative(info.filename)
                    if info.is_dir() or stat.S_ISLNK(info.external_attr >> 16) or info.file_size > 128 * 1024 * 1024:
                        raise RuntimeUnavailable('unsupported runtime archive member')
                self.manifest = json.loads(bundle.read('manifest.json'))
                self._validate_manifest(target, source)
                if set(names) != {'manifest.json', *self.manifest['files']}:
                    raise RuntimeUnavailable('runtime archive inventory disagrees with manifest')
                cache = Path(os.environ.get('XDG_CACHE_HOME', str(Path.home() / '.cache')))
                self.root = cache / 'kpopper' / 'reasoning' / archive_digest
                if not self.root.exists():
                    self.root.parent.mkdir(parents=True, exist_ok=True)
                    with tempfile.TemporaryDirectory(prefix='.extract-', dir=self.root.parent) as temp:
                        stage = Path(temp) / 'runtime'
                        stage.mkdir(mode=0o700)
                        for name, expected in self.manifest['files'].items():
                            data = bundle.read(name)
                            if hashlib.sha256(data).hexdigest() != expected:
                                raise RuntimeUnavailable('runtime archive checksum mismatch: ' + name)
                            path = stage / name
                            path.parent.mkdir(parents=True, exist_ok=True)
                            path.write_bytes(data)
                            path.chmod(0o755 if name == self.manifest['executable'] else 0o644)
                        try:
                            stage.rename(self.root)
                        except OSError:
                            if not self.root.is_dir():
                                raise
        except (OSError, KeyError, TypeError, json.JSONDecodeError, zipfile.BadZipFile) as error:
            raise RuntimeUnavailable('invalid packaged reasoning runtime: ' + str(error)) from error
        self.binary = self.root / self.manifest['executable']
        self._observed = self._verify_files()
        self.implementation = {
            'protocol': 'KP1', 'lean_version': self.manifest['lean_version'],
            'adapter_source_sha256': adapter_identity(),
            'source_sha256': self.manifest['source_sha256'],
            'archive_sha256': archive_digest, 'binary_sha256': self._observed[self.manifest['executable']],
            'target': target,
            'libraries': {name: self._observed[name] for name in self.manifest['libraries']},
            'modified_libraries': [name for name in self.manifest['libraries']
                                   if self._observed[name] != self.manifest['files'][name]],
        }

    def _validate_manifest(self, target, source):
        data = self.manifest
        if not isinstance(data, dict) or type(data.get('version')) is not int or data['version'] != 1 \
                or data.get('protocol') != 'KP1' or data.get('target') != target \
                or data.get('lean_version') != '4.33.1' or data.get('modules') != ['arithmetic/v1'] \
                or not isinstance(data.get('files'), dict) or not isinstance(data.get('libraries'), list):
            raise RuntimeUnavailable('unsupported runtime manifest')
        if data.get('source_sha256') != source_hash(source / 'lean'):
            raise RuntimeUnavailable('packaged runtime does not match its source revision')
        for name, expected in data['files'].items():
            _relative(name)
            if not isinstance(expected, str) or len(expected) != 64 \
                    or any(c not in '0123456789abcdef' for c in expected):
                raise RuntimeUnavailable('invalid runtime file digest')
        if data.get('executable') not in data['files'] or any(name not in data['files'] for name in data['libraries']) \
                or data['executable'] in data['libraries'] or len(set(data['libraries'])) != len(data['libraries']):
            raise RuntimeUnavailable('invalid runtime executable/library inventory')

    def _verify_files(self):
        observed = {}
        for name, expected in self.manifest['files'].items():
            path = self.root / name
            if not path.is_file() or path.is_symlink():
                raise RuntimeUnavailable('runtime member is unavailable: ' + name)
            actual = _sha(path)
            # LGPL libraries remain replaceable. Their observed identity is disclosed
            # separately; replacing the executable or notices is an integrity error.
            if name not in self.manifest['libraries'] and actual != expected:
                raise RuntimeUnavailable('runtime member checksum changed: ' + name)
            observed[name] = actual
        return observed

    def request(self, request):
        return self.request_many([request])[0]

    def request_many(self, requests):
        from .transport import encode_request, decode_response
        if not requests:
            return []
        adapter_identity()
        if self._verify_files() != self._observed:
            raise RuntimeUnavailable('runtime changed; reopen before evaluating')
        lines = [encode_request(request).rstrip('\n') for request in requests]
        if any(len(line.encode('utf-8')) > MAX_REQUEST_BYTES for line in lines):
            raise RuntimeUnavailable('native request exceeds transport limit')
        result = subprocess.run([str(self.binary)], input='\n'.join(lines) + '\n',
                                text=True, encoding='utf-8', capture_output=True, timeout=30)
        adapter_identity()
        if self._verify_files() != self._observed:
            raise RuntimeUnavailable('runtime changed during evaluation')
        if result.returncode != 0:
            raise RuntimeUnavailable('native reasoning process failed')
        output = result.stdout.splitlines()
        if len(output) != len(requests):
            raise RuntimeUnavailable('native response count does not match requests')
        decoded = [decode_response(line) for line in output]
        for value in decoded:
            if value['status'] == 'ok':
                validate_value(value['value'])
        return decoded
