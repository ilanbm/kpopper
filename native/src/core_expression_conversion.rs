//! Detached conversion reports; this module never reads or writes a source file.
use crate::{
    Error, Result,
    history_contract::{Map, map, text},
    history_view::map_mut,
    ordinary_reader as O, reasoning_assessment, reasoning_authoring as A,
    reasoning_evaluate::Evaluator,
    reasoning_fields as F, reasoning_language as L,
    reasoning_runtime::{OperationalBounds, Runtime},
    reasoning_snapshot::{CaptureOptions, Snapshot, entries},
    value::TypedValue as V,
};
use serde_json::{Value as J, json};
use std::collections::{BTreeMap, BTreeSet};
static ISO_DATE: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"^\s*\d{4}-\d{2}-\d{2}\s*$").unwrap());
pub(super) const TRANSFORMATION: &str = "legacy-to-core/v1";
pub(super) fn s(v: &str) -> V {
    V::Text(v.into())
}
pub(super) fn empty() -> V {
    V::Map(Map::new())
}
pub(super) fn obj(items: impl IntoIterator<Item = (&'static str, V)>) -> V {
    V::Map(items.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
fn val(j: J) -> Result<V> {
    V::from_json(&j)
}
fn get<'a>(v: &'a V, k: &str) -> &'a V {
    if let V::Map(m) = v {
        m.get(k).unwrap_or(&V::Null)
    } else {
        &V::Null
    }
}
fn list(v: &V) -> &[V] {
    if let V::List(a) = v { a } else { &[] }
}
fn append(report: &mut V, key: &str, v: V) -> Result<()> {
    if let V::List(a) = map_mut(report)?.get_mut(key).unwrap() {
        a.push(v);
    }
    Ok(())
}
pub(super) fn world(
    document: &V,
    template: &Snapshot,
    hypotheses: Option<V>,
    context: Option<V>,
) -> Result<Snapshot> {
    let data = template.to_data();
    Snapshot::from_data(
        document,
        CaptureOptions {
            context: Some(context.unwrap_or_else(|| get(&data, "context").clone())),
            hypotheses: Some(hypotheses.unwrap_or_else(|| get(&data, "hypotheses").clone())),
            as_of: Some(get(&data, "as_of").clone()),
            authored_revision: None,
        },
    )
}
fn report(source: &Snapshot, document: &V) -> Result<V> {
    let mut r = val(
        json!({"version":1,"transformation":TRANSFORMATION,"source_snapshot_id":source.snapshot_id(),"candidate_snapshot_id":null,"changes":[],"blockers":[],"problems":[],"fired":[],"preserved":[],"comparisons":[],"legacy_runtime":{"profile":"ordinary-reader/v1","status":"not_checked"},"complete":false}),
    )?;
    map_mut(&mut r)?.insert("document".into(), document.clone());
    Ok(r)
}
fn block(
    report: &mut V,
    code: &str,
    detail: &str,
    id: Option<&str>,
    field: Option<&str>,
) -> Result<()> {
    let mut item = obj([("code", s(code)), ("detail", s(detail))]);
    if let Some(id) = id {
        map_mut(&mut item)?.insert("id".into(), s(id));
    }
    if let Some(field) = field {
        map_mut(&mut item)?.insert("field".into(), s(field));
    }
    if !list(get(report, "blockers")).contains(&item) {
        append(report, "blockers", item)?;
        append(
            report,
            "problems",
            s(&format!(
                "{}{detail}",
                id.map(|id| format!("{id}: ")).unwrap_or_default()
            )),
        )?;
    }
    Ok(())
}
fn finish(mut report: V) -> Result<V> {
    let names = list(get(&report, "fired"))
        .iter()
        .filter_map(|v| text(v).ok())
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    let complete = list(get(&report, "blockers")).is_empty();
    let m = map_mut(&mut report)?;
    m.insert(
        "fired".into(),
        V::List(names.iter().map(|id| s(id)).collect()),
    );
    m.insert("complete".into(), V::Bool(complete));
    Ok(report)
}
fn builtin(id: &str) -> bool {
    F::BUILTINS.contains(&id)
}
pub(super) fn implicit(body: &Map, ids: &BTreeSet<String>) -> bool {
    matches!(body.get("v"),Some(V::Text(v)) if O::EXPR.is_match(v)&&O::ID.find_iter(v).any(|r|ids.contains(r.as_str())))
}
fn scalar_type(
    tree: &J,
    bodies: &BTreeMap<String, (String, V)>,
    visiting: &BTreeSet<String>,
) -> Result<Option<&'static str>> {
    if let Some(id) = tree.get("ref").and_then(J::as_str) {
        if visiting.contains(id) {
            return Ok(None);
        }
        let Some((_, body)) = bodies.get(id) else {
            return Ok(None);
        };
        let mut v = body;
        if let V::Map(m) = body {
            if let Some(rule @ V::Map(_)) = m.get("rule") {
                let mut visiting = visiting.clone();
                visiting.insert(id.into());
                return scalar_type(&L::lower(rule)?, bodies, &visiting);
            }
            v = m.get("v").or_else(|| m.get("quoted")).unwrap_or(&V::Null);
        }
        return Ok(Some(match v {
            V::Bool(_) => "boolean",
            V::Integer(_) | V::Float(_) => "number",
            V::Text(_) => "text",
            V::Null => "null",
            _ => "unsupported",
        }));
    }
    for (key, kind) in [
        ("text", "text"),
        ("num", "number"),
        ("bool", "boolean"),
        ("null", "null"),
    ] {
        if tree.get(key).is_some() {
            return Ok(Some(kind));
        }
    }
    let kinds = tree
        .get("args")
        .and_then(J::as_array)
        .map(|a| {
            a.iter()
                .map(|v| scalar_type(v, bodies, visiting))
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
    if ["add", "sub", "mul", "div"].contains(&tree["op"].as_str().unwrap_or("")) {
        if kinds
            .iter()
            .flatten()
            .any(|k| ["text", "boolean", "null", "unsupported"].contains(k))
        {
            return Err(Error(
                "text, boolean or nonscalar arithmetic requires explicit typed reconciliation"
                    .into(),
            ));
        }
        Ok(Some("number"))
    } else {
        Ok(Some("boolean"))
    }
}
fn ordered<'a>(entries: &'a BTreeMap<String, (String, V)>, order: &'a [String]) -> Vec<&'a String> {
    let mut seen = BTreeSet::new();
    order
        .iter()
        .chain(entries.keys())
        .filter(|id| entries.contains_key(*id) && seen.insert((*id).clone()))
        .collect()
}
fn error_code(error: &Error) -> &str {
    if error.0.starts_with("unsupported_capability") {
        "unsupported_capability"
    } else if error.0.starts_with("invalid_capability") {
        "invalid_capability"
    } else {
        "invalid_document"
    }
}
pub(super) fn convert(
    snapshot: &Snapshot,
    runtime: Option<&Runtime>,
    order: &[String],
) -> Result<V> {
    let data = snapshot.to_data();
    let source = get(&data, "document");
    let mut report = report(snapshot, source)?;
    let (declared, fields) = match (F::capabilities(source, None), F::snapshot_fields(source)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            block(&mut report, error_code(&e), &e.0, None, None)?;
            return finish(report);
        }
    };
    let mut candidate = source.clone();
    if get(source, "meta") != &V::Null && !matches!(get(source, "meta"), V::Map(_)) {
        block(
            &mut report,
            "invalid_metadata",
            "metadata must be a mapping before explicit core conversion",
            None,
            None,
        )?;
        return finish(report);
    }
    let declaration = val(json!({"version":2,"profile":"core/v1","requires":["arithmetic/v1"]}))?;
    map_mut(
        map_mut(&mut candidate)?
            .entry("meta".into())
            .or_insert_with(empty),
    )?
    .insert("reasoning".into(), declaration);
    if get(&declared, "profile") == &s("core/v1") {
        map_mut(
            map_mut(map_mut(&mut candidate)?.get_mut("meta").unwrap())?
                .get_mut("reasoning")
                .unwrap(),
        )?
        .insert("requires".into(), get(&declared, "requires").clone());
    }
    let all = entries(source)?;
    let ids = all.keys().cloned().collect::<BTreeSet<_>>();
    let mut implicit_trees = vec![];
    let deps_field = text(&fields["deps"])?;
    let predicate_field = text(&fields["predicate"])?;
    for id in ordered(&all, order) {
        let (collection, body) = &all[id];
        let Ok(body) = map(body) else { continue };
        let deps = body.get(deps_field).unwrap_or(&V::Null);
        if list(deps).iter().filter_map(|v| text(v).ok()).any(builtin) {
            block(
                &mut report,
                "unsupported_core_builtin",
                "computed builtin dependencies are outside core/v1",
                Some(id),
                Some(deps_field),
            )?;
        }
        let mut candidates = vec![("rule", false), (predicate_field, true)];
        if !body.contains_key("rule") && implicit(body, &ids) {
            candidates.push(("v", false));
        }
        for (field, predicate) in candidates {
            let value = body.get(field).unwrap_or(&V::Null);
            if value == &V::Null || value == &s("") {
                continue;
            }
            let result = (|| -> Result<Option<J>> {
                if matches!(value, V::Map(_)) {
                    let tree = L::lower(value)?;
                    if get(&declared, "profile") != &s("core/v1")
                        && L::legacy_expression_detailed(value, predicate)?.to_json()? != tree
                    {
                        return Err(Error(
                            "structured expression is outside the characterized shared subset"
                                .into(),
                        ));
                    }
                    if L::references(&tree).iter().any(|id| builtin(id)) {
                        return Err(Error("computed builtin input is outside core/v1".into()));
                    }
                    return Ok(None);
                }
                let V::Text(source) = value else {
                    return Err(Error(
                        "executable fields need text or a supported expression mapping".into(),
                    ));
                };
                let tree = match super::authored_expression(source, predicate)
                    .and_then(|tree| L::lower(&tree))
                {
                    Ok(tree) => tree,
                    Err(e) => {
                        if ISO_DATE.is_match(source)
                            || (!O::EXPR.is_match(source) && !O::CMP.is_match(source))
                        {
                            append(
                                &mut report,
                                "preserved",
                                obj([
                                    ("collection", s(collection)),
                                    ("id", s(id)),
                                    ("field", s(field)),
                                    ("reason", s("qualitative_prose")),
                                    ("value", value.clone()),
                                ]),
                            )?;
                            return Ok(None);
                        }
                        return Err(e);
                    }
                };
                let refs = L::references(&tree);
                if refs.iter().any(|id| builtin(id)) {
                    return Err(Error("computed builtin input is outside core/v1".into()));
                }
                let unknown = refs
                    .iter()
                    .filter(|id| !ids.contains(*id))
                    .cloned()
                    .collect::<BTreeSet<_>>();
                if !unknown.is_empty() {
                    return Err(Error(format!(
                        "unknown or ambiguous reference: {}",
                        unknown.into_iter().collect::<Vec<_>>().join(", ")
                    )));
                }
                if predicate && refs.iter().any(|id| !list(deps).contains(&s(id))) {
                    return Err(Error(
                        "predicate references are not declared dependencies".into(),
                    ));
                }
                if field == "rule" && (body.contains_key("v") || body.contains_key("quoted")) {
                    return Err(Error(
                        "a stored reading and a calculation require explicit reconciliation".into(),
                    ));
                }
                Ok(Some(tree))
            })();
            match result {
                Ok(Some(tree)) => {
                    let target = if field == "v" { "rule" } else { field };
                    let replacement = obj([("expr", value.clone())]);
                    let body = map_mut(
                        map_mut(map_mut(&mut candidate)?.get_mut(collection).unwrap())?
                            .get_mut(id)
                            .unwrap(),
                    )?;
                    if target != field {
                        body.remove(field);
                    }
                    body.insert(target.into(), replacement.clone());
                    append(
                        &mut report,
                        "changes",
                        obj([
                            ("collection", s(collection)),
                            ("id", s(id)),
                            ("field", s(field)),
                            ("target", s(target)),
                            ("before", value.clone()),
                            ("after", replacement),
                        ]),
                    )?;
                    implicit_trees.push((id.clone(), field.to_owned(), tree, predicate));
                }
                Ok(None) => {}
                Err(e) => {
                    let code = if e.0 == "computed builtin input is outside core/v1" {
                        "unsupported_core_builtin"
                    } else {
                        "unsupported_conversion"
                    };
                    block(&mut report, code, &e.0, Some(id), Some(field))?;
                }
            }
        }
    }
    let bodies = entries(&candidate)?;
    for (id, field, tree, predicate) in implicit_trees {
        let result = (|| -> Result<()> {
            scalar_type(&tree, &bodies, &BTreeSet::new())?;
            if predicate {
                let args = tree["args"].as_array().unwrap();
                let kinds = args
                    .iter()
                    .map(|v| scalar_type(v, &bodies, &BTreeSet::new()))
                    .collect::<Result<Vec<_>>>()?;
                if kinds.contains(&Some("text"))
                    && (kinds.contains(&Some("number"))
                        || args.iter().all(|v| v.get("ref").is_some()))
                {
                    return Err(Error(
                        "text comparison requires explicit typed reconciliation".into(),
                    ));
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            block(
                &mut report,
                "coercive_expression",
                &e.0,
                Some(&id),
                Some(&field),
            )?;
        }
    }
    match A::declare_document(&candidate) {
        Ok(doc) => candidate = doc,
        Err(e) => block(&mut report, "invalid_expression", &e.0, None, None)?,
    };
    map_mut(&mut report)?.insert("document".into(), candidate.clone());
    let compared = compare(
        snapshot,
        &world(&candidate, snapshot, None, None)?,
        runtime,
        order,
    )?;
    for key in [
        "candidate_snapshot_id",
        "comparisons",
        "legacy_runtime",
        "fired",
    ] {
        map_mut(&mut report)?.insert(key.into(), get(&compared, key).clone());
    }
    for item in list(get(&compared, "blockers")) {
        block(
            &mut report,
            text(get(item, "code"))?,
            text(get(item, "detail"))?,
            text(get(item, "id")).ok(),
            text(get(item, "field")).ok(),
        )?;
    }
    finish(report)
}
fn typed(value: &V, legacy_numeric: bool) -> Result<Option<V>> {
    if value == &V::Null {
        return Ok(Some(obj([("type", s("null"))])));
    }
    if legacy_numeric && let Some((n, d)) = crate::ordinary_assessment::number(value) {
        return Ok(Some(obj([
            ("type", s("number")),
            ("numerator", s(&n.to_string())),
            ("denominator", s(&d.to_string())),
        ])));
    }
    let result = match value {
        V::List(a) => {
            let mut items = vec![];
            for v in a {
                let Some(v) = typed(v, false)? else {
                    return Ok(None);
                };
                items.push(v);
            }
            obj([("type", s("list")), ("items", V::List(items))])
        }
        V::Map(m) => {
            let mut fields = Map::new();
            for (k, v) in m {
                let Some(v) = typed(v, false)? else {
                    return Ok(None);
                };
                fields.insert(k.clone(), v);
            }
            obj([("type", s("record")), ("fields", V::Map(fields))])
        }
        V::Bool(_) => obj([("type", s("boolean")), ("value", value.clone())]),
        V::Text(_) => obj([("type", s("text")), ("value", value.clone())]),
        _ => {
            let Some((n, d)) = crate::ordinary_assessment::number(value) else {
                return Ok(None);
            };
            obj([
                ("type", s("number")),
                ("numerator", s(&n.to_string())),
                ("denominator", s(&d.to_string())),
            ])
        }
    };
    Ok(Some(result))
}
fn assessment_json(value: &V) -> Result<J> {
    Ok(match value {
        V::Map(m) => J::Object(
            m.iter()
                .map(|(k, v)| Ok((k.clone(), assessment_json(v)?)))
                .collect::<Result<_>>()?,
        ),
        V::List(a) => J::Array(a.iter().map(assessment_json).collect::<Result<_>>()?),
        _ => value
            .to_json()
            .unwrap_or(json!({"typed":value.to_tagged()?})),
    })
}
#[allow(clippy::too_many_arguments)]
fn legacy_observation(
    raw: &Map,
    ids: &BTreeSet<String>,
    id: &str,
    body: &V,
    field: &str,
    predicate: bool,
    probe: &V,
    runtime: Option<&Runtime>,
) -> Result<V> {
    let program = runtime.and_then(Runtime::ordinary_program);
    let value = if predicate {
        O::evaluate(get(body, field), raw, ids, program)?
            .map(V::Bool)
            .unwrap_or(V::Null)
    } else {
        O::value_of(raw, ids, id, program)?
    };
    if value != V::Null {
        let value = typed(&value, predicate || field == "rule")?;
        return Ok(obj([
            (
                "status",
                s(if value.is_some() {
                    "known"
                } else {
                    "unrepresentable"
                }),
            ),
            ("value", value.unwrap_or(V::Null)),
        ]));
    }
    let explicit = field != "collection_scope" && matches!(get(body, field), V::Map(_));
    let dependent = if predicate {
        O::predicate_refs(get(body, field))
            .iter()
            .any(|id| matches!(raw.get(id).map(|v| get(v, "rule")), Some(V::Map(_))))
    } else {
        explicit
    };
    Ok(obj([
        (
            "status",
            s(
                if (explicit || dependent) && get(probe, "status") == &s("unavailable") {
                    "comparison_unavailable"
                } else {
                    "unavailable"
                },
            ),
        ),
        ("value", V::Null),
    ]))
}
fn result(
    report: &mut V,
    id: &str,
    field: &str,
    before: &V,
    after: &J,
    blocked: bool,
    required: bool,
) -> Result<()> {
    let status = after["status"].as_str().unwrap_or("unknown");
    let after_value = if status == "ok" {
        V::from_json(&after["value"])?
    } else {
        V::Null
    };
    let comparison = match text(get(before, "status"))? {
        "known" => {
            if status == "ok" && get(before, "value") == &after_value {
                "same"
            } else {
                block(
                    report,
                    "result_changed",
                    "conversion changes a known type, value or availability",
                    Some(id),
                    Some(field),
                )?;
                "changed"
            }
        }
        "unrepresentable" => {
            block(
                report,
                "unsupported_value",
                "known legacy value is outside supported core scalar types",
                Some(id),
                Some(field),
            )?;
            "unrepresentable"
        }
        "comparison_unavailable" => "comparison_unavailable",
        _ => {
            if status == "ok" {
                "newly_executable"
            } else {
                "unavailable"
            }
        }
    };
    if status == "operational_error" {
        block(
            report,
            "operational_error",
            "packaged core runtime could not validate the candidate",
            Some(id),
            Some(field),
        )?;
    } else if required && !A::admitted(after, blocked) {
        block(
            report,
            "unavailable_core_result",
            "required core result is unavailable without a supported declared hole",
            Some(id),
            Some(field),
        )?;
    }
    append(
        report,
        "comparisons",
        obj([
            ("id", s(id)),
            ("field", s(field)),
            ("status", s(comparison)),
            ("before", before.clone()),
            (
                "after",
                obj([
                    ("status", s(status)),
                    ("value", after_value),
                    (
                        "diagnostics",
                        V::from_json(after.get("diagnostics").unwrap_or(&json!([])))?,
                    ),
                ]),
            ),
        ]),
    )
}
pub(super) fn compare(
    source: &Snapshot,
    candidate: &Snapshot,
    runtime: Option<&Runtime>,
    order: &[String],
) -> Result<V> {
    let old_data = source.to_data();
    let new_data = candidate.to_data();
    let old = get(&old_data, "document");
    let new = get(&new_data, "document");
    let mut report = report(source, new)?;
    map_mut(&mut report)?.insert("candidate_snapshot_id".into(), s(candidate.snapshot_id()));
    let validation = (|| -> Result<_> {
        let old_cap = F::capabilities(old, None)?;
        let new_cap = F::capabilities(new, None)?;
        if get(&new_cap, "profile") != &s("core/v1") {
            return Err(Error("candidate must explicitly declare core/v1".into()));
        }
        crate::reasoning_capabilities::document_capabilities(new)?;
        Ok((
            old_cap,
            F::snapshot_fields(old)?,
            F::snapshot_fields(new)?,
            entries(old)?,
            entries(new)?,
        ))
    })();
    let (old_cap, old_fields, new_fields, old_entries, new_entries) = match validation {
        Ok(v) => v,
        Err(e) => {
            block(&mut report, error_code(&e), &e.0, None, None)?;
            return finish(report);
        }
    };
    if old_fields != new_fields {
        block(
            &mut report,
            "field_roles_changed",
            "conversion must preserve captured field-role mappings",
            None,
            None,
        )?;
    }
    let raw = old_entries
        .iter()
        .map(|(id, (_, body))| (id.clone(), body.clone()))
        .collect::<Map>();
    let ids = raw.keys().cloned().collect();
    let old_assessment = if get(&old_cap, "profile") == &s("core/v1") {
        map_mut(&mut report)?.insert(
            "legacy_runtime".into(),
            obj([("profile", s("core/v1")), ("status", s("not_applicable"))]),
        );
        Some(assessment_json(&reasoning_assessment::assess(
            source,
            None,
            "focused-review/v1",
            runtime,
            OperationalBounds::default(),
        )?)?)
    } else {
        let probe = O::compute(
            &raw,
            &ids,
            None,
            runtime.and_then(Runtime::ordinary_program),
        );
        let mut observation = obj([
            ("profile", get(&old_cap, "profile").clone()),
            (
                "status",
                s(if probe.get("error").is_some() {
                    "unavailable"
                } else {
                    "available"
                }),
            ),
        ]);
        if let Some(error) = probe.get("error") {
            map_mut(&mut observation)?.insert("reason".into(), V::from_json(error)?);
        } else {
            let identity = Map::from([
                (
                    "core_source_sha256".into(),
                    s(env!("KPOP_ORDINARY_SOURCE_SHA256")),
                ),
                (
                    "expression_source_sha256".into(),
                    s(&crate::identity::sha256(include_bytes!(
                        "../../scripts/expressions.py"
                    ))),
                ),
            ]);
            map_mut(&mut observation)?.insert("identity".into(), V::Map(identity));
        }
        map_mut(&mut report)?.insert("legacy_runtime".into(), observation);
        None
    };
    let assessed = match reasoning_assessment::assess(
        candidate,
        None,
        "focused-review/v1",
        runtime,
        OperationalBounds::default(),
    ) {
        Ok(v) => assessment_json(&v)?,
        Err(e) => {
            block(&mut report, error_code(&e), &e.0, None, None)?;
            return finish(report);
        }
    };
    let mut old_engine = Evaluator::new(source, runtime, None, OperationalBounds::default())?;
    let mut new_engine = Evaluator::new(candidate, runtime, None, OperationalBounds::default())?;
    for id in ordered(&old_entries, order) {
        let (_, body) = &old_entries[id];
        let Some((_, target)) = new_entries.get(id) else {
            block(
                &mut report,
                "missing_entry",
                "candidate omits a captured entry",
                Some(id),
                None,
            )?;
            continue;
        };
        let snapshot_field = text(&old_fields["snapshot"])?;
        if matches!(body, V::Map(_)) && get(body, snapshot_field) != get(target, snapshot_field) {
            block(
                &mut report,
                "history_changed",
                "profile conversion must preserve original historical evidence",
                Some(id),
                Some(snapshot_field),
            )?;
        }
        let node = &assessed["nodes"][id];
        let blocked = !A::blocked_text(target).is_empty();
        let value_node = map(body).is_err()
            || ["v", "quoted", "rule", "collection_scope"]
                .iter()
                .any(|k| map(body).is_ok_and(|m| m.contains_key(*k)));
        if value_node {
            let field = if map(body).is_ok_and(|m| m.contains_key("collection_scope")) {
                "collection_scope"
            } else if map(body).is_ok_and(|m| m.contains_key("rule")) {
                "rule"
            } else {
                "v"
            };
            let mut after = node["computation"].clone();
            if after.is_null() && map(target).is_ok_and(|m| m.contains_key("collection_scope")) {
                after = reasoning_assessment::dependency_result(candidate, id, &mut new_engine)?;
            }
            if after.is_null() {
                after = json!({"status":"unknown","value":null,"diagnostics":[]});
            }
            let before = if let Some(old) = &old_assessment {
                let mut result = old["nodes"][id]["computation"].clone();
                if result.is_null() && map(body).is_ok_and(|m| m.contains_key("collection_scope")) {
                    result = reasoning_assessment::dependency_result(source, id, &mut old_engine)?;
                }
                val(
                    json!({"status":if result["status"]=="ok"{"known"}else{"unavailable"},"value":result["value"]}),
                )?
            } else {
                legacy_observation(
                    &raw,
                    &ids,
                    id,
                    body,
                    field,
                    false,
                    get(&report, "legacy_runtime"),
                    runtime,
                )?
            };
            let required = matches!(get(target, "rule"), V::Map(_))
                || map(target).is_ok_and(|m| m.contains_key("collection_scope"));
            result(&mut report, id, field, &before, &after, blocked, required)?;
        }
        let field = text(&old_fields["predicate"])?;
        if map(body).is_ok_and(|m| m.contains_key(field)) {
            let mut after = node["state"]["falsifier"]["computation"].clone();
            if after.is_null() {
                after = json!({"status":"unknown","value":null,"diagnostics":[]});
            }
            let before = if let Some(old) = &old_assessment {
                let result = &old["nodes"][id]["state"]["falsifier"]["computation"];
                val(
                    json!({"status":if result["status"]=="ok"{"known"}else{"unavailable"},"value":result["value"]}),
                )?
            } else {
                legacy_observation(
                    &raw,
                    &ids,
                    id,
                    body,
                    field,
                    true,
                    get(&report, "legacy_runtime"),
                    runtime,
                )?
            };
            let required = matches!(get(target, text(&new_fields["predicate"])?), V::Map(_));
            result(&mut report, id, field, &before, &after, blocked, required)?;
            if after["status"] == "ok"
                && after["value"] == json!({"type":"boolean","value":true})
                && get(&before, "value") != &val(json!({"type":"boolean","value":true}))?
            {
                append(&mut report, "fired", s(id))?;
            }
        }
        if let Some(dependencies) = node["state"]["basis"]["dependencies"].as_object() {
            for (dependency, finding) in dependencies {
                if !new_entries.contains_key(dependency) {
                    result(
                        &mut report,
                        id,
                        text(&new_fields["deps"])?,
                        &obj([("status", s("unavailable")), ("value", V::Null)]),
                        &finding["computation"],
                        blocked,
                        true,
                    )?;
                }
            }
        }
        if let Some(issues) = node["state"]["integrity"]["issues"].as_array() {
            for issue in issues {
                let code = issue["code"].as_str().unwrap_or("");
                if [
                    "invalid_dependencies",
                    "invalid_snapshot",
                    "invalid_history",
                    "unsupported_history",
                    "invalid_capability",
                ]
                .contains(&code)
                {
                    block(
                        &mut report,
                        code,
                        "candidate contains unsupported captured evidence",
                        Some(id),
                        issue["field"].as_str(),
                    )?;
                }
            }
        }
    }
    finish(report)
}

pub(super) fn references_in_body(value: &V) -> Vec<String> {
    let mut out = vec![];
    fn visit(v: &V, out: &mut Vec<String>) {
        match v {
            V::Text(s) => {
                out.push(s.clone());
                out.extend(O::ID.find_iter(s).map(|m| m.as_str().to_owned()));
            }
            V::Map(m) => {
                for (k, v) in m {
                    out.extend(O::ID.find_iter(k).map(|m| m.as_str().to_owned()));
                    visit(v, out)
                }
            }
            V::List(a) => {
                for v in a {
                    visit(v, out)
                }
            }
            _ => {}
        }
    }
    visit(value, &mut out);
    out
}
