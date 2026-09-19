//! Temporal evidence follows committed causal frontiers, never recording time.
use crate::{
    Result, history_adapter as Adapter, history_authority as A,
    history_capture::Capture,
    history_contract::*,
    history_transaction as T,
    history_view::{self as W, list, map_mut},
    history_yaml as Y,
    reasoning_snapshot::{CaptureOptions, Snapshot},
    require,
    value::{Integer, TypedValue as V},
};
use std::collections::{BTreeMap, BTreeSet};
const ROLES: [&str; 3] = ["deps", "snapshot", "predicate"];
const MAX_VISITS: usize = 1_000_000;
fn s(v: &str) -> V {
    V::Text(v.into())
}
fn obj(v: impl IntoIterator<Item = (&'static str, V)>) -> V {
    V::Map(v.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
fn one() -> V {
    V::Integer(Integer::new("1").unwrap())
}
fn finding(code: &str, subject: &str, version: &str, detail: &str) -> V {
    obj([
        ("code", s(code)),
        ("subject", s(subject)),
        ("object_id", s(version)),
        ("detail", s(detail)),
    ])
}
fn numeric_one(v: &V) -> bool {
    is_int(v, "1") || *v == V::Bool(true) || matches!(v,V::Float(n) if n.get() == 1.0)
}
struct Causal<'a> {
    capture: &'a Capture,
    manifests: BTreeMap<String, V>,
    closures: BTreeMap<(String, bool), BTreeSet<String>>,
    visits: usize,
}
impl Causal<'_> {
    fn parents(&self, operation: &str) -> Result<&Map> {
        let m = self
            .manifests
            .get(operation)
            .ok_or_else(|| error("incomplete_commit"))?;
        map(&map(m)?["parents"])
    }
    fn operations(&self, operation: &str, after: bool) -> Result<BTreeSet<String>> {
        let mut pending = if after {
            vec![operation.into()]
        } else {
            self.parents(operation)?.keys().cloned().collect()
        };
        let mut seen = BTreeSet::new();
        while let Some(current) = pending.pop() {
            if !seen.insert(current.clone()) {
                continue;
            }
            pending.extend(self.parents(&current)?.keys().cloned());
            require(seen.len() <= MAX_OBJECTS, "history_limit")?;
        }
        Ok(seen)
    }
    fn frontier(&mut self, operation: &str, after: bool) -> Result<Map> {
        let key = (operation.into(), after);
        if !self.closures.contains_key(&key) {
            // Charge while traversing, including failed attempts, as the oracle does.
            let mut pending = if after {
                vec![operation.into()]
            } else {
                self.parents(operation)?.keys().cloned().collect()
            };
            let mut seen = BTreeSet::new();
            let mut ids = BTreeSet::new();
            while let Some(current) = pending.pop() {
                if !seen.insert(current.clone()) {
                    continue;
                }
                let m = self
                    .manifests
                    .get(&current)
                    .ok_or_else(|| error("incomplete_commit"))?;
                self.visits += 1;
                require(self.visits <= MAX_VISITS, "temporal_frontier_limit")?;
                for item in list(&map(m)?["objects"])? {
                    ids.insert(text(&map(item)?["id"])?.into());
                }
                pending.extend(self.parents(&current)?.keys().cloned());
                require(seen.len() <= MAX_OBJECTS, "history_limit")?;
            }
            self.closures.insert(key.clone(), ids);
        }
        let objects = self.closures[&key]
            .iter()
            .map(|v| {
                self.capture
                    .objects
                    .get(v)
                    .cloned()
                    .map(|o| (v.clone(), o))
                    .ok_or_else(|| error("incomplete_commit"))
            })
            .collect::<Result<Map>>()?;
        let state = W::selected_state(self.capture, &objects, &self.capture.object_bytes)?;
        Ok(map(&map(&state)?["subjects"])?
            .iter()
            .filter_map(|(subject, state)| {
                let state = map(state).ok()?;
                if !string_is(&state["acceptance"], "accepted") {
                    return None;
                }
                state
                    .get("head")
                    .map(|head| (subject.clone(), head.clone()))
            })
            .collect())
    }
    fn reconstructed(&self, operation: &str, after: bool, frontier: &Map) -> Result<Snapshot> {
        let operations = self.operations(operation, after)?;
        let commits = operations
            .into_iter()
            .map(|op| (op.clone(), self.capture.commits[&op].clone()))
            .collect();
        let mut document = W::template(&commits)?;
        let mut profile = None;
        for (subject, version) in frontier {
            let object = &self.capture.objects[text(version)?];
            let authored = Adapter::authored(object)?;
            let next_profile = text(&authored["profile"])?;
            require(
                profile.is_none_or(|p| p == next_profile),
                "incompatible_authored_profiles",
            )?;
            profile = Some(next_profile);
            let schema = map_mut(
                map_mut(&mut document)?
                    .entry("schema".into())
                    .or_insert_with(|| V::Map(Map::new())),
            )
            .map_err(|_| error("invalid_authored_mapping"))?;
            for role in ROLES {
                let field = &map(&authored["fields"])?[role];
                require(
                    schema.get(role).is_none_or(|old| old == field),
                    "incompatible_field_roles",
                )?;
                schema.insert(role.into(), field.clone());
            }
            let collection = map_mut(&mut document)?
                .entry(text(&authored["collection"])?.into())
                .or_insert_with(|| V::Map(Map::new()));
            let collection = map_mut(collection).map_err(|_| error("duplicate_projection"))?;
            require(!collection.contains_key(subject), "duplicate_projection")?;
            collection.insert(subject.clone(), Adapter::adapt_body(object)?);
        }
        require(profile == Some("core/v1"), "incompatible_authored_profiles")?;
        map_mut(
            map_mut(&mut document)?
                .entry("meta".into())
                .or_insert_with(|| V::Map(Map::new())),
        )?
        .remove("history");
        Snapshot::from_data(&document, CaptureOptions::default())
    }
}

pub(crate) fn capture(captured: &Capture, required: &BTreeSet<String>) -> Result<Option<V>> {
    if !required.contains(A::TEMPORAL_APPLICABILITY) {
        return Ok(None);
    }
    let mut manifests = BTreeMap::new();
    for (op, raw) in &captured.commits {
        let manifest = Y::decode_document(raw)?;
        A::validate_commit(&manifest)?;
        manifests.insert(op.clone(), manifest);
    }
    let mut causal = Causal {
        capture: captured,
        manifests,
        closures: BTreeMap::new(),
        visits: 0,
    };
    let mut children: BTreeMap<String, Vec<String>> = causal
        .manifests
        .keys()
        .map(|k| (k.clone(), Vec::new()))
        .collect();
    let mut remaining = BTreeMap::new();
    for operation in causal.manifests.keys() {
        let parents = causal.parents(operation)?;
        remaining.insert(operation.clone(), parents.len());
        for parent in parents.keys() {
            children
                .get_mut(parent)
                .ok_or_else(|| error("incomplete_commit"))?
                .push(operation.clone());
        }
    }
    let mut ready: BTreeSet<String> = remaining
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(op, _)| op.clone())
        .collect();
    let mut lineage = BTreeMap::new();
    while let Some(operation) = ready.pop_first() {
        let m = map(&causal.manifests[&operation])?;
        let active = matches!(m.get("requires"),Some(V::List(v)) if v.iter().any(|v|string_is(v,A::TEMPORAL_APPLICABILITY)))
            || causal.parents(&operation)?.keys().any(|p| lineage[p]);
        lineage.insert(operation.clone(), active);
        for child in &children[&operation] {
            let n = remaining.get_mut(child).unwrap();
            *n -= 1;
            if *n == 0 {
                ready.insert(child.clone());
            }
        }
    }
    require(lineage.len() == causal.manifests.len(), "cyclic_commits")?;
    let mut temporal = Map::new();
    for (vid, object) in &captured.objects {
        if A::has_temporal_metadata(object)? {
            temporal.insert(vid.clone(), object.clone());
        }
    }
    let mut observations = Vec::new();
    let mut findings = Vec::new();
    let mut used = Y::compact_json_size(&obj([
        ("version", one()),
        ("complete", V::Bool(true)),
        ("observations", V::List(vec![])),
        ("findings", V::List(vec![])),
    ]));
    let mut truncated = false;
    let mut missing_contexts = 0;
    let mut missing: BTreeMap<(String, String), usize> = BTreeMap::new();
    for operation in captured.commits.keys() {
        let receipt = T::validate_receipt(&map(&causal.manifests[operation])?["receipt"])?;
        for phase in ["before", "after"] {
            let evidence = map(&map(&receipt)?[phase])?;
            let recorded = evidence.get("temporal_replay").filter(|v| **v != V::Null);
            let after = phase == "after";
            if recorded.is_none()
                && !(if after {
                    lineage[operation]
                } else {
                    causal.parents(operation)?.keys().any(|p| lineage[p])
                })
            {
                continue;
            }
            let frontier = match causal.frontier(operation, after) {
                Ok(frontier) => frontier,
                Err(e) if e.0 == "temporal_frontier_limit" => {
                    truncated = true;
                    continue;
                }
                Err(e) => return Err(e),
            };
            let expected: Map = frontier
                .iter()
                .filter(|(_, v)| text(v).is_ok_and(|v| temporal.contains_key(v)))
                .map(|(s, v)| (s.clone(), v.clone()))
                .collect();
            let (replay, assessment, kind) = if let Some(replay) = recorded {
                (
                    replay.clone(),
                    evidence.get("assessment").cloned().unwrap_or(V::Null),
                    "recorded_receipt",
                )
            } else {
                if expected.is_empty() {
                    continue;
                }
                match causal
                    .reconstructed(operation, after, &frontier)
                    .and_then(|snapshot| snapshot.to_json())
                {
                    Ok(snapshot) => (
                        obj([
                            ("version", one()),
                            ("snapshot", s(&snapshot)),
                            ("claims", V::Map(expected.clone())),
                        ]),
                        V::Null,
                        "reconstructed_committed_world",
                    ),
                    Err(_) => {
                        missing_contexts += 1;
                        for (subject, vid) in &expected {
                            *missing
                                .entry((subject.clone(), text(vid)?.into()))
                                .or_default() += 1;
                        }
                        continue;
                    }
                }
            };
            let r = schema(&replay, &["version", "snapshot", "claims"], &[])
                .map_err(|_| error("invalid_temporal_replay"))?;
            require(
                numeric_one(&r["version"])
                    && matches!(r["snapshot"], V::Text(_))
                    && matches!(r["claims"], V::Map(_))
                    && matches!(assessment, V::Null | V::Map(_)),
                "invalid_temporal_replay",
            )?;
            require(
                r["claims"] == V::Map(expected.clone()),
                "temporal_causal_frontier_mismatch",
            )?;
            let snapshot = Snapshot::from_json(text(&r["snapshot"])?.as_bytes())
                .map_err(|_| error("invalid_temporal_replay"))?;
            let snapshot_data = snapshot.to_data();
            let nodes = map(&map(&snapshot_data)?["nodes"])?;
            require(
                nodes.keys().eq(frontier.keys()),
                "temporal_causal_frontier_mismatch",
            )?;
            for (subject, version) in &frontier {
                let object = &captured.objects[text(version)?];
                let authored = Adapter::authored(object)?;
                let node = map(&nodes[subject])?;
                let fields = ROLES
                    .iter()
                    .map(|role| {
                        (
                            role.to_string(),
                            map(&authored["fields"]).unwrap()[*role].clone(),
                        )
                    })
                    .collect();
                require(
                    node["body"].digest()? == Adapter::adapt_body(object)?.digest()?
                        && node["collection"] == authored["collection"]
                        && node["fields"] == V::Map(fields),
                    "temporal_causal_frontier_mismatch",
                )?;
            }
            let mut claims = Vec::new();
            for (subject, version) in &expected {
                let object = map(&temporal[text(version)?])?;
                require(
                    string_is(&object["subject"], subject),
                    "temporal_claim_mismatch",
                )?;
                if assessment != V::Null {
                    let body = map(&assessment)?
                        .get("nodes")
                        .and_then(|v| map(v).ok())
                        .and_then(|v| v.get(subject))
                        .and_then(|v| map(v).ok())
                        .and_then(|v| v.get("body"))
                        .unwrap_or(&V::Null);
                    require(
                        body.digest()? == object["body"].digest()?,
                        "temporal_claim_mismatch",
                    )?;
                }
                let body = map(&object["body"])?;
                let predicate = text(&map(&map(&object["authored"])?["fields"])?["predicate"])?;
                claims.push(obj([
                    ("subject", s(subject)),
                    ("claim_id", version.clone()),
                    (
                        "applicability",
                        map(&body["temporal"])?["applicability"].clone(),
                    ),
                    (
                        "predicate_digest",
                        s(&body.get(predicate).unwrap_or(&V::Null).digest()?),
                    ),
                    (
                        "anchors",
                        V::Map(
                            ["on", "at", "applies"]
                                .iter()
                                .map(|k| {
                                    (k.to_string(), object.get(*k).cloned().unwrap_or(V::Null))
                                })
                                .collect(),
                        ),
                    ),
                ]));
            }
            if claims.is_empty() {
                continue;
            }
            let observation = obj([
                ("operation", s(operation)),
                ("phase", s(phase)),
                ("evidence_kind", s(kind)),
                ("snapshot", r["snapshot"].clone()),
                ("assessment", assessment),
                ("claims", V::List(claims)),
            ]);
            if observations.len() >= 64 {
                truncated = true;
                continue;
            }
            used = used.saturating_add(Y::compact_json_size(&observation));
            if used > 4 * 1024 * 1024 {
                truncated = true;
                continue;
            }
            observations.push(observation);
        }
    }
    if truncated {
        findings.push(finding(
            "temporal_history_limit",
            "",
            "",
            "retained temporal observations exceed replay bounds",
        ));
    }
    for ((subject, version), count) in missing.iter().take(63) {
        findings.push(finding(
            "temporal_replay_unavailable",
            subject,
            version,
            &format!("{count} committed contexts lack exact Snapshot replay evidence"),
        ));
    }
    if missing_contexts > 0 && (missing.is_empty() || missing.len() > 63) {
        findings.push(finding(
            "temporal_replay_unavailable",
            "",
            "",
            &format!("{missing_contexts} committed contexts lack exact Snapshot replay evidence"),
        ));
    }
    // Every retained observation was validated above; missing contexts have their
    // own explicit finding. Unaccepted proposals never create missing evidence.
    Ok(Some(obj([
        ("version", one()),
        ("complete", V::Bool(findings.is_empty())),
        ("observations", V::List(observations)),
        ("findings", V::List(findings)),
    ])))
}
