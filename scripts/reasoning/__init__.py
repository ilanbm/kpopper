"""Versioned, deterministic reasoning over an explicitly captured record.

The portable profile is experimental until all existing consumers adopt it.
Importing this package never compiles, downloads, or evaluates anything.
"""
import hashlib
import json
from pathlib import Path

_ROOT = Path(__file__).resolve().parent
_SOURCES = {path.name: hashlib.sha256(path.read_bytes()).hexdigest()
            for path in sorted(_ROOT.glob('*.py')) if path.name != 'build_runtime.py'}
_SOURCE_ID = hashlib.sha256(json.dumps(_SOURCES, sort_keys=True, separators=(',', ':')).encode()).hexdigest()


def adapter_identity():
    """Identify this loaded adapter revision and refuse a mixed-source process."""
    current = {path.name: hashlib.sha256(path.read_bytes()).hexdigest()
               for path in sorted(_ROOT.glob('*.py')) if path.name != 'build_runtime.py'}
    if current != _SOURCES:
        raise ValueError('reasoning adapters changed; restart the reader')
    return _SOURCE_ID
