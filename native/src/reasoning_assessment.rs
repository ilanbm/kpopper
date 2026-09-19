//! Independent recorded findings, computation evidence and explicit attention.
use crate::{
    Result,
    history_contract::*,
    history_view::{list, truth},
    history_yaml,
    reasoning_capabilities::{compare_basis, historical_envelope},
    reasoning_evaluate::Evaluator,
    reasoning_language as L,
    reasoning_runtime::{OperationalBounds, Runtime},
    reasoning_snapshot::{Snapshot, digest},
    require,
    value::{Integer, TypedValue as V},
};
use serde_json::{Value as J, json};
use std::collections::{BTreeMap, BTreeSet};
fn s(value: &str) -> V {
    V::Text(value.into())
}
fn obj(fields: impl IntoIterator<Item = (&'static str, V)>) -> V {
    V::Map(fields.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
fn strings(values: impl IntoIterator<Item = String>) -> V {
    V::List(values.into_iter().map(V::Text).collect())
}
fn names(v: &V) -> Result<Vec<String>> {
    list(v)?
        .iter()
        .map(|v| text(v).map(str::to_owned))
        .collect()
}
fn empty() -> V {
    V::Map(Map::new())
}
fn val(j: &J) -> Result<V> {
    V::from_json_bounded(j, 64 * 1024 * 1024 / 8)
}
fn charge(remaining: &mut usize, value: &V) -> Result<()> {
    let size = history_yaml::compact_json_size(value);
    require(size <= *remaining, "output_limit")?;
    *remaining -= size;
    Ok(())
}
pub fn history(old: &V, document: &V) -> Result<(Option<V>, Option<V>, Option<String>)> {
    let prior = map(old).ok().and_then(|m| m.get("computed"));
    let literal = |v: &V| -> Option<V> {
        if matches!(v, V::Map(m) if m.len() != 1 || !m.contains_key("rational")) {
            return None;
        }
        let value = crate::reasoning_scope::query_value(v, 0).ok()?;
        let m = map(&value).ok()?;
        (!["list", "record"].iter().any(|t| string_is(&m["type"], t))).then_some(value)
    };
    let Some(V::Map(h)) = prior else {
        return Ok((
            if prior.is_none() { literal(old) } else { None },
            None,
            prior.map(|_| "invalid_history".into()),
        ));
    };
    if !h.contains_key("version") {
        return Ok((
            h.get("value").and_then(literal),
            Some(V::Map(h.clone())),
            (!h.contains_key("value")).then(|| "invalid_history".into()),
        ));
    }
    if let Err(e) = historical_envelope(h, document) {
        return Ok((None, Some(V::Map(h.clone())), Some(e.0)));
    }
    Ok((Some(h["value"].clone()), Some(V::Map(h.clone())), None))
}
fn rule_changed(old: &V, current: &V) -> Result<V> {
    if *old == V::Null || *current == V::Null {
        return Ok(V::Bool(old != current));
    }
    let query = |v: &V| matches!(v,V::Map(m)if m.len()==1&&m.contains_key("query"));
    if query(old) || query(current) {
        return Ok(V::Bool(if query(old) && query(current) {
            digest(old)? != digest(current)?
        } else {
            true
        }));
    }
    Ok(match (L::lower(old), L::lower(current)) {
        (Ok(a), Ok(b)) => V::Bool(a != b),
        _ => V::Null,
    })
}
fn is_scope(data: &Map, id: &str) -> bool {
    map(&data["nodes"])
        .ok()
        .and_then(|n| n.get(id))
        .and_then(|v| map(v).ok())
        .and_then(|n| n.get("body"))
        .and_then(|v| map(v).ok())
        .is_some_and(|b| b.contains_key("collection_scope"))
}
pub fn dependency_result(snapshot: &Snapshot, id: &str, engine: &mut Evaluator<'_>) -> Result<J> {
    require(
        snapshot.snapshot_id() == engine.snapshot_id(),
        "dependency evaluator belongs to a different snapshot",
    )?;
    let data = map(snapshot.data())?;
    if !is_scope(data, id) {
        return engine.evaluate(obj([("ref", s(id))]), vec![id.into()]);
    }
    let bounds = engine.bounds();
    let mut resources = engine.limits().clone();
    resources["version"] = json!("resources/v2");
    let mut result = json!({"schema_version":1,"profile":"core/v1","modules":[],"snapshot_id":snapshot.snapshot_id(),"computation_id":null,"implementation":null,"resource_profile":resources,"operational_limits":{"timeout_seconds":bounds.timeout.as_secs(),"batch_requests":bounds.batch_requests,"input_bytes":bounds.input_bytes,"output_bytes":bounds.output_bytes},"status":"unknown","value":null,"diagnostics":[],"executed_reads":[],"potential_dependencies":[],"potential_ids":[id],"basis":null,"assurance":{"kind":"computed","formal_scope":[]},"cost":{"steps":0,"preflight_steps":0,"node_evaluations":{}}});
    let prepare = (|| -> Result<()> {
        let cap =
            crate::reasoning_fields::capabilities(&data["document"], Some("core/v1"))?.to_json()?;
        result["modules"] = cap["requires"].clone();
        if cap["experimental_override"] == true {
            result["interpretation"] =
                json!({"declared_profile":cap["declared_profile"],"explicit_override":"core/v1"});
        }
        let capture = snapshot.capture_scope(id, None)?;
        result["modules"] = map(capture.basis())?["modules"].to_json()?;
        result["status"] = json!("ok");
        result["value"] = capture.value().to_json()?;
        result["basis"] = capture.basis().to_json()?;
        result["potential_dependencies"] = json!([capture.witness().to_json()?]);
        let ids = map(capture.candidates())?
            .keys()
            .cloned()
            .chain([id.into()])
            .collect::<BTreeSet<_>>();
        result["potential_ids"] = json!(ids);
        result["computation_id"] = json!(digest(&val(
            &json!({"snapshot_id":snapshot.snapshot_id(),"scope_basis":result["basis"],"resources":result["resource_profile"]})
        )?)?);
        Ok(())
    })();
    if let Err(e) = prepare {
        let code = e.0.split(':').next().unwrap();
        result["status"] = json!(if code == "limit" {
            "limit"
        } else if code == "unsupported_capability" {
            "unsupported_capability"
        } else {
            "error"
        });
        result["diagnostics"] = json!([{"code":code,"related_ids":[id]}]);
    }
    charge(&mut bounds.output_bytes.clone(), &val(&result)?)?;
    Ok(result)
}
fn attention(state: &V, policy: &str, dependency_order: &[String]) -> Result<V> {
    require(
        ["focused-review/v1", "falsifiers-only/v1"].contains(&policy),
        "unknown_attention_policy",
    )?;
    let st = map(state)?;
    let f = map(&st["falsifier"])?;
    let focused = policy == "focused-review/v1";
    let mut reasons = Vec::new();
    let reads = f
        .get("reads")
        .filter(|v| **v != V::Null)
        .map(names)
        .transpose()?
        .unwrap_or_default();
    let reason =
        |code: &str, ids: Vec<String>| obj([("code", s(code)), ("related_ids", strings(ids))]);
    if string_is(&f["status"], "holds") {
        reasons.push(reason(
            "falsifier_holds",
            reads
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        ));
    }
    if focused {
        let dependencies = map(&map(&st["basis"])?["dependencies"])?;
        for id in dependency_order {
            let finding = &dependencies[id];
            let finding = map(finding)?;
            if finding.get("rule_changed") == Some(&V::Bool(true)) {
                reasons.push(reason("formula_changed", vec![id.clone()]));
            } else if string_is(&finding["comparison"], "changed")
                && !(string_is(&f["status"], "does_not_hold") && reads.contains(id))
            {
                reasons.push(reason("premise_changed", vec![id.clone()]));
            }
        }
    }
    let mut items = Vec::new();
    if !reasons.is_empty() {
        items.push(obj([
            ("action", s("review")),
            ("reasons", V::List(reasons)),
        ]));
    }
    if focused {
        let issues = list(&map(&st["integrity"])?["issues"])?;
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
            let reasons = issues
                .iter()
                .filter_map(|v| map(v).ok())
                .filter(|m| {
                    m.get("code")
                        .and_then(|v| text(v).ok())
                        .is_some_and(|c| codes.contains(&c))
                })
                .map(|m| {
                    obj([
                        ("code", m["code"].clone()),
                        (
                            "related_ids",
                            m.get("related_ids")
                                .cloned()
                                .unwrap_or_else(|| V::List(Vec::new())),
                        ),
                    ])
                })
                .collect::<Vec<_>>();
            if !reasons.is_empty() {
                items.push(obj([("action", s(action)), ("reasons", V::List(reasons))]));
            }
        }
        let contention = map(&st["contention"])?;
        if string_is(&contention["status"], "detected") {
            let ids = list(&contention["alternatives"])?
                .iter()
                .filter_map(|v| map(v).ok())
                .filter_map(|m| m.get("id"))
                .filter_map(|v| text(v).ok())
                .map(str::to_owned)
                .collect::<BTreeSet<_>>();
            items.push(obj([
                ("action", s("inspect_alternatives")),
                (
                    "reasons",
                    V::List(vec![reason(
                        "competing_hypotheses",
                        ids.into_iter().collect(),
                    )]),
                ),
            ]));
        }
    }
    Ok(V::List(items))
}
fn scope_projection(data: &Map, visible: &BTreeSet<String>, budget: &mut usize) -> Result<V> {
    let original = map(&data["context"])?;
    let mut context = original
        .iter()
        .filter(|(k, _)| {
            [
                "read_mode",
                "original_read_mode",
                "project",
                "target",
                "source_collection",
            ]
            .contains(&k.as_str())
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect::<Map>();
    if let Some(V::Map(project)) = context.get_mut("project") {
        project.retain(|k, _| {
            [
                "version",
                "mode",
                "generation",
                "routing_identity",
                "publication_identity",
            ]
            .contains(&k.as_str())
        });
    }
    let none = empty();
    let conflicts = map(original.get("conflicts").unwrap_or(&none))?;
    context.insert(
        "conflicts".into(),
        V::Map(
            conflicts
                .iter()
                .filter(|(id, _)| visible.contains(*id))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        ),
    );
    let pending = map(original.get("pending").unwrap_or(&none))?;
    let bundles = map(pending.get("bundles").unwrap_or(&none))?;
    context.insert(
        "pending".into(),
        obj([
            ("ref", pending.get("ref").cloned().unwrap_or(V::Null)),
            ("bundle_revisions", strings(bundles.keys().cloned())),
            ("observations", s("bound_in_snapshot")),
            ("remote_acceptance", s("unassessed")),
        ]),
    );
    let mut hypotheses = Map::new();
    for (name, hyp) in map(&data["hypotheses"])? {
        let hyp = map(hyp)?;
        let doc = map(&hyp["document"])?;
        let mut document = Map::new();
        for (collection, entries) in doc {
            if ["meta", "schema", "record", "also"].contains(&collection.as_str()) {
                continue;
            }
            if let V::Map(entries) = entries {
                let selected = entries
                    .iter()
                    .filter(|(id, _)| visible.contains(*id))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<Map>();
                if !selected.is_empty() {
                    document.insert(collection.clone(), V::Map(selected));
                }
            }
        }
        if !document.is_empty()
            && let Some(schema) = doc.get("schema")
        {
            document.insert("schema".into(), schema.clone());
        }
        hypotheses.insert(
            name.clone(),
            obj([
                ("kind", hyp.get("kind").cloned().unwrap_or(s("hypothesis"))),
                ("document", V::Map(document)),
                (
                    "status",
                    s(if truth(&hyp["error"]) {
                        "unreadable"
                    } else {
                        "inspected"
                    }),
                ),
            ]),
        );
    }
    let scope = obj([
        ("context", V::Map(context)),
        ("hypotheses", V::Map(hypotheses)),
        ("external_sources_fetched", V::Bool(false)),
        (
            "evidence",
            s(
                "full input remains bound by snapshot_id; bodies are projected to the selected dependency closure",
            ),
        ),
    ]);
    charge(budget, &scope)?;
    Ok(scope)
}

pub fn assess(
    snapshot: &Snapshot,
    selection: Option<&[String]>,
    policy: &str,
    runtime: Option<&Runtime>,
    bounds: OperationalBounds,
) -> Result<V> {
    bounds.validate()?;
    let data = map(snapshot.data())?;
    let source_nodes = map(&data["nodes"])?;
    let supplied = selection
        .map(|s| s.to_vec())
        .unwrap_or_else(|| source_nodes.keys().cloned().collect());
    let mut selected = Vec::new();
    let mut selected_set = BTreeSet::new();
    for (id, nid) in supplied.into_iter().enumerate() {
        require(id < bounds.batch_requests, "batch_request_limit")?;
        if selected_set.insert(nid.clone()) {
            selected.push(nid);
        }
    }
    require(
        selected.iter().all(|id| source_nodes.contains_key(id)),
        "unknown_assessment_id",
    )?;
    let mut engine = Evaluator::new(snapshot, runtime, None, bounds.clone())?;
    type Task = ((String, String), (V, Vec<String>));
    let mut tasks: Vec<Task> = Vec::new();
    let mut keys = BTreeSet::new();
    let mut budget = bounds.output_bytes;
    for id in &selected {
        let node = map(&source_nodes[id])?;
        let body = &node["body"];
        let fields = map(&node["fields"])?;
        let b = map(body).ok();
        let deps = b.and_then(|m| m.get(text(&fields["deps"]).unwrap()));
        let deps = deps.and_then(|v| names(v).ok());
        charge(&mut budget.clone(), body)?;
        let mut add = |kind: &str, id: &str, expression: V, declared: Vec<String>| -> Result<()> {
            let key = (kind.into(), id.into());
            if keys.insert(key.clone()) {
                require(tasks.len() < bounds.batch_requests, "batch_request_limit")?;
                tasks.push((key, (expression, declared)));
            }
            Ok(())
        };
        if let Some(deps) = &deps {
            for dep in deps {
                add("value", dep, obj([("ref", s(dep))]), vec![dep.clone()])?;
            }
        }
        if let Some(value @ V::Map(_)) = b.and_then(|m| m.get(text(&fields["predicate"]).unwrap()))
        {
            add("predicate", id, value.clone(), deps.unwrap_or_default())?;
        }
        if b.is_none_or(|m| {
            !m.contains_key(text(&fields["deps"]).unwrap()) || m.contains_key("rule")
        }) {
            add("value", id, obj([("ref", s(id))]), vec![id.clone()])?;
        }
    }
    let scalar = tasks
        .iter()
        .filter(|((kind, id), _)| kind != "value" || !is_scope(data, id))
        .collect::<Vec<_>>();
    let requests = scalar
        .iter()
        .map(|(_, request)| request.clone())
        .collect::<Vec<_>>();
    let results = engine.evaluate_many(&requests)?;
    let mut computed = BTreeMap::new();
    for (task, result) in scalar.into_iter().zip(results) {
        computed.insert(task.0.clone(), val(&result)?);
    }
    for ((kind, id), _) in &tasks {
        if let std::collections::btree_map::Entry::Vacant(entry) =
            computed.entry((kind.clone(), id.clone()))
        {
            entry.insert(val(&dependency_result(snapshot, id, &mut engine)?)?);
        }
    }
    let mut visible = selected_set;
    for result in computed.values() {
        visible.extend(names(&map(result)?["potential_ids"])?);
    }
    let none = empty();
    let context = map(&data["context"])?;
    let conflicts = map(context.get("conflicts").unwrap_or(&none))?;
    let mut nodes = Map::new();
    for id in &selected {
        let node = map(&source_nodes[id])?;
        let body = &node["body"];
        let b = map(body).unwrap_or_else(|_| map(&none).unwrap());
        let fields = map(&node["fields"])?;
        let dep_field = text(&fields["deps"])?;
        let seen_field = text(&fields["snapshot"])?;
        let predicate_field = text(&fields["predicate"])?;
        let judgment = b.contains_key(dep_field);
        let deps = b.get(dep_field).and_then(|v| names(v).ok());
        let mut issues = Vec::new();
        let mut readings = Map::new();
        let issue = |code: &str, field: &str, ids: Vec<String>| {
            obj([
                ("code", s(code)),
                ("field", s(field)),
                ("related_ids", strings(ids)),
            ])
        };
        if judgment && deps.is_none() {
            issues.push(issue("invalid_dependencies", dep_field, vec![]));
        }
        let seen = b.get(seen_field).unwrap_or(&none);
        let seen = match map(seen) {
            Ok(m) => m,
            Err(_) => {
                issues.push(issue("invalid_snapshot", seen_field, vec![]));
                map(&none)?
            }
        };
        let mut seen_deps = BTreeSet::new();
        let mut dependency_order = Vec::new();
        for dep in deps.as_ref().into_iter().flatten() {
            if !seen_deps.insert(dep.clone()) {
                continue;
            }
            dependency_order.push(dep.clone());
            let now = &computed[&("value".into(), dep.clone())];
            let now_map = map(now)?;
            let old = seen.get(dep).unwrap_or(&V::Null);
            let (old_typed, historical, history_error) = if seen.contains_key(dep) {
                history(old, &data["document"])?
            } else {
                (None, None, None)
            };
            let comparison = if string_is(&now_map["status"], "ok") && old_typed.is_some() {
                if old_typed.as_ref() == Some(&now_map["value"]) {
                    "same"
                } else {
                    "changed"
                }
            } else {
                "unknown"
            };
            let rule = source_nodes
                .get(dep)
                .and_then(|v| map(v).ok())
                .and_then(|m| m.get("body"))
                .and_then(|v| map(v).ok())
                .and_then(|m| m.get("rule"))
                .unwrap_or(&V::Null);
            let previous = historical.as_ref().and_then(|v| map(v).ok());
            let changed = if history_error.is_none() {
                previous
                    .and_then(|m| m.get("rule"))
                    .map(|old| rule_changed(old, rule))
                    .transpose()?
                    .unwrap_or(V::Null)
            } else {
                V::Null
            };
            let comparison_basis = if history_error.is_some() {
                "unavailable"
            } else {
                compare_basis(
                    &now_map["basis"],
                    previous.and_then(|m| m.get("basis")).unwrap_or(&V::Null),
                )?
            };
            if let Some(error) = &history_error {
                issues.push(issue(error, seen_field, vec![dep.clone()]));
            }
            readings.insert(
                dep.clone(),
                obj([
                    (
                        "current",
                        obj([
                            ("status", now_map["status"].clone()),
                            ("value", now_map["value"].clone()),
                        ]),
                    ),
                    (
                        "at_review",
                        obj([
                            (
                                "status",
                                s(if history_error.is_some() {
                                    "unavailable"
                                } else if seen.contains_key(dep) {
                                    "recorded"
                                } else {
                                    "missing"
                                }),
                            ),
                            ("value", old.clone()),
                        ]),
                    ),
                    ("comparison", s(comparison)),
                    ("rule_changed", changed),
                    ("basis_comparison", s(comparison_basis)),
                    ("computation", now.clone()),
                ]),
            );
            if !source_nodes.contains_key(dep) {
                issues.push(issue("missing_dependency", dep_field, vec![dep.clone()]));
            }
            if !seen.contains_key(dep) {
                issues.push(issue("missing_snapshot", seen_field, vec![dep.clone()]));
            }
            for diagnostic in list(&now_map["diagnostics"])? {
                let mut d = map(diagnostic)?.clone();
                d.insert("field".into(), s(dep_field));
                issues.push(V::Map(d));
            }
        }
        let predicate = b.get(predicate_field).unwrap_or(&V::Null);
        let mut falsifier = Map::from([
            (
                "status".into(),
                s(if judgment {
                    "not_declared"
                } else {
                    "not_applicable"
                }),
            ),
            ("expression".into(), predicate.clone()),
            ("reads".into(), V::List(Vec::new())),
        ]);
        if *predicate != V::Null && *predicate != s("") {
            if let Some(result) = computed.get(&("predicate".into(), id.clone())) {
                let r = map(result)?;
                let value = map(&r["value"]).ok();
                let boolean = value.is_some_and(|v| string_is(&v["type"], "boolean"));
                let status = if string_is(&r["status"], "ok") && boolean {
                    if value.unwrap()["value"] == V::Bool(true) {
                        "holds"
                    } else {
                        "does_not_hold"
                    }
                } else if string_is(&r["status"], "unknown") {
                    "unknown"
                } else {
                    "error"
                };
                falsifier.insert("status".into(), s(status));
                falsifier.insert("computation".into(), result.clone());
                falsifier.insert(
                    "reads".into(),
                    V::List(
                        list(&r["executed_reads"])?
                            .iter()
                            .map(|w| field(map(w)?, "id").cloned())
                            .collect::<Result<Vec<_>>>()?,
                    ),
                );
                if ["error", "unknown"].contains(&status) {
                    falsifier.insert(
                        "reason".into(),
                        if string_is(&r["status"], "ok") {
                            s("type_error")
                        } else {
                            r["status"].clone()
                        },
                    );
                }
                for d in list(&r["diagnostics"])? {
                    let mut d = map(d)?.clone();
                    d.insert("field".into(), s(predicate_field));
                    issues.push(V::Map(d));
                }
            } else {
                falsifier.insert("status".into(), s("unknown"));
                falsifier.insert("reason".into(), s("declared_prose"));
                falsifier.insert("reads".into(), V::Null);
            }
        }
        let alternatives = map(&data["hypotheses"])?
            .iter()
            .filter_map(|(name, hyp)| {
                map(hyp)
                    .ok()
                    .filter(|h| !truth(&h["error"]))
                    .and_then(|h| map(&h["document"]).ok())
                    .filter(|d| d.values().any(|v| map(v).is_ok_and(|m| m.contains_key(id))))
                    .map(|_| obj([("hypothesis", s(name)), ("id", s(id))]))
            })
            .collect();
        let state = obj([
            (
                "basis",
                obj([
                    (
                        "status",
                        s(if judgment {
                            if deps.is_some() { "assessed" } else { "error" }
                        } else {
                            "not_applicable"
                        }),
                    ),
                    ("dependencies", V::Map(readings.clone())),
                ]),
            ),
            ("falsifier", V::Map(falsifier)),
            (
                "contention",
                obj([
                    (
                        "status",
                        s(if conflicts.contains_key(id) {
                            "detected"
                        } else {
                            "none_detected"
                        }),
                    ),
                    (
                        "witnesses",
                        conflicts
                            .get(id)
                            .cloned()
                            .unwrap_or_else(|| V::List(Vec::new())),
                    ),
                    ("alternatives", V::List(alternatives)),
                ]),
            ),
            (
                "integrity",
                obj([
                    ("status", s("assessed")),
                    ("checks", strings(["core/v1".into()])),
                    ("issues", V::List(issues)),
                ]),
            ),
        ]);
        let mut actions = list(&attention(&state, policy, &dependency_order)?)?.to_vec();
        let changed = dependency_order
            .iter()
            .filter(|id| {
                map(&readings[*id]).is_ok_and(|m| string_is(&m["basis_comparison"], "changed"))
            })
            .cloned()
            .collect::<Vec<_>>();
        if !changed.is_empty() && policy == "focused-review/v1" {
            actions.push(obj([
                ("action", s("review")),
                (
                    "reasons",
                    V::List(vec![obj([
                        ("code", s("computational_basis_changed")),
                        ("related_ids", strings(changed)),
                    ])]),
                ),
            ]));
        }
        let node = obj([
            ("body", body.clone()),
            ("fields", node["fields"].clone()),
            ("state", state),
            ("attention", V::List(actions)),
            (
                "computation",
                computed
                    .get(&("value".into(), id.clone()))
                    .cloned()
                    .unwrap_or(V::Null),
            ),
        ]);
        charge(
            &mut budget,
            &V::Map(Map::from([(id.clone(), node.clone())])),
        )?;
        nodes.insert(id.clone(), node);
    }
    let scope = scope_projection(data, &visible, &mut budget)?;
    let mut report = Map::from([
        ("schema_version".into(), V::Integer(Integer::new("2")?)),
        ("assessment_profile".into(), s("core/v1")),
        ("attention_policy".into(), s(policy)),
        ("snapshot_id".into(), s(snapshot.snapshot_id())),
        ("as_of".into(), data["as_of"].clone()),
        ("scope".into(), scope),
        ("selection".into(), strings(selected)),
        ("nodes".into(), V::Map(nodes)),
        (
            "operational_limits".into(),
            val(
                &json!({"timeout_seconds":bounds.timeout.as_secs(),"batch_requests":bounds.batch_requests,"input_bytes":bounds.input_bytes,"output_bytes":bounds.output_bytes}),
            )?,
        ),
    ]);
    charge(&mut bounds.output_bytes.clone(), &V::Map(report.clone()))?;
    report.insert(
        "assessment_revision".into(),
        s(&digest(&V::Map(report.clone()))?),
    );
    let report = V::Map(report);
    charge(&mut bounds.output_bytes.clone(), &report)?;
    Ok(report)
}
