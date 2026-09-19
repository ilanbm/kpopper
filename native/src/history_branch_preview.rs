//! Source-free adoption choices, named proposals and target evidence availability.
use crate::{
    Result, history_adapter as D,
    history_authoring::{empty, obj, s, strings},
    history_authority as A,
    history_authority::Files,
    history_branch::{self as B, Observation},
    history_branch_audit as Audit,
    history_capture::Capture,
    history_contract::*,
    history_hypotheses as HH, history_reduce as R,
    history_view::{list, map_mut},
    identity::sha256,
    reasoning_snapshot::entries,
    require,
    value::TypedValue as V,
};
use std::collections::{BTreeMap, BTreeSet};
fn subjects(target: &Capture, sources: &[&Capture]) -> Result<Map> {
    let actual = A::committed_objects(&target.marker, &target.commits, &target.object_bytes)?;
    let target_raw = target
        .object_bytes
        .iter()
        .filter(|((_, id), _)| actual.contains_key(id))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    require(
        actual == target.objects
            && R::reduce_bytes(&target_raw, Some(map(&map(&target.state)?["rules"])?), None)?
                == target.state,
        "invalid_target_capture",
    )?;
    let mut incoming = Map::new();
    let mut raw = target.object_bytes.clone();
    let mut count = BTreeMap::<String, usize>::new();
    for source in sources {
        for subject in map(&map(&source.state)?["subjects"])?.keys() {
            *count.entry(subject.clone()).or_default() += 1;
        }
        for (id, value) in &source.objects {
            require(
                incoming.get(id).is_none_or(|v| v == value),
                "identity_mismatch",
            )?;
            incoming.insert(id.clone(), value.clone());
        }
        for (key, bytes) in &source.object_bytes {
            require(
                raw.get(key).is_none_or(|v| v == bytes),
                "object_bytes_mismatch",
            )?;
            raw.insert(key.clone(), bytes.clone());
        }
    }
    validate_closure(&incoming)?;
    let mut combined = target.objects.clone();
    for (id, value) in &incoming {
        require(
            combined.get(id).is_none_or(|v| v == value),
            "identity_mismatch",
        )?;
        combined.insert(id.clone(), value.clone());
    }
    let incoming_raw = raw
        .iter()
        .filter(|((_, id), _)| incoming.contains_key(id))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let combined_raw = raw
        .iter()
        .filter(|((_, id), _)| combined.contains_key(id))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let incoming_state = R::reduce_bytes(
        &incoming_raw,
        Some(map(&map(&sources[0].state)?["rules"])?),
        None,
    )?;
    let state = R::reduce_bytes(
        &combined_raw,
        Some(map(&map(&target.state)?["rules"])?),
        None,
    )?;
    let targets = map(&map(&target.state)?["subjects"])?;
    let incoming_subjects = map(&map(&incoming_state)?["subjects"])?;
    let combined_subjects = map(&map(&state)?["subjects"])?;
    let mut result = Map::new();
    for (subject, incoming) in incoming_subjects {
        result.insert(
            subject.clone(),
            obj([
                (
                    "requires_choice",
                    V::Bool(targets.contains_key(subject) || count[subject] > 1),
                ),
                (
                    "target_heads",
                    targets
                        .get(subject)
                        .and_then(|v| map(v).ok())
                        .and_then(|m| m.get("heads"))
                        .cloned()
                        .unwrap_or(V::List(vec![])),
                ),
                ("incoming_heads", map(incoming)?["heads"].clone()),
                (
                    "combined_heads",
                    map(&combined_subjects[subject])?["heads"].clone(),
                ),
                (
                    "claims",
                    strings(
                        combined
                            .iter()
                            .filter(|(_, v)| {
                                map(v).is_ok_and(|m| {
                                    string_is(&m["subject"], subject)
                                        && !string_is(&m["kind"], "act")
                                })
                            })
                            .map(|(id, _)| id.clone()),
                    ),
                ),
            ]),
        );
    }
    Ok(result)
}
fn evidence_status(required: &V, evidence: Option<&Files>) -> Result<Vec<V>> {
    let mut result = vec![];
    let mut total = 0usize;
    for (relative, item) in map(required)? {
        let status = if let Some(evidence) = evidence {
            if let Some(raw) = evidence.get(relative) {
                total = total.saturating_add(raw.len());
                require(
                    raw.len() <= crate::reasoning_snapshot::MAX_REQUEST_BYTES
                        && total <= B::MAX_BYTES,
                    "branch_target_evidence_limit",
                )?;
                if string_is(&map(item)?["sha256"], &sha256(raw)) {
                    "matching"
                } else {
                    "conflicting"
                }
            } else {
                "missing"
            }
        } else {
            "unassessed"
        };
        let mut row = item.clone();
        map_mut(&mut row)?.insert("status".into(), s(status));
        result.push(row);
    }
    Ok(result)
}
pub fn preview(target: &Capture, observation: &Observation, evidence: Option<&Files>) -> Result<V> {
    let source = B::validate(&observation.envelope, &observation.files)?;
    let e = map(&observation.envelope)?;
    let m = map(&e["manifest"])?;
    let mut subjects = subjects(target, &[&source])?;
    let adapted = D::from_store_capture(&source)?;
    let (named, _) = HH::layers(adapted.projection(), adapted.document())?;
    let mut proposals = Map::new();
    for (name, item) in map(&named)? {
        let item = map(item)?;
        proposals.insert(
            name.clone(),
            obj([
                ("kind", s("immutable")),
                ("head", item["head"].clone()),
                ("subjects", strings(entries(&item["doc"])?.into_keys())),
            ]),
        );
    }
    for (name, item) in map(&map(&m["snapshot"])?["hypotheses"])? {
        if map(&named)?.contains_key(name) {
            continue;
        }
        let item = map(item)?;
        let entries = entries(&item["document"])?;
        proposals.insert(
            name.clone(),
            obj([
                ("kind", s("physical_observation")),
                ("head", item["head"].clone()),
                ("subjects", strings(entries.keys().cloned())),
                ("admission", s("proposed_only")),
            ]),
        );
        for subject in entries.keys() {
            if subjects.contains_key(subject) {
                continue;
            }
            let targets = map(&map(&target.state)?["subjects"])?;
            let heads = targets
                .get(subject)
                .and_then(|v| map(v).ok())
                .and_then(|v| v.get("heads"))
                .cloned()
                .unwrap_or(V::List(vec![]));
            subjects.insert(
                subject.clone(),
                obj([
                    ("requires_choice", V::Bool(targets.contains_key(subject))),
                    ("target_heads", heads.clone()),
                    ("incoming_heads", V::List(vec![])),
                    ("combined_heads", heads),
                    (
                        "claims",
                        strings(
                            target
                                .objects
                                .iter()
                                .filter(|(_, v)| {
                                    map(v).is_ok_and(|m| {
                                        string_is(&m["subject"], subject)
                                            && !string_is(&m["kind"], "act")
                                    })
                                })
                                .map(|(id, _)| id.clone()),
                        ),
                    ),
                ]),
            );
        }
    }
    for (subject, item) in &mut subjects {
        let source_state = map(&map(&source.state)?["subjects"])?
            .get(subject)
            .cloned()
            .unwrap_or_else(empty);
        let source_state = map(&source_state)?;
        map_mut(item)?.insert(
            "source_acceptance".into(),
            source_state
                .get("acceptance")
                .cloned()
                .unwrap_or(s("proposed")),
        );
        map_mut(item)?.insert(
            "source_proposals".into(),
            source_state
                .get("proposals")
                .cloned()
                .unwrap_or(V::List(vec![])),
        );
    }
    Ok(obj([
        ("source_revision", e["revision"].clone()),
        ("source", m["source"].clone()),
        ("target_authority", target.marker.clone()),
        ("target_baseline", target.baseline.clone()),
        ("subjects", V::Map(subjects)),
        ("named_proposals", V::Map(proposals)),
        (
            "evidence",
            V::List(evidence_status(
                &Audit::evidence_requirements(observation, &source)?,
                evidence,
            )?),
        ),
    ]))
}
pub fn preview_set(
    target: &Capture,
    observations: &[Observation],
    evidence: Option<&Files>,
) -> Result<V> {
    let (ordered, revision) = Audit::source_set(observations)?;
    let mut subjects = subjects(target, &ordered.iter().map(|(_, c)| c).collect::<Vec<_>>())?;
    let mut proposals = Map::new();
    let mut rows = vec![];
    let mut count = BTreeMap::<String, usize>::new();
    let mut source_rows = vec![];
    let mut revisions = vec![];
    for (observation, _) in &ordered {
        let e = map(&observation.envelope)?;
        let revision = &e["revision"];
        revisions.push(revision.clone());
        let mut source = map(&e["manifest"])?["source"].clone();
        map_mut(&mut source)?.insert("revision".into(), revision.clone());
        source_rows.push(source);
        let part = preview(target, observation, evidence)?;
        let part = map(&part)?;
        for (subject, item) in map(&part["subjects"])? {
            *count.entry(subject.clone()).or_default() += 1;
            let dest = subjects.entry(subject.clone()).or_insert(item.clone());
            let item = map(item)?;
            let dispositions = map_mut(dest)?
                .entry("source_dispositions".into())
                .or_insert(V::List(vec![]));
            if let V::List(v) = dispositions {
                v.push(obj([
                    ("revision", revision.clone()),
                    ("acceptance", item["source_acceptance"].clone()),
                    ("proposals", item["source_proposals"].clone()),
                ]));
            }
        }
        for (name, item) in map(&part["named_proposals"])? {
            let mut row = item.clone();
            map_mut(&mut row)?.insert("revision".into(), revision.clone());
            let items = proposals.entry(name.clone()).or_insert(V::List(vec![]));
            if let V::List(v) = items {
                v.push(row);
            }
        }
        for row in list(&part["evidence"])? {
            let mut row = row.clone();
            map_mut(&mut row)?.insert("source_revision".into(), revision.clone());
            rows.push(row);
        }
    }
    for (subject, count) in count {
        if count > 1 {
            map_mut(subjects.get_mut(&subject).unwrap())?
                .insert("requires_choice".into(), V::Bool(true));
        }
    }
    let mut hashes = BTreeMap::<String, BTreeSet<String>>::new();
    for row in &rows {
        let row = map(row)?;
        hashes
            .entry(text(&row["target_relative"])?.into())
            .or_default()
            .insert(text(&row["sha256"])?.into());
    }
    Ok(obj([
        ("source_set_revision", s(&revision)),
        ("source_revisions", V::List(revisions)),
        ("sources", V::List(source_rows)),
        ("target_authority", target.marker.clone()),
        ("target_baseline", target.baseline.clone()),
        ("subjects", V::Map(subjects)),
        ("named_proposals", V::Map(proposals)),
        ("evidence", V::List(rows)),
        (
            "evidence_conflicts",
            strings(
                hashes
                    .into_iter()
                    .filter(|(_, v)| v.len() > 1)
                    .map(|(k, _)| k),
            ),
        ),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde_json::Value as J;
    fn files(value: &J) -> Files {
        value
            .as_object()
            .unwrap()
            .iter()
            .map(|(p, r)| (p.clone(), STANDARD.decode(r.as_str().unwrap()).unwrap()))
            .collect()
    }
    #[test]
    fn previews_match_choices_named_groups_and_all_evidence_availability_states() {
        let cases: J = serde_json::from_str(include_str!(
            "../tests/fixtures/history-branch-adoption.json"
        ))
        .unwrap();
        let expected: J = serde_json::from_str(include_str!(
            "../tests/fixtures/history-branch-preview.json"
        ))
        .unwrap();
        for row in expected.as_array().unwrap() {
            let index = row["index"].as_u64().unwrap() as usize;
            let case = &cases[index];
            let target = crate::history_bundle::capture(&files(&case["target"]), None).unwrap();
            let sources = case["sources"]
                .as_array()
                .unwrap()
                .iter()
                .map(|o| Observation {
                    envelope: V::from_tagged(&o["envelope"]).unwrap(),
                    files: files(&o["files"]),
                })
                .collect::<Vec<_>>();
            let mode = row["mode"].as_str().unwrap();
            let mut evidence = files(&case["evidence"]);
            if mode == "missing" {
                evidence.clear();
            } else if mode == "changed" {
                for value in evidence.values_mut() {
                    *value = b"changed evidence".to_vec();
                }
            }
            let evidence = if mode == "unassessed" {
                None
            } else {
                Some(&evidence)
            };
            let result = if case["multiple"].as_bool().unwrap() {
                preview_set(&target, &sources, evidence)
            } else {
                preview(&target, &sources[0], evidence)
            };
            if let Some(output) = row.get("output") {
                assert_eq!(
                    result.unwrap_or_else(|e| panic!("{index} {mode}: {e}")),
                    V::from_tagged(output).unwrap(),
                    "{index} {mode}"
                );
            } else {
                assert!(result.is_err(), "{index} {mode}");
            }
        }
    }
}
