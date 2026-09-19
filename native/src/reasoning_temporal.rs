//! Temporal policy over exact captured evidence; frozen validation never evaluates.
use crate::{
    Result,
    history_contract::*,
    history_view::{list, map_mut},
    reasoning_basis::InputBasis,
    reasoning_history_assessment as H, reasoning_language as L,
    reasoning_snapshot::{Snapshot, digest},
    require,
    value::{Integer, TypedValue as V},
};
use std::collections::{BTreeMap, BTreeSet};
fn s(v: &str) -> V {
    V::Text(v.into())
}
fn one() -> V {
    V::Integer(Integer::new("1").unwrap())
}
fn obj(v: impl IntoIterator<Item = (&'static str, V)>) -> V {
    V::Map(v.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
fn at<'a>(v: &'a V, path: &[&str]) -> &'a V {
    path.iter()
        .try_fold(v, |v, k| map(v).ok()?.get(*k))
        .unwrap_or(&V::Null)
}
fn pick(v: &V, names: &[&str]) -> Result<V> {
    let m = map(v)?;
    Ok(V::Map(
        names
            .iter()
            .map(|k| field(m, k).map(|v| (k.to_string(), v.clone())))
            .collect::<Result<_>>()?,
    ))
}
fn hex(v: &V) -> bool {
    text(v).is_ok_and(|s| s.len() == 64 && crate::history_paths::object_id(s))
}
fn mode(v: &V) -> bool {
    text(v).is_ok_and(|s| ["current", "anchored", "general"].contains(&s))
}
fn eq_one(v: &V) -> bool {
    crate::source_clock::python_equal(v, &one())
}
fn observations(p: &V) -> Result<&[V]> {
    match at(p, &["temporal", "observations"]) {
        V::Null => Ok(&[]),
        v => list(v),
    }
}
fn episode(observation: &V, claim: &V) -> Result<V> {
    let mut e = map(&pick(
        claim,
        &[
            "subject",
            "claim_id",
            "applicability",
            "predicate_digest",
            "anchors",
        ],
    )?)?
    .clone();
    for k in ["operation", "phase", "evidence_kind"] {
        e.insert(k.into(), map(observation)?[k].clone());
    }
    e.extend(
        map(&obj([
            ("evidence_version", one()),
            ("snapshot_id", V::Null),
            ("as_of", V::Null),
            ("verification", s("unknown")),
            ("outcome", s("unknown")),
            ("finding", V::Null),
            ("result", V::Null),
        ]))?
        .clone(),
    );
    Ok(V::Map(e))
}
pub(crate) fn validate(value: &V) -> Result<()> {
    let v = schema(
        value,
        &[
            "version",
            "applicability",
            "status",
            "current_falsifier",
            "complete",
            "counterexample_claim_ids",
            "episodes",
            "findings",
        ],
        &[],
    )?;
    require(
        eq_one(&v["version"])
            && mode(&v["applicability"])
            && text(&v["status"])
                .is_ok_and(|s| ["clear", "recovered", "counterexample", "unknown"].contains(&s))
            && matches!(v["current_falsifier"], V::Text(_))
            && matches!(v["complete"], V::Bool(_))
            && list(&v["episodes"])?.len() <= 64
            && matches!(v["findings"], V::List(_)),
        "v3 schema: invalid temporal assessment",
    )?;
    let ids = list(&v["counterexample_claim_ids"])?;
    require(
        ids.iter().all(|v| matches!(v, V::Text(_)))
            && ids.iter().map(text).collect::<Result<BTreeSet<_>>>()?.len() == ids.len(),
        "v3 schema: invalid temporal counterexamples",
    )?;
    for e in list(&v["episodes"])? {
        let e = schema(
            e,
            &[
                "evidence_version",
                "operation",
                "phase",
                "subject",
                "claim_id",
                "evidence_kind",
                "applicability",
                "predicate_digest",
                "anchors",
                "snapshot_id",
                "as_of",
                "verification",
                "outcome",
                "finding",
                "result",
            ],
            &[],
        )?;
        require(
            eq_one(&e["evidence_version"])
                && mode(&e["applicability"])
                && text(&e["phase"]).is_ok_and(|s| ["before", "after"].contains(&s))
                && text(&e["evidence_kind"]).is_ok_and(|s| {
                    ["recorded_receipt", "reconstructed_committed_world"].contains(&s)
                })
                && text(&e["verification"]).is_ok_and(|s| ["verified", "unknown"].contains(&s))
                && text(&e["outcome"]).is_ok_and(|s| {
                    ["counterexample", "not_counterexample", "unknown"].contains(&s)
                })
                && (e["snapshot_id"] == V::Null || hex(&e["snapshot_id"]))
                && matches!(e["as_of"], V::Null | V::Text(_))
                && matches!(e["finding"], V::Null | V::Text(_))
                && matches!(e["result"], V::Null | V::Map(_))
                && map(&e["anchors"])
                    .is_ok_and(|m| m.keys().map(String::as_str).eq(["applies", "at", "on"])),
            "v3 schema: invalid temporal episode",
        )?;
    }
    Ok(())
}
pub(crate) fn unknown(projection: &V) -> Result<Map> {
    let mut results = Map::new();
    for observation in observations(projection)? {
        let historical = Snapshot::from_json(text(at(observation, &["snapshot"]))?.as_bytes()).ok();
        for claim in list(at(observation, &["claims"]))? {
            let mut e = episode(observation, claim)?;
            let m = map_mut(&mut e)?;
            if let Some(h) = &historical {
                m.insert("snapshot_id".into(), s(h.snapshot_id()));
                m.insert("as_of".into(), at(h.data(), &["as_of"]).clone());
            }
            m.insert("finding".into(), s("temporal_replay_not_performed"));
            let subject = text(at(claim, &["subject"]))?;
            let V::List(episodes) = results
                .entry(subject.into())
                .or_insert_with(|| V::List(vec![]))
            else {
                unreachable!()
            };
            episodes.push(e);
        }
    }
    Ok(results)
}
fn strip_implementation(computation: &mut V) {
    if let V::Map(c) = computation {
        c.remove("implementation");
        if let Some(V::Map(a)) = c.get_mut("assurance") {
            a.remove("implementation");
        }
    }
}
pub(crate) fn semantic_node(node: &V) -> Result<V> {
    let mut node = node.clone();
    let n = map_mut(&mut node)?;
    if let Some(c) = n.get_mut("computation") {
        strip_implementation(c);
    }
    if let Some(V::Map(state)) = n.get_mut("state") {
        if let Some(V::Map(f)) = state.get_mut("falsifier")
            && let Some(c) = f.get_mut("computation")
        {
            strip_implementation(c);
        }
        if let Some(V::Map(basis)) = state.get_mut("basis")
            && let Some(V::Map(deps)) = basis.get_mut("dependencies")
        {
            for d in deps.values_mut() {
                if let V::Map(d) = d
                    && let Some(c) = d.get_mut("computation")
                {
                    strip_implementation(c);
                }
            }
        }
    }
    Ok(node)
}
fn semantic_falsifier(v: &V) -> Result<V> {
    let mut v = v.clone();
    if let Some(c) = map_mut(&mut v)?.get_mut("computation") {
        strip_implementation(c);
    }
    Ok(v)
}
fn outcome(status: &V) -> &'static str {
    match text(status).ok() {
        Some("holds") => "counterexample",
        Some("does_not_hold" | "not_declared") => "not_counterexample",
        _ => "unknown",
    }
}
type Key = (String, String, String, String, String);
fn key(v: &V) -> Result<Key> {
    let m = map(v)?;
    Ok((
        text(&m["operation"])?.into(),
        text(&m["phase"])?.into(),
        text(&m["subject"])?.into(),
        text(&m["claim_id"])?.into(),
        text(&m["predicate_digest"])?.into(),
    ))
}
pub(crate) fn retained(projection: &V, results: &Map) -> Result<Map> {
    let mut expected = BTreeMap::new();
    for observation in observations(projection)? {
        let mut snapshot_id = V::Null;
        let mut as_of = V::Null;
        let mut retained = None;
        let mut snapshot = None;
        if let Ok(h) = Snapshot::from_json(text(at(observation, &["snapshot"]))?.as_bytes()) {
            let report = at(observation, &["assessment"]);
            let r = if *report == V::Null {
                Ok(None)
            } else {
                H::validate_v2(&h, report).map(Some)
            };
            if let Ok(r) = r {
                retained = r;
                snapshot_id = s(h.snapshot_id());
                as_of = at(h.data(), &["as_of"]).clone();
            }
            snapshot = Some(h);
        }
        // All claims in an observation share its potentially large frozen report
        // and Snapshot. Validation never needs independent mutable copies.
        let retained = retained.map(std::rc::Rc::new);
        let snapshot = snapshot.map(std::rc::Rc::new);
        for claim in list(at(observation, &["claims"]))? {
            let e = episode(observation, claim)?;
            expected.insert(
                key(&e)?,
                (
                    claim.clone(),
                    snapshot_id.clone(),
                    as_of.clone(),
                    retained.clone(),
                    at(observation, &["evidence_kind"]).clone(),
                    snapshot.clone(),
                ),
            );
        }
    }
    let mut seen = BTreeSet::new();
    for (subject, episodes) in results {
        for episode in list(episodes)? {
            validate(&obj([
                ("version", one()),
                ("applicability", at(episode, &["applicability"]).clone()),
                ("status", s("unknown")),
                ("current_falsifier", s("unavailable")),
                ("complete", V::Bool(false)),
                ("counterexample_claim_ids", V::List(vec![])),
                ("episodes", V::List(vec![episode.clone()])),
                ("findings", V::List(vec![])),
            ]))?;
            let k = key(episode)?;
            let e = map(episode)?;
            require(
                string_is(&e["subject"], subject)
                    && expected.contains_key(&k)
                    && seen.insert(k.clone()),
                "retained temporal evidence does not match source receipts",
            )?;
            let (claim, snapshot_id, as_of, retained, kind, historical) = &expected[&k];
            let claim = map(claim)?;
            require(
                e["applicability"] == claim["applicability"]
                    && e["evidence_kind"] == *kind
                    && e["anchors"] == claim["anchors"]
                    && e["snapshot_id"] == *snapshot_id
                    && e["as_of"] == *as_of,
                "retained temporal evidence does not match source receipts",
            )?;
            if !string_is(&e["verification"], "verified") {
                continue;
            }
            let result =
                map(&e["result"]).map_err(|_| error("verified temporal evidence has no result"))?;
            require(
                ["status", "expression", "reads"]
                    .iter()
                    .all(|k| result.contains_key(*k)),
                "verified temporal evidence has no result",
            )?;
            let pins = map(at(projection, &["pins"]))?;
            let witness = at(&pins[text(&e["claim_id"])?], &["object"]);
            let predicate = text(at(witness, &["authored", "fields", "predicate"]))?;
            let expression = at(witness, &["body", predicate]);
            require(
                digest(&result["expression"])? == digest(expression)?
                    && string_is(&e["predicate_digest"], &digest(expression)?),
                "retained temporal expression does not match immutable claim",
            )?;
            let comp = result.get("computation").unwrap_or(&V::Null);
            if ["holds", "does_not_hold"]
                .iter()
                .any(|v| string_is(&result["status"], v))
            {
                require(
                    string_is(at(comp, &["status"]), "ok")
                        && at(comp, &["value"])
                            == &obj([
                                ("type", s("boolean")),
                                ("value", V::Bool(string_is(&result["status"], "holds"))),
                            ]),
                    "retained temporal verdict needs its computed boolean",
                )?;
            }
            if *comp != V::Null {
                require(
                    *snapshot_id != V::Null,
                    "verified temporal evidence has no snapshot",
                )?;
                H::computation(comp, text(snapshot_id)?)?;
                let c = map(comp)?;
                let mut executed = list(&c["executed_reads"])?
                    .iter()
                    .map(|item| {
                        let m = map(item)?;
                        text(if string_is(&m["kind"], "node") {
                            &m["id"]
                        } else {
                            &m["scope_id"]
                        })
                        .map(s)
                    })
                    .collect::<Result<Vec<_>>>()?;
                executed.sort_by(|a, b| text(a).unwrap().cmp(text(b).unwrap()));
                require(
                    result["reads"] == V::List(executed),
                    "retained temporal reads do not match computation witnesses",
                )?;
                if string_is(at(comp, &["basis", "recipe"]), "merkle-inputs/v1") {
                    let tree = L::lower(expression)?;
                    let tree_value = V::from_json(&tree)?;
                    let refs = L::references(&tree);
                    let historical = historical
                        .as_ref()
                        .ok_or_else(|| error("verified temporal evidence has no snapshot"))?;
                    let dependencies = InputBasis::new(historical.data())?.dependencies(&refs)?;
                    let basis = map(&c["basis"])?;
                    require(
                        basis.get("expression") == Some(&tree_value)
                            && basis.get("as_of") == Some(as_of)
                            && basis.get("dependencies") == Some(&c["potential_dependencies"])
                            && c["potential_dependencies"] == dependencies
                            && list(&c["executed_reads"])?
                                .iter()
                                .all(|item| list(&dependencies).unwrap().contains(item))
                            && refs
                                .iter()
                                .all(|id| list(&c["potential_ids"]).unwrap().contains(&s(id))),
                        "retained temporal basis does not match immutable predicate",
                    )?;
                    let mut declared = BTreeSet::new();
                    for field in ["pins", "pin_gaps"] {
                        if let V::Map(m) = at(witness, &[field]) {
                            declared.extend(m.keys().cloned());
                        }
                    }
                    let id = obj([
                        ("snapshot_id", snapshot_id.clone()),
                        ("expression", tree_value),
                        ("profile", c["profile"].clone()),
                        ("modules", c["modules"].clone()),
                        (
                            "declared",
                            V::List(declared.into_iter().map(V::Text).collect()),
                        ),
                        ("as_of", as_of.clone()),
                        ("resources", c["resource_profile"].clone()),
                    ]);
                    require(
                        string_is(&c["computation_id"], &digest(&id)?),
                        "retained temporal computation identity mismatch",
                    )?;
                }
            }
            if let Some(retained) = retained {
                require(
                    digest(&semantic_falsifier(&e["result"])?)?
                        == digest(&semantic_falsifier(at(
                            retained,
                            &["nodes", subject, "state", "falsifier"],
                        ))?)?,
                    "retained temporal result does not match source receipt",
                )?;
            }
            require(
                string_is(&e["outcome"], outcome(&result["status"])),
                "retained temporal outcome does not match source receipt",
            )?;
        }
    }
    require(
        seen.len() == expected.len(),
        "incomplete retained temporal evidence",
    )?;
    Ok(results.clone())
}
pub(crate) fn state(
    projection: &V,
    subject: &str,
    current: Option<&V>,
    episodes: &[V],
) -> Result<Option<V>> {
    let heads = list(at(projection, &["subjects", subject, "heads"]))?
        .iter()
        .map(text)
        .collect::<Result<BTreeSet<_>>>()?;
    let pins = map(at(projection, &["pins"]))?;
    let objects = heads
        .iter()
        .filter_map(|vid| pins.get(*vid))
        .filter(|w| string_is(at(w, &["status"]), "recorded"))
        .map(|w| at(w, &["object"]))
        .collect::<Vec<_>>();
    let metadata = objects
        .iter()
        .map(|o| at(o, &["body", "temporal"]))
        .filter(|v| **v != V::Null)
        .collect::<Vec<_>>();
    if metadata.is_empty() {
        return Ok(None);
    }
    require(
        metadata.len() == objects.len()
            && metadata
                .iter()
                .map(|v| digest(v))
                .collect::<Result<BTreeSet<_>>>()?
                .len()
                == 1,
        "incompatible temporal heads",
    )?;
    let applicability = at(metadata[0], &["applicability"]);
    let exact = episodes
        .iter()
        .filter(|e| text(at(e, &["claim_id"])).is_ok_and(|id| heads.contains(id)))
        .collect::<Vec<_>>();
    let mut findings = list(at(projection, &["temporal", "findings"]))?
        .iter()
        .filter(|f| text(at(f, &["object_id"])).is_ok_and(|id| id.is_empty() || heads.contains(id)))
        .cloned()
        .collect::<Vec<_>>();
    let counterexamples = exact
        .iter()
        .copied()
        .filter(|e| {
            string_is(at(e, &["verification"]), "verified")
                && string_is(at(e, &["outcome"]), "counterexample")
        })
        .collect::<Vec<_>>();
    let unknown = !findings.is_empty()
        || exact.is_empty()
        || exact.iter().any(|e| {
            !string_is(at(e, &["verification"]), "verified")
                || string_is(at(e, &["outcome"]), "unknown")
        });
    let current = current
        .map(|n| at(n, &["state", "falsifier", "status"]).clone())
        .unwrap_or(s("unavailable"));
    let status = if string_is(&current, "holds") {
        "counterexample"
    } else if string_is(applicability, "current") && string_is(&current, "does_not_hold") {
        if !counterexamples.is_empty() {
            "recovered"
        } else if unknown {
            "unknown"
        } else {
            "clear"
        }
    } else if !string_is(applicability, "current") && !counterexamples.is_empty() {
        "counterexample"
    } else if string_is(&current, "unknown") || string_is(&current, "error") || unknown {
        "unknown"
    } else {
        "clear"
    };
    for e in exact {
        if at(e, &["finding"]) != &V::Null {
            findings.push(obj([
                ("code", at(e, &["finding"]).clone()),
                ("subject", s(subject)),
                ("object_id", at(e, &["claim_id"]).clone()),
                ("detail", s("historical replay was not verified")),
            ]));
        }
    }
    let ids = counterexamples
        .iter()
        .map(|e| text(at(e, &["claim_id"])).map(str::to_owned))
        .collect::<Result<BTreeSet<_>>>()?;
    Ok(Some(obj([
        ("version", one()),
        ("applicability", applicability.clone()),
        ("status", s(status)),
        ("current_falsifier", current),
        ("complete", V::Bool(!unknown)),
        (
            "counterexample_claim_ids",
            V::List(ids.into_iter().map(V::Text).collect()),
        ),
        ("episodes", V::List(episodes.to_vec())),
        ("findings", V::List(findings)),
    ])))
}

pub(crate) fn replay(
    projection: &V,
    runtime: Option<&crate::reasoning_runtime::Runtime>,
) -> Result<Map> {
    use crate::{reasoning_assessment as A, reasoning_runtime::OperationalBounds};
    let mut results = Map::new();
    let mut count = 0;
    for observation in observations(projection)? {
        for claim in list(at(observation, &["claims"]))? {
            let subject = text(at(claim, &["subject"]))?;
            let mut e = episode(observation, claim)?;
            let m = map_mut(&mut e)?;
            let result = (|| -> Result<()> {
                count += 1;
                require(count <= 64, "temporal_replay_limit")?;
                let historical =
                    Snapshot::from_json(text(at(observation, &["snapshot"]))?.as_bytes())?;
                m.insert("snapshot_id".into(), s(historical.snapshot_id()));
                m.insert("as_of".into(), at(historical.data(), &["as_of"]).clone());
                let report = at(observation, &["assessment"]);
                let retained = if *report == V::Null {
                    None
                } else {
                    Some(H::validate_v2(&historical, report)?)
                };
                let mut bounds = OperationalBounds::default();
                let mut policy = "focused-review/v1";
                if let Some(r) = &retained {
                    require(
                        map(at(r, &["nodes"]))?.contains_key(subject),
                        "temporal subject absent from retained assessment",
                    )?;
                    policy = text(at(r, &["attention_policy"]))?;
                    let b = map(at(r, &["operational_limits"]))?;
                    let number = |k: &str| -> Result<usize> {
                        let V::Integer(n) = &b[k] else {
                            return Err(error("invalid assessment operational limits"));
                        };
                        n.as_str()
                            .parse()
                            .map_err(|_| error("invalid assessment operational limits"))
                    };
                    bounds.timeout =
                        std::time::Duration::from_secs(number("timeout_seconds")? as u64);
                    bounds.batch_requests = number("batch_requests")?;
                    bounds.input_bytes = number("input_bytes")?;
                    bounds.output_bytes = number("output_bytes")?;
                }
                let replayed = A::assess(
                    &historical,
                    Some(&[subject.into()]),
                    policy,
                    runtime,
                    bounds,
                )?;
                let node = at(&replayed, &["nodes", subject]);
                if let Some(r) = &retained {
                    require(
                        digest(&semantic_node(node)?)?
                            == digest(&semantic_node(at(r, &["nodes", subject]))?)?,
                        "temporal replay result mismatch",
                    )?;
                }
                let result = at(node, &["state", "falsifier"]);
                let outcome = outcome(at(result, &["status"]));
                m.insert("verification".into(), s("verified"));
                m.insert("result".into(), result.clone());
                m.insert("outcome".into(), s(outcome));
                if outcome == "unknown" {
                    m.insert(
                        "finding".into(),
                        s(&format!(
                            "temporal_falsifier_{}",
                            text(at(result, &["status"]))?
                        )),
                    );
                }
                Ok(())
            })();
            if let Err(error) = result {
                m.insert(
                    "finding".into(),
                    s(error
                        .0
                        .split(':')
                        .next()
                        .filter(|s| !s.is_empty())
                        .unwrap_or("temporal_replay_failed")),
                );
            }
            let V::List(episodes) = results
                .entry(subject.into())
                .or_insert_with(|| V::List(vec![]))
            else {
                unreachable!()
            };
            episodes.push(e);
        }
    }
    for episodes in results.values_mut() {
        let V::List(e) = episodes else { unreachable!() };
        e.sort_by_key(|e| {
            ["claim_id", "snapshot_id", "operation", "phase"]
                .map(|k| text(at(e, &[k])).unwrap_or("").to_owned())
        });
    }
    Ok(results)
}
