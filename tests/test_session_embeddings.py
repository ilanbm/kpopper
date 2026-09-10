"""Mechanical ranking contracts; no model downloads or optional packages required."""
import copy
from concurrent.futures import ThreadPoolExecutor
import hashlib
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from scripts.session import embeddings as e5


class SyntheticIndex(e5.E5Index):
    """One-hot vectors isolate chunk/index behavior from semantic model quality."""
    def __init__(self, directory='unused-model'):
        super().__init__(directory)
        self.encoded = []

    def _ensure_runtime(self):
        self._runtime_metadata = {'test_encoder': 'one-hot'}

    def _tokens(self, text, special=False):
        if text.startswith('passage:'):
            # Like the pinned tokenizer, trailing whitespace alone is a token,
            # but its encoding changes when document text follows it.
            suffix = text[len('passage:'):]
            values = [10] + ([12] if suffix == ' ' else [ord(c) + 100 for c in suffix[1:]])
        else:
            values = [11] + [ord(c) + 100 for c in text[len('query: '):]]
        return [0] + values + [2] if special else values

    def _encode_ids(self, ids):
        if not 0 < len(ids) <= e5.TOKEN_LIMIT:
            raise ValueError('oversized fake inference')
        self.encoded.append(tuple(ids))
        vector = [0.0] * e5.DIMENSIONS
        vector[0 if ord('!') + 100 in ids else 1] = 1.0
        return tuple(vector)


class VerifiedSyntheticIndex(SyntheticIndex):
    _ensure_runtime = e5.E5Index._ensure_runtime

    def _load_runtime(self, paths):
        self._session = object()
        self._runtime_metadata = {'test_encoder': 'one-hot'}


class OptionalE5Contract(unittest.TestCase):
    def assets(self, folder):
        specs = {}
        for kind, name in [('weights', 'model_quantized.onnx'), ('tokenizer', 'tokenizer.json')]:
            data = ('fixture-' + kind).encode()
            (folder / name).write_bytes(data)
            specs[kind] = (name, len(data), hashlib.sha256(data).hexdigest())
        return specs

    def test_module_and_constructor_import_without_optional_runtime_or_assets(self):
        script = '''
import sys
from scripts.session.embeddings import E5Index
E5Index('a-model-directory-that-is-not-present')
assert not any(name in sys.modules for name in ['numpy', 'onnxruntime', 'tokenizers'])
'''
        result = subprocess.run([sys.executable, '-c', script], cwd=ROOT, capture_output=True,
                                text=True, encoding='utf-8')
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_missing_or_wrong_hash_assets_reject_before_runtime_import(self):
        with tempfile.TemporaryDirectory() as directory:
            folder = Path(directory)
            with patch.object(e5.importlib, 'import_module') as imports:
                with self.assertRaisesRegex(ValueError, 'assets are unavailable'):
                    e5.E5Index(folder).rank({'doc': 'content'}, 'query')
                specs = self.assets(folder)
                (folder / 'model_quantized.onnx').write_bytes(b'x' * specs['weights'][1])
                with patch.dict(e5._ARTIFACTS, specs, clear=True):
                    with self.assertRaisesRegex(ValueError, 'artifact mismatch'):
                        e5.E5Index(folder).rank({'doc': 'content'}, 'query')
                imports.assert_not_called()

    def test_absent_optional_dependencies_produce_a_predictable_fallback_error(self):
        with tempfile.TemporaryDirectory() as directory:
            folder = Path(directory); specs = self.assets(folder)
            with patch.dict(e5._ARTIFACTS, specs, clear=True), \
                    patch.object(e5.importlib, 'import_module', side_effect=ImportError('not installed')):
                with self.assertRaisesRegex(ValueError, 'runtime unavailable or incompatible'):
                    e5.E5Index(folder).rank({'doc': 'content'}, 'query')

    def test_changed_assets_cannot_reuse_an_existing_in_memory_index(self):
        with tempfile.TemporaryDirectory() as directory:
            folder = Path(directory); specs = self.assets(folder)
            with patch.dict(e5._ARTIFACTS, specs, clear=True):
                index = VerifiedSyntheticIndex(folder)
                index.rank({'doc': 'content'}, 'query')
                (folder / 'model_quantized.onnx').write_bytes(b'changed-size')
                with self.assertRaisesRegex(ValueError, 'model assets changed'):
                    index.rank({'doc': 'content'}, 'query')

    def test_chunk_plan_retains_all_tokens_including_multilingual_tail(self):
        index = SyntheticIndex()
        text = 'word שלום 世界 ' * 160 + 'tail!'
        chunks = list(index._chunks(text))
        prefix = index._tokens('passage:')
        bodies = [list(c[1 + len(prefix):-1]) for c in chunks]
        reconstructed = bodies[0] + [token for body in bodies[1:] for token in body[e5.OVERLAP_TOKENS:]]
        self.assertEqual(reconstructed, index._tokens('passage: ' + text)[len(prefix):])
        self.assertGreater(len(chunks), 1)
        for i, chunk in enumerate(chunks):
            self.assertLessEqual(len(chunk), 512)
            self.assertEqual(list(chunk[:1 + len(prefix)]), [0] + prefix)
            self.assertEqual(chunk[-1], 2)
            if i: self.assertEqual(bodies[i - 1][-e5.OVERLAP_TOKENS:], bodies[i][:e5.OVERLAP_TOKENS])

    def test_short_chunks_match_whole_encoding_with_context_sensitive_whitespace(self):
        index = SyntheticIndex()
        self.assertNotEqual(index._tokens('passage: '), index._tokens('passage: short')[:2])
        for text in ['', 'short', ' leading', 'שלום 世界']:
            with self.subTest(text=text):
                self.assertEqual(list(index._chunks(text)),
                                 [tuple(index._tokens('passage: ' + text, special=True))])

    def test_document_score_uses_the_best_chunk_not_only_the_beginning(self):
        documents = {'late': 'a' * 1300 + '!', 'plain': 'a' * 20, 'empty': ''}
        original = copy.deepcopy(documents)
        index = SyntheticIndex(); result = index.rank(documents, '!')
        self.assertEqual(set(result['scores']), set(documents))
        self.assertEqual(result['document_count'], 3)
        self.assertGreater(result['chunk_count'], result['document_count'])
        self.assertEqual(result['scores']['late'], 1.0)
        self.assertEqual(result['scores']['plain'], 0.0)
        self.assertEqual(documents, original)

    def test_query_overflow_rejects_instead_of_clipping(self):
        index = SyntheticIndex()
        with self.assertRaisesRegex(ValueError, 'query exceeds 512 tokens; no truncation'):
            index.rank({'doc': 'short'}, 'q' * 510)
        self.assertEqual(index.encoded, [])
        self.assertEqual(index.rank({'doc': 'short'}, 'q' * 509)['document_count'], 1)

    def test_cache_is_order_independent_but_changes_with_body_or_id(self):
        index = SyntheticIndex(); documents = {'b': 'B', 'a': 'A!'}
        first = index.rank(documents, '!'); calls = len(index.encoded)
        again = index.rank(dict(reversed(list(documents.items()))), '!')
        self.assertTrue(again['index_reused'])
        self.assertEqual(len(index.encoded), calls + 1)  # only the query is encoded again
        self.assertEqual(first['scores'], again['scores'])
        self.assertEqual(first['content_fingerprint'], again['content_fingerprint'])
        documents['a'] = 'A without tail'
        changed = index.rank(documents, '!')
        self.assertFalse(changed['index_reused'])
        self.assertNotEqual(changed['content_fingerprint'], first['content_fingerprint'])
        self.assertEqual(changed['scores']['a'], 0.0)
        renamed = index.rank({'new': documents['a'], 'b': documents['b']}, '!')
        self.assertFalse(renamed['index_reused'])
        self.assertEqual(set(renamed['scores']), {'new', 'b'})
        renamed['scores'].clear()
        self.assertEqual(set(index.rank({'new': documents['a'], 'b': documents['b']}, '!')['scores']), {'new', 'b'})

    def test_concurrent_snapshots_do_not_exchange_cached_body_vectors(self):
        barrier = threading.Barrier(2)

        class ConcurrentIndex(SyntheticIndex):
            def __setattr__(self, name, value):
                super().__setattr__(name, value)
                if name == '_cached' and value is not None:
                    barrier.wait(timeout=5)

        index = ConcurrentIndex()
        with ThreadPoolExecutor(max_workers=2) as pool:
            results = list(pool.map(lambda text: index.rank({'same.id': text}, '!')['scores']['same.id'],
                                    ['no marker', 'has marker!']))
        self.assertEqual(results, [0.0, 1.0])

    def test_work_bounds_fail_the_entire_call_before_any_inference(self):
        cases = [({str(i): 'x' for i in range(e5.MAX_DOCUMENTS + 1)}, 'document count'),
                 ({'large': 'x' * (e5.MAX_DOCUMENT_BYTES + 1)}, '128 KiB'),
                 ({str(i): 'x' * e5.MAX_DOCUMENT_BYTES for i in range(17)}, '2 MiB')]
        for documents, reason in cases:
            index = SyntheticIndex()
            with self.subTest(reason=reason), self.assertRaisesRegex(ValueError, reason):
                index.rank(documents, 'q')
            self.assertEqual(index.encoded, [])
        index = SyntheticIndex()
        with patch.object(e5, 'MAX_CHUNKS', 1):
            with self.assertRaisesRegex(ValueError, 'index exceeds'):
                index.rank({'first': 'short', 'second': 'also short'}, 'q')
        self.assertEqual(index.encoded, [])

    def test_failed_rebuild_never_returns_or_publishes_a_partial_index(self):
        index = SyntheticIndex(); original = {'one': 'original'}
        first = index.rank(original, 'q')
        encode = index._encode_ids
        with patch.object(index, '_encode_ids', side_effect=[encode([0, 10, 2]), ValueError('inference failed')]):
            with self.assertRaisesRegex(ValueError, 'inference failed'):
                index.rank({'one': 'changed', 'two': 'new'}, 'q')
        restored = index.rank(original, 'q')
        self.assertTrue(restored['index_reused'])
        self.assertEqual(restored['scores'], first['scores'])

    def test_invalid_types_and_empty_query_fail_clearly(self):
        for documents, query in [([], 'q'), ({'id': None}, 'q'), ({False: 'text'}, 'q'),
                                 ({'id': 'text'}, ''), ({'id': 'text'}, None)]:
            with self.subTest(documents=documents, query=query), self.assertRaises(ValueError):
                SyntheticIndex().rank(documents, query)
        empty = SyntheticIndex().rank({}, 'query')
        self.assertEqual((empty['scores'], empty['document_count'], empty['chunk_count']), ({}, 0, 0))


if __name__ == '__main__': unittest.main()
