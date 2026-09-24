//! Detached captured-history contracts. Coverage and acceptance are independent.
use crate::{
    Result, history_authority as A, history_contract::*, history_yaml as Y, require,
    value::TypedValue as V,
};
const LIMIT: usize = 8 * 1024 * 1024;
fn hash(v: &V) -> Result<()> {
    require(
        text(v).is_ok_and(|s| s.len() == 64 && crate::history_paths::object_id(s)),
        "invalid_identifier",
    )
}
fn list<'a>(v: &'a V, code: &str) -> Result<&'a [V]> {
    if let V::List(v) = v {
        Ok(v)
    } else {
        Err(error(code))
    }
}
fn strings(v: &V, code: &str) -> Result<Vec<String>> {
    list(v, code)?
        .iter()
        .map(|v| text(v).map(str::to_owned))
        .collect()
}
fn map_code<'a>(v: &'a V, code: &str) -> Result<&'a Map> {
    map(v).map_err(|_| error(code))
}
fn bool_field(v: &V, code: &str) -> Result<bool> {
    if let V::Bool(v) = v {
        Ok(*v)
    } else {
        Err(error(code))
    }
}
fn s(v: &str) -> V {
    V::Text(v.into())
}
pub fn pin_gap_findings(obj: &V) -> Result<Vec<V>> {
    let m = map(obj)?;
    let gaps = m
        .get("pin_gaps")
        .map(map)
        .transpose()?
        .cloned()
        .unwrap_or_default();
    gaps.iter()
        .map(|(dep, reason)| {
            Ok(V::Map(Map::from([
                (
                    "code".into(),
                    s(&format!("dependency_pin_{}", text(reason)?)),
                ),
                ("subject".into(), m["subject"].clone()),
                ("object_id".into(), m["id"].clone()),
                ("detail".into(), s(dep)),
            ])))
        })
        .collect()
}
pub fn validate_subset_origin(value: &V) -> Result<()> {
    Y::validate_value(value, LIMIT)?;
    let m = schema(
        value,
        &[
            "version",
            "kind",
            "operation",
            "recorded_at",
            "source_authority",
            "source_capture_digest",
            "source_entry",
            "roots",
            "disclosed_locators",
            "prepared_digest",
            "subjects",
        ],
        &["inactive_audit"],
    )?;
    require(
        is_int(&m["version"], "1") && string_is(&m["kind"], "selected-subject-observation"),
        "invalid_subset_origin",
    )?;
    token(&m["operation"])?;
    require(
        text(&m["recorded_at"]).is_ok_and(|s| !s.is_empty()),
        "invalid_subset_origin",
    )?;
    A::validate_authority(&m["source_authority"])?;
    require(
        string_is(&map(&m["source_authority"])?["authority"], "history"),
        "invalid_subset_origin",
    )?;
    hash(&m["source_capture_digest"])?;
    A::relative_path(text(&m["source_entry"])?)?;
    let roots = strings(&m["roots"], "invalid_subset_origin")?;
    require(
        !roots.is_empty() && roots.windows(2).all(|w| w[0] < w[1]),
        "invalid_subset_origin",
    )?;
    let subjects = map_code(&m["subjects"], "invalid_subset_origin")?;
    require(
        roots.iter().all(|s| subjects.contains_key(s)),
        "invalid_subset_origin",
    )?;
    let mut prepared = false;
    for (name, item) in subjects {
        subject(&s(name))?;
        let item = schema(
            item,
            &["source_state", "objects_digest", "reduction_digest"],
            &[],
        )?;
        require(
            text(&item["source_state"])
                .is_ok_and(|s| ["committed", "prepared_candidate"].contains(&s)),
            "invalid_source_state",
        )?;
        prepared |= string_is(&item["source_state"], "prepared_candidate");
        hash(&item["objects_digest"])?;
        hash(&item["reduction_digest"])?;
    }
    for item in list(&m["disclosed_locators"], "invalid_subset_origin")? {
        let item = schema(item, &["path", "sha256"], &[])?;
        A::relative_path(text(&item["path"])?)?;
        if item["sha256"] != V::Null {
            hash(&item["sha256"])?;
        }
    }
    if let Some(a) = m.get("inactive_audit") {
        let a = schema(a, &["status", "digest"], &[])?;
        require(
            string_is(&a["status"], "not_transferred"),
            "invalid_subset_origin",
        )?;
        hash(&a["digest"])?;
    }
    if m["prepared_digest"] != V::Null {
        hash(&m["prepared_digest"])?;
    }
    require(
        !prepared || m["prepared_digest"] != V::Null,
        "invalid_source_state",
    )
}
pub fn validate_projection(value: &V) -> Result<()> {
    Y::validate_value(value, LIMIT)?;
    let m = schema(
        value,
        &[
            "projection_version",
            "authority",
            "baseline",
            "identity_schemes",
            "rules",
            "rules_digest",
            "closure_digest",
            "coverage",
            "subjects",
            "pins",
            "integrity",
        ],
        &["dispositions", "origin", "requires", "temporal"],
    )?;
    require(
        is_int(&m["projection_version"], "1") || is_int(&m["projection_version"], "2"),
        "unsupported_projection",
    )?;
    if is_int(&m["projection_version"], "2") {
        crate::history_node_publication::validate_authority(&m["authority"])?;
        A::validate_baseline(&m["baseline"])?;
        let authority = map(&m["authority"])?;
        let baseline = map(&m["baseline"])?;
        require(
            authority["record_id"] == baseline["record_id"]
                && authority["generation"] == baseline["authority_generation"],
            "authority_mismatch",
        )?;
    } else {
        A::bind_authority(&m["authority"], &m["baseline"])?;
    }
    if let Some(r) = m.get("requires") {
        A::validate_history_requires(r)?;
    }
    crate::history_temporal::validate_projection(m)?;
    let schemes = strings(&m["identity_schemes"], "unsupported_identity")
        .map_err(|_| error("unsupported_identity"))?;
    require(
        schemes.windows(2).all(|w| w[0] < w[1])
            && schemes
                .iter()
                .all(|s| ["typed-history/v2", "prototype/v1"].contains(&s.as_str())),
        "unsupported_identity",
    )?;
    require(
        matches!(m["rules"], V::Map(_)) && string_is(&m["rules_digest"], &m["rules"].digest()?),
        "rules_mismatch",
    )?;
    hash(&m["closure_digest"])?;
    let coverage = schema(&m["coverage"], &["scope", "subjects", "complete"], &[])?;
    require(
        text(&coverage["scope"]).is_ok_and(|s| ["all", "selected"].contains(&s)),
        "invalid_coverage",
    )?;
    let coverage_complete = bool_field(&coverage["complete"], "invalid_coverage")?;
    let covered = strings(&coverage["subjects"], "invalid_coverage")?;
    for name in &covered {
        subject(&s(name))?;
    }
    require(covered.windows(2).all(|w| w[0] < w[1]), "invalid_coverage")?;
    let subjects = map_code(&m["subjects"], "invalid_coverage")?;
    require(
        subjects.keys().cloned().collect::<Vec<_>>() == covered,
        "invalid_coverage",
    )?;
    let baseline = map(&m["baseline"])?;
    let heads = map(&baseline["heads"])?;
    let acts = map(&baseline["open_acts"])?;
    if string_is(&coverage["scope"], "all") {
        require(
            heads
                .keys()
                .chain(acts.keys())
                .all(|k| subjects.contains_key(k)),
            "invalid_coverage",
        )?;
    }
    if let Some(origin) = m.get("origin") {
        validate_subset_origin(origin)?;
        require(
            string_is(&coverage["scope"], "selected")
                && map(&map(origin)?["subjects"])?.keys().eq(subjects.keys()),
            "invalid_subset_coverage",
        )?;
    }
    let empty = V::List(vec![]);
    for (name, result) in subjects {
        let r = schema(result, &["acceptance", "heads", "open_acts"], &[])?;
        require(
            text(&r["acceptance"]).is_ok_and(|s| {
                [
                    "accepted",
                    "proposed",
                    "contested",
                    "refuted",
                    "corrected",
                    "retired",
                    "unreviewed",
                    "unavailable",
                ]
                .contains(&s)
            }),
            "invalid_acceptance",
        )?;
        ids(&r["heads"], true)?;
        ids(&r["open_acts"], true)?;
        require(
            r["heads"] == *heads.get(name).unwrap_or(&empty)
                && r["open_acts"] == *acts.get(name).unwrap_or(&empty),
            "baseline_mismatch",
        )?;
    }
    let pins = map_code(&m["pins"], "invalid_pins")?;
    let scheme_ok = |obj: &V| -> Result<()> {
        let scheme = if map(obj)?.contains_key("id_scheme") {
            "typed-history/v2"
        } else {
            "prototype/v1"
        };
        require(schemes.iter().any(|s| s == scheme), "unsupported_identity")
    };
    for (vid, w) in pins {
        let w = schema(w, &["subject", "version", "status", "object"], &[])?;
        id(&s(vid))?;
        subject(&w["subject"])?;
        require(
            string_is(&w["version"], vid)
                && text(&w["status"])
                    .is_ok_and(|s| ["recorded", "unavailable", "corrupt"].contains(&s)),
            "invalid_pin_witness",
        )?;
        if string_is(&w["status"], "recorded") {
            validate_object(&w["object"])?;
            scheme_ok(&w["object"])?;
            let obj = map(&w["object"])?;
            require(
                string_is(&obj["id"], vid)
                    && obj["subject"] == w["subject"]
                    && !string_is(&obj["kind"], "act"),
                "reference_mismatch",
            )?;
        } else {
            require(w["object"] == V::Null, "invalid_pin_witness")?;
        }
    }
    crate::history_temporal::validate_witnesses(m, pins)?;
    let empty_map = V::Map(Map::new());
    let dispositions = map_code(
        m.get("dispositions").unwrap_or(&empty_map),
        "invalid_dispositions",
    )?;
    require(
        dispositions.keys().all(|s| subjects.contains_key(s)),
        "invalid_dispositions",
    )?;
    for (name, disposition) in dispositions {
        let d = schema(
            disposition,
            &[
                "marks",
                "proposals",
                "contested_claims",
                "reviews",
                "implied",
            ],
            &["source_state"],
        )?;
        if let Some(origin) = m.get("origin") {
            let source = &map(&map(origin)?["subjects"])?[name];
            require(
                d.get("source_state") == map(source)?.get("source_state"),
                "invalid_source_state",
            )?;
        } else {
            require(!d.contains_key("source_state"), "missing_subset_origin")?;
        }
        for (vid, mark) in map_code(&d["marks"], "invalid_dispositions")? {
            id(&s(vid))?;
            require(
                text(mark).is_ok_and(|s| {
                    ["corrected", "refuted", "retired", "replaced", "superseded"].contains(&s)
                }),
                "invalid_dispositions",
            )?;
        }
        ids(&d["proposals"], true)?;
        ids(&d["contested_claims"], true)?;
        let mut review_ids = Vec::new();
        for review in list(&d["reviews"], "invalid_dispositions")? {
            validate_object(review)?;
            let r = map(review)?;
            require(
                string_is(&r["subject"], name) && string_is(&r["kind"], "act"),
                "invalid_review_scope",
            )?;
            let body = map(&r["body"])?;
            let eligible = list(&map(&subjects[name])?["heads"], "invalid_references")?
                .iter()
                .chain(list(&d["proposals"], "invalid_references")?)
                .any(|v| Some(v) == body.get("of"));
            require(
                string_is(&r["subject"], name)
                    && string_is(&r["kind"], "act")
                    && body.get("act").is_some_and(|v| string_is(v, "review"))
                    && eligible,
                "invalid_review_scope",
            )?;
            scheme_ok(review)?;
            if let Some(read) = body.get("read") {
                for (dep, vid) in map(read)? {
                    let witness = pins
                        .get(text(vid)?)
                        .ok_or_else(|| error("missing_pin_witness"))?;
                    require(
                        string_is(&map(witness)?["subject"], dep),
                        "missing_pin_witness",
                    )?;
                }
            }
            review_ids.push(text(&r["id"])?.to_owned());
        }
        require(
            review_ids.windows(2).all(|w| w[0] < w[1]),
            "invalid_references",
        )?;
        for finding in list(&d["implied"], "invalid_dispositions")? {
            let f = schema(
                finding,
                &["rule", "subject", "superseded", "by", "why"],
                &[],
            )?;
            require(
                string_is(&f["rule"], "source_clock")
                    && string_is(&f["subject"], name)
                    && matches!(f["why"], V::Text(_)),
                "invalid_dispositions",
            )?;
            id(&f["superseded"])?;
            id(&f["by"])?;
        }
    }
    if m.contains_key("origin") {
        require(
            dispositions.keys().eq(subjects.keys()),
            "invalid_subset_coverage",
        )?;
    }
    let integrity = schema(&m["integrity"], &["complete", "findings"], &[])?;
    let complete = bool_field(&integrity["complete"], "invalid_integrity")?;
    let findings = list(&integrity["findings"], "invalid_integrity")?;
    for f in findings {
        let f = schema(f, &["code", "subject", "object_id", "detail"], &[])?;
        require(
            f.values().all(|v| matches!(v, V::Text(_))),
            "invalid_integrity",
        )?;
    }
    for w in pins.values() {
        let w = map(w)?;
        if string_is(&w["status"], "recorded") {
            for f in pin_gap_findings(&w["object"])? {
                require(
                    !complete && !coverage_complete && findings.contains(&f),
                    "unreported_pin_gap",
                )?;
            }
        }
    }
    require(
        !complete || (findings.is_empty() && coverage_complete),
        "invalid_integrity",
    )
}
#[derive(Clone, Debug)]
pub struct CapturedHistory {
    document: V,
    projection: V,
}
impl CapturedHistory {
    pub fn new(document: V, projection: V) -> Result<Self> {
        Y::validate_value(&document, 16 * 1024 * 1024)?;
        let d = map_code(&document, "invalid_document")?;
        validate_projection(&projection)?;
        let meta = d
            .get("meta")
            .and_then(|v| map(v).ok())
            .ok_or_else(|| error("baseline_mismatch"))?;
        let actual = meta
            .get("history")
            .ok_or_else(|| error("baseline_mismatch"))?;
        A::validate_baseline(actual)?;
        require(
            actual.digest()? == map(&projection)?["baseline"].digest()?,
            "baseline_mismatch",
        )?;
        let origin = meta.get("history_subset").filter(|v| **v != V::Null);
        if origin.is_some() || map(&projection)?.contains_key("origin") {
            let origin = origin.ok_or_else(|| error("subset_origin_mismatch"))?;
            validate_subset_origin(origin)?;
            require(
                map(&projection)?
                    .get("origin")
                    .is_some_and(|v| v.digest().ok() == origin.digest().ok()),
                "subset_origin_mismatch",
            )?;
        }
        Ok(Self {
            document,
            projection,
        })
    }
    pub fn document(&self) -> &V {
        &self.document
    }
    pub fn projection(&self) -> &V {
        &self.projection
    }
}
