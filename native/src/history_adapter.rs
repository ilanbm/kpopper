//! Pure history-to-computation adaptation. No store read or acceptance is inferred.
use crate::{
    Result, history_authority as A,
    history_capture::Capture,
    history_contract::*,
    history_projection::{self as P, CapturedHistory},
    history_reduce,
    history_view::{self as W, list, map_mut},
    history_yaml as Y,
    identity::sha256,
    require,
    value::{Integer, TypedValue as V},
};
use std::collections::{BTreeMap, BTreeSet};
fn s(v: &str) -> V {
    V::Text(v.into())
}
fn n() -> V {
    V::Integer(Integer::new("1").unwrap())
}
fn obj(fields: impl IntoIterator<Item = (&'static str, V)>) -> V {
    V::Map(fields.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
fn strings(values: impl IntoIterator<Item = String>) -> V {
    V::List(values.into_iter().map(V::Text).collect())
}
fn authored(obj: &V) -> Result<&Map> {
    let m = map(obj)?;
    let a = m
        .get("authored")
        .and_then(|v| map(v).ok())
        .ok_or_else(|| error("unresolved_authored_mapping"))?;
    let fields = map(&a["fields"])?;
    require(
        ["deps", "snapshot", "predicate"]
            .iter()
            .all(|r| fields.contains_key(*r))
            && ["deps", "snapshot", "predicate"]
                .iter()
                .filter_map(|r| fields.get(*r))
                .map(|v| text(v).unwrap_or(""))
                .collect::<BTreeSet<_>>()
                .len()
                == 3,
        "invalid_authored_mapping",
    )?;
    require(
        !["meta", "schema", "record", "also"].contains(&text(&a["collection"])?),
        "invalid_authored_mapping",
    )?;
    Ok(a)
}
pub(crate) fn adapt_body(value: &V) -> Result<V> {
    let a = authored(value)?;
    let o = map(value)?;
    let mut body = o["body"].clone();
    let V::Map(body_map) = &mut body else {
        return Ok(body);
    };
    let field = text(&map(&a["fields"])?["deps"])?;
    let pins = map(&o["pins"])?;
    let empty = Map::new();
    let gaps = o.get("pin_gaps").map(map).transpose()?.unwrap_or(&empty);
    let names = pins
        .keys()
        .chain(gaps.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    if let Some(deps) = body_map.get(field) {
        if matches!(deps, V::Map(_)) {
            require(
                !gaps.is_empty() || deps.digest()? == o["pins"].digest()?,
                "pin_dependency_mismatch",
            )?;
            body_map.insert(field.into(), strings(names));
        } else {
            let deps = list(deps).map_err(|_| error("pin_dependency_mismatch"))?;
            let got = deps
                .iter()
                .map(|v| text(v).map(str::to_owned))
                .collect::<Result<BTreeSet<_>>>()
                .map_err(|_| error("pin_dependency_mismatch"))?;
            require(got == names, "pin_dependency_mismatch")?;
        }
    } else if !pins.is_empty() || string_is(&o["kind"], "judgment") {
        body_map.insert(field.into(), strings(pins.keys().cloned()));
    }
    Ok(body)
}
struct Work<'a> {
    objects: &'a Map,
    projection: V,
    budget: usize,
}
impl Work<'_> {
    fn add_finding(&mut self, finding: V) -> Result<()> {
        let p = map_mut(&mut self.projection)?;
        let i = map_mut(p.get_mut("integrity").unwrap())?;
        let V::List(findings) = i.get_mut("findings").unwrap() else {
            return Err(error("invalid_integrity"));
        };
        if !findings.contains(&finding) {
            findings.push(finding);
        }
        map_mut(p.get_mut("coverage").unwrap())?.insert("complete".into(), V::Bool(false));
        Ok(())
    }
    fn gap(&mut self, finding: V) -> Result<()> {
        map_mut(map_mut(&mut self.projection)?.get_mut("integrity").unwrap())?
            .insert("complete".into(), V::Bool(false));
        self.add_finding(finding)
    }
    fn incomplete(&mut self, code: &str, subject: &str, version: &str, detail: &str) -> Result<()> {
        require(
            map(&map(&self.projection)?["integrity"])?["complete"] == V::Bool(false),
            code,
        )?;
        self.add_finding(obj([
            ("code", s(code)),
            ("subject", s(subject)),
            ("object_id", s(version)),
            ("detail", s(detail)),
        ]))
    }
    fn witness(&mut self, subject: &str, version: &str) -> Result<V> {
        let existing = map(&map(&self.projection)?["pins"])?.get(version).cloned();
        let candidate = self.objects.get(version).cloned();
        if let Some(candidate) = &candidate {
            let c = map(candidate)?;
            require(
                string_is(&c["subject"], subject) && !string_is(&c["kind"], "act"),
                "reference_mismatch",
            )?;
            for finding in P::pin_gap_findings(candidate)? {
                self.gap(finding)?;
            }
        }
        if let Some(existing) = existing {
            let e = map(&existing)?;
            require(string_is(&e["subject"], subject), "reference_mismatch")?;
            if string_is(&e["status"], "recorded") {
                let c = candidate
                    .as_ref()
                    .ok_or_else(|| error("pin_object_mismatch"))?;
                require(e["object"].digest()? == c.digest()?, "pin_object_mismatch")?;
            } else {
                self.incomplete(
                    &format!("pin_{}", text(&e["status"])?),
                    subject,
                    version,
                    "pin evidence is unavailable",
                )?;
            }
            return Ok(existing);
        }
        let result = obj([
            ("subject", s(subject)),
            ("version", s(version)),
            (
                "status",
                s(if candidate.is_some() {
                    "recorded"
                } else {
                    "unavailable"
                }),
            ),
            ("object", candidate.unwrap_or(V::Null)),
        ]);
        if map(&result)?["object"] == V::Null {
            self.incomplete("missing_pin", subject, version, "pin was not captured")?;
        }
        let addition = V::Map(Map::from([(version.into(), result.clone())]));
        self.budget += Y::compact_json_size(&addition);
        require(self.budget <= 8 * 1024 * 1024, "history_limit")?;
        map_mut(map_mut(&mut self.projection)?.get_mut("pins").unwrap())?
            .insert(version.into(), result.clone());
        Ok(result)
    }
}
pub fn capture_history(
    objects: &Map,
    projection: &V,
    document: Option<&V>,
) -> Result<CapturedHistory> {
    P::validate_projection(projection)?;
    require(objects.len() <= MAX_OBJECTS, "history_limit")?;
    for (id, o) in objects {
        validate_object(o)?;
        require(string_is(&map(o)?["id"], id), "identity_mismatch")?;
    }
    let mut document = document.cloned().unwrap_or(V::Map(Map::new()));
    Y::validate_value(&document, 16 * 1024 * 1024)?;
    let d = map_mut(&mut document).map_err(|_| error("invalid_document"))?;
    let meta = d.entry("meta".into()).or_insert_with(|| V::Map(Map::new()));
    let meta = map_mut(meta).map_err(|_| error("invalid_document"))?;
    if let Some(history) = meta.get("history") {
        require(
            history.digest()? == map(projection)?["baseline"].digest()?,
            "baseline_mismatch",
        )?;
    }
    meta.remove("history");
    require(
        A::document_template(&document)?.digest()? == document.digest()?,
        "invalid_document",
    )?;
    let d = map_mut(&mut document)?;
    map_mut(d.get_mut("meta").unwrap())?
        .insert("history".into(), map(projection)?["baseline"].clone());
    let schema = d
        .entry("schema".into())
        .or_insert_with(|| V::Map(Map::new()));
    map(schema).map_err(|_| error("invalid_authored_mapping"))?;
    let mut work = Work {
        objects,
        projection: projection.clone(),
        budget: Y::compact_json_size(projection),
    };
    let mut profile: Option<String> = None;
    let existing = map(&map(projection)?["pins"])?
        .iter()
        .map(|(v, w)| Ok((v.clone(), text(&map(w)?["subject"])?.to_owned())))
        .collect::<Result<Vec<_>>>()?;
    for (version, subject) in existing {
        work.witness(&subject, &version)?;
    }
    for (subject, state) in map(&map(projection)?["subjects"])? {
        let state = map(state)?;
        let mut heads = Vec::new();
        for version in list(&state["heads"])? {
            let witness = work.witness(subject, text(version)?)?;
            let w = map(&witness)?;
            if string_is(&w["status"], "recorded") {
                heads.push(w["object"].clone());
            }
        }
        for version in list(&state["open_acts"])? {
            let version = text(version)?;
            if let Some(act) = objects.get(version) {
                let a = map(act)?;
                require(
                    string_is(&a["subject"], subject) && string_is(&a["kind"], "act"),
                    "reference_mismatch",
                )?;
            } else {
                work.incomplete("missing_act", subject, version, "open act was not captured")?;
            }
        }
        if !string_is(&state["acceptance"], "accepted") {
            continue;
        }
        if heads.is_empty() || heads.len() != list(&state["heads"])?.len() {
            work.incomplete(
                "missing_accepted_head",
                subject,
                "",
                "accepted head evidence is unavailable",
            )?;
            continue;
        }
        let meanings = heads
            .iter()
            .map(|o| history_reduce::claim_meaning(map(o)?))
            .collect::<Result<BTreeSet<_>>>()?;
        require(meanings.len() == 1, "ambiguous_accepted_selection")?;
        for o in &heads {
            require_interpretable_claim(o)?;
        }
        heads.sort_by_key(|o| text(&map(o).unwrap()["id"]).unwrap().to_owned());
        let selected = &heads[0];
        let a = authored(selected)?;
        let selected_profile = text(&a["profile"])?;
        require(
            profile.as_deref().is_none_or(|p| p == selected_profile),
            "incompatible_authored_profiles",
        )?;
        profile = Some(selected_profile.into());
        let schema = map_mut(map_mut(&mut document)?.get_mut("schema").unwrap())?;
        for (role, field) in map(&a["fields"])? {
            require(
                schema.get(role).is_none_or(|v| v == field),
                "incompatible_field_roles",
            )?;
            schema.insert(role.clone(), field.clone());
        }
        for (dep, version) in map(&map(selected)?["pins"])? {
            work.witness(dep, text(version)?)?;
        }
        let collection = text(&a["collection"])?;
        let body = map_mut(&mut document)?
            .entry(collection.into())
            .or_insert_with(|| V::Map(Map::new()));
        map_mut(body)
            .map_err(|_| error("unresolved_mapping"))?
            .insert(subject.clone(), adapt_body(selected)?);
    }
    let meta = map(&map(&document)?["meta"])?;
    let declaration = meta.get("reasoning").filter(|v| **v != V::Null);
    if let Some(d) = declaration {
        require(
            map(d).ok().is_some_and(|d| {
                profile
                    .as_ref()
                    .is_none_or(|p| d.get("profile").is_some_and(|v| string_is(v, p)))
            }),
            "incompatible_authored_profiles",
        )?;
    }
    if profile.as_deref() == Some("core/v1") {
        W::core_declaration(
            &document,
            declaration.ok_or_else(|| error("missing_reasoning_declaration"))?,
        )?;
    }
    CapturedHistory::new(document, work.projection)
}
pub fn from_store_capture(capture: &Capture) -> Result<CapturedHistory> {
    let objects = &capture.objects;
    let states = map(&map(&capture.state)?["subjects"])?;
    let base = map(&capture.baseline)?;
    let mut subjects = Map::new();
    for (name, state) in states {
        let state = map(state)?;
        subjects.insert(
            name.clone(),
            obj([
                ("acceptance", state["acceptance"].clone()),
                ("heads", state["heads"].clone()),
                (
                    "open_acts",
                    map(&base["open_acts"])?
                        .get(name)
                        .cloned()
                        .unwrap_or(V::List(vec![])),
                ),
            ]),
        );
    }
    let mut reviews: BTreeMap<String, Vec<V>> =
        subjects.keys().map(|k| (k.clone(), Vec::new())).collect();
    for o in objects.values() {
        let o_map = map(o)?;
        if !string_is(&o_map["kind"], "act") {
            continue;
        }
        let body = map(&o_map["body"])?;
        if !string_is(&body["act"], "review") {
            continue;
        }
        let subject = text(&o_map["subject"])?;
        let state = map(states
            .get(subject)
            .ok_or_else(|| error("invalid_review_scope"))?)?;
        if list(&state["heads"])?
            .iter()
            .chain(list(&state["proposals"])?)
            .any(|v| v == &body["of"])
        {
            reviews.get_mut(subject).unwrap().push(o.clone());
        }
    }
    let mut dispositions = Map::new();
    let mut versions = BTreeSet::new();
    for (subject, state) in states {
        let state = map(state)?;
        dispositions.insert(
            subject.clone(),
            obj([
                ("marks", state["marks"].clone()),
                ("proposals", state["proposals"].clone()),
                ("contested_claims", state["disputed_acts"].clone()),
                ("reviews", V::List(reviews[subject].clone())),
                ("implied", state["implied"].clone()),
            ]),
        );
        for v in list(&state["heads"])?
            .iter()
            .chain(list(&state["proposals"])?)
        {
            versions.insert(text(v)?.to_owned());
        }
        for review in &reviews[subject] {
            let body = map(&map(review)?["body"])?;
            if let Some(read) = body.get("read") {
                for v in map(read)?.values() {
                    versions.insert(text(v)?.to_owned());
                }
            }
        }
    }
    let mut pins = Map::new();
    for version in &versions {
        let o = objects
            .get(version)
            .ok_or_else(|| error("incomplete_closure"))?;
        pins.insert(
            version.clone(),
            obj([
                ("subject", map(o)?["subject"].clone()),
                ("version", s(version)),
                ("status", s("recorded")),
                ("object", o.clone()),
            ]),
        );
    }
    let raw_objects = objects
        .iter()
        .map(|(vid, o)| {
            let name = text(&map(o)?["subject"])?;
            let raw = capture
                .object_bytes
                .get(&(name.into(), vid.clone()))
                .ok_or_else(|| error("incomplete_commit"))?;
            Ok((
                vid.clone(),
                obj([("subject", s(name)), ("sha256", s(&sha256(raw)))]),
            ))
        })
        .collect::<Result<Map>>()?;
    let closure = obj([
        ("authority", capture.marker.clone()),
        (
            "commits",
            V::Map(
                capture
                    .commits
                    .iter()
                    .map(|(op, raw)| (op.clone(), s(&sha256(raw))))
                    .collect(),
            ),
        ),
        ("objects", V::Map(raw_objects)),
    ]);
    let schemes = objects
        .values()
        .map(|o| {
            if map(o).unwrap().contains_key("id_scheme") {
                "typed-history/v2".into()
            } else {
                "prototype/v1".into()
            }
        })
        .collect::<BTreeSet<String>>();
    let mut projection = obj([
        ("projection_version", n()),
        ("authority", capture.marker.clone()),
        ("baseline", capture.baseline.clone()),
        ("identity_schemes", strings(schemes)),
        ("rules", map(&capture.state)?["rules"].clone()),
        ("rules_digest", s(&map(&capture.state)?["rules"].digest()?)),
        ("closure_digest", s(&closure.digest()?)),
        (
            "coverage",
            obj([
                ("scope", s("all")),
                ("subjects", strings(subjects.keys().cloned())),
                ("complete", V::Bool(true)),
            ]),
        ),
        ("subjects", V::Map(subjects)),
        ("pins", V::Map(pins)),
        ("dispositions", V::Map(dispositions)),
        (
            "integrity",
            obj([("complete", V::Bool(true)), ("findings", V::List(vec![]))]),
        ),
    ]);
    let mut required = BTreeSet::new();
    for raw in capture.commits.values() {
        let m = Y::decode_document(raw)?;
        A::validate_commit(&m)?;
        if let Some(req) = map(&m)?.get("requires") {
            for v in list(req)? {
                required.insert(text(v)?.to_owned());
            }
        }
    }
    for o in objects.values() {
        let o = map(o)?;
        if string_is(&o["kind"], "act")
            && text(&map(&o["body"])?["act"]).is_ok_and(|s| ["propose", "retire"].contains(&s))
        {
            required.insert(A::ROOT_DISPOSITION.into());
        }
    }
    if !required.is_empty() {
        map_mut(&mut projection)?.insert("requires".into(), strings(required));
    }
    let mut gaps = Vec::new();
    for v in &versions {
        gaps.extend(P::pin_gap_findings(&objects[v])?);
    }
    if !gaps.is_empty() {
        let p = map_mut(&mut projection)?;
        p.insert(
            "integrity".into(),
            obj([("complete", V::Bool(false)), ("findings", V::List(gaps))]),
        );
        map_mut(p.get_mut("coverage").unwrap())?.insert("complete".into(), V::Bool(false));
    }
    let template = W::template(&capture.commits)?;
    if let Some(origin) = map(&template)?
        .get("meta")
        .and_then(|v| map(v).ok())
        .and_then(|m| m.get("history_subset"))
        .filter(|v| **v != V::Null)
    {
        P::validate_subset_origin(origin)?;
        let origin_map = map(origin)?;
        let origin_subjects = map(&origin_map["subjects"])?;
        require(
            origin_subjects.keys().eq(states.keys()),
            "invalid_subset_coverage",
        )?;
        require(capture.commits.len() == 1, "invalid_subset_commits")?;
        let manifest = Y::decode_document(capture.commits.values().next().unwrap())?;
        let manifest = map(&manifest)?;
        let receipt = map(&manifest["receipt"])?;
        require(
            manifest["operation"] == origin_map["operation"]
                && map(&manifest["parents"])?.is_empty()
                && receipt.get("after").is_some_and(|v| {
                    v.digest().ok() == obj([("history_subset", origin.clone())]).digest().ok()
                }),
            "invalid_subset_receipt",
        )?;
        for (name, evidence) in origin_subjects {
            let mut inventory = Map::new();
            for (vid, o) in objects {
                if string_is(&map(o)?["subject"], name) {
                    inventory.insert(
                        vid.clone(),
                        s(&sha256(&capture.object_bytes[&(name.clone(), vid.clone())])),
                    );
                }
            }
            let e = map(evidence)?;
            require(
                string_is(&e["objects_digest"], &V::Map(inventory).digest()?)
                    && string_is(&e["reduction_digest"], &states[name].digest()?),
                "invalid_subset_receipt",
            )?;
        }
        let p = map_mut(&mut projection)?;
        map_mut(p.get_mut("coverage").unwrap())?.insert("scope".into(), s("selected"));
        p.insert("origin".into(), origin.clone());
        let d = map_mut(p.get_mut("dispositions").unwrap())?;
        for (name, evidence) in origin_subjects {
            map_mut(d.get_mut(name).unwrap())?.insert(
                "source_state".into(),
                map(evidence)?["source_state"].clone(),
            );
        }
    }
    capture_history(objects, &projection, Some(&template))
}
