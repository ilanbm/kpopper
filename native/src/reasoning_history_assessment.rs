//! Canonical history-aware assessment envelopes over one captured observation.
//! Projection is pure: accepted history, computed findings and attention retain
//! separate identities and no source is opened while adapting a report.
use crate::{
    Error, Result,
    history_contract::*,
    history_projection,
    history_view::{list, truth},
    reasoning_assessment as A, reasoning_history_support as H,
    reasoning_runtime::{OperationalBounds, Runtime},
    reasoning_snapshot::{Snapshot, digest},
    reasoning_temporal as Temporal, require,
    value::TypedValue as V,
};
use serde_json::{Value as J, json};
use std::collections::{BTreeMap, BTreeSet};
fn s(v: &str) -> V {
    V::Text(v.into())
}
fn obj(v: impl IntoIterator<Item = (&'static str, V)>) -> V {
    V::Map(v.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
fn empty() -> V {
    V::Map(Map::new())
}
fn arr() -> V {
    V::List(vec![])
}
fn val(j: J) -> Result<V> {
    V::from_json_bounded(&j, 64 * 1024 * 1024 / 8)
}
fn names(v: &V) -> Result<Vec<String>> {
    list(v)?
        .iter()
        .map(|v| text(v).map(str::to_owned))
        .collect()
}
fn ids(v: &V, canonical: bool) -> Result<Vec<String>> {
    let a = names(v)?;
    let set = a.iter().collect::<BTreeSet<_>>();
    require(
        a.len() == set.len() && (!canonical || a.windows(2).all(|w| w[0] < w[1])),
        "invalid assessment ids",
    )?;
    Ok(a)
}
fn hex(v: &V) -> bool {
    text(v).is_ok_and(|s| {
        s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
fn select(m: &Map, keys: &[&str]) -> Result<V> {
    Ok(V::Map(
        keys.iter()
            .map(|k| field(m, k).map(|v| ((*k).into(), v.clone())))
            .collect::<Result<_>>()?,
    ))
}
fn maps(v: &V) -> Result<()> {
    for v in list(v)? {
        map(v)?;
    }
    Ok(())
}
fn hash_without(v: &V, key: &str) -> Result<String> {
    digest(&V::Map(
        map(v)?
            .iter()
            .filter(|(k, _)| *k != key)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    ))
}
fn bounds(v: &V) -> Result<usize> {
    let m = schema(
        v,
        &[
            "timeout_seconds",
            "batch_requests",
            "input_bytes",
            "output_bytes",
        ],
        &[],
    )?;
    for (k, max) in [
        ("timeout_seconds", 30),
        ("batch_requests", 1000),
        ("input_bytes", 16 * 1024 * 1024),
        ("output_bytes", 64 * 1024 * 1024),
    ] {
        let V::Integer(n) = &m[k] else {
            return Err(Error("invalid assessment operational limits".into()));
        };
        require(
            n.as_str().parse::<usize>().is_ok_and(|n| n > 0 && n <= max),
            "invalid assessment operational limits",
        )?
    }
    text_int(&m["output_bytes"])
}
fn text_int(v: &V) -> Result<usize> {
    if let V::Integer(n) = v {
        return n
            .as_str()
            .parse()
            .map_err(|_| Error("invalid assessment integer".into()));
    }
    Err(Error("invalid assessment integer".into()))
}
fn attention(v: &V) -> Result<()> {
    for a in list(v)? {
        let a = schema(a, &["action", "reasons"], &[])?;
        text(&a["action"])?;
        for r in list(&a["reasons"])? {
            let r = schema(r, &["code", "related_ids"], &[])?;
            text(&r["code"])?;
            names(&r["related_ids"])?;
        }
    }
    Ok(())
}
fn node_v2(v: &V) -> Result<()> {
    let n = schema(
        v,
        &["body", "fields", "state", "attention", "computation"],
        &[],
    )?;
    for v in schema(&n["fields"], &["deps", "snapshot", "predicate"], &[])?.values() {
        text(v)?;
    }
    let state = schema(
        &n["state"],
        &["basis", "falsifier", "contention", "integrity"],
        &[],
    )?;
    let basis = map(&state["basis"])?;
    field(basis, "status")?;
    map(field(basis, "dependencies")?)?;
    let f = map(&state["falsifier"])?;
    for k in ["status", "expression", "reads"] {
        field(f, k)?;
    }
    let c = schema(
        &state["contention"],
        &["status", "witnesses", "alternatives"],
        &[],
    )?;
    require(
        ["detected", "none_detected"]
            .iter()
            .any(|s| string_is(&c["status"], s)),
        "invalid contention",
    )?;
    list(&c["witnesses"])?;
    list(&c["alternatives"])?;
    let i = schema(&state["integrity"], &["status", "checks", "issues"], &[])?;
    require(string_is(&i["status"], "assessed"), "invalid integrity")?;
    names(&i["checks"])?;
    list(&i["issues"])?;
    attention(&n["attention"])?;
    require(
        n["computation"] == V::Null || matches!(&n["computation"], V::Map(_)),
        "invalid computation",
    )?;
    digest(v)?;
    Ok(())
}
fn jkeys(v: &J, required: &[&str], optional: &[&str]) -> Result<()> {
    let m = v
        .as_object()
        .ok_or_else(|| Error("invalid computation object".into()))?;
    require(
        required.iter().all(|k| m.contains_key(*k))
            && m.keys()
                .all(|k| required.contains(&k.as_str()) || optional.contains(&k.as_str())),
        "invalid computation fields",
    )
}
fn nonnegative(v: &J) -> bool {
    v.as_number().is_some_and(|n| {
        let t = n.to_string();
        !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit())
    })
}
fn jids(v: &J, canonical: bool) -> Result<Vec<String>> {
    ids(&val(v.clone())?, canonical)
}
fn witness(v: &J) -> Result<Vec<String>> {
    let kind = v["kind"].as_str();
    let fields = match kind {
        Some("node") => vec!["kind", "id", "fingerprint"],
        Some("scope") => vec![
            "kind",
            "scope_id",
            "definition_digest",
            "membership_digest",
            "projected_inputs_digest",
        ],
        _ => return Err(Error("invalid dependency witness".into())),
    };
    jkeys(v, &fields, &[])?;
    let mut key = vec![if kind == Some("node") {
        "0".into()
    } else {
        "1".into()
    }];
    for f in &fields[1..] {
        let x = v[*f]
            .as_str()
            .ok_or_else(|| Error("invalid witness value".into()))?;
        require(
            !x.is_empty() && (f == &"id" || f == &"scope_id" || hex(&s(x))),
            "invalid witness value",
        )?;
        key.push(x.into());
    }
    Ok(key)
}
fn witnesses(v: &J) -> Result<Vec<Vec<String>>> {
    let a = v
        .as_array()
        .ok_or_else(|| Error("invalid witnesses".into()))?;
    let keys = a.iter().map(witness).collect::<Result<Vec<_>>>()?;
    require(
        keys.windows(2).all(|w| w[0] < w[1]),
        "noncanonical witnesses",
    )?;
    Ok(keys)
}
fn locations(v: &J) -> Result<Vec<Vec<String>>> {
    let a = v
        .as_array()
        .ok_or_else(|| Error("invalid diagnostic locations".into()))?;
    let mut out = vec![];
    for v in a {
        jkeys(v, &["candidate", "column", "phase"], &[])?;
        let candidate = v["candidate"]
            .as_str()
            .ok_or_else(|| Error("invalid location".into()))?;
        let column = v["column"]
            .as_str()
            .ok_or_else(|| Error("invalid location".into()))?;
        let phase = v["phase"]
            .as_str()
            .ok_or_else(|| Error("invalid location".into()))?;
        require(
            !candidate.is_empty()
                && !column.is_empty()
                && ["where", "value", "preflight", "aggregate"].contains(&phase),
            "invalid location",
        )?;
        out.push(vec![candidate.into(), column.into(), phase.into()]);
    }
    require(
        out.windows(2).all(|w| w[0] < w[1]),
        "noncanonical locations",
    )?;
    Ok(out)
}
pub(crate) fn computation(value: &V, snapshot_id: &str) -> Result<()> {
    let r = value.to_json()?;
    let numeric = |v: &J| match v {
        J::Bool(v) => Some(if *v { 1.0 } else { 0.0 }),
        J::Number(n) => n.as_f64(),
        _ => None,
    };
    let version = match numeric(&r["schema_version"]) {
        Some(1.0) => 1,
        Some(2.0) => 2,
        _ => return Err(Error("invalid result version".into())),
    };
    let mut required = vec![
        "schema_version",
        "profile",
        "modules",
        "snapshot_id",
        "computation_id",
        "implementation",
        "resource_profile",
        "status",
        "value",
        "diagnostics",
        "executed_reads",
        "potential_dependencies",
        "potential_ids",
        "basis",
        "assurance",
        "cost",
        "operational_limits",
    ];
    if version == 2 {
        required.push("query_counts")
    }
    jkeys(&r, &required, &["interpretation"])?;
    require(
        r["profile"] == "core/v1"
            && r["snapshot_id"] == snapshot_id
            && (r["computation_id"].is_null() || r["computation_id"].is_string())
            && (r["implementation"].is_null() || r["implementation"].is_object()),
        "invalid computation identity",
    )?;
    jids(&r["modules"], true)?;
    let resources = &r["resource_profile"];
    let rv = resources["version"].as_str().unwrap_or("");
    let mut fields = vec!["version", "steps", "depth", "digits"];
    if ["resources/v3", "resources/v4"].contains(&rv) {
        fields.extend(["value_nodes", "value_depth", "value_bytes"])
    }
    if rv == "resources/v4" {
        fields.extend(["candidates", "field_reads"])
    }
    require(
        ["resources/v2", "resources/v3", "resources/v4"].contains(&rv),
        "invalid resource profile",
    )?;
    jkeys(resources, &fields, &[])?;
    for k in ["steps", "depth", "digits"] {
        require(nonnegative(&resources[k]), "invalid resource profile")?
    }
    for (k, max) in [
        ("value_nodes", 10000),
        ("value_depth", 128),
        ("value_bytes", 16777216),
        ("candidates", 10000),
        ("field_reads", 100000),
    ] {
        if fields.contains(&k) {
            require(
                numeric(&resources[k]).is_some_and(|v| v > 0.0 && v <= f64::from(max)),
                "invalid resource profile",
            )?
        }
    }
    require(
        (version == 2) == (rv == "resources/v4"),
        "result/resource version mismatch",
    )?;
    let status = r["status"]
        .as_str()
        .ok_or_else(|| Error("invalid status".into()))?;
    require(
        [
            "ok",
            "unknown",
            "error",
            "limit",
            "unsupported_capability",
            "operational_error",
        ]
        .contains(&status),
        "invalid status",
    )?;
    if status == "ok" {
        crate::reasoning_values::validate(&val(r["value"].clone())?)?
    } else {
        require(r["value"].is_null(), "non-ok computation has value")?
    }
    let mut ds = Vec::new();
    for d in r["diagnostics"]
        .as_array()
        .ok_or_else(|| Error("invalid diagnostics".into()))?
    {
        jkeys(
            d,
            if version == 2 {
                &["code", "related_ids", "locations"]
            } else {
                &["code", "related_ids"]
            },
            &[],
        )?;
        let code = d["code"]
            .as_str()
            .ok_or_else(|| Error("invalid diagnostic code".into()))?;
        let ids = jids(&d["related_ids"], true)?;
        let loc = if version == 2 {
            locations(&d["locations"])?
        } else {
            vec![]
        };
        ds.push((code.to_owned(), ids, loc))
    }
    require(
        ds.windows(2).all(|w| w[0] < w[1]),
        "noncanonical diagnostics",
    )?;
    let executed = witnesses(&r["executed_reads"])?;
    let potential = witnesses(&r["potential_dependencies"])?;
    require(
        executed.iter().all(|x| potential.contains(x)),
        "executed reads exceed potential dependencies",
    )?;
    let potential_ids = jids(&r["potential_ids"], true)?;
    if version == 2 {
        let mut found = potential.iter().map(|w| w[1].clone()).collect::<Vec<_>>();
        found.sort();
        require(found == potential_ids, "potential ids mismatch")?
    }
    require(
        r["basis"].is_null() || r["basis"].is_object(),
        "invalid computation basis",
    )?;
    let assurance = &r["assurance"];
    jkeys(assurance, &["kind", "formal_scope"], &["implementation"])?;
    require(
        assurance["kind"] == "computed" && assurance.get("implementation").is_none_or(J::is_string),
        "invalid assurance",
    )?;
    names(&val(assurance["formal_scope"].clone())?)?;
    let cost = &r["cost"];
    let mut cost_fields = vec!["steps", "preflight_steps", "node_evaluations"];
    if version == 2 {
        cost_fields.extend(["candidates", "field_reads", "evaluated_field_reads"])
    }
    jkeys(cost, &cost_fields, &[])?;
    for k in cost_fields.iter().filter(|k| **k != "node_evaluations") {
        require(nonnegative(&cost[*k]), "invalid computation cost")?
    }
    let nodes = cost["node_evaluations"]
        .as_object()
        .ok_or_else(|| Error("invalid cost nodes".into()))?;
    require(nodes.values().all(nonnegative), "invalid cost node")?;
    if version == 2 {
        let counts = &r["query_counts"];
        if !counts.is_null() {
            jkeys(
                counts,
                &[
                    "input_count",
                    "definite_match_count",
                    "unknown_membership_count",
                    "unknown_value_count",
                    "error_count",
                ],
                &[],
            )?;
            require(
                counts.as_object().unwrap().values().all(nonnegative),
                "invalid query counts",
            )?;
            let n = |key: &str| -> Result<num_bigint::BigInt> {
                counts[key]
                    .to_string()
                    .parse()
                    .map_err(|_| Error("invalid query count".into()))
            };
            require(
                n("definite_match_count")? <= n("input_count")?
                    && n("unknown_membership_count")? <= n("input_count")?
                    && n("unknown_value_count")? <= n("definite_match_count")?
                    && n("error_count")? <= n("input_count")?
                    && cost["candidates"] == counts["input_count"],
                "inconsistent query counts",
            )?;
        }
        require(
            counts.is_null() || ["ok", "unknown", "error"].contains(&status),
            "preflight status claims counts",
        )?;
        require(
            counts.is_null() == r["basis"].is_null()
                && (!counts.is_null() || r["computation_id"].is_null()),
            "query basis mismatch",
        )?;
        if !counts.is_null() {
            require(executed == potential, "completed query witness mismatch")?;
            let basis = &r["basis"];
            jkeys(
                basis,
                &[
                    "version",
                    "recipe",
                    "profile",
                    "modules",
                    "as_of",
                    "operation",
                    "scope",
                    "dependencies",
                    "resources",
                    "preflight_cost",
                    "query_counts",
                    "digest",
                ],
                &[],
            )?;
            require(
                basis["version"] == 1
                    && basis["recipe"] == "query-inputs/v1"
                    && basis["profile"] == "core/v1"
                    && basis["modules"] == r["modules"]
                    && basis["dependencies"] == r["potential_dependencies"]
                    && basis["resources"] == *resources
                    && basis["query_counts"] == *counts
                    && basis["digest"] == hash_without(&val(basis.clone())?, "digest")?,
                "invalid finalized query basis",
            )?;
            let op = &basis["operation"];
            jkeys(op, &["query"], &[])?;
            let scope = &basis["scope"];
            let scopes = r["potential_dependencies"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|w| w["kind"] == "scope")
                .collect::<Vec<_>>();
            require(
                scopes.len() == 1
                    && op["query"].is_object()
                    && op["query"]["scope"].is_string()
                    && scope.is_object(),
                "invalid finalized scope",
            )?;
            require(
                scope["witness"] == *scopes[0]
                    && op["query"]["scope"] == scopes[0]["scope_id"]
                    && scope["digest"] == hash_without(&val(scope.clone())?, "digest")?,
                "invalid finalized scope",
            )?;
            let preflight = &basis["preflight_cost"];
            jkeys(
                preflight,
                &[
                    "candidates",
                    "field_reads",
                    "preflight_steps",
                    "step_upper_bound",
                ],
                &[],
            )?;
            require(
                preflight.as_object().unwrap().values().all(nonnegative)
                    && ["candidates", "field_reads", "preflight_steps"]
                        .iter()
                        .all(|k| preflight[*k] == cost[*k]),
                "preflight cost mismatch",
            )?;
            require(
                r["computation_id"]
                    == digest(&val(
                        json!({"snapshot_id":snapshot_id,"basis":basis,"resources":resources}),
                    )?)?,
                "query computation identity mismatch",
            )?;
        }
        require(
            status != "ok" || !counts.is_null(),
            "successful query lacks counts",
        )?;
        if status == "ok" {
            let fields = &r["value"]["fields"];
            jkeys(
                fields,
                &[
                    "input_count",
                    "definite_match_count",
                    "unknown_membership_count",
                    "unknown_value_count",
                    "error_count",
                    "result",
                ],
                &[],
            )?;
            require(
                r["value"]["type"] == "record",
                "invalid successful query value",
            )?;
            for (k, v) in counts.as_object().unwrap() {
                require(
                    fields[k]
                        == json!({"type":"number","numerator":v.to_string(),"denominator":"1"}),
                    "query value/count mismatch",
                )?;
            }
        }
    }
    bounds(&val(r["operational_limits"].clone())?)?;
    if let Some(i) = r.get("interpretation") {
        jkeys(i, &["declared_profile", "explicit_override"], &[])?;
        require(
            i["declared_profile"].is_string() && i["explicit_override"] == "core/v1",
            "invalid interpretation",
        )?
    }
    Ok(())
}
pub fn validate_v2(snapshot: &Snapshot, report: &V) -> Result<V> {
    let r = schema(
        report,
        &[
            "schema_version",
            "assessment_profile",
            "attention_policy",
            "snapshot_id",
            "as_of",
            "scope",
            "selection",
            "assessment_revision",
            "nodes",
            "operational_limits",
        ],
        &[],
    )?;
    require(
        crate::source_clock::python_equal(&r["schema_version"], &val(json!(2))?)
            && string_is(&r["assessment_profile"], "core/v1")
            && ["focused-review/v1", "falsifiers-only/v1"]
                .iter()
                .any(|p| string_is(&r["attention_policy"], p)),
        "v2 profile mismatch",
    )?;
    let data = map(snapshot.data())?;
    require(
        r["snapshot_id"] == s(snapshot.snapshot_id()) && r["as_of"] == data["as_of"],
        "v2 snapshot mismatch",
    )?;
    let scope = schema(
        &r["scope"],
        &[
            "context",
            "hypotheses",
            "external_sources_fetched",
            "evidence",
        ],
        &[],
    )?;
    map(&scope["context"])?;
    map(&scope["hypotheses"])?;
    require(
        scope["external_sources_fetched"] == V::Bool(false),
        "invalid v2 scope",
    )?;
    text(&scope["evidence"])?;
    let selection = ids(&r["selection"], false)?
        .into_iter()
        .collect::<BTreeSet<_>>();
    let nodes = map(&r["nodes"])?;
    let sources = map(&data["nodes"])?;
    require(
        selection == nodes.keys().cloned().collect()
            && nodes.keys().all(|k| sources.contains_key(k)),
        "v2 selection mismatch",
    )?;
    require(
        r["assessment_revision"] == s(&hash_without(report, "assessment_revision")?),
        "v2 assessment revision mismatch",
    )?;
    for (id, node) in nodes {
        node_v2(node)?;
        let node = map(node)?;
        let source = map(&sources[id])?;
        require(
            digest(&node["body"])? == digest(&source["body"])?
                && digest(&node["fields"])? == digest(&source["fields"])?,
            "v2 node mismatch",
        )?;
        let state = map(&node["state"])?;
        let mut results = vec![&node["computation"]];
        if let Some(c) = map(&state["falsifier"])?.get("computation") {
            results.push(c)
        }
        for d in map(&map(&state["basis"])?["dependencies"])?.values() {
            if let Some(c) = map(d)?.get("computation") {
                results.push(c)
            }
        }
        for c in results {
            if *c != V::Null {
                computation(c, snapshot.snapshot_id())?
            }
        }
    }
    bounds(&r["operational_limits"])?;
    Ok(report.clone())
}
fn coverage(v: &V) -> Result<()> {
    let m = schema(v, &["included", "complete", "findings"], &[])?;
    require(
        matches!(m["included"], V::Bool(_)) && matches!(m["complete"], V::Bool(_)),
        "invalid coverage",
    )?;
    maps(&m["findings"])
}
fn pin(v: &V) -> Result<()> {
    let m = schema(
        v,
        &[
            "status",
            "evidence_kind",
            "subject",
            "version",
            "body",
            "profile",
            "value_status",
            "value",
            "basis_status",
            "basis",
            "findings",
        ],
        &[],
    )?;
    require(
        ["recorded", "unavailable"]
            .iter()
            .any(|s| string_is(&m["status"], s))
            && string_is(&m["evidence_kind"], "version_pin")
            && (m["subject"] == V::Null || matches!(m["subject"], V::Text(_)))
            && (m["profile"] == V::Null || matches!(m["profile"], V::Text(_)))
            && ["recorded", "unavailable"]
                .iter()
                .any(|s| string_is(&m["value_status"], s))
            && ["recorded", "not_recorded", "unavailable"]
                .iter()
                .any(|s| string_is(&m["basis_status"], s)),
        "invalid pin evidence",
    )?;
    text(&m["version"])?;
    maps(&m["findings"])
}
fn reviews(v: &V) -> Result<()> {
    for v in list(v)? {
        let m = schema(v, &["review_id", "of", "read"], &[])?;
        text(&m["review_id"])?;
        text(&m["of"])?;
        for v in map(&m["read"])?.values() {
            pin(v)?;
        }
    }
    Ok(())
}
fn support(v: &V) -> Result<()> {
    let m = schema(
        v,
        &[
            "reducer",
            "status",
            "visited",
            "visited_count",
            "reservations",
        ],
        &[],
    )?;
    require(
        string_is(&m["reducer"], H::REDUCER)
            && ["clear", "reserved"]
                .iter()
                .any(|s| string_is(&m["status"], s)),
        "invalid support",
    )?;
    let visited = ids(&m["visited"], true)?;
    require(
        text_int(&m["visited_count"])? == visited.len(),
        "invalid support count",
    )?;
    let reservations = list(&m["reservations"])?;
    for r in reservations {
        let r = schema(r, &["code", "state", "subject", "version", "path"], &[])?;
        for k in ["code", "state", "subject", "version"] {
            text(&r[k])?;
        }
        require(
            H::support_state(text(&r["state"])?),
            "invalid reservation state",
        )?;
        names(&r["path"])?;
    }
    require(
        string_is(&m["status"], "reserved") != reservations.is_empty(),
        "support status mismatch",
    )
}
fn assurance(v: &V) -> Result<()> {
    let m = schema(v, &["actual", "recorded_evidence_kinds"], &[])?;
    let a = schema(&m["actual"], &["node", "falsifier", "dependencies"], &[])?;
    for v in [&a["node"], &a["falsifier"]]
        .into_iter()
        .chain(map(&a["dependencies"])?.values())
    {
        require(*v == V::Null || matches!(v, V::Map(_)), "invalid assurance")?;
    }
    require(
        ids(&m["recorded_evidence_kinds"], true)?
            .iter()
            .all(|s| ["version_pin", "review_pin"].contains(&s.as_str())),
        "invalid evidence kind",
    )
}
fn dispositions(m: &Map) -> Result<()> {
    ids(field(m, "proposals")?, false)?;
    ids(field(m, "contested_claims")?, false)?;
    maps(field(m, "reviews")?)?;
    maps(field(m, "implied_reservations")?)?;
    for mark in map(field(m, "disposition_marks")?)?.values() {
        require(
            ["corrected", "refuted", "retired", "replaced", "superseded"]
                .iter()
                .any(|s| string_is(mark, s)),
            "invalid disposition",
        )?;
    }
    for v in map(field(m, "recorded_support")?)?.values() {
        pin(v)?;
    }
    reviews(field(m, "pin_review_evidence")?)
}
fn node_history(v: &V) -> Result<()> {
    let m = schema(
        v,
        &[
            "subject_id",
            "current_head_witnesses",
            "proposals",
            "reviews",
            "disposition_marks",
            "contested_claims",
            "implied_reservations",
            "recorded_support",
            "pin_review_evidence",
        ],
        &[],
    )?;
    require(
        m["subject_id"] == V::Null || matches!(m["subject_id"], V::Text(_)),
        "invalid history subject",
    )?;
    for p in list(&m["current_head_witnesses"])? {
        pin(p)?;
    }
    dispositions(m)
}
fn history_summary(v: &V) -> Result<()> {
    let m = map(v)?;
    if m.get("authority_status")
        .is_some_and(|v| string_is(v, "not_active"))
    {
        schema(v, &["authority_status"], &[])?;
        return Ok(());
    }
    let m = schema(
        v,
        &[
            "authority_status",
            "projection_version",
            "authority",
            "committed_set_digest",
            "closure_digest",
            "coverage",
            "identity_schemes",
            "integrity",
        ],
        &["capabilities"],
    )?;
    if let Some(capabilities) = m.get("capabilities") {
        crate::history_authority::validate_history_requires(capabilities)?;
    }
    require(
        string_is(&m["authority_status"], "active")
            && crate::source_clock::python_equal(&m["projection_version"], &val(json!(1))?),
        "invalid active history",
    )?;
    let a = schema(
        &m["authority"],
        &["record_id", "generation", "identity"],
        &[],
    )?;
    text(&a["record_id"])?;
    text_int(&a["generation"])?;
    require(
        hex(&a["identity"]) && hex(&m["committed_set_digest"]) && hex(&m["closure_digest"]),
        "invalid history identity",
    )?;
    let c = schema(&m["coverage"], &["scope", "subjects", "complete"], &[])?;
    require(
        ["all", "selected"]
            .iter()
            .any(|s| string_is(&c["scope"], s))
            && matches!(c["complete"], V::Bool(_)),
        "invalid history coverage",
    )?;
    ids(&c["subjects"], true)?;
    ids(&m["identity_schemes"], true)?;
    let i = schema(&m["integrity"], &["complete", "findings"], &[])?;
    require(
        matches!(i["complete"], V::Bool(_)),
        "invalid history integrity",
    )?;
    maps(&i["findings"])
}
fn findings_preimage(r: &Map) -> Result<V> {
    let mut p = map(&select(
        r,
        &[
            "schema_version",
            "assessment_profile",
            "snapshot_id",
            "as_of",
            "assessment_selection",
            "history_selection",
            "scope",
            "history",
        ],
    )?)?
    .clone();
    p.insert("history_subjects".into(), r["history_subjects"].clone());
    let nodes = map(&r["nodes"])?
        .iter()
        .map(|(id, node)| {
            Ok((
                id.clone(),
                V::Map(
                    map(node)?
                        .iter()
                        .filter(|(k, _)| *k != "attention")
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                ),
            ))
        })
        .collect::<Result<Map>>()?;
    p.insert("nodes".into(), V::Map(nodes));
    Ok(V::Map(p))
}
fn charge(v: &V, budget: usize) -> Result<()> {
    require(
        crate::history_yaml::compact_json_size(v) <= budget,
        "output_limit",
    )
}
pub fn validate(report: &V) -> Result<V> {
    let r = schema(
        report,
        &[
            "schema_version",
            "assessment_profile",
            "attention_policy",
            "snapshot_id",
            "as_of",
            "assessment_selection",
            "history_selection",
            "display_selection",
            "operational_limits",
            "scope",
            "history",
            "base_assessment_revision",
            "nodes",
            "history_subjects",
            "findings_revision",
            "envelope_revision",
        ],
        &[],
    )?;
    require(
        crate::source_clock::python_equal(&r["schema_version"], &val(json!(3))?)
            && string_is(&r["assessment_profile"], "core/v1")
            && ["focused-review/v1", "falsifiers-only/v1"]
                .iter()
                .any(|p| string_is(&r["attention_policy"], p)),
        "invalid v3 profile",
    )?;
    let budget = bounds(&r["operational_limits"])?;
    require(
        hex(&r["snapshot_id"]) && hex(&r["base_assessment_revision"]),
        "invalid v3 identity",
    )?;
    let nodes = map(&r["nodes"])?;
    let subjects = map(&r["history_subjects"])?;
    require(
        ids(&r["assessment_selection"], false)?
            .into_iter()
            .collect::<BTreeSet<_>>()
            == nodes.keys().cloned().collect()
            && ids(&r["history_selection"], false)?
                .into_iter()
                .collect::<BTreeSet<_>>()
                == subjects.keys().cloned().collect(),
        "v3 selection mismatch",
    )?;
    require(
        ids(&r["display_selection"], false)?
            .iter()
            .all(|s| nodes.contains_key(s) || subjects.contains_key(s)),
        "invalid display selection",
    )?;
    for n in nodes.values() {
        let n = schema(
            n,
            &[
                "body",
                "fields",
                "state",
                "attention",
                "computation",
                "acceptance",
                "history",
                "coverage",
                "assurance",
                "support",
            ],
            &["temporal"],
        )?;
        node_v2(&select(
            n,
            &["body", "fields", "state", "attention", "computation"],
        )?)?;
        let a = schema(
            &n["acceptance"],
            &["status", "head_ids", "open_act_ids"],
            &[],
        )?;
        require(
            H::ACCEPTANCE.contains(&text(&a["status"])?)
                || string_is(&a["status"], "not_applicable"),
            "invalid acceptance",
        )?;
        ids(&a["head_ids"], false)?;
        ids(&a["open_act_ids"], false)?;
        coverage(&n["coverage"])?;
        assurance(&n["assurance"])?;
        support(&n["support"])?;
        node_history(&n["history"])?;
        if let Some(temporal) = n.get("temporal") {
            Temporal::validate(temporal)?;
        }
    }
    for h in subjects.values() {
        let h = schema(
            h,
            &[
                "acceptance",
                "head_ids",
                "open_act_ids",
                "head_witnesses",
                "proposals",
                "reviews",
                "disposition_marks",
                "contested_claims",
                "implied_reservations",
                "recorded_support",
                "pin_review_evidence",
                "coverage",
                "support",
            ],
            &["source_state", "temporal"],
        )?;
        require(
            H::ACCEPTANCE.contains(&text(&h["acceptance"])?),
            "invalid history acceptance",
        )?;
        ids(&h["head_ids"], false)?;
        ids(&h["open_act_ids"], false)?;
        for v in list(&h["head_witnesses"])? {
            pin(v)?;
        }
        dispositions(h)?;
        if let Some(v) = h.get("source_state") {
            require(
                ["committed", "prepared_candidate"]
                    .iter()
                    .any(|s| string_is(v, s)),
                "invalid source state",
            )?;
        }
        coverage(&h["coverage"])?;
        support(&h["support"])?;
        if let Some(temporal) = h.get("temporal") {
            Temporal::validate(temporal)?;
        }
    }
    history_summary(&r["history"])?;
    if string_is(&map(&r["history"])?["authority_status"], "not_active") {
        require(
            subjects.is_empty()
                && nodes.values().all(|n| {
                    map(n)
                        .ok()
                        .and_then(|m| m.get("acceptance"))
                        .and_then(|v| map(v).ok())
                        .and_then(|m| m.get("status"))
                        .is_some_and(|v| string_is(v, "not_applicable"))
                }),
            "inactive history claimed acceptance",
        )?;
    }
    require(
        r["findings_revision"] == s(&digest(&findings_preimage(r)?)?)
            && r["envelope_revision"] == s(&hash_without(report, "envelope_revision")?),
        "v3 identity mismatch",
    )?;
    charge(report, budget)?;
    Ok(report.clone())
}
fn subject_state(p: &Map, subject: &str, version: &str) -> Result<String> {
    let d = p
        .get("dispositions")
        .and_then(|v| map(v).ok())
        .and_then(|m| m.get(subject))
        .and_then(|v| map(v).ok());
    if let Some(mark) = d
        .and_then(|m| m.get("marks"))
        .and_then(|v| map(v).ok())
        .and_then(|m| m.get(version))
        .and_then(|v| text(v).ok())
    {
        if ["replaced", "superseded"].contains(&mark) {
            return Ok("moved".into());
        }
        if H::ACCEPTANCE.contains(&mark) {
            return Ok(mark.into());
        }
    }
    let Some(current) = map(&p["subjects"])?.get(subject) else {
        return Ok("unavailable".into());
    };
    let current = map(current)?;
    if names(&current["heads"])?.contains(&version.into()) {
        return Ok(text(&current["acceptance"])?.into());
    }
    if d.and_then(|m| m.get("proposals"))
        .is_some_and(|v| names(v).is_ok_and(|a| a.contains(&version.into())))
    {
        return Ok("proposed".into());
    }
    Ok("moved".into())
}
fn graph(p: &Map) -> Result<V> {
    let mut graph = Map::new();
    for (version, w) in map(&p["pins"])? {
        let w = map(w)?;
        let subject = text(&w["subject"])?;
        let recorded = string_is(&w["status"], "recorded");
        let mut deps = vec![];
        if recorded {
            let o = map(&w["object"])?;
            for (field, prefix) in [("pins", ""), ("pin_gaps", "unavailable:")] {
                if let Some(v) = o.get(field) {
                    for (d, version) in map(v)? {
                        deps.push(obj([
                            ("subject", s(d)),
                            ("version", s(&format!("{prefix}{}", text(version)?))),
                        ]));
                    }
                }
            }
        }
        graph.insert(
            format!("{subject}@{version}"),
            obj([
                ("subject", s(subject)),
                ("version", s(version)),
                (
                    "state",
                    s(&if recorded {
                        subject_state(p, subject, version)?
                    } else {
                        "unavailable".into()
                    }),
                ),
                ("dependencies", V::List(deps)),
            ]),
        );
    }
    Ok(V::Map(graph))
}
fn outcomes(nodes: &Map) -> Result<V> {
    let mut results = BTreeMap::<String, BTreeSet<String>>::new();
    let mut add = |id: &str, state: &str| {
        results.entry(id.into()).or_default().insert(state.into());
    };
    for (subject, node) in nodes {
        let node = map(node)?;
        let state = map(&node["state"])?;
        match text(&map(&state["falsifier"])?["status"])? {
            "holds" => add(subject, "fired"),
            "unknown" => add(subject, "unknown"),
            "error" => add(subject, "unavailable"),
            _ => {
                if let Ok(c) = map(&node["computation"]) {
                    match c.get("status").and_then(|v| text(v).ok()) {
                        Some("unknown") => add(subject, "unknown"),
                        Some(s) if s != "ok" => add(subject, "unavailable"),
                        _ => {}
                    }
                }
            }
        }
        for (d, f) in map(&map(&state["basis"])?["dependencies"])? {
            let f = map(f)?;
            if f.get("comparison").is_some_and(|v| string_is(v, "changed"))
                || f.get("rule_changed") == Some(&V::Bool(true))
                || f.get("basis_comparison")
                    .is_some_and(|v| string_is(v, "changed"))
            {
                add(d, "moved")
            }
            if let Some(c) = f.get("current").and_then(|v| map(v).ok()) {
                match c.get("status").and_then(|v| text(v).ok()) {
                    Some("unknown") => add(d, "unknown"),
                    Some(s) if s != "ok" => add(d, "unavailable"),
                    _ => {}
                }
            }
        }
    }
    val(json!(results))
}
fn assure(node: &Map, recorded: &V, reviews: &V) -> Result<V> {
    let state = map(&node["state"])?;
    let take = |v: &V| {
        map(v)
            .ok()
            .and_then(|m| m.get("assurance"))
            .cloned()
            .unwrap_or(V::Null)
    };
    let deps = map(&map(&state["basis"])?["dependencies"])?
        .iter()
        .map(|(d, f)| {
            Ok((
                d.clone(),
                map(f)?.get("computation").map(take).unwrap_or(V::Null),
            ))
        })
        .collect::<Result<Map>>()?;
    let mut kinds = vec![];
    if truth(reviews) {
        kinds.push(s("review_pin"))
    }
    if truth(recorded) {
        kinds.push(s("version_pin"))
    }
    Ok(obj([
        (
            "actual",
            obj([
                ("node", take(&node["computation"])),
                (
                    "falsifier",
                    map(&state["falsifier"])?
                        .get("computation")
                        .map(take)
                        .unwrap_or(V::Null),
                ),
                ("dependencies", V::Map(deps)),
            ]),
        ),
        ("recorded_evidence_kinds", V::List(kinds)),
    ]))
}
fn review_evidence(projection: &V, reviews: &V) -> Result<V> {
    let mut result = vec![];
    for r in list(reviews)? {
        let r = map(r)?;
        let b = map(&r["body"])?;
        let mut read = Map::new();
        if let Some(v) = b.get("read") {
            for (d, v) in map(v)? {
                read.insert(
                    d.clone(),
                    H::pin_review_evidence(projection, text(v)?, Some(d))?,
                );
            }
        }
        result.push(obj([
            ("review_id", r["id"].clone()),
            ("of", b["of"].clone()),
            ("read", V::Map(read)),
        ]));
    }
    Ok(V::List(result))
}
fn project_subject(
    projection: &V,
    subject: &str,
    graph: &V,
    outcomes: &V,
    budget: &mut H::SupportBudget,
) -> Result<V> {
    let p = map(projection)?;
    let state = map(&map(&p["subjects"])?[subject])?;
    let defaults = obj([
        ("marks", empty()),
        ("proposals", arr()),
        ("contested_claims", arr()),
        ("reviews", arr()),
        ("implied", arr()),
    ]);
    let d = p
        .get("dispositions")
        .and_then(|v| map(v).ok())
        .and_then(|m| m.get(subject))
        .unwrap_or(&defaults);
    let d = map(d)?;
    let heads = names(&state["heads"])?;
    let witnesses = heads
        .iter()
        .map(|v| H::pin_review_evidence(projection, v, Some(subject)))
        .collect::<Result<Vec<_>>>()?;
    let selected = heads
        .iter()
        .min()
        .and_then(|h| map(&p["pins"]).ok()?.get(h))
        .and_then(|v| map(v).ok());
    let mut recorded = Map::new();
    if let Some(w) = selected.filter(|m| string_is(&m["status"], "recorded"))
        && let Some(pins) = map(&w["object"])?.get("pins")
    {
        for (d, v) in map(pins)? {
            recorded.insert(
                d.clone(),
                H::pin_review_evidence(projection, text(v)?, Some(d))?,
            );
        }
    }
    let reviews = d.get("reviews").cloned().unwrap_or_else(arr);
    let coverage = map(&p["coverage"])?;
    let covered = names(&coverage["subjects"])?.contains(&subject.into());
    let findings = list(&map(&p["integrity"])?["findings"])?
        .iter()
        .filter(|f| {
            map(f)
                .ok()
                .and_then(|m| m.get("subject"))
                .is_some_and(|v| string_is(v, subject))
        })
        .cloned()
        .collect::<Vec<_>>();
    let roots = heads
        .iter()
        .map(|v| format!("{subject}@{v}"))
        .collect::<Vec<_>>();
    let mut result = map(&obj([
        ("acceptance", state["acceptance"].clone()),
        ("head_ids", state["heads"].clone()),
        ("open_act_ids", state["open_acts"].clone()),
        ("head_witnesses", V::List(witnesses)),
        ("proposals", d.get("proposals").cloned().unwrap_or_else(arr)),
        ("reviews", reviews.clone()),
        (
            "disposition_marks",
            d.get("marks").cloned().unwrap_or_else(empty),
        ),
        (
            "contested_claims",
            d.get("contested_claims").cloned().unwrap_or_else(arr),
        ),
        (
            "implied_reservations",
            d.get("implied").cloned().unwrap_or_else(arr),
        ),
        ("recorded_support", V::Map(recorded)),
        (
            "pin_review_evidence",
            review_evidence(projection, &reviews)?,
        ),
        (
            "coverage",
            obj([
                ("included", V::Bool(covered)),
                (
                    "complete",
                    V::Bool(covered && truth(&coverage["complete"]) && findings.is_empty()),
                ),
                ("findings", V::List(findings)),
            ]),
        ),
        (
            "support",
            H::reduce_support_graph(&roots, graph, outcomes, H::MAX_VISITS, budget)?,
        ),
    ]))?
    .clone();
    if let Some(v) = d.get("source_state") {
        result.insert("source_state".into(), v.clone());
    }
    Ok(V::Map(result))
}
fn enrich(id: &str, node: &V, projection: Option<&V>, subjects: &Map) -> Result<V> {
    let mut n = map(node)?.clone();
    let projected = subjects.get(id);
    let inactive = projection.is_none();
    let status = if inactive {
        "not_applicable"
    } else if let Some(p) = projected {
        text(&map(p)?["acceptance"])?
    } else {
        "unavailable"
    };
    let p = projected.map(map).transpose()?;
    n.insert(
        "acceptance".into(),
        obj([
            ("status", s(status)),
            (
                "head_ids",
                p.map(|m| m["head_ids"].clone()).unwrap_or_else(arr),
            ),
            (
                "open_act_ids",
                p.map(|m| m["open_act_ids"].clone()).unwrap_or_else(arr),
            ),
        ]),
    );
    let mut history = Map::from([("subject_id".into(), if inactive { V::Null } else { s(id) })]);
    for (out, key, default) in [
        ("current_head_witnesses", "head_witnesses", arr()),
        ("proposals", "proposals", arr()),
        ("reviews", "reviews", arr()),
        ("disposition_marks", "disposition_marks", empty()),
        ("contested_claims", "contested_claims", arr()),
        ("implied_reservations", "implied_reservations", arr()),
        ("recorded_support", "recorded_support", empty()),
        ("pin_review_evidence", "pin_review_evidence", arr()),
    ] {
        history.insert(out.into(), p.map(|m| m[key].clone()).unwrap_or(default));
    }
    n.insert(
        "assurance".into(),
        assure(
            &n,
            &history["recorded_support"],
            &history["pin_review_evidence"],
        )?,
    );
    n.insert("history".into(), V::Map(history));
    if let Some(p) = p {
        n.insert("coverage".into(), p["coverage"].clone());
        n.insert("support".into(), p["support"].clone());
        if let Some(temporal) = p.get("temporal") {
            n.insert("temporal".into(), temporal.clone());
            let t = map(temporal)?;
            if string_is(&t["status"], "counterexample") || string_is(&t["status"], "unknown") {
                let reason = obj([
                    (
                        "code",
                        s(if string_is(&t["status"], "counterexample") {
                            "historical_counterexample"
                        } else {
                            "historical_evidence_unknown"
                        }),
                    ),
                    ("related_ids", t["counterexample_claim_ids"].clone()),
                ]);
                let attention = n.get_mut("attention").unwrap();
                let V::List(actions) = attention else {
                    return Err(error("invalid attention"));
                };
                if !actions.iter().any(|a| {
                    map(a).is_ok_and(|a| {
                        string_is(&a["action"], "review")
                            && list(&a["reasons"]).is_ok_and(|r| r.contains(&reason))
                    })
                }) {
                    actions.push(obj([
                        ("action", s("review")),
                        ("reasons", V::List(vec![reason])),
                    ]));
                }
            }
        }
    } else {
        n.insert("coverage".into(),obj([("included",V::Bool(false)),("complete",V::Bool(inactive)),("findings",if inactive{arr()}else{val(json!([{"code":"history_subject_unavailable","subject":id,"object_id":"","detail":"computational node is outside captured history coverage"}]))?})]));
        n.insert("support".into(),val(json!({"reducer":H::REDUCER,"status":if inactive{"clear"}else{"reserved"},"visited":[],"visited_count":0,"reservations":if inactive{json!([])}else{json!([{"code":"missing_support","state":"unavailable","subject":id,"version":"","path":[]}])}}))?);
    }
    Ok(V::Map(n))
}
fn summary(projection: Option<&V>) -> Result<V> {
    let Some(p) = projection else {
        return Ok(obj([("authority_status", s("not_active"))]));
    };
    let p = map(p)?;
    let a = map(&p["authority"])?;
    let mut result = obj([
        ("authority_status", s("active")),
        ("projection_version", p["projection_version"].clone()),
        (
            "authority",
            obj([
                ("record_id", a["record_id"].clone()),
                ("generation", a["generation"].clone()),
                ("identity", s(&digest(&p["authority"])?)),
            ]),
        ),
        (
            "committed_set_digest",
            map(&p["baseline"])?["committed_set_digest"].clone(),
        ),
        ("closure_digest", p["closure_digest"].clone()),
        ("coverage", p["coverage"].clone()),
        ("identity_schemes", p["identity_schemes"].clone()),
        ("integrity", p["integrity"].clone()),
    ]);
    if matches!(p.get("requires"), Some(V::List(v)) if v.iter().any(|v| string_is(v,crate::history_authority::TEMPORAL_APPLICABILITY)))
    {
        crate::history_view::map_mut(&mut result)?.insert(
            "capabilities".into(),
            V::List(vec![s(crate::history_authority::TEMPORAL_APPLICABILITY)]),
        );
    }
    Ok(result)
}
pub fn from_v2(snapshot: &Snapshot, report: &V, display_selection: Option<&[String]>) -> Result<V> {
    from_v2_temporal(snapshot, report, display_selection, None)
}
/// Adapt retained evidence without opening a source or invoking an evaluator.
pub fn from_v2_temporal(
    snapshot: &Snapshot,
    report: &V,
    display_selection: Option<&[String]>,
    temporal_evidence: Option<&V>,
) -> Result<V> {
    let base = validate_v2(snapshot, report)?;
    let base = map(&base)?;
    let projection = map(&map(snapshot.data())?["context"])?
        .get("history")
        .filter(|v| **v != V::Null);
    let outcomes = outcomes(map(&base["nodes"])?)?;
    let mut subjects = Map::new();
    if let Some(p) = projection {
        history_projection::validate_projection(p)?;
        let temporal_replays = if let Some(evidence) = temporal_evidence {
            Temporal::retained(p, map(evidence)?)?
        } else {
            Temporal::unknown(p)?
        };
        let graph = graph(map(p)?)?;
        let mut budget = H::SupportBudget::default();
        for id in map(&map(p)?["subjects"])?.keys() {
            subjects.insert(
                id.clone(),
                project_subject(p, id, &graph, &outcomes, &mut budget)?,
            );
        }
        if map(p)?.get("temporal").is_some_and(|v| *v != V::Null) {
            for (id, projected) in &mut subjects {
                let episodes = temporal_replays
                    .get(id)
                    .map(list)
                    .transpose()?
                    .unwrap_or(&[]);
                if let Some(temporal) =
                    Temporal::state(p, id, map(&base["nodes"])?.get(id), episodes)?
                {
                    crate::history_view::map_mut(projected)?.insert("temporal".into(), temporal);
                }
            }
        }
    }
    let nodes = map(&base["nodes"])?
        .iter()
        .map(|(id, n)| Ok((id.clone(), enrich(id, n, projection, &subjects)?)))
        .collect::<Result<Map>>()?;
    let default = names(&base["selection"])?;
    let display = display_selection.unwrap_or(&default);
    require(
        display.iter().collect::<BTreeSet<_>>().len() == display.len()
            && display
                .iter()
                .all(|id| nodes.contains_key(id) || subjects.contains_key(id)),
        "invalid display selection",
    )?;
    let mut r = map(&obj([
        ("schema_version", val(json!(3))?),
        ("assessment_profile", s("core/v1")),
        ("attention_policy", base["attention_policy"].clone()),
        ("snapshot_id", base["snapshot_id"].clone()),
        ("as_of", base["as_of"].clone()),
        ("assessment_selection", base["selection"].clone()),
        (
            "history_selection",
            V::List(subjects.keys().map(|s| V::Text(s.clone())).collect()),
        ),
        (
            "display_selection",
            V::List(display.iter().map(|s| V::Text(s.clone())).collect()),
        ),
        ("operational_limits", base["operational_limits"].clone()),
        ("scope", base["scope"].clone()),
        ("history", summary(projection)?),
        (
            "base_assessment_revision",
            base["assessment_revision"].clone(),
        ),
        ("nodes", V::Map(nodes)),
        ("history_subjects", V::Map(subjects)),
    ]))?
    .clone();
    r.insert(
        "findings_revision".into(),
        s(&digest(&findings_preimage(&r)?)?),
    );
    let revision = digest(&V::Map(r.clone()))?;
    r.insert("envelope_revision".into(), s(&revision));
    validate(&V::Map(r))
}
pub fn assess(
    snapshot: &Snapshot,
    selection: Option<&[String]>,
    policy: &str,
    runtime: Option<&Runtime>,
    bounds: OperationalBounds,
    display: Option<&[String]>,
) -> Result<V> {
    let base = A::assess(snapshot, selection, policy, runtime, bounds)?;
    let projection = map(&map(snapshot.data())?["context"])?
        .get("history")
        .filter(|v| **v != V::Null);
    let evidence = if let Some(p) = projection {
        history_projection::validate_projection(p)?;
        V::Map(Temporal::replay(p, runtime)?)
    } else {
        empty()
    };
    from_v2_temporal(snapshot, &base, display, Some(&evidence))
}
