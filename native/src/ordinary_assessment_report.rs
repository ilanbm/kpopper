//! Ordinary report scope and identities. Captured knowledge is carried alongside
//! authored YAML; it is never reconstructed from an empty synthetic live ledger.
use crate::{
    Result,
    history_contract::*,
    history_view::{list, map_mut, truth},
    ordinary_assessment::{self as A, empty, get, obj, s, texts},
    ordinary_reader::Reader,
    reasoning_fields as F,
    reasoning_runtime::Runtime,
    source_capture::{CapturedSource, ReadMode},
    value::TypedValue as V,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
/// Python json.dumps(default=str, ensure_ascii=False, sort_keys=True), with its
/// default spaces. This is the ordinary revision format, not typed-history IDs.
pub fn legacy_json(value: &V) -> Result<String> {
    json_text(value, true)
}
pub fn compact_json(value: &V) -> Result<String> {
    json_text(value, false)
}
fn json_text(value: &V, spaces: bool) -> Result<String> {
    fn write(v: &V, out: &mut String, depth: usize, spaces: bool) -> Result<()> {
        crate::require(depth <= 400, "assessment nesting limit")?;
        match v {
            V::Null => out.push_str("null"),
            V::Bool(v) => out.push_str(if *v { "true" } else { "false" }),
            V::Integer(v) => out.push_str(v.as_str()),
            V::Float(v) => out.push_str(&if v.get().is_nan() {
                "NaN".into()
            } else if v.get() == f64::INFINITY {
                "Infinity".into()
            } else if v.get() == f64::NEG_INFINITY {
                "-Infinity".into()
            } else {
                crate::identity::python_float(v.get())
            }),
            V::Text(v) => out.push_str(&serde_json::to_string(v)?),
            V::Date(v) => out.push_str(&serde_json::to_string(v.as_str())?),
            V::DateTime(v) => {
                out.push_str(&serde_json::to_string(&v.as_str().replacen('T', " ", 1))?)
            }
            V::List(a) => {
                out.push('[');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        out.push_str(if spaces { ", " } else { "," })
                    }
                    write(v, out, depth + 1, spaces)?;
                }
                out.push(']');
            }
            V::Map(m) => {
                out.push('{');
                for (i, (k, v)) in m.iter().enumerate() {
                    if i > 0 {
                        out.push_str(if spaces { ", " } else { "," })
                    }
                    out.push_str(&serde_json::to_string(k)?);
                    out.push_str(if spaces { ": " } else { ":" });
                    write(v, out, depth + 1, spaces)?;
                }
                out.push('}');
            }
        }
        crate::require(out.len() <= 512 * 1024 * 1024, "assessment byte limit")
    }
    let mut out = String::new();
    write(value, &mut out, 0, spaces)?;
    Ok(out)
}
pub fn legacy_digest(value: &V) -> Result<String> {
    Ok(crate::identity::sha256(legacy_json(value)?.as_bytes()))
}
fn claim(body: &V) -> V {
    let Ok(b) = map(body) else {
        return body.clone();
    };
    for key in ["verdict", "v", "quoted", "rule", "title"] {
        if let Some(v) = b.get(key).filter(|v| **v != V::Null) {
            return if key == "rule" && matches!(v, V::Map(_)) {
                crate::reasoning_language::legacy_rule(v).unwrap_or_else(|_| v.clone())
            } else {
                v.clone()
            };
        }
    }
    body.clone()
}
fn same_claim(a: &V, b: &V) -> bool {
    if matches!(a, V::Map(_) | V::List(_)) || matches!(b, V::Map(_) | V::List(_)) {
        crate::source_clock::python_equal(a, b)
    } else {
        crate::ordinary_reader::same_legacy(a, b)
    }
}
/// Assess supplied authored data and explicit observed layers. Omitted context is
/// marked supplied/unassessed, and cannot imply a live read or branch acceptance.
pub fn assess(
    document: &V,
    hypotheses: &Map,
    context: &V,
    runtime: Option<&Runtime>,
    policy: &str,
) -> Result<V> {
    crate::require(
        [A::POLICY, "falsifiers-only/v1"].contains(&policy),
        &format!("unknown attention policy: {policy}"),
    )?;
    let context = map(context)?;
    let no_conflicts = empty();
    let conflicts = map(context.get("conflicts").unwrap_or(&no_conflicts))?;
    let layers = hypotheses
        .iter()
        .map(|(name, h)| {
            let mut h = map(h)?.clone();
            if !h.contains_key("document") {
                h.insert("document".into(), get(&h, "doc").clone());
            }
            Ok((name.clone(), V::Map(h)))
        })
        .collect::<Result<Map>>()?;
    let mut reader = Reader::new(document, runtime)?
        .with_layers(layers.clone(), conflicts.keys().cloned().collect())?;
    let entries = F::collections(document)?
        .into_iter()
        .filter(|(k, _)| k != "meta")
        .flat_map(|(_, m)| m.into_keys())
        .collect::<BTreeSet<_>>();
    reader
        .ids
        .retain(|id| entries.contains(id) || F::BUILTINS.contains(&id.as_str()));
    // infer() historically refuses non-iterable, truthy dependency declarations.
    for b in reader.raw.values().filter_map(|b| map(b).ok()) {
        if let Some(d) = b.get(text(&reader.fields["deps"])?)
            && truth(d)
            && !matches!(d, V::Text(_) | V::Map(_) | V::List(_))
        {
            return Err(error(
                "cannot interpret record structure: dependency declaration is not iterable",
            ));
        }
    }
    let mut skipped = Map::new();
    let mut checked = BTreeSet::new();
    let mut contributions = vec![];
    let mut holders = BTreeMap::<String, Vec<(String, V)>>::new();
    let mut revision_hypotheses = Map::new();
    for (name, h) in &layers {
        let h = map(h)?;
        let doc = &h["document"];
        revision_hypotheses.insert(
            name.clone(),
            obj([
                ("doc", doc.clone()),
                ("head", h.get("head").cloned().unwrap_or_else(empty)),
                ("error", get(h, "error").clone()),
            ]),
        );
        if truth(get(h, "error")) {
            skipped.insert(name.clone(), h["error"].clone());
            continue;
        }
        if string_is(get(h, "kind"), "contribution") {
            contributions.push(name.clone());
        } else {
            checked.insert(name.clone());
        }
        for (id, body) in F::collections(doc)?.values().flat_map(|m| m.iter()) {
            holders
                .entry(id.clone())
                .or_default()
                .push((name.clone(), claim(body)));
        }
    }
    let mut disputed = holders
        .iter()
        .filter(|(_, hs)| hs.len() > 1 && hs[1..].iter().any(|(_, c)| !same_claim(&hs[0].1, c)))
        .map(|(id, v)| (id.clone(), v.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut conflict_sources = Map::new();
    for (id, variants) in conflicts {
        let variants = list(variants)?
            .iter()
            .map(|v| {
                let a = list(v)?;
                crate::require(a.len() == 2, "invalid ordinary conflict")?;
                Ok((text(&a[0])?.to_owned(), claim(&a[1])))
            })
            .collect::<Result<Vec<_>>>()?;
        conflict_sources.insert(id.clone(), texts(variants.iter().map(|(n, _)| n.clone())));
        disputed.insert(id.clone(), variants);
    }
    let mut nodes = Map::new();
    for id in &reader.ids {
        let body = get(&reader.raw, id);
        let mut state = if map(body)
            .is_ok_and(|b| b.contains_key(text(&reader.fields["deps"]).unwrap_or("")))
        {
            A::judgment_state(&reader, body)?
        } else {
            obj([
                (
                    "basis",
                    obj([("status", s("not_applicable")), ("dependencies", empty())]),
                ),
                (
                    "falsifier",
                    obj([
                        ("status", s("not_applicable")),
                        ("expression", V::Null),
                        ("reads", V::List(vec![])),
                    ]),
                ),
                (
                    "integrity",
                    obj([
                        ("status", s("unassessed")),
                        ("reason", s("non_judgment_checks_not_run")),
                        ("checks", V::List(vec![])),
                        ("issues", V::List(vec![])),
                    ]),
                ),
            ])
        };
        let alternatives = holders
            .get(id)
            .into_iter()
            .flatten()
            .map(|(name, _)| obj([("hypothesis", s(name)), ("id", s(id))]))
            .collect();
        let witnesses = disputed
            .get(id)
            .into_iter()
            .flatten()
            .map(|(name, claim)| obj([("hypothesis", s(name)), ("claim", claim.clone())]))
            .collect();
        map_mut(&mut state)?.insert(
            "contention".into(),
            obj([
                (
                    "status",
                    s(if disputed.contains_key(id) {
                        "detected"
                    } else {
                        "none_detected"
                    }),
                ),
                (
                    "method",
                    s(if conflicts.contains_key(id) {
                        "readable_hypothesis_pairs_and_knowledge_identity"
                    } else {
                        "readable_hypothesis_pairs"
                    }),
                ),
                ("alternatives", V::List(alternatives)),
                ("witnesses", V::List(witnesses)),
            ]),
        );
        let order = map(body)
            .ok()
            .and_then(|b| b.get(text(&reader.fields["deps"]).unwrap_or("")))
            .and_then(|v| list(v).ok())
            .map(|a| {
                a.iter()
                    .filter_map(|v| text(v).ok().map(str::to_owned))
                    .collect::<Vec<_>>()
            });
        let attention = A::attention_ordered(&state, policy, order.as_deref())?;
        nodes.insert(
            id.clone(),
            obj([
                ("body", body.clone()),
                ("state", state),
                ("attention", attention),
            ]),
        );
    }
    let read_mode = context
        .get("read_mode")
        .cloned()
        .unwrap_or_else(|| s("supplied"));
    let target = get(context, "target");
    let target_error = get(context, "target_unavailable");
    let t = map(target).ok();
    let scope = obj([
        ("base", s("supplied_record")),
        ("hypotheses_checked", texts(checked)),
        ("hypotheses_skipped", V::Map(skipped.clone())),
        ("external_sources_fetched", V::Bool(false)),
        (
            "coverage",
            s(if !skipped.is_empty() || truth(target_error) {
                "partial"
            } else {
                "supplied_record"
            }),
        ),
        (
            "integrity_checks",
            s("listed per node; not a complete check or page verification"),
        ),
        (
            "knowledge",
            obj([
                ("read_mode", read_mode.clone()),
                ("contributions_checked", texts(contributions)),
                ("conflict_sources", V::Map(conflict_sources)),
                (
                    "target",
                    obj([
                        (
                            "status",
                            s(if truth(target_error) {
                                "unavailable"
                            } else if truth(target) {
                                "observed"
                            } else {
                                "unassessed"
                            }),
                        ),
                        ("ref", t.map(|t| get(t, "ref").clone()).unwrap_or(V::Null)),
                        (
                            "revision",
                            t.map(|t| get(t, "revision").clone()).unwrap_or(V::Null),
                        ),
                        ("reason", target_error.clone()),
                    ]),
                ),
            ]),
        ),
    ]);
    let knowledge = obj([
        ("read_mode", read_mode),
        ("conflicts", V::Map(conflicts.clone())),
        ("target", target.clone()),
        ("target_unavailable", target_error.clone()),
    ]);
    let revision = legacy_digest(&obj([
        ("record", document.clone()),
        ("hypotheses", V::Map(revision_hypotheses)),
        ("knowledge_context", s(&knowledge.digest()?)),
    ]))?;
    let states = nodes
        .iter()
        .map(|(id, n)| Ok((id.clone(), field(map(n)?, "state")?.clone())))
        .collect::<Result<Map>>()?;
    let assessment_revision = legacy_digest(&obj([
        ("record_revision", s(&revision)),
        ("profile", s(A::PROFILE)),
        ("scope", scope.clone()),
        ("states", V::Map(states)),
    ]))?;
    Ok(obj([
        (
            "schema_version",
            V::Integer(crate::value::Integer::new("1")?),
        ),
        ("assessment_profile", s(A::PROFILE)),
        ("attention_policy", s(policy)),
        ("record_revision", s(&revision)),
        ("scope", scope),
        ("nodes", V::Map(nodes)),
        ("assessment_revision", s(&assessment_revision)),
    ]))
}
pub fn from_capture(
    capture: &CapturedSource,
    runtime: Option<&Runtime>,
    policy: &str,
) -> Result<V> {
    let report = assess(
        &capture.document(),
        map(capture.hypotheses())?,
        &capture.ordinary_context(),
        runtime,
        policy,
    )?;
    capture.verify()?;
    Ok(report)
}
pub fn load(
    paths: &[PathBuf],
    cwd: &Path,
    mode: ReadMode,
    runtime: Option<&Runtime>,
    policy: &str,
) -> Result<V> {
    let capture =
        crate::source_capture::capture_source_with_runtime(paths, cwd, mode, None, runtime)?;
    from_capture(&capture, runtime, policy)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn differences(a: &V, b: &V, path: &str, out: &mut Vec<String>) {
        if a == b {
            return;
        }
        match (a, b) {
            (V::Map(a), V::Map(b)) if a.keys().eq(b.keys()) => {
                for k in a.keys() {
                    differences(&a[k], &b[k], &format!("{path}.{k}"), out)
                }
            }
            (V::List(a), V::List(b)) if a.len() == b.len() => {
                for (i, (a, b)) in a.iter().zip(b).enumerate() {
                    differences(a, b, &format!("{path}[{i}]"), out)
                }
            }
            _ => out.push(format!("{path}: {a:?} != {b:?}")),
        }
    }
    #[test]
    fn complete_ordinary_assessments_match_final_python() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/ordinary-assessment.json"))
                .unwrap();
        let cache = tempfile::tempdir().unwrap();
        let runtime = crate::history_authoring::tests::runtime(cache.path())
            .with_ordinary_program(crate::ordinary_reader::tests::program());
        let mut failures = vec![];
        for c in fixture["cases"].as_array().unwrap() {
            let doc = V::from_tagged(&c["document"]).unwrap();
            let layers = V::from_tagged(&c["layers"]).unwrap();
            let context = V::from_tagged(&c["context"]).unwrap();
            for policy in [A::POLICY, "falsifiers-only/v1"] {
                let actual = assess(
                    &doc,
                    map(&layers).unwrap(),
                    &context,
                    Some(&runtime),
                    policy,
                )
                .unwrap();
                let expected = V::from_tagged(&c["reports"][policy]).unwrap();
                differences(
                    &actual,
                    &expected,
                    &format!("{} {policy}", c["name"]),
                    &mut failures,
                );
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}
