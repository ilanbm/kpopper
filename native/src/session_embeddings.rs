//! Optional, pinned, local-only E5 ranking for checked-session search.
//!
//! Model files are verified before the tokenizer or ONNX runtime sees them. The
//! provider keeps only an in-memory index and never downloads or persists data.

use crate::session_search::{SemanticProvider, SemanticRanking};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::Mutex,
};
use tokenizers::Tokenizer;
use tract_onnx::prelude::*;

const MODEL: &str = "Xenova/multilingual-e5-small";
const REVISION: &str = "761b726dd34fb83930e26aab4e9ac3899aa1fa78";
const TOKEN_LIMIT: usize = 512;
const OVERLAP_TOKENS: usize = 64;
const DIMENSIONS: usize = 384;
const MAX_DOCUMENTS: usize = 512;
const MAX_DOCUMENT_BYTES: usize = 128 * 1024;
const MAX_TOTAL_BYTES: usize = 2 * 1024 * 1024;
const MAX_CHUNKS: usize = 1024;
const MAX_QUERY_BYTES: usize = 16 * 1024;
const MAX_ID_BYTES: usize = 4096;
const WEIGHTS: (&str, u64, &str) = (
    "model_quantized.onnx",
    118_308_185,
    "f80102d3f2a1229f387d3c81909990d8945513e347b0eab049f7de3c6f98c193",
);
const TOKENIZER: (&str, u64, &str) = (
    "tokenizer.json",
    17_082_730,
    "0b44a9d7b51c3c62626640cda0e2c2f70fdacdc25bbbd68038369d14ebdf4c39",
);

fn unavailable(reason: impl AsRef<str>) -> String {
    format!("Local E5 unavailable for this ranking: {}", reason.as_ref())
}

fn require(condition: bool, reason: &str) -> Result<(), String> {
    condition.then_some(()).ok_or_else(|| unavailable(reason))
}

fn sha256(path: &Path) -> Result<String, String> {
    let mut file =
        File::open(path).map_err(|_| "Local E5 model assets are unreadable".to_owned())?;
    let mut digest = Sha256::new();
    let mut block = [0_u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut block)
            .map_err(|_| "Local E5 model assets are unreadable".to_owned())?;
        if read == 0 {
            break;
        }
        digest.update(&block[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp(Vec<(PathBuf, u128, u64, u128)>);

#[cfg(unix)]
fn file_identity(metadata: &std::fs::Metadata) -> u128 {
    use std::os::unix::fs::MetadataExt;
    (u128::from(metadata.dev()) << 64) | u128::from(metadata.ino())
}

#[cfg(windows)]
fn file_identity(metadata: &std::fs::Metadata) -> u128 {
    use std::os::windows::fs::MetadataExt;
    (u128::from(metadata.volume_serial_number().unwrap_or(0)) << 64)
        | u128::from(metadata.file_index().unwrap_or(0))
}

fn stamp(directory: &Path) -> Result<Stamp, String> {
    let mut result = Vec::new();
    for (name, _, _) in [WEIGHTS, TOKENIZER] {
        let path = directory.join(name);
        let metadata = path
            .metadata()
            .map_err(|_| "Local E5 model assets are unavailable".to_owned())?;
        let modified = metadata
            .modified()
            .map_err(|_| "Local E5 model assets are unavailable".to_owned())?
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| "Local E5 model assets are unavailable".to_owned())?
            .as_nanos();
        result.push((
            path.canonicalize()
                .map_err(|_| "Local E5 model assets are unavailable".to_owned())?,
            file_identity(&metadata),
            metadata.len(),
            modified,
        ));
    }
    Ok(Stamp(result))
}

fn verified_assets(directory: &Path) -> Result<(PathBuf, PathBuf, Stamp), String> {
    let before = stamp(directory)?;
    for (name, size, digest) in [WEIGHTS, TOKENIZER] {
        let path = directory.join(name);
        require(
            path.is_file()
                && path.metadata().map(|m| m.len()).ok() == Some(size)
                && sha256(&path)? == digest,
            &format!(
                "pinned {} artifact mismatch",
                if name == WEIGHTS.0 {
                    "weights"
                } else {
                    "tokenizer"
                }
            ),
        )?;
    }
    require(
        stamp(directory)? == before,
        "assets changed during verification",
    )?;
    Ok((
        directory.join(WEIGHTS.0),
        directory.join(TOKENIZER.0),
        before,
    ))
}

trait Encoder {
    fn tokens(&self, text: &str, special: bool) -> Result<Vec<u32>, String>;
    fn encode_ids(&mut self, ids: &[u32]) -> Result<Vec<f32>, String>;
}

struct TractEncoder {
    tokenizer: Tokenizer,
    model: TypedRunnableModel<TypedModel>,
    input_names: Vec<String>,
    output_index: usize,
}

impl TractEncoder {
    fn load(weights: &Path, tokenizer: &Path) -> Result<Self, String> {
        let mut tokenizer = Tokenizer::from_file(tokenizer)
            .map_err(|_| "Local E5 runtime unavailable or incompatible: tokenizer".to_owned())?;
        tokenizer
            .with_truncation(None)
            .map_err(|_| "Local E5 runtime unavailable or incompatible: tokenizer".to_owned())?;
        tokenizer.with_padding(None);
        require(
            tokenizer.token_to_id("<s>") == Some(0) && tokenizer.token_to_id("</s>") == Some(2),
            "unsupported tokenizer special tokens",
        )?;
        let model = tract_onnx::onnx()
            .model_for_path(weights)
            .and_then(|model| model.into_optimized())
            .and_then(|model| model.into_runnable())
            .map_err(|_| "Local E5 runtime unavailable or incompatible: ONNX model".to_owned())?;
        let input_names = model
            .model()
            .input_outlets()
            .map_err(|_| "Local E5 runtime unavailable or incompatible: ONNX inputs".to_owned())?
            .iter()
            .map(|outlet| {
                model
                    .model()
                    .outlet_label(*outlet)
                    .unwrap_or(&model.model().node(outlet.node).name)
                    .to_owned()
            })
            .collect::<Vec<_>>();
        require(
            input_names.iter().all(|name| {
                ["input_ids", "attention_mask", "token_type_ids"].contains(&name.as_str())
            }) && input_names.iter().any(|name| name == "input_ids"),
            "unsupported ONNX input contract",
        )?;
        for outlet in model
            .model()
            .input_outlets()
            .map_err(|_| "Local E5 runtime unavailable or incompatible: ONNX inputs".to_owned())?
        {
            let fact = model.model().outlet_fact(*outlet).map_err(|_| {
                "Local E5 runtime unavailable or incompatible: ONNX inputs".to_owned()
            })?;
            require(
                fact.datum_type == i64::datum_type(),
                "unsupported ONNX input contract",
            )?;
        }
        let outputs = model
            .model()
            .output_outlets()
            .map_err(|_| "Local E5 runtime unavailable or incompatible: ONNX outputs".to_owned())?;
        let output_index = outputs
            .iter()
            .position(|outlet| {
                model.model().outlet_label(*outlet) == Some("last_hidden_state")
                    || model.model().node(outlet.node).name == "last_hidden_state"
            })
            .ok_or_else(|| unavailable("missing token hidden states"))?;
        let encoder = Self {
            tokenizer,
            model,
            input_names,
            output_index,
        };
        let bare = encoder.tokens("passage: index check", false)?;
        let special = encoder.tokens("passage: index check", true)?;
        require(
            special == [vec![0], bare, vec![2]].concat(),
            "unsupported tokenizer postprocessor",
        )?;
        Ok(encoder)
    }
}

impl Encoder for TractEncoder {
    fn tokens(&self, text: &str, special: bool) -> Result<Vec<u32>, String> {
        self.tokenizer
            .encode(text, special)
            .map(|encoding| encoding.get_ids().to_vec())
            .map_err(|_| "Local E5 tokenizer failed".to_owned())
    }

    fn encode_ids(&mut self, ids: &[u32]) -> Result<Vec<f32>, String> {
        require(
            !ids.is_empty() && ids.len() <= TOKEN_LIMIT,
            "encoder input exceeds 512 tokens; no truncation",
        )?;
        let ids_i64 = ids.iter().map(|id| i64::from(*id)).collect::<Vec<_>>();
        let ones = vec![1_i64; ids.len()];
        let zeros = vec![0_i64; ids.len()];
        let mut inputs = TVec::new();
        for name in &self.input_names {
            let values = match name.as_str() {
                "input_ids" => &ids_i64,
                "attention_mask" => &ones,
                "token_type_ids" => &zeros,
                _ => return Err(unavailable("unsupported ONNX input contract")),
            };
            inputs.push(
                Tensor::from_shape(&[1, ids.len()], values)
                    .map_err(|_| "Local E5 encoding failed: input tensor".to_owned())?
                    .into(),
            );
        }
        let outputs = self
            .model
            .run(inputs)
            .map_err(|_| "Local E5 encoding failed: inference".to_owned())?;
        require(
            self.output_index < outputs.len(),
            "unexpected ONNX output contract",
        )?;
        let hidden = outputs[self.output_index]
            .to_array_view::<f32>()
            .map_err(|_| "Local E5 encoding failed: output tensor".to_owned())?;
        require(
            hidden.shape() == [1, ids.len(), DIMENSIONS],
            "unexpected hidden-state shape",
        )?;
        let mut pooled = vec![0.0_f32; DIMENSIONS];
        for token in 0..ids.len() {
            for dimension in 0..DIMENSIONS {
                pooled[dimension] += hidden[[0, token, dimension]];
            }
        }
        let divisor = ids.len() as f32;
        pooled.iter_mut().for_each(|value| *value /= divisor);
        let norm = pooled.iter().map(|value| value * value).sum::<f32>().sqrt();
        require(
            norm.is_finite() && norm > 0.0 && pooled.iter().all(|value| value.is_finite()),
            "invalid encoder vector",
        )?;
        pooled.iter_mut().for_each(|value| *value /= norm);
        Ok(pooled)
    }
}

type Cached = (String, Vec<(String, Vec<f32>)>);

struct State {
    encoder: Option<Box<dyn Encoder + Send>>,
    stamp: Option<Stamp>,
    cached: Option<Cached>,
}

pub struct E5Index {
    model_dir: PathBuf,
    state: Mutex<State>,
}

impl E5Index {
    pub fn new(model_dir: impl Into<PathBuf>) -> Self {
        Self {
            model_dir: model_dir.into(),
            state: Mutex::new(State {
                encoder: None,
                stamp: None,
                cached: None,
            }),
        }
    }

    fn ensure_runtime(&self, state: &mut State) -> Result<(), String> {
        if state.encoder.is_some() {
            require(
                stamp(&self.model_dir)? == state.stamp.clone().unwrap(),
                "model assets changed; create a new index",
            )?;
            return Ok(());
        }
        let (weights, tokenizer, before) = verified_assets(&self.model_dir)?;
        let encoder = TractEncoder::load(&weights, &tokenizer)?;
        require(
            stamp(&self.model_dir)? == before,
            "assets changed while loading",
        )?;
        state.encoder = Some(Box::new(encoder));
        state.stamp = Some(before);
        Ok(())
    }
}

fn validate<'a>(
    documents: &'a BTreeMap<String, String>,
    query: &str,
) -> Result<Vec<(&'a String, &'a String)>, String> {
    require(
        documents.len() <= MAX_DOCUMENTS,
        "document count exceeds 512",
    )?;
    require(!query.trim().is_empty(), "query must be nonempty text")?;
    require(
        query.len() <= MAX_QUERY_BYTES,
        "query exceeds 16 KiB; no truncation",
    )?;
    let mut total = 0;
    for (id, text) in documents {
        require(!id.is_empty(), "document IDs must be nonempty strings")?;
        require(id.len() <= MAX_ID_BYTES, "document ID exceeds 4096 bytes")?;
        require(
            text.len() <= MAX_DOCUMENT_BYTES,
            "a document exceeds 128 KiB",
        )?;
        total += text.len();
    }
    require(total <= MAX_TOTAL_BYTES, "document text exceeds 2 MiB")?;
    Ok(documents.iter().collect())
}

fn chunks(encoder: &dyn Encoder, text: &str) -> Result<Vec<Vec<u32>>, String> {
    let prefix = encoder.tokens("passage:", false)?;
    let tokens = encoder.tokens(&format!("passage: {text}"), false)?;
    require(
        !prefix.is_empty() && tokens.starts_with(&prefix),
        "passage prefix tokenization changed",
    )?;
    let body = &tokens[prefix.len()..];
    let capacity = TOKEN_LIMIT - prefix.len() - 2;
    require(capacity > OVERLAP_TOKENS, "no room for passage content")?;
    let mut result = Vec::new();
    let mut start = 0;
    loop {
        let end = (start + capacity).min(body.len());
        let mut chunk = Vec::with_capacity(prefix.len() + end - start + 2);
        chunk.push(0);
        chunk.extend_from_slice(&prefix);
        chunk.extend_from_slice(&body[start..end]);
        chunk.push(2);
        result.push(chunk);
        if end == body.len() {
            break;
        }
        start = end - OVERLAP_TOKENS;
    }
    Ok(result)
}

fn rank_with_encoder(
    encoder: &mut dyn Encoder,
    cached: &mut Option<Cached>,
    documents: &BTreeMap<String, String>,
    query: &str,
) -> Result<SemanticRanking, String> {
    let items = validate(documents, query)?;
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&items).map_err(|e| unavailable(e.to_string()))?)
    );
    let query_ids = encoder.tokens(&format!("query: {query}"), true)?;
    require(
        query_ids.len() <= TOKEN_LIMIT,
        "query exceeds 512 tokens; no truncation",
    )?;
    let reused = cached.as_ref().is_some_and(|value| value.0 == fingerprint);
    if !reused {
        let mut plan = Vec::new();
        for (id, text) in items {
            for ids in chunks(encoder, text)? {
                require(plan.len() < MAX_CHUNKS, "index exceeds 1024 chunks")?;
                plan.push((id.clone(), ids));
            }
        }
        let mut vectors = Vec::with_capacity(plan.len());
        for (id, ids) in plan {
            vectors.push((id, encoder.encode_ids(&ids)?));
        }
        *cached = Some((fingerprint.clone(), vectors));
    }
    let query_vector = (!documents.is_empty())
        .then(|| encoder.encode_ids(&query_ids))
        .transpose()?;
    let vectors = &cached.as_ref().expect("complete cache").1;
    let mut scores = BTreeMap::new();
    if let Some(query_vector) = query_vector {
        for (id, vector) in vectors {
            require(
                vector.len() == query_vector.len(),
                "unexpected encoder vector size",
            )?;
            let score = query_vector
                .iter()
                .zip(vector)
                .map(|(a, b)| f64::from(*a) * f64::from(*b))
                .sum::<f64>();
            require(score.is_finite(), "nonfinite similarity")?;
            scores
                .entry(id.clone())
                .and_modify(|old: &mut f64| *old = old.max(score))
                .or_insert(score);
        }
    }
    require(
        scores.keys().eq(documents.keys()),
        "index does not cover every document",
    )?;
    let metadata = json!({
        "document_count": documents.len(), "chunk_count": vectors.len(), "index_reused": reused,
        "content_fingerprint": fingerprint,
        "model": {"name": MODEL, "revision": REVISION, "weights_sha256": WEIGHTS.2,
            "tokenizer_sha256": TOKENIZER.2, "dimensions": DIMENSIONS, "token_limit": TOKEN_LIMIT,
            "overlap_tokens": OVERLAP_TOKENS, "query_prefix": "query: ", "passage_prefix": "passage: ",
            "pooling": "attention-mask mean", "normalization": "L2",
            "execution": "CPU, sequential, batch 1, one thread", "aggregation": "maximum chunk cosine per document",
            "runtime": {"tract-onnx": "0.22.4", "tokenizers": "0.23.2"}},
        "limits": {"documents": MAX_DOCUMENTS, "document_bytes": MAX_DOCUMENT_BYTES,
            "total_document_bytes": MAX_TOTAL_BYTES, "index_chunks": MAX_CHUNKS}
    }).as_object().unwrap().clone();
    Ok(SemanticRanking { scores, metadata })
}

impl SemanticProvider for E5Index {
    fn rank(
        &self,
        documents: &BTreeMap<String, String>,
        query: &str,
    ) -> Result<SemanticRanking, String> {
        validate(documents, query)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| unavailable("local E5 state is unavailable"))?;
        self.ensure_runtime(&mut state)?;
        let State {
            encoder, cached, ..
        } = &mut *state;
        let result = rank_with_encoder(encoder.as_deref_mut().unwrap(), cached, documents, query);
        self.ensure_runtime(&mut state)?;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        encoded: usize,
    }
    impl Encoder for Fake {
        fn tokens(&self, text: &str, special: bool) -> Result<Vec<u32>, String> {
            let mut ids = if text.starts_with("passage:") {
                vec![10]
            } else {
                vec![11]
            };
            ids.extend(
                text.bytes()
                    .skip(text.find(':').unwrap() + 1)
                    .map(u32::from),
            );
            Ok(if special {
                [vec![0], ids, vec![2]].concat()
            } else {
                ids
            })
        }
        fn encode_ids(&mut self, ids: &[u32]) -> Result<Vec<f32>, String> {
            self.encoded += 1;
            let mut vector = vec![0.0; DIMENSIONS];
            vector[usize::from(ids.contains(&(b'!' as u32)))] = 1.0;
            Ok(vector)
        }
    }

    #[test]
    fn deterministic_encoder_covers_every_document_and_reuses_only_complete_index() {
        let mut encoder = Fake { encoded: 0 };
        let mut cache = None;
        let documents =
            BTreeMap::from([("a".into(), "plain".into()), ("b".into(), "marked!".into())]);
        let first = rank_with_encoder(&mut encoder, &mut cache, &documents, "!").unwrap();
        assert_eq!(
            first.metadata["content_fingerprint"],
            "04f8233aabf17644796e8bb78c0e2d9d793f8d3ef89a349127762f555fb39559"
        );
        assert_eq!(
            first.scores.keys().collect::<Vec<_>>(),
            documents.keys().collect::<Vec<_>>()
        );
        assert!(first.scores["b"] > first.scores["a"]);
        let calls = encoder.encoded;
        let second = rank_with_encoder(&mut encoder, &mut cache, &documents, "!").unwrap();
        assert_eq!(second.metadata["index_reused"], true);
        assert_eq!(encoder.encoded, calls + 1);
    }

    #[test]
    fn oversized_query_fails_before_document_inference_or_cache_replacement() {
        let mut encoder = Fake { encoded: 0 };
        let mut cache = None;
        let original = BTreeMap::from([("a".into(), "plain".into())]);
        rank_with_encoder(&mut encoder, &mut cache, &original, "ok").unwrap();
        let cached = cache.clone();
        let calls = encoder.encoded;
        let changed = BTreeMap::from([("a".into(), "changed".into())]);
        let error =
            rank_with_encoder(&mut encoder, &mut cache, &changed, &"q".repeat(512)).unwrap_err();
        assert!(error.contains("query exceeds 512 tokens"));
        assert_eq!(encoder.encoded, calls);
        assert_eq!(cache, cached);
    }

    #[test]
    fn missing_assets_fail_before_runtime_loading() {
        let index = E5Index::new("definitely-missing-e5-assets");
        let error = index
            .rank(&BTreeMap::from([("a".into(), "text".into())]), "query")
            .unwrap_err();
        assert!(error.contains("assets are unavailable"));
    }
}
