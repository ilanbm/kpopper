//! Versioned ordinary findings. Attention consumes these findings without evaluating
//! again. Explicit calculations retain the ordinary Lean program and profile.
pub use crate::ordinary_assessment_report::{
    assess, from_capture, legacy_digest, legacy_json, load,
};
use crate::{
    Result,
    history_contract::*,
    history_view::{list, map_mut, truth},
    ordinary_counts::{legacy_rule, same_rule},
    ordinary_reader::{self as R, Reader},
    reasoning_authoring::{named, py},
    reasoning_language as L,
    source_clock::python_equal,
    value::TypedValue as V,
};
use std::collections::BTreeSet;
pub const PROFILE: &str = "ordinary-reader/v1";
pub const POLICY: &str = "focused-review/v1";
pub(crate) fn s(v: &str) -> V {
    V::Text(v.into())
}
pub(crate) fn obj(items: impl IntoIterator<Item = (&'static str, V)>) -> V {
    V::Map(items.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
pub(crate) fn empty() -> V {
    V::Map(Map::new())
}
pub(crate) fn get<'a>(m: &'a Map, key: &str) -> &'a V {
    m.get(key).unwrap_or(&V::Null)
}
pub(crate) fn texts(values: impl IntoIterator<Item = String>) -> V {
    V::List(values.into_iter().map(V::Text).collect())
}
fn side(status: &str) -> V {
    obj([("status", s(status))])
}
fn unavailable(reason: &str) -> V {
    obj([("status", s("unavailable")), ("reason", s(reason))])
}
fn recorded(value: V) -> V {
    obj([("status", s("recorded")), ("value", value)])
}
fn issue(code: &str, field: &V, dep: Option<&str>) -> V {
    let mut v = obj([("code", s(code)), ("field", field.clone())]);
    if let Some(d) = dep {
        map_mut(&mut v)
            .unwrap()
            .insert("related_ids".into(), V::List(vec![s(d)]));
    }
    v
}
fn complete_reads(pred: &V) -> Option<Vec<String>> {
    if pred == &V::Null || pred == &s("") {
        return Some(vec![]);
    }
    if !R::why_undecided(pred).is_empty() {
        return None;
    }
    if matches!(pred, V::Map(_)) {
        return Some(L::legacy_references(pred));
    }
    let source = py(pred);
    let c = R::CMP.captures(&source)?;
    let mut reads = BTreeSet::from([c[1].to_owned()]);
    let rhs = c[3].trim();
    if R::ID.find(rhs).is_some_and(|m| m.as_str() == rhs) {
        reads.insert(rhs.into());
    }
    Some(reads.into_iter().collect())
}
fn computation_error(reader: &Reader<'_>, pred: &V) -> String {
    let converted;
    let pred = if matches!(pred, V::Map(_)) {
        pred
    } else {
        let source = py(pred);
        let Some(c) = R::CMP.captures(&source) else {
            return String::new();
        };
        if ![&c[1], c[3].trim()].iter().any(|key| {
            reader
                .raw
                .get(*key)
                .and_then(|b| map(b).ok())
                .is_some_and(|b| matches!(b.get("rule"), Some(V::Map(_))))
        }) {
            return String::new();
        }
        converted = obj([("expr", s(&source))]);
        if L::legacy_expression(&converted, true).is_err() {
            return String::new();
        }
        &converted
    };
    R::compute(&reader.raw, &reader.ids, Some(pred), reader.program())["error"]
        .as_str()
        .unwrap_or("")
        .into()
}
/// Exact ordinary output numbers, without interpreting strings or authored maps
/// which merely resemble the rational output shape.
pub(crate) fn number(v: &V) -> Option<(num_bigint::BigInt, num_bigint::BigInt)> {
    if let V::Map(m) = v {
        if m.len() != 1 {
            return None;
        }
        let a = list(m.get("rational")?).ok()?;
        if a.len() != 2 {
            return None;
        }
        let n = text(&a[0]).ok()?;
        let d = text(&a[1]).ok()?;
        if n.len() > 1024
            || d.len() > 1024
            || n.strip_prefix('-').unwrap_or(n).is_empty()
            || d.is_empty()
            || !n
                .strip_prefix('-')
                .unwrap_or(n)
                .bytes()
                .all(|b| b.is_ascii_digit())
            || !d.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        let n = n.parse().ok()?;
        let d = d.parse().ok()?;
        return (d != num_bigint::BigInt::from(0)).then_some((n, d));
    }
    if !matches!(v, V::Integer(_) | V::Float(_)) {
        return None;
    }
    let value = crate::reasoning_scope::query_value(v, 0).ok()?;
    let m = map(&value).ok()?;
    Some((
        text(m.get("numerator")?).ok()?.parse().ok()?,
        text(m.get("denominator")?).ok()?.parse().ok()?,
    ))
}
pub(crate) fn equal_value(a: &V, b: &V) -> bool {
    match (number(a), number(b)) {
        (Some((a, b)), Some((c, d))) => a * d == c * b,
        _ => python_equal(a, b),
    }
}
pub(crate) fn snapshot_value(reader: &Reader<'_>, dep: &str) -> Result<V> {
    let body = get(&reader.raw, dep);
    let Ok(b) = map(body) else {
        return Ok(body.clone());
    };
    if b.contains_key(text(&reader.fields["deps"])?) {
        return Ok(s(&b
            .get("verdict")
            .filter(|v| truth(v))
            .or_else(|| b.get("title").filter(|v| truth(v)))
            .map(py)
            .unwrap_or_else(|| dep.into())));
    }
    if crate::reasoning_fields::BUILTINS.contains(&dep) && dep.starts_with("page.") {
        return Ok(V::Null);
    }
    if matches!(b.get("rule"), Some(V::Map(_))) {
        let result = R::compute(&reader.raw, &reader.ids, None, reader.program());
        let value = V::from_json(&result["values"][dep]["value"])?;
        if value == V::Null {
            return Err(error(&format!(
                "cannot snapshot {dep}: {}",
                result["values"][dep]["reason"]
                    .as_str()
                    .or_else(|| result["error"].as_str())
                    .unwrap_or("unavailable")
            )));
        }
        return Ok(obj([(
            "computed",
            obj([("value", value), ("rule", b["rule"].clone())]),
        )]));
    }
    let value = reader.value(dep)?;
    if value != V::Null {
        return Ok(value);
    }
    let rule = b
        .get("rule")
        .filter(|v| truth(v))
        .unwrap_or_else(|| get(b, "v"));
    if let V::Text(t) = rule
        && !t.is_empty()
    {
        return Ok(rule.clone());
    }
    if let Some(when) = b
        .get("read")
        .filter(|v| truth(v))
        .or_else(|| b.get("of").filter(|v| truth(v)))
    {
        return Ok(s(&format!("read {}", py(when))));
    }
    let name = named(body);
    Ok(s(if name.is_empty() { "present" } else { &name }))
}
/// Assess independent dimensions, retaining authored values and unavailable sides.
pub fn judgment_state(reader: &Reader<'_>, body: &V) -> Result<V> {
    let b = map(body)?;
    let fields = &reader.fields;
    let declared = get(b, text(&fields["deps"])?);
    let valid = matches!(declared,V::List(a) if a.iter().all(|d|matches!(d,V::Text(_))));
    let mut issues = vec![];
    if !valid {
        issues.push(issue("invalid_dependencies", &fields["deps"], None));
    }
    let snapshot = get(b, text(&fields["snapshot"]).unwrap_or(""));
    let invalid_snapshot = snapshot != &V::Null && !matches!(snapshot, V::Map(_));
    if invalid_snapshot {
        issues.push(issue("invalid_snapshot", &fields["snapshot"], None));
    }
    let empty_map = Map::new();
    let seen = map(snapshot).unwrap_or(&empty_map);
    let mut seen_deps = BTreeSet::new();
    let deps = if valid {
        list(declared)?
            .iter()
            .filter_map(|d| text(d).ok())
            .filter(|d| seen_deps.insert((*d).to_owned()))
            .collect::<Vec<_>>()
    } else {
        vec![]
    };
    let mut readings = Map::new();
    for dep in &deps {
        let old = get(seen, dep);
        let prior = if invalid_snapshot {
            unavailable("invalid_snapshot")
        } else if seen.contains_key(*dep) {
            recorded(old.clone())
        } else {
            side("missing")
        };
        let mut current = side("missing");
        let mut comparable = V::Null;
        let mut current_rule = &V::Null;
        if reader.ids.contains(*dep) {
            let entry = map(get(&reader.raw, dep)).ok();
            current_rule = entry.map(|e| get(e, "rule")).unwrap_or(&V::Null);
            comparable = reader.value(dep)?;
            let explicit_null = entry.is_some_and(|e| {
                e.get("v") == Some(&V::Null) && get(e, "quoted") == &V::Null && !truth(current_rule)
            });
            let value = if comparable != V::Null || explicit_null {
                Ok(comparable.clone())
            } else {
                snapshot_value(reader, dep)
            };
            current = match value {
                Ok(v) if v != V::Null || explicit_null => recorded(v),
                Ok(_) => unavailable("reading_unavailable"),
                Err(e) => {
                    let mut v = unavailable("calculation_unavailable");
                    map_mut(&mut v)?.insert("detail".into(), s(&e.0));
                    v
                }
            };
        }
        let computed = map(old)
            .ok()
            .and_then(|m| m.get("computed"))
            .and_then(|v| map(v).ok())
            .filter(|m| m.contains_key("value") && m.contains_key("rule"));
        let legacy = legacy_rule(old, current_rule);
        let rule_changed = computed
            .map(|m| &m["rule"])
            .or(legacy.as_ref())
            .map(|r| !same_rule(r, current_rule));
        let mut reasons = vec![];
        let p = map(&prior)?;
        let c = map(&current)?;
        if string_is(&p["status"], "missing") {
            reasons.push(s("baseline_missing"))
        } else if string_is(&p["status"], "unavailable") {
            reasons.push(p["reason"].clone())
        }
        if string_is(&c["status"], "missing") {
            reasons.push(s("current_missing"))
        } else if string_is(&c["status"], "unavailable") {
            reasons.push(c["reason"].clone())
        } else if legacy.is_some() {
            reasons.push(s("historical_formula_only"))
        } else if comparable == V::Null
            || matches!(comparable, V::Map(_) | V::List(_)) && number(&comparable).is_none()
            || matches!(old, V::Map(_) | V::List(_)) && computed.is_none()
        {
            reasons.push(s("not_compared_by_reader"))
        }
        let mut comparison = "unknown";
        if reasons.is_empty() {
            if let Some(computed) = computed {
                let previous = &computed["value"];
                if previous == &V::Null || comparable == V::Null {
                    reasons.push(s("historical_value_unavailable"))
                } else {
                    comparison = if equal_value(previous, &comparable) {
                        "same"
                    } else {
                        "changed"
                    };
                }
            } else {
                // moved_deps reports formula values by exact numeric equality and
                // plain values by the legacy whitespace/Decimal comparator.
                let same = if matches!(current_rule, V::Map(_)) {
                    equal_value(old, &comparable)
                } else {
                    R::same_legacy(old, &comparable)
                };
                comparison = if same { "same" } else { "changed" };
            }
        }
        let mut reading = obj([
            ("current", current),
            ("at_review", prior),
            ("comparison", s(comparison)),
            ("reasons", V::List(reasons)),
            ("rule_changed", rule_changed.map(V::Bool).unwrap_or(V::Null)),
        ]);
        if let Some(computed) = computed {
            map_mut(&mut reading)?.insert(
                "historical_calculation".into(),
                obj([
                    ("value", computed["value"].clone()),
                    ("rule", computed["rule"].clone()),
                ]),
            );
        }
        if legacy.is_some() {
            map_mut(&mut reading)?.insert("historical_formula_only".into(), V::Bool(true));
        }
        if !reader.ids.contains(*dep) {
            issues.push(issue("missing_dependency", &fields["deps"], Some(dep)))
        }
        if !invalid_snapshot && !seen.contains_key(*dep) {
            issues.push(issue("missing_snapshot", &fields["snapshot"], Some(dep)))
        }
        readings.insert((*dep).into(), reading);
    }
    let pred = R::predicate_of(body, fields);
    let declared_pred = pred != V::Null && pred != s("");
    let reads = complete_reads(&pred);
    let result = if declared_pred {
        reader.predicate(&pred)?
    } else {
        None
    };
    let reason = if declared_pred {
        R::why_undecided(&pred)
    } else {
        "not_declared".into()
    };
    let err = if declared_pred {
        computation_error(reader, &pred)
    } else {
        String::new()
    };
    let status = if !declared_pred {
        "not_declared"
    } else if !err.is_empty() {
        "error"
    } else {
        match result {
            Some(true) => "holds",
            Some(false) => "does_not_hold",
            None => "unknown",
        }
    };
    let mut falsifier = obj([
        ("status", s(status)),
        ("expression", pred.clone()),
        ("reads", reads.clone().map(texts).unwrap_or(V::Null)),
    ]);
    let f = map_mut(&mut falsifier)?;
    if ["unknown", "error"].contains(&status) {
        f.insert(
            "reason".into(),
            s(if !err.is_empty() {
                "evaluation_error"
            } else if !reason.is_empty() {
                "unsupported_predicate"
            } else {
                "reading_unavailable"
            }),
        );
        if !err.is_empty() || !reason.is_empty() {
            f.insert(
                "detail".into(),
                s(if !err.is_empty() { &err } else { &reason }),
            );
        }
    }
    let authored = get(b, text(&fields["predicate"]).unwrap_or(""));
    if authored != &V::Null && !matches!(authored, V::Text(_) | V::Map(_)) {
        f.extend(Map::from([
            ("status".into(), s("error")),
            ("reason".into(), s("invalid_predicate_type")),
            ("reads".into(), V::Null),
            ("expression".into(), authored.clone()),
        ]));
        issues.push(issue("invalid_predicate_type", &fields["predicate"], None));
    } else if authored == &empty() {
        f.extend(Map::from([
            ("status".into(), s("error")),
            ("reason".into(), s("invalid_predicate_shape")),
            ("reads".into(), V::Null),
        ]));
        issues.push(issue("invalid_predicate_shape", &fields["predicate"], None));
    }
    if string_is(&f["status"], "unknown") && string_is(get(f, "reason"), "unsupported_predicate") {
        let mentions = R::predicate_refs(&pred)
            .into_iter()
            .filter(|r| reader.ids.contains(r) && !deps.contains(&r.as_str()))
            .collect::<BTreeSet<_>>();
        let mut v = issue("unsupported_predicate", &fields["predicate"], None);
        map_mut(&mut v)?.insert("related_ids".into(), texts(mentions));
        issues.push(v);
    }
    if valid && let Some(reads) = reads {
        let undeclared = reads
            .into_iter()
            .filter(|r| !deps.contains(&r.as_str()))
            .collect::<BTreeSet<_>>();
        if !undeclared.is_empty() {
            let mut v = issue(
                "undeclared_predicate_dependencies",
                &fields["predicate"],
                None,
            );
            map_mut(&mut v)?.insert("related_ids".into(), texts(undeclared));
            issues.push(v);
        }
    }
    let mut basis = obj([
        ("status", s(if valid { "assessed" } else { "error" })),
        ("dependencies", V::Map(readings)),
    ]);
    if !valid {
        map_mut(&mut basis)?.insert("reason".into(), s("invalid_dependencies"));
    }
    Ok(obj([
        ("basis", basis),
        ("falsifier", falsifier),
        (
            "contention",
            obj([
                ("status", s("unassessed")),
                ("reason", s("hypotheses_not_supplied")),
                ("witnesses", V::List(vec![])),
                ("alternatives", V::List(vec![])),
            ]),
        ),
        (
            "integrity",
            obj([
                ("status", s("assessed")),
                (
                    "checks",
                    texts(
                        [
                            "dependency_shape",
                            "snapshot_shape",
                            "dependency_presence",
                            "snapshot_presence",
                            "predicate_type",
                            "predicate_shape",
                            "predicate_dependencies",
                        ]
                        .map(str::to_owned),
                    ),
                ),
                ("issues", V::List(issues)),
            ]),
        ),
    ]))
}
pub fn attention(state: &V, policy: &str) -> Result<V> {
    attention_ordered(state, policy, None)
}
/// Authored dependency order is retained by the caller, since typed maps are
/// canonical. This changes list presentation only, never the findings or policy.
pub fn attention_ordered(state: &V, policy: &str, order: Option<&[String]>) -> Result<V> {
    crate::require(
        [POLICY, "falsifiers-only/v1"].contains(&policy),
        &format!("unknown attention policy: {policy}"),
    )?;
    let state = map(state)?;
    let f = map(&state["falsifier"])?;
    let reads = match get(f, "reads") {
        V::List(a) => a
            .iter()
            .filter_map(|v| text(v).ok())
            .collect::<BTreeSet<_>>(),
        _ => BTreeSet::new(),
    };
    let mut reasons = vec![];
    if string_is(&f["status"], "holds") {
        reasons.push(obj([
            ("code", s("falsifier_holds")),
            ("related_ids", texts(reads.iter().map(|s| (*s).to_owned()))),
        ]));
    }
    if policy == POLICY {
        let dependencies = map(&map(&state["basis"])?["dependencies"])?;
        let mut ordered = dependencies.iter().collect::<Vec<_>>();
        if let Some(order) = order {
            ordered
                .sort_by_key(|(id, _)| order.iter().position(|v| v == *id).unwrap_or(usize::MAX));
        }
        for (dep, finding) in ordered {
            let finding = map(finding)?;
            let code = if get(finding, "rule_changed") == &V::Bool(true) {
                Some("formula_changed")
            } else if string_is(&finding["comparison"], "changed")
                && !(string_is(&f["status"], "does_not_hold") && reads.contains(dep.as_str()))
            {
                Some("premise_changed")
            } else {
                None
            };
            if let Some(code) = code {
                reasons.push(obj([
                    ("code", s(code)),
                    ("related_ids", V::List(vec![s(dep)])),
                ]));
            }
        }
    }
    let mut items = vec![];
    if !reasons.is_empty() {
        items.push(obj([
            ("action", s("review")),
            ("reasons", V::List(reasons)),
        ]));
    }
    if policy == POLICY {
        let issues = list(&map(&state["integrity"])?["issues"])?;
        for (action, codes) in [
            (
                "repair_record",
                &[
                    "invalid_dependencies",
                    "invalid_snapshot",
                    "invalid_predicate_type",
                    "invalid_predicate_shape",
                    "unsupported_predicate",
                    "undeclared_predicate_dependencies",
                ][..],
            ),
            ("resolve_gap", &["missing_dependency"][..]),
        ] {
            let items_ = issues
                .iter()
                .filter(|i| {
                    map(i)
                        .ok()
                        .and_then(|m| text(get(m, "code")).ok())
                        .is_some_and(|c| codes.contains(&c))
                })
                .cloned()
                .collect::<Vec<_>>();
            if !items_.is_empty() {
                items.push(obj([("action", s(action)), ("reasons", V::List(items_))]));
            }
        }
        let contention = map(&state["contention"])?;
        if string_is(&contention["status"], "detected") {
            let related = list(&contention["alternatives"])?
                .iter()
                .filter_map(|v| {
                    map(v)
                        .ok()
                        .and_then(|m| text(get(m, "id")).ok())
                        .map(str::to_owned)
                })
                .collect::<BTreeSet<_>>();
            items.push(obj([
                ("action", s("inspect_alternatives")),
                (
                    "reasons",
                    V::List(vec![obj([
                        ("code", s("competing_hypotheses")),
                        ("related_ids", texts(related)),
                    ])]),
                ),
            ]));
        }
    }
    Ok(V::List(items))
}
pub fn selected_attention(
    report: &V,
    ids: Option<&[String]>,
    actions: Option<&[String]>,
) -> Result<V> {
    let r = map(report)?;
    let items = if let Some(nodes) = r.get("nodes") {
        map(nodes)?
            .iter()
            .map(|(k, n)| Ok((k.clone(), field(map(n)?, "attention")?.clone())))
            .collect::<Result<Map>>()?
    } else {
        map(r
            .get("attention")
            .ok_or_else(|| error("report has neither findings nor attention"))?)?
        .clone()
    };
    let available = if !r.contains_key("nodes") && r.contains_key("selection") {
        list(&r["selection"])?
            .iter()
            .map(|v| text(v).map(str::to_owned))
            .collect::<Result<BTreeSet<_>>>()?
    } else {
        items.keys().cloned().collect()
    };
    let selected = ids
        .map(|a| a.iter().cloned().collect())
        .unwrap_or(available.clone());
    crate::require(
        selected.is_subset(&available),
        "attention selection contains IDs outside this assessment",
    )?;
    let mut out = Map::new();
    for (id, items) in items {
        if !selected.contains(&id) {
            continue;
        }
        let wanted = list(&items)?
            .iter()
            .filter(|i| {
                actions.is_none_or(|a| {
                    map(i)
                        .ok()
                        .and_then(|m| text(get(m, "action")).ok())
                        .is_some_and(|v| a.iter().any(|a| a == v))
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        if !wanted.is_empty() {
            out.insert(id, V::List(wanted));
        }
    }
    Ok(V::Map(out))
}
