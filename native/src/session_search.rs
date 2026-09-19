//! Source-free, revision-bound search over one validated checked-session graph.
//!
//! The caller owns graph validation, source capture, and tokenization. Search
//! only ranks supplied node bodies and emits references for a later exact read.
use base64::{Engine as _, alphabet, engine::DecodePaddingMode, engine::general_purpose};
use serde::{Deserializer as _, de};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};
use unicode_casefold::UnicodeCaseFold;

const MIN_TOKENS: usize = 64;
const MAX_TOKENS: usize = 65_536;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SearchMode {
    Lexical,
    Semantic,
    #[default]
    Hybrid,
}

impl SearchMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Lexical => "lexical",
            Self::Semantic => "semantic",
            Self::Hybrid => "hybrid",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchRequest {
    pub query: String,
    pub ids: Option<Vec<String>>,
    pub tokens: usize,
    pub limit: usize,
    pub branch: Option<String>,
    pub mode: SearchMode,
    pub cursor: Option<String>,
}

impl Default for SearchRequest {
    fn default() -> Self {
        Self {
            query: String::new(),
            ids: None,
            tokens: 1_000,
            limit: 8,
            branch: None,
            mode: SearchMode::Hybrid,
            cursor: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchResponse {
    /// Canonical UTF-8 JSON followed by the newline included in the token budget.
    pub text: String,
    pub tokens: usize,
    pub packet: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SemanticRanking {
    /// Must cover every supplied node exactly once with finite scores.
    pub scores: BTreeMap<String, f64>,
    /// Python-compatible metadata object, excluding `scores`.
    pub metadata: Map<String, Value>,
}

pub trait SemanticProvider {
    /// Return already computed local scores. Search never runs or downloads a model.
    fn rank(
        &self,
        documents: &BTreeMap<String, String>,
        query: &str,
    ) -> std::result::Result<SemanticRanking, String>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchError(pub String);

impl fmt::Display for SearchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SearchError {}

type Result<T> = std::result::Result<T, SearchError>;

#[derive(Clone, Debug)]
struct Index {
    documents: BTreeMap<String, String>,
    frequencies: BTreeMap<String, BTreeMap<String, usize>>,
    lengths: BTreeMap<String, usize>,
    document_frequency: BTreeMap<String, usize>,
    average_length: f64,
    fingerprint: String,
}

#[derive(Clone, Debug)]
struct Cursor {
    version: i64,
    offset: i64,
    binding: String,
}

#[derive(Clone, Debug)]
struct Ranked {
    hits: Vec<Value>,
    matched_candidates: usize,
    unresolved_ids: Vec<String>,
    backend: &'static str,
    fallback: Option<String>,
    semantic_index: Option<Map<String, Value>>,
    index_fingerprint: String,
    ranking_fingerprint: String,
    offset: usize,
}

fn error(message: impl Into<String>) -> SearchError {
    SearchError(message.into())
}

fn canonical(value: &Value) -> String {
    // serde_json's default Map is ordered; its compact UTF-8 spelling matches
    // json.dumps(..., ensure_ascii=False, sort_keys=True, separators=(',', ':')).
    serde_json::to_string(value).expect("validated JSON is serializable")
}

fn sha256(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn unique(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    values
        .into_iter()
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

fn python_casefold(text: &str) -> String {
    let mut folded = String::new();
    for character in text.chars() {
        // unicode-casefold 0.2 carries Unicode 9.0. These are the complete
        // single-scalar additions through Python 3.14.5's Unicode 16.0 data,
        // established by an exhaustive 0..=0x10ffff oracle sweep on 2026-09-19.
        let updated = match character as u32 {
            0x1c89 => Some(0x1c8a),
            value @ 0x1c90..=0x1cba => Some(value - 0xbc0),
            value @ 0x1cbd..=0x1cbf => Some(value - 0xbc0),
            0x2c2f => Some(0x2c5f),
            value @ (0xa7b8 | 0xa7ba | 0xa7bc | 0xa7be | 0xa7c0 | 0xa7c2 | 0xa7c7 | 0xa7c9
            | 0xa7cc | 0xa7d0 | 0xa7d6 | 0xa7d8 | 0xa7da | 0xa7f5) => Some(value + 1),
            0xa7c4 => Some(0xa794),
            0xa7c5 => Some(0x282),
            0xa7c6 => Some(0x1d8e),
            0xa7cb => Some(0x264),
            0xa7dc => Some(0x19b),
            value @ 0x10570..=0x10595 if ![0x1057b, 0x1058b, 0x10593].contains(&value) => {
                Some(value + 0x27)
            }
            value @ 0x10d50..=0x10d65 => Some(value + 0x20),
            value @ 0x16e40..=0x16e5f => Some(value + 0x20),
            _ => None,
        };
        if let Some(updated) = updated {
            folded.push(char::from_u32(updated).expect("casefold delta is a Unicode scalar"));
        } else {
            folded.extend(character.case_fold());
        }
    }
    folded
}

#[cfg(test)]
pub(crate) fn casefold_for_oracle(text: &str) -> String {
    python_casefold(text)
}

fn terms(text: &str) -> Vec<String> {
    let folded = python_casefold(text);
    let mut result = Vec::new();
    let mut current = String::new();
    for character in folded.chars() {
        if character.is_alphanumeric() {
            current.push(character);
        } else if !current.is_empty() {
            result.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        result.push(current);
    }
    result
}

fn identifier_atom(character: char) -> bool {
    !character.is_whitespace()
        && ![
            '"', '\'', '`', '(', ')', '[', ']', '{', '}', ',', ';', '<', '>', ':', '#', '/',
        ]
        .contains(&character)
}

fn mentioned_ids(text: &str, identifiers: impl Iterator<Item = String>) -> Vec<String> {
    let mut found = Vec::new();
    for identifier in identifiers {
        for (start, _) in text.match_indices(&identifier) {
            let end = start + identifier.len();
            let left = text[..start].chars().next_back();
            let right = text[end..].chars().next();
            if left.is_none_or(|character| !identifier_atom(character))
                && right.is_none_or(|character| !identifier_atom(character))
            {
                found.push((start, end, identifier.chars().count(), identifier.clone()));
            }
        }
    }
    found.sort_by(|a, b| {
        b.2.cmp(&a.2)
            .then_with(|| a.0.cmp(&b.0))
            .then_with(|| a.3.cmp(&b.3))
    });
    let mut accepted: Vec<(usize, usize, String)> = Vec::new();
    for (start, end, _, identifier) in found {
        if !accepted
            .iter()
            .any(|(other_start, other_end, _)| start < *other_end && end > *other_start)
        {
            accepted.push((start, end, identifier));
        }
    }
    accepted.sort();
    unique(accepted.into_iter().map(|(_, _, identifier)| identifier))
}

fn prepare(nodes: &BTreeMap<String, Value>) -> Result<Index> {
    let mut documents = BTreeMap::new();
    for (identifier, node) in nodes {
        let body = node
            .as_object()
            .and_then(|object| object.get("body"))
            .ok_or_else(|| error("invalid checked-session search node body"))?;
        documents.insert(
            identifier.clone(),
            format!("{identifier}\n{}", canonical(body)),
        );
    }
    let fingerprint = sha256(&canonical(&serde_json::to_value(&documents).unwrap()));
    let mut frequencies = BTreeMap::new();
    let mut lengths = BTreeMap::new();
    let mut document_frequency = BTreeMap::new();
    for (identifier, document) in &documents {
        let mut counts = BTreeMap::new();
        for term in terms(document) {
            *counts.entry(term).or_insert(0) += 1;
        }
        lengths.insert(identifier.clone(), counts.values().sum());
        for term in counts.keys() {
            *document_frequency.entry(term.clone()).or_insert(0) += 1;
        }
        frequencies.insert(identifier.clone(), counts);
    }
    let average_length = lengths.values().sum::<usize>() as f64 / lengths.len().max(1) as f64;
    Ok(Index {
        documents,
        frequencies,
        lengths,
        document_frequency,
        average_length,
        fingerprint,
    })
}

fn lexical(index: &Index, query: &str) -> BTreeMap<String, f64> {
    // Python 1.8 iterated a hash-randomized set here. Its last-bit sums and
    // ranking fingerprint can vary between processes for long queries. The
    // native runtime keeps one lexical term order so its cursors are stable.
    let query_terms: BTreeSet<_> = terms(query).into_iter().collect();
    let count_documents = index.documents.len() as f64;
    index
        .frequencies
        .iter()
        .map(|(identifier, frequency)| {
            let mut score = 0.0;
            for term in &query_terms {
                let count = *frequency.get(term).unwrap_or(&0);
                if count == 0 {
                    continue;
                }
                let document_frequency = *index.document_frequency.get(term).unwrap() as f64;
                let inverse = (1.0
                    + (count_documents - document_frequency + 0.5) / (document_frequency + 0.5))
                    .ln();
                score += inverse * count as f64 * 2.2
                    / (count as f64
                        + 1.2
                            * (0.25
                                + 0.75 * index.lengths[identifier] as f64
                                    / index.average_length.max(1.0)));
            }
            (identifier.clone(), score)
        })
        .collect()
}

fn order(scores: &BTreeMap<String, f64>, branch: &BTreeSet<String>) -> Vec<String> {
    let mut identifiers: Vec<_> = scores.keys().cloned().collect();
    identifiers.sort_by(|a, b| {
        scores[b]
            .total_cmp(&scores[a])
            .then_with(|| (!branch.contains(a)).cmp(&(!branch.contains(b))))
            .then_with(|| a.cmp(b))
    });
    identifiers
}

fn bare(value: &str) -> &str {
    if value.starts_with("node:") && !value.contains('#') {
        &value[5..]
    } else {
        value
    }
}

fn rank(
    nodes: &BTreeMap<String, Value>,
    request: &SearchRequest,
    branch_members: &BTreeSet<String>,
    offset: usize,
    semantic: Option<&dyn SemanticProvider>,
) -> Result<Ranked> {
    if request.query.chars().count() > 8_000 {
        return Err(error("query must be at most8000 characters"));
    }
    let supplied_ids = request.ids.as_deref().unwrap_or(&[]);
    if supplied_ids.len() > 64
        || supplied_ids
            .iter()
            .any(|id| id.is_empty() || id.chars().count() > 500)
    {
        return Err(error(
            "ids must contain at most64 nonempty identifier strings of at most500 characters",
        ));
    }
    if !(1..=32).contains(&request.limit) {
        return Err(error("limit must be1..32"));
    }
    if offset > nodes.len() {
        return Err(error("invalid search cursor offset"));
    }
    if request.query.trim().is_empty() && supplied_ids.is_empty() {
        return Err(error("supply a query or exact IDs"));
    }

    let index = prepare(nodes)?;
    let known: BTreeSet<_> = nodes.keys().cloned().collect();
    let requested = unique(supplied_ids.iter().cloned());
    let explicit = unique(requested.iter().map(|value| bare(value).to_owned()));
    let unresolved_ids = requested
        .iter()
        .filter(|value| !known.contains(bare(value.as_str())))
        .cloned()
        .collect();
    let mentions = mentioned_ids(&request.query, known.iter().cloned());
    let exact = unique(
        explicit
            .iter()
            .filter(|key| known.contains(*key))
            .cloned()
            .chain(mentions.iter().cloned()),
    );
    let lexical = lexical(&index, &request.query);
    let lexical_scores: BTreeMap<_, _> = lexical
        .iter()
        .filter(|(_, score)| **score > 0.0)
        .map(|(key, score)| (key.clone(), *score))
        .collect();
    let lexical_order = order(&lexical_scores, branch_members);

    let mut dense = None;
    let mut metadata = None;
    let mut fallback = None;
    if request.mode != SearchMode::Lexical && !request.query.trim().is_empty() {
        match semantic {
            None => {
                fallback =
                    Some("Local embeddings are not configured; lexical ranking used.".into());
            }
            Some(provider) => match provider.rank(&index.documents, &request.query) {
                Ok(result)
                    if result.scores.keys().eq(known.iter())
                        && result.scores.values().all(|score| score.is_finite()) =>
                {
                    dense = Some(result.scores);
                    if !result.metadata.is_empty() {
                        metadata = Some(result.metadata);
                    }
                }
                Ok(_) => {
                    fallback = Some("Local embeddings unavailable: semantic scorer did not cover the complete record with finite scores".into());
                }
                Err(message) => {
                    fallback = Some(format!("Local embeddings unavailable: {message}"));
                }
            },
        }
    }

    let (backend, scores): (&'static str, BTreeMap<String, f64>) = match dense {
        None => (
            "lexical",
            lexical_order
                .iter()
                .map(|key| (key.clone(), lexical[key]))
                .collect(),
        ),
        Some(dense) if request.mode == SearchMode::Semantic => ("semantic", dense),
        Some(dense) => {
            let mut scores = BTreeMap::new();
            for ranking in [order(&dense, branch_members), lexical_order.clone()] {
                for (position, key) in ranking.into_iter().enumerate() {
                    *scores.entry(key).or_insert(0.0) += 1.0 / (61 + position) as f64;
                }
            }
            ("hybrid", scores)
        }
    };
    let ranking = unique(exact.into_iter().chain(order(&scores, branch_members)));
    if offset > ranking.len() {
        return Err(error("search cursor offset exceeds ranked matches"));
    }
    let ranking_rows: Vec<_> = ranking
        .iter()
        .map(|key| json!([key, scores.get(key).copied().unwrap_or(0.0)]))
        .collect();
    let model = metadata
        .as_ref()
        .and_then(|value| value.get("model"))
        .cloned()
        .unwrap_or(Value::Null);
    let ranking_fingerprint = sha256(&canonical(&json!({
        "version": 1,
        "backend": backend,
        "model": model,
        "ranking": ranking_rows,
    })));
    let hits = ranking
        .iter()
        .skip(offset)
        .take(request.limit)
        .map(|key| {
            let matched = if explicit.contains(key) {
                "explicit_id"
            } else if mentions.contains(key) {
                "literal_id"
            } else {
                backend
            };
            json!({
                "id": key,
                "ref": format!("node:{key}"),
                "links_ref": format!("edges:{key}"),
                "match": matched,
                "score": scores.get(key).copied().unwrap_or(0.0),
            })
        })
        .collect();
    Ok(Ranked {
        hits,
        matched_candidates: ranking.len(),
        unresolved_ids,
        backend,
        fallback,
        semantic_index: metadata,
        index_fingerprint: index.fingerprint,
        ranking_fingerprint,
        offset,
    })
}

fn cursor_engine() -> general_purpose::GeneralPurpose {
    let config = general_purpose::GeneralPurposeConfig::new()
        .with_decode_padding_mode(DecodePaddingMode::Indifferent);
    general_purpose::GeneralPurpose::new(&alphabet::URL_SAFE, config)
}

fn continuation(offset: usize, binding: &str) -> String {
    cursor_engine()
        .encode(canonical(&json!({
            "version": 1,
            "offset": offset,
            "binding": binding,
        })))
        .trim_end_matches('=')
        .to_owned()
}

struct UniqueCursorVisitor;

impl<'de> de::Visitor<'de> for UniqueCursorVisitor {
    type Value = Cursor;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a unique-field search cursor")
    }

    fn visit_map<A>(self, mut map: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: de::MapAccess<'de>,
    {
        let mut fields: BTreeMap<String, Value> = BTreeMap::new();
        while let Some((key, value)) = map.next_entry::<String, Value>()? {
            if fields.insert(key, value).is_some() {
                return Err(de::Error::custom("duplicate cursor field"));
            }
        }
        if fields.keys().map(String::as_str).collect::<Vec<_>>() != ["binding", "offset", "version"]
        {
            return Err(de::Error::custom("invalid cursor fields"));
        }
        Ok(Cursor {
            version: fields["version"]
                .as_i64()
                .ok_or_else(|| de::Error::custom("invalid cursor version"))?,
            offset: fields["offset"]
                .as_i64()
                .ok_or_else(|| de::Error::custom("invalid cursor offset"))?,
            binding: fields["binding"]
                .as_str()
                .ok_or_else(|| de::Error::custom("invalid cursor binding"))?
                .to_owned(),
        })
    }
}

fn read_continuation(value: &str) -> Result<Cursor> {
    if !(1..=512).contains(&value.len()) {
        return Err(error("invalid search cursor"));
    }
    let raw = cursor_engine()
        .decode(value)
        .map_err(|_| error("invalid search cursor"))?;
    let mut deserializer = serde_json::Deserializer::from_slice(&raw);
    let cursor = deserializer
        .deserialize_map(UniqueCursorVisitor)
        .map_err(|_| error("invalid search cursor"))?;
    deserializer
        .end()
        .map_err(|_| error("invalid search cursor"))?;
    if cursor.version != 1
        || !(0..=i64::from(i32::MAX)).contains(&cursor.offset)
        || cursor.binding.len() != 64
        || !cursor
            .binding
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(error("invalid search cursor"));
    }
    Ok(cursor)
}

fn sanitized_semantic_index(metadata: Option<Map<String, Value>>) -> Option<Value> {
    let metadata = metadata?;
    let mut result = Map::new();
    for key in ["document_count", "chunk_count"] {
        if let Some(value) = metadata.get(key) {
            result.insert(key.into(), value.clone());
        }
    }
    let mut model = Map::new();
    if let Some(Value::Object(source)) = metadata.get("model") {
        for key in ["name", "revision", "weights_sha256", "tokenizer_sha256"] {
            if let Some(value) = source.get(key) {
                model.insert(key.into(), value.clone());
            }
        }
    }
    result.insert("model".into(), Value::Object(model));
    Some(Value::Object(result))
}

/// Rank supplied checked-session nodes and return an exact, budgeted JSON packet.
///
/// `nodes` and `groups` are the already validated graph projection. This function
/// performs no source reads, evaluation, model execution, download, or network I/O.
pub fn search_checked_session(
    project: &str,
    revision: &str,
    nodes: &BTreeMap<String, Value>,
    groups: &BTreeMap<String, BTreeSet<String>>,
    request: &SearchRequest,
    count_tokens: impl Fn(&str) -> usize,
    semantic: Option<&dyn SemanticProvider>,
) -> Result<SearchResponse> {
    if !(MIN_TOKENS..=MAX_TOKENS).contains(&request.tokens) {
        return Err(error("tokens must be 64..65536"));
    }
    let held = request
        .cursor
        .as_deref()
        .map(read_continuation)
        .transpose()?;
    let offset = held.as_ref().map_or(0, |cursor| cursor.offset as usize);
    let branch_members = match request.branch.as_ref() {
        Some(branch) => groups
            .get(branch)
            .ok_or_else(|| error("unknown branch; read the map for a declared route"))?,
        None => &BTreeSet::new(),
    };
    let mut ranked = rank(nodes, request, branch_members, offset, semantic)?;
    let binding = sha256(&canonical(&json!({
        "project": project,
        "revision": revision,
        "query": request.query,
        "ids": request.ids.as_deref().unwrap_or(&[]),
        "branch": request.branch,
        "mode": request.mode.as_str(),
        "ranking": ranked.ranking_fingerprint,
    })));
    if held
        .as_ref()
        .is_some_and(|cursor| cursor.binding != binding)
    {
        return Err(error(
            "search cursor belongs to a different search or ranking; start a fresh search",
        ));
    }

    let semantic_index = sanitized_semantic_index(ranked.semantic_index.take());
    loop {
        let returned = ranked.hits.len();
        let following = offset + returned;
        let limited = following < ranked.matched_candidates;
        let budget_omitted = request.limit.min(ranked.matched_candidates - offset) - returned;
        let mut packet = json!({
            "project": project,
            "revision": revision,
            "scope": "Ranked candidate subset of this record; scores are not evidence or claim confidence.",
            "root_ref": "/",
            "branch_hint": request.branch,
            "hits": ranked.hits,
            "matched_candidates": ranked.matched_candidates,
            "record_nodes": nodes.len(),
            "unresolved_ids": ranked.unresolved_ids,
            "requested_mode": request.mode.as_str(),
            "backend": ranked.backend,
            "fallback": ranked.fallback,
            "index_fingerprint": ranked.index_fingerprint,
            "offset": ranked.offset,
            "returned": returned,
            "limited": limited,
            "budget_omitted": budget_omitted,
            "next_offset": limited.then_some(following),
            "next_cursor": limited.then(|| continuation(following, &binding)),
            "next": "Read refs for evidence. Continue with next_cursor and the same search arguments; refine the query or read / for other candidates.",
        });
        if let Some(semantic_index) = &semantic_index {
            packet
                .as_object_mut()
                .unwrap()
                .insert("semantic_index".into(), semantic_index.clone());
        }
        let text = canonical(&packet) + "\n";
        let tokens = count_tokens(&text);
        if tokens <= request.tokens {
            return Ok(SearchResponse {
                text,
                tokens,
                packet,
            });
        }
        if ranked.hits.len() <= 1 {
            return Err(error(
                "budget cannot carry a complete search response; raise tokens",
            ));
        }
        ranked.hits.pop();
    }
}
