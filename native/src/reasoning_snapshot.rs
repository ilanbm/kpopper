//! Detached observations. Identity binds normalized inputs; authored byte
//! revisions guard concurrency separately and never grant publication authority.
use crate::{
    Error, Result,
    history_contract::*,
    history_hypotheses as HH,
    history_projection::CapturedHistory,
    history_view::{list, truth},
    identity::sha256,
    reasoning_fields::{collections, snapshot_fields},
    require,
    value::{Date, Integer, TypedValue as V},
};
use std::collections::BTreeMap;

pub const MAX_REQUEST_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_NODES: usize = 20_000;
// Even the smallest typed JSON value consumes eight bytes. Snapshot documents,
// nodes and history repeat data, so the per-object 100k visit cap is inappropriate.
const MAX_VALUES: usize = MAX_REQUEST_BYTES / 8;
fn s(value: &str) -> V {
    V::Text(value.into())
}
fn empty() -> V {
    V::Map(Map::new())
}
fn code<'a>(value: &'a V, failure: &str) -> Result<&'a Map> {
    map(value).map_err(|_| error(failure))
}
pub(crate) fn digest(value: &V) -> Result<String> {
    Ok(sha256(&serde_json::to_vec(
        &value
            .to_tagged_bounded(MAX_VALUES)
            .map_err(|_| error("limit"))?,
    )?))
}
pub(crate) fn entries(document: &V) -> Result<BTreeMap<String, (String, V)>> {
    let mut result = BTreeMap::new();
    for (collection, members) in collections(document)? {
        for (id, body) in members {
            require(!result.contains_key(&id), "duplicate_entry")?;
            result.insert(id, (collection.clone(), body));
        }
    }
    Ok(result)
}
/// What a snapshot holds a document to besides the field roles it infers from that document
/// alone and each entry held once: a bounded size. A document laid over another record is
/// held to it, and its roles are read over that record; an entry it holds twice is refused
/// where it is laid, once what that record already holds is left out.
pub(crate) fn validate_layer(document: &V) -> Result<()> {
    code(document, "invalid_snapshot")?;
    document
        .validate_bounded(MAX_VALUES)
        .map_err(|_| error("limit"))?;
    let held = collections(document)?
        .values()
        .map(|members| members.len())
        .sum::<usize>();
    require(held <= MAX_NODES, "limit")
}
fn nodes(document: &V) -> Result<V> {
    let fields = V::Map(snapshot_fields(document)?);
    let entries = entries(document)?;
    require(entries.len() <= MAX_NODES, "limit")?;
    Ok(V::Map(
        entries
            .into_iter()
            .map(|(id, (collection, body))| {
                (
                    id,
                    V::Map(Map::from([
                        ("collection".into(), s(&collection)),
                        ("body".into(), body),
                        ("fields".into(), fields.clone()),
                    ])),
                )
            })
            .collect(),
    ))
}
fn hypotheses(value: &V) -> Result<V> {
    let mut result = Map::new();
    for (name, item) in code(value, "invalid_snapshot")? {
        let item = code(item, "invalid_snapshot")?;
        let mut normalized = Map::from([
            (
                "document".into(),
                item.get("document")
                    .or_else(|| item.get("doc"))
                    .cloned()
                    .unwrap_or_else(empty),
            ),
            (
                "head".into(),
                item.get("head").cloned().unwrap_or_else(empty),
            ),
            (
                "error".into(),
                item.get("error").cloned().unwrap_or(V::Null),
            ),
        ]);
        if let Some(kind) = item.get("kind").filter(|v| truth(v)) {
            normalized.insert("kind".into(), kind.clone());
        }
        result.insert(name.clone(), V::Map(normalized));
    }
    Ok(V::Map(result))
}
fn conflicts(hypotheses: &V) -> Result<Map> {
    let mut holders: BTreeMap<String, Vec<(String, V, String)>> = BTreeMap::new();
    for (name, hypothesis) in map(hypotheses)? {
        let hypothesis = code(hypothesis, "invalid_snapshot")?;
        if hypothesis.get("error").is_some_and(truth) {
            continue;
        }
        for (id, (_, body)) in entries(&hypothesis["document"])? {
            let mut claim = &body;
            let mut lowered = None;
            if let V::Map(fields) = &body {
                for key in ["verdict", "v", "quoted", "rule", "title"] {
                    if let Some(value) = fields.get(key).filter(|v| **v != V::Null) {
                        claim = value;
                        if key == "rule" && matches!(value, V::Map(_)) {
                            lowered = crate::reasoning_language::legacy_rule(value).ok();
                        }
                        break;
                    }
                }
            }
            let fingerprint = digest(lowered.as_ref().unwrap_or(claim))?;
            holders
                .entry(id)
                .or_default()
                .push((name.clone(), body, fingerprint));
        }
    }
    Ok(holders
        .into_iter()
        .filter_map(|(id, variants)| {
            if variants.iter().all(|(_, _, d)| *d == variants[0].2) {
                return None;
            }
            Some((
                id,
                V::List(
                    variants
                        .into_iter()
                        .map(|(name, body, _)| V::List(vec![s(&name), body]))
                        .collect(),
                ),
            ))
        })
        .collect())
}

pub fn validate_authored_revision(revision: &V) -> Result<()> {
    if *revision == V::Null {
        return Ok(());
    }
    let invalid = "invalid_authored_revision";
    let revision = schema(revision, &["files", "digest"], &[]).map_err(|_| error(invalid))?;
    let files = list(&revision["files"]).map_err(|_| error(invalid))?;
    let mut previous: Option<&str> = None;
    for file in files {
        let file = code(file, invalid)?;
        let origin = file
            .get("origin")
            .and_then(|v| text(v).ok())
            .ok_or_else(|| error(invalid))?;
        require(
            !origin.is_empty() && previous.is_none_or(|p| p < origin),
            invalid,
        )?;
        previous = Some(origin);
        let status = file.get("status").and_then(|v| text(v).ok()).unwrap_or("");
        match status {
            "read" => {
                require(file.len() == 3, invalid)?;
                let hash = file.get("sha256").and_then(|v| text(v).ok()).unwrap_or("");
                require(
                    hash.len() == 64
                        && hash
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                    invalid,
                )?;
            }
            "unreadable" => {
                require(
                    file.len() == 4
                        && file.get("sha256") == Some(&V::Null)
                        && file
                            .get("error")
                            .and_then(|v| text(v).ok())
                            .is_some_and(|s| !s.is_empty()),
                    invalid,
                )?;
            }
            _ => return Err(error(invalid)),
        }
    }
    require(
        revision["digest"]
            == s(&digest(&V::Map(Map::from([(
                "files".into(),
                revision["files"].clone(),
            )])))?),
        "stale_authored_revision",
    )
}

pub fn normalize_as_of(value: &V) -> Result<V> {
    if *value == V::Null {
        return Ok(V::Null);
    }
    let invalid = || error("invalid_as_of");
    let input = text(value).map_err(|_| invalid())?;
    if input.len() == 10 {
        Date::new(input).map_err(|_| invalid())?;
        return Ok(value.clone());
    }
    require(
        input.as_bytes().get(10) == Some(&b'T')
            && input.get(..10).is_some_and(|v| Date::new(v).is_ok()),
        "invalid_as_of",
    )?;
    require(input[11..].contains(['+', '-', 'Z']), "invalid_as_of")?;
    let instant = crate::source_clock::instant(input).ok_or_else(invalid)?;
    let day = instant.div_euclid(86_400_000_000);
    let maximum = crate::source_clock::instant("9999-12-31").unwrap() / 86_400_000_000;
    require((1..=maximum).contains(&day), "invalid_as_of")?;
    let ordinal = |year, month, day| {
        crate::source_clock::instant(&format!("{year:04}-{month:02}-{day:02}")).unwrap()
            / 86_400_000_000
    };
    let (mut lo, mut hi) = (1, 10000);
    while hi - lo > 1 {
        let mid = (hi + lo) / 2;
        if ordinal(mid, 1, 1) <= day {
            lo = mid
        } else {
            hi = mid
        }
    }
    let year = lo;
    let month = (1..=12)
        .rev()
        .find(|m| ordinal(year, *m, 1) <= day)
        .unwrap();
    let date = day - ordinal(year, month, 1) + 1;
    let time = instant.rem_euclid(86_400_000_000);
    let seconds = time / 1_000_000;
    let microseconds = time % 1_000_000;
    let fraction = if microseconds == 0 {
        String::new()
    } else {
        format!(".{microseconds:06}")
    };
    Ok(s(&format!(
        "{year:04}-{month:02}-{date:02}T{:02}:{:02}:{:02}{fraction}Z",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )))
}

fn declares_history(document: &V) -> bool {
    map(document)
        .ok()
        .and_then(|m| m.get("meta"))
        .and_then(|v| map(v).ok())
        .is_some_and(|m| m.contains_key("history"))
}
fn validate_history(document: &V, context: &V, hypotheses: &V) -> Result<()> {
    let context = code(context, "invalid_snapshot")?;
    let hyps = code(hypotheses, "invalid_snapshot")?;
    if let Some(history) = context.get("history") {
        let validated = (|| -> Result<()> {
            CapturedHistory::new(document.clone(), history.clone())?;
            let (derived, index) = HH::layers(history, document)?;
            let normalized = self::hypotheses(&derived)?;
            if !map(&derived)?.is_empty() || context.contains_key("history_hypotheses") {
                require(
                    digest(context.get("history_hypotheses").unwrap_or(&V::Null))?
                        == digest(&index)?,
                    "history_hypothesis_index_mismatch",
                )?;
                for (name, expected) in map(&normalized)? {
                    require(
                        digest(hyps.get(name).unwrap_or(&V::Null))? == digest(expected)?,
                        "history_hypothesis_layer_mismatch",
                    )?;
                }
            }
            for (name, hypothesis) in hyps {
                if map(hypothesis)?
                    .get("kind")
                    .is_some_and(|v| string_is(v, HH::KIND))
                {
                    require(
                        map(&derived)?.contains_key(name),
                        "unrecorded_history_hypothesis",
                    )?;
                }
            }
            Ok(())
        })();
        validated.map_err(|e| Error(format!("invalid_history: {e}")))?;
    } else {
        require(!declares_history(document), "missing_history_context")?;
    }
    let none = empty();
    let pending = code(context.get("pending").unwrap_or(&none), "invalid_snapshot")?;
    let bundles = code(pending.get("bundles").unwrap_or(&none), "invalid_snapshot")?;
    let revisions = bundles
        .iter()
        .filter_map(|(id, bundle)| {
            map(bundle)
                .ok()
                .and_then(|m| m.get("manifest"))
                .and_then(|v| map(v).ok())
                .and_then(|m| m.get("version"))
                .filter(|v| {
                    crate::source_clock::python_equal(v, &V::Integer(Integer::new("3").unwrap()))
                })
                .map(|_| id.clone())
        })
        .collect::<std::collections::BTreeSet<_>>();
    let witnesses = code(
        context.get("history_contributions").unwrap_or(&none),
        "invalid_snapshot",
    )?;
    require(
        witnesses
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            == revisions,
        "missing_history_context",
    )?;
    let target_history = context
        .get("target")
        .and_then(|v| map(v).ok())
        .and_then(|m| m.get("snapshot"))
        .and_then(|v| map(v).ok())
        .is_some_and(|m| m.contains_key("history"));
    if !revisions.is_empty() || target_history {
        crate::history_yaml::validate_value(&V::Map(context.clone()), MAX_REQUEST_BYTES)?;
    }
    for revision in &revisions {
        let portable = &bundles[revision];
        let portable_map = map(portable)?;
        let files = history_bytes(field(portable_map, "files")?)?;
        let adapted = crate::history_bundle::validate_contribution(portable, &files)?;
        require(portable_map["revision"] == s(revision), "invalid_history")?;
        let decisions = pending
            .get("observations")
            .and_then(|v| map(v).ok())
            .and_then(|m| m.get("decisions"))
            .and_then(|v| map(v).ok());
        let state = decisions
            .and_then(|m| m.get(revision))
            .and_then(|v| map(v).ok())
            .and_then(|m| m.get("state"));
        let retired = state.is_some_and(|v| {
            ["withdrawn", "rejected", "superseded"]
                .iter()
                .any(|s| string_is(v, s))
        });
        let manifest = map(&portable_map["manifest"])?;
        let expected = V::Map(Map::from([
            (
                "artifact_revision".into(),
                map(&manifest["history"])?["revision"].clone(),
            ),
            ("projection".into(), adapted.projection().clone()),
            ("scope".into(), manifest["scope"].clone()),
            ("roots".into(), manifest["roots"].clone()),
            (
                "status".into(),
                s(if retired { "retired" } else { "active" }),
            ),
        ]));
        require(
            digest(&witnesses[revision])? == digest(&expected)?,
            "invalid_history",
        )?;
        let hypothesis = hyps.get(&format!("pending-{revision}"));
        if retired {
            require(hypothesis.is_none(), "invalid_history")?;
        } else {
            let hypothesis = hypothesis
                .and_then(|v| map(v).ok())
                .ok_or_else(|| error("invalid_history"))?;
            require(
                hypothesis
                    .get("kind")
                    .is_some_and(|v| string_is(v, "contribution"))
                    && digest(
                        hypothesis
                            .get("document")
                            .or_else(|| hypothesis.get("doc"))
                            .unwrap_or(&V::Null),
                    )? == digest(adapted.document())?,
                "invalid_history",
            )?;
        }
    }
    for (revision, portable) in bundles {
        let portable_map = map(portable)?;
        let version = map(field(portable_map, "manifest")?)?
            .get("version")
            .cloned()
            .unwrap_or(V::Null);
        require(
            ["1", "2", "3", "4"]
                .iter()
                .any(|v| crate::source_clock::python_equal(&version, &V::Integer(Integer::new(v).unwrap()))),
            "unsupported_contribution_version",
        )?;
        if !crate::source_clock::python_equal(&version, &V::Integer(Integer::new("4").unwrap())) {
            continue;
        }
        let files = history_bytes(field(portable_map, "files")?)?;
        let bundle = V::Map(Map::from([
            ("revision".into(), portable_map["revision"].clone()),
            ("manifest".into(), portable_map["manifest"].clone()),
        ]));
        let closure = crate::history_node_contribution::validate(&bundle, &files)?;
        require(portable_map["revision"] == s(revision), "invalid_history")?;
        let retired = pending
            .get("observations")
            .and_then(|v| map(v).ok())
            .and_then(|m| m.get("decisions"))
            .and_then(|v| map(v).ok())
            .and_then(|m| m.get(revision))
            .and_then(|v| map(v).ok())
            .and_then(|m| m.get("state"))
            .is_some_and(|v| {
                ["withdrawn", "rejected", "superseded"]
                    .iter()
                    .any(|name| string_is(v, name))
            });
        let hypothesis = hyps.get(&format!("pending-{revision}"));
        if retired {
            require(hypothesis.is_none(), "invalid_history")?;
        } else {
            let hypothesis = hypothesis
                .and_then(|v| map(v).ok())
                .ok_or_else(|| error("invalid_history"))?;
            require(
                hypothesis
                    .get("kind")
                    .is_some_and(|v| string_is(v, "contribution"))
                    && digest(
                        hypothesis
                            .get("document")
                            .or_else(|| hypothesis.get("doc"))
                            .unwrap_or(&V::Null),
                    )? == digest(&closure.document)?,
                "invalid_history",
            )?;
        }
    }
    for (name, hypothesis) in hyps {
        let hyp = code(hypothesis, "invalid_snapshot")?;
        let body = hyp
            .get("document")
            .or_else(|| hyp.get("doc"))
            .unwrap_or(&none);
        require(
            !declares_history(body) || revisions.iter().any(|r| *name == format!("pending-{r}")),
            "missing_history_context",
        )?;
    }
    let target = code(context.get("target").unwrap_or(&none), "invalid_snapshot")?;
    let target = code(target.get("snapshot").unwrap_or(&none), "invalid_snapshot")?;
    if let Some(evidence) = target.get("history") {
        let files = history_bytes(field(map(evidence)?, "files")?)?;
        let adapted = crate::history_bundle::validate_observation(evidence, &files)?;
        require(
            digest(field(target, "doc")?)? == digest(adapted.document())?,
            "invalid_history",
        )?;
    } else {
        require(
            !declares_history(target.get("doc").unwrap_or(&none)),
            "missing_history_context",
        )?;
    }
    Ok(())
}
pub(crate) fn history_bytes(files: &V) -> Result<crate::history_authority::Files> {
    let files = code(files, "limit")?;
    require(files.len() <= 2 * MAX_NODES + 2, "limit")?;
    let mut result = crate::history_authority::Files::new();
    let mut total = 0;
    for (path, value) in files {
        let value = schema(value, &["encoding", "data", "sha256"], &[])
            .map_err(|_| error("invalid_history"))?;
        require(string_is(&value["encoding"], "hex"), "invalid_history")?;
        let hex = text(&value["data"]).map_err(|_| error("invalid_history"))?;
        total += hex.len();
        require(total <= MAX_REQUEST_BYTES && hex.len() % 2 == 0, "limit")?;
        require(
            hex.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "invalid_history",
        )?;
        let raw = hex
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
            .collect::<Vec<_>>();
        require(value["sha256"] == s(&sha256(&raw)), "invalid_history")?;
        result.insert(path.clone(), raw);
    }
    Ok(result)
}
fn preimage(data: &Map) -> V {
    V::Map(
        data.iter()
            .filter(|(k, _)| !["snapshot_id", "authored_revision"].contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    )
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    data: V,
}
#[derive(Default)]
pub struct CaptureOptions {
    pub context: Option<V>,
    pub hypotheses: Option<V>,
    pub as_of: Option<V>,
    pub authored_revision: Option<V>,
}
impl CapturedHistory {
    /// Bind a validated retained history projection to a detached computational snapshot.
    pub fn snapshot(&self, mut options: CaptureOptions) -> Result<Snapshot> {
        let mut context = options
            .context
            .take()
            .filter(|v| *v != V::Null)
            .unwrap_or_else(empty);
        let context_map = crate::history_view::map_mut(&mut context)?;
        require(
            !context_map.contains_key("history"),
            "duplicate_history_context",
        )?;
        context_map.insert("history".into(), self.projection().clone());
        let (named, index) = HH::layers(self.projection(), self.document())?;
        if !map(&named)?.is_empty() {
            require(
                context_map
                    .get("history_hypotheses")
                    .is_none_or(|v| v.digest().ok() == index.digest().ok()),
                "history_hypothesis_index_mismatch",
            )?;
            context_map.insert("history_hypotheses".into(), index);
            let mut supplied = options
                .hypotheses
                .take()
                .filter(|v| *v != V::Null)
                .unwrap_or_else(empty);
            let current = crate::history_view::map_mut(&mut supplied)?;
            for (name, group) in map(&named)? {
                if let Some(old) = current.get(name) {
                    let wrap = |v: &V| V::Map(Map::from([(name.clone(), v.clone())]));
                    require(
                        hypotheses(&wrap(old))?.digest()? == hypotheses(&wrap(group))?.digest()?,
                        "history_hypothesis_layer_mismatch",
                    )?;
                }
                current.insert(name.clone(), group.clone());
            }
            options.hypotheses = Some(supplied);
        }
        options.context = Some(context);
        Snapshot::from_data(self.document(), options)
    }
}
impl Snapshot {
    pub fn from_data(document: &V, options: CaptureOptions) -> Result<Self> {
        code(document, "invalid_snapshot")?;
        document
            .validate_bounded(MAX_VALUES)
            .map_err(|_| error("limit"))?;
        let mut context = options
            .context
            .filter(|v| *v != V::Null)
            .unwrap_or_else(|| {
                V::Map(Map::from([
                    ("read_mode".into(), s("supplied")),
                    ("source_collection".into(), s("caller-owned")),
                ]))
            });
        let mut hypotheses = self::hypotheses(
            &options
                .hypotheses
                .filter(|v| *v != V::Null)
                .unwrap_or_else(empty),
        )?;
        let context_map =
            crate::history_view::map_mut(&mut context).map_err(|_| error("invalid_snapshot"))?;
        context_map
            .entry("read_mode".into())
            .or_insert(s("supplied"));
        if let Some(history) = context_map.get("history") {
            let (derived, index) = HH::layers(history, document)?;
            let derived = self::hypotheses(&derived)?;
            let current = crate::history_view::map_mut(&mut hypotheses)?;
            for (name, hypothesis) in map(&derived)? {
                require(
                    current.get(name).is_none_or(|v| {
                        map(v).is_ok_and(|m| m.get("kind").is_some_and(|v| string_is(v, HH::KIND)))
                    }),
                    "hypothesis_authority_collision",
                )?;
                current.entry(name.clone()).or_insert(hypothesis.clone());
            }
            if !map(&derived)?.is_empty() {
                context_map
                    .entry("history_hypotheses".into())
                    .or_insert(index);
            }
        }
        validate_history(document, &context, &hypotheses)?;
        crate::reasoning_scenario::validate(
            document,
            &context,
            &hypotheses,
            &normalize_as_of(&options.as_of.clone().unwrap_or(V::Null))?,
            None,
        )?;
        validate_mode(&context)?;
        let revision = options.authored_revision.unwrap_or(V::Null);
        validate_authored_revision(&revision)?;
        let mut derived_conflicts = conflicts(&hypotheses)?;
        if !derived_conflicts.is_empty() {
            let ctx = crate::history_view::map_mut(&mut context)?;
            if let Some(previous) = ctx.get("conflicts") {
                derived_conflicts.extend(code(previous, "invalid_snapshot")?.clone());
            }
            ctx.insert("conflicts".into(), V::Map(derived_conflicts));
        }
        let mut data = Map::from([
            ("schema_version".into(), V::Integer(Integer::new("1")?)),
            ("document".into(), document.clone()),
            ("nodes".into(), nodes(document)?),
            ("hypotheses".into(), hypotheses),
            ("context".into(), context),
            (
                "as_of".into(),
                normalize_as_of(&options.as_of.unwrap_or(V::Null))?,
            ),
            ("authored_revision".into(), revision),
        ]);
        data.insert("snapshot_id".into(), s(&digest(&preimage(&data))?));
        if map(&data["context"])?.contains_key("scenario") {
            let complete = V::Map(data.clone());
            crate::reasoning_scenario::validate(
                &data["document"],
                &data["context"],
                &data["hypotheses"],
                &data["as_of"],
                Some(&complete),
            )?;
        }
        Ok(Self { data: V::Map(data) })
    }
    pub fn from_snapshot(data: &V) -> Result<Self> {
        let m = schema(
            data,
            &[
                "schema_version",
                "document",
                "nodes",
                "hypotheses",
                "context",
                "as_of",
                "authored_revision",
                "snapshot_id",
            ],
            &[],
        )
        .map_err(|_| error("invalid_snapshot"))?;
        require(is_int(&m["schema_version"], "1"), "invalid_snapshot")?;
        code(&m["document"], "invalid_snapshot")?;
        validate_history(&m["document"], &m["context"], &m["hypotheses"])?;
        crate::reasoning_scenario::validate(
            &m["document"],
            &m["context"],
            &m["hypotheses"],
            &m["as_of"],
            Some(data),
        )?;
        validate_authored_revision(&m["authored_revision"])?;
        require(
            m["snapshot_id"] == s(&digest(&preimage(m))?),
            "stale_snapshot",
        )?;
        require(
            normalize_as_of(&m["as_of"])? == m["as_of"]
                && digest(&nodes(&m["document"])?)? == digest(&m["nodes"])?,
            "invalid_snapshot",
        )?;
        validate_mode(&m["context"])?;
        Ok(Self { data: data.clone() })
    }
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        require(bytes.len() <= MAX_REQUEST_BYTES, "limit")?;
        // Bound raw nesting before serde recursion; data depth is checked again
        // by the typed decoder. JSON objects cannot be valid typed values.
        let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
        for byte in bytes {
            if quoted {
                if escaped {
                    escaped = false
                } else if *byte == b'\\' {
                    escaped = true
                } else if *byte == b'"' {
                    quoted = false
                }
            } else {
                match byte {
                    b'"' => quoted = true,
                    b'[' | b'{' => {
                        depth += 1;
                        require(depth <= 400, "limit")?;
                    }
                    b']' | b'}' => depth = depth.saturating_sub(1),
                    _ => {}
                }
            }
        }
        let tagged = crate::json_ingress::parse_slice_bounded(
            bytes,
            crate::json_ingress::DuplicateKeys::LastWins,
            400,
        )
        .map_err(|_| error("invalid_snapshot_json"))?;
        check_digits(&tagged)?;
        let data = V::from_tagged_bounded(&tagged, MAX_VALUES).map_err(|e| {
            error(if e.0 == "value_limit" {
                "limit"
            } else {
                "invalid_snapshot_json"
            })
        })?;
        Self::from_snapshot(&data)
    }
    pub fn to_json(&self) -> Result<String> {
        let tagged = self
            .data
            .to_tagged_bounded(MAX_VALUES)
            .map_err(|_| error("limit"))?;
        check_digits(&tagged)?;
        let bytes = serde_json::to_string(&tagged)?;
        require(bytes.len() <= MAX_REQUEST_BYTES, "limit")?;
        Ok(bytes)
    }
    pub fn to_data(&self) -> V {
        self.data.clone()
    }
    pub(crate) fn data(&self) -> &V {
        &self.data
    }
    pub fn snapshot_id(&self) -> &str {
        text(&map(&self.data).unwrap()["snapshot_id"]).unwrap()
    }
}
fn validate_mode(context: &V) -> Result<()> {
    require(
        map(context)?
            .get("read_mode")
            .and_then(|v| text(v).ok())
            .is_some_and(|v| ["supplied", "live", "frozen", "captured-live"].contains(&v)),
        "invalid_snapshot",
    )
}
fn check_digits(tagged: &serde_json::Value) -> Result<()> {
    let mut pending = vec![(tagged, 0)];
    while let Some((value, depth)) = pending.pop() {
        require(depth <= 128, "limit")?;
        if let Some(parts) = value.as_array() {
            match parts.first().and_then(|v| v.as_str()) {
                Some("int") => {
                    if let Some(value) = parts.get(1).and_then(|v| v.as_str()) {
                        require(
                            value.trim_start_matches('-').chars().count() <= 4096,
                            "limit",
                        )?;
                    }
                }
                Some("list") => {
                    if let Some(children) = parts.get(1).and_then(|v| v.as_array()) {
                        pending.extend(children.iter().map(|v| (v, depth + 1)));
                    }
                }
                Some("map") => {
                    if let Some(children) = parts.get(1).and_then(|v| v.as_array()) {
                        pending.extend(
                            children
                                .iter()
                                .filter_map(|v| v.get(1))
                                .map(|v| (v, depth + 1)),
                        );
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}
