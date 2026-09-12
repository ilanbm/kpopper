"""Optional pinned local E5 ranking. No downloads, source edits or persistent cache.

Chunks exist only in the in-memory ranking index. Callers retain and deliver the
original evidence; similarity is neither entailment nor evidence verification.
Any unsupported runtime or work limit raises ValueError for whole-record lexical
fallback, never a partial document roster. Importing this module needs no extras.
"""
from __future__ import annotations

import hashlib
import importlib
import json
import math
from pathlib import Path

MODEL = 'Xenova/multilingual-e5-small'
REVISION = '761b726dd34fb83930e26aab4e9ac3899aa1fa78'
TOKEN_LIMIT = 512
OVERLAP_TOKENS = 64
DIMENSIONS = 384
MAX_DOCUMENTS = 512
MAX_DOCUMENT_BYTES = 128 * 1024
MAX_TOTAL_BYTES = 2 * 1024 * 1024
MAX_CHUNKS = 1024
MAX_QUERY_BYTES = 16 * 1024
MAX_ID_BYTES = 4096
_ARTIFACTS = {
    'weights': ('model_quantized.onnx', 118308185,
                'f80102d3f2a1229f387d3c81909990d8945513e347b0eab049f7de3c6f98c193'),
    'tokenizer': ('tokenizer.json', 17082730,
                  '0b44a9d7b51c3c62626640cda0e2c2f70fdacdc25bbbd68038369d14ebdf4c39'),
}


def _require(condition, reason):
    if not condition:
        raise ValueError('Local E5 unavailable for this ranking: ' + reason)


def _sha(path):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()


class E5Index:
    """Lazy CPU scorer with one content-keyed, in-memory document index.

    model_dir contains the exact pinned model_quantized.onnx and tokenizer.json.
    rank returns every supplied ID or raises ValueError; callers can then fall back
    to lexical ranking for the complete record. A changed asset requires a new
    instance. Changing any ID or document string rebuilds the document index.
    """
    def __init__(self, model_dir):
        try:
            self.model_dir = Path(model_dir).expanduser().absolute()
        except (TypeError, ValueError) as error:
            raise ValueError('Local E5 needs a model directory') from error
        self._session = None
        self._tokenizer = None
        self._np = None
        self._asset_stamp = None
        self._cached = None
        self._runtime_metadata = {}

    def _stamp(self):
        try:
            return tuple((str(path.resolve()), path.stat().st_ino, path.stat().st_size, path.stat().st_mtime_ns)
                         for name, _, _ in _ARTIFACTS.values() for path in [self.model_dir / name])
        except OSError as error:
            raise ValueError('Local E5 model assets are unavailable') from error

    def _verified_assets(self):
        before = self._stamp()
        paths = {}
        try:
            for kind, (name, size, expected) in _ARTIFACTS.items():
                path = self.model_dir / name
                _require(path.is_file() and path.stat().st_size == size and _sha(path) == expected,
                         'pinned ' + kind + ' artifact mismatch')
                paths[kind] = path
        except OSError as error:
            raise ValueError('Local E5 model assets are unreadable') from error
        _require(self._stamp() == before, 'assets changed during verification')
        return paths, before

    def _load_runtime(self, paths):
        # No optional dependency is imported before rank and artifact validation.
        try:
            np = importlib.import_module('numpy')
            ort = importlib.import_module('onnxruntime')
            tokenizers = importlib.import_module('tokenizers')
            tokenizer = tokenizers.Tokenizer.from_file(str(paths['tokenizer']))
            tokenizer.no_truncation()
            tokenizer.no_padding()
            _require(tokenizer.token_to_id('<s>') == 0 and tokenizer.token_to_id('</s>') == 2,
                     'unsupported tokenizer special tokens')
            for prefix in ('passage: ', 'query: '):
                bare = tokenizer.encode(prefix + 'index check', add_special_tokens=False).ids
                _require(tokenizer.encode(prefix + 'index check').ids == [0] + bare + [2],
                         'unsupported tokenizer postprocessor')
            options = ort.SessionOptions()
            options.intra_op_num_threads = 1
            options.inter_op_num_threads = 1
            options.execution_mode = ort.ExecutionMode.ORT_SEQUENTIAL
            session = ort.InferenceSession(str(paths['weights']), sess_options=options,
                                           providers=['CPUExecutionProvider'])
            inputs = {item.name: item.type for item in session.get_inputs()}
            _require('input_ids' in inputs and set(inputs) <= {'input_ids', 'attention_mask', 'token_type_ids'}
                     and all(dtype == 'tensor(int64)' for dtype in inputs.values()), 'unsupported ONNX input contract')
            _require('last_hidden_state' in {item.name for item in session.get_outputs()},
                     'missing token hidden states')
            _require(session.get_providers() == ['CPUExecutionProvider'], 'runtime is not CPU-only')
            self._np, self._tokenizer, self._session, self._inputs = np, tokenizer, session, inputs
            self._runtime_metadata = {'numpy': np.__version__, 'onnxruntime': ort.__version__,
                                      'tokenizers': tokenizers.__version__}
        except Exception as error:
            self._session = None
            raise ValueError('Local E5 runtime unavailable or incompatible: ' + type(error).__name__) from error

    def _ensure_runtime(self):
        if self._session is not None:
            _require(self._stamp() == self._asset_stamp, 'model assets changed; create a new index')
            return
        paths, stamp = self._verified_assets()
        self._load_runtime(paths)
        if self._stamp() != stamp:
            self._session = None
            raise ValueError('Local E5 assets changed while loading')
        self._asset_stamp = stamp

    def _tokens(self, text, special=False):
        try:
            return self._tokenizer.encode(text, add_special_tokens=special).ids
        except Exception as error:
            raise ValueError('Local E5 tokenizer failed: ' + type(error).__name__) from error

    def _chunks(self, text):
        # Tokenize the entire document before dividing its token sequence. Decode/
        # retokenize loops could lose offsets or tails in multilingual documents.
        # Anchor before the space: a standalone trailing space has a different
        # token than whitespace attached to following content. Keep the entire
        # suffix from the full encoding, including that token for an empty body.
        prefix = self._tokens('passage:')
        tokens = self._tokens('passage: ' + text)
        _require(prefix and tokens[:len(prefix)] == prefix, 'passage prefix tokenization changed')
        body = tokens[len(prefix):]
        capacity = TOKEN_LIMIT - len(prefix) - 2
        _require(capacity > OVERLAP_TOKENS, 'no room for passage content')
        start = 0
        while True:
            end = min(start + capacity, len(body))
            yield tuple([0] + prefix + body[start:end] + [2])
            if end == len(body):
                break
            start = end - OVERLAP_TOKENS

    def _encode_ids(self, ids):
        _require(0 < len(ids) <= TOKEN_LIMIT, 'encoder input exceeds 512 tokens; no truncation')
        try:
            np = self._np
            values = {'input_ids': list(ids), 'attention_mask': [1] * len(ids), 'token_type_ids': [0] * len(ids)}
            feed = {key: np.asarray([values[key]], dtype=np.int64) for key in self._inputs}
            hidden = self._session.run(['last_hidden_state'], feed)[0]
            _require(hidden.shape == (1, len(ids), DIMENSIONS), 'unexpected hidden-state shape')
            mask = np.asarray(values['attention_mask'], dtype=np.float32)[None, :, None]
            pooled = (hidden.astype(np.float32) * mask).sum(axis=1) / mask.sum(axis=1)
            norm = np.linalg.norm(pooled, axis=1, keepdims=True)
            _require(np.isfinite(pooled).all() and np.isfinite(norm).all() and (norm > 0).all(), 'invalid encoder vector')
            return tuple(float(value) for value in (pooled / norm)[0])
        except Exception as error:
            raise ValueError('Local E5 encoding failed: ' + type(error).__name__) from error

    def _documents(self, documents, query):
        _require(type(documents) is dict and len(documents) <= MAX_DOCUMENTS,
                 'document count exceeds 512 or documents are not a dictionary')
        _require(type(query) is str and query.strip(), 'query must be nonempty text')
        try:
            _require(len(query.encode('utf-8')) <= MAX_QUERY_BYTES, 'query exceeds 16 KiB; no truncation')
            total = 0
            for key, text in documents.items():
                _require(type(key) is str and key and type(text) is str, 'document IDs/text must be strings')
                _require(len(key.encode('utf-8')) <= MAX_ID_BYTES, 'document ID exceeds 4096 bytes')
                size = len(text.encode('utf-8'))
                _require(size <= MAX_DOCUMENT_BYTES, 'a document exceeds 128 KiB')
                total += size
            _require(total <= MAX_TOTAL_BYTES, 'document text exceeds 2 MiB')
        except UnicodeError as error:
            raise ValueError('Local E5 input is not valid UTF-8 text') from error
        return sorted(documents.items())

    def rank(self, documents: dict[str, str], query: str) -> dict:
        items = self._documents(documents, query)
        fingerprint = hashlib.sha256(json.dumps(items, ensure_ascii=False, separators=(',', ':')).encode('utf-8')).hexdigest()
        self._ensure_runtime()
        query_ids = self._tokens('query: ' + query, special=True)
        _require(len(query_ids) <= TOKEN_LIMIT, 'query exceeds 512 tokens; no truncation')
        cached = self._cached
        reused = cached is not None and cached[0] == fingerprint
        if not reused:
            chunks = []
            for key, text in items:
                for ids in self._chunks(text):
                    _require(len(chunks) < MAX_CHUNKS, 'index exceeds 1024 chunks')
                    chunks.append((key, ids))
            # Check the whole plan before any document inference. Publish only a
            # complete index: a failed build never replaces a valid prior cache.
            vectors = tuple((key, self._encode_ids(ids)) for key, ids in chunks)
            self._ensure_runtime()
            cached = (fingerprint, vectors)
            self._cached = cached
        # Another caller may publish a different complete snapshot. Score this
        # call against its own retained tuple, never reread that shared pointer.
        vectors = cached[1]
        scores = {}
        if items:
            query_vector = self._encode_ids(query_ids)
            for key, vector in vectors:
                score = sum(a * b for a, b in zip(query_vector, vector))
                _require(math.isfinite(score), 'nonfinite similarity')
                scores[key] = max(scores[key], score) if key in scores else score
        _require(set(scores) == set(documents), 'index does not cover every document')
        self._ensure_runtime()
        return {'scores': scores, 'document_count': len(items), 'chunk_count': len(vectors),
                'index_reused': reused, 'content_fingerprint': fingerprint,
                'model': {'name': MODEL, 'revision': REVISION,
                          'weights_sha256': _ARTIFACTS['weights'][2], 'tokenizer_sha256': _ARTIFACTS['tokenizer'][2],
                          'dimensions': DIMENSIONS, 'token_limit': TOKEN_LIMIT, 'overlap_tokens': OVERLAP_TOKENS,
                          'query_prefix': 'query: ', 'passage_prefix': 'passage: ',
                          'pooling': 'attention-mask mean', 'normalization': 'L2',
                          'execution': 'CPU, sequential, batch 1, one thread',
                          'aggregation': 'maximum chunk cosine per document', 'runtime': dict(self._runtime_metadata)},
                'limits': {'documents': MAX_DOCUMENTS, 'document_bytes': MAX_DOCUMENT_BYTES,
                           'total_document_bytes': MAX_TOTAL_BYTES, 'index_chunks': MAX_CHUNKS}}
