//! Followup readings from validated core assessments and their retained evidence.
use crate::{
    Result,
    history_contract::{self as H, Map},
    history_view::{list, map_mut},
    require,
    value::TypedValue as V,
};
use serde_json::{Value as J, json};
use std::collections::BTreeSet;

fn object(items: impl IntoIterator<Item = (&'static str, V)>) -> V {
    V::Map(items.into_iter().map(|(k, v)| (k.into(), v)).collect())
}

fn computation(value: &V) -> Result<V> {
    if *value == V::Null {
        return Ok(V::Null);
    }
    let value = H::map(value)?;
    Ok(object(
        [
            "status",
            "value",
            "basis",
            "diagnostics",
            "potential_dependencies",
        ]
        .into_iter()
        .map(|key| (key, value.get(key).cloned().unwrap_or(V::Null))),
    ))
}

fn evidence(node: &V) -> Result<V> {
    let node = H::map(node)?;
    let mut state = node["state"].clone();
    let state_fields = map_mut(&mut state)?;
    let basis = map_mut(
        state_fields
            .get_mut("basis")
            .ok_or_else(|| H::error("invalid_followup_evidence"))?,
    )?;
    for dependency in map_mut(
        basis
            .get_mut("dependencies")
            .ok_or_else(|| H::error("invalid_followup_evidence"))?,
    )?
    .values_mut()
    {
        let dependency = map_mut(dependency)?;
        if let Some(value) = dependency.get_mut("computation") {
            *value = computation(value)?;
        }
    }
    let falsifier = map_mut(
        state_fields
            .get_mut("falsifier")
            .ok_or_else(|| H::error("invalid_followup_evidence"))?,
    )?;
    if let Some(value) = falsifier.get_mut("computation") {
        *value = computation(value)?;
    }
    let mut result = [
        "body",
        "fields",
        "acceptance",
        "history",
        "support",
        "coverage",
    ]
    .into_iter()
    .map(|key| Ok((key.to_owned(), H::field(node, key)?.clone())))
    .collect::<Result<Map>>()?;
    result.insert("state".into(), state);
    result.insert("computation".into(), computation(&node["computation"])?);
    Ok(V::Map(result))
}

fn reading(value: V, available: bool, findings: V, mut evidence: V) -> Result<J> {
    let reading = object([
        ("available", V::Bool(available)),
        ("value", value),
        ("findings", findings),
    ]);
    map_mut(&mut evidence)?.insert("reading".into(), reading.clone());
    let reading = crate::followup_store::typed_json(&reading)?;
    Ok(json!({"core":{
        "version":1,"available":reading["available"],"value":reading["value"],"findings":reading["findings"],
        "evidence":{"encoding":"typed-json/v1","payload":evidence.to_tagged()?,"digest":evidence.digest()?}
    }}))
}

pub(crate) struct Projection {
    pub values: serde_json::Map<String, J>,
    pub maintenance: Vec<J>,
}

pub(crate) fn project(
    report: &V,
    snapshot_id: &str,
    findings_revision: &str,
) -> Result<Projection> {
    let report = H::map(report)?;
    let history = H::map(&report["history"])?;
    if H::string_is(&history["authority_status"], "active") {
        require(
            H::map(&history["coverage"])?["complete"] == V::Bool(true)
                && H::map(&history["integrity"])?["complete"] == V::Bool(true),
            "captured history coverage or integrity is incomplete",
        )?;
    }
    let mut result = Projection {
        values: serde_json::Map::new(),
        maintenance: Vec::new(),
    };
    for (id, node) in H::map(&report["nodes"])? {
        let n = H::map(node)?;
        let state = H::map(&n["state"])?;
        let findings = crate::reasoning_projection::project_node_status(node)?;
        let mut available = H::map(&n["coverage"])?["complete"] == V::Bool(true)
            && list(&H::map(&state["integrity"])?["issues"])?.is_empty()
            && H::string_is(&H::map(&state["contention"])?["status"], "none_detected")
            && ["accepted", "not_applicable"]
                .contains(&H::text(&H::map(&n["acceptance"])?["status"])?);
        let mut value = V::Null;
        if n["computation"] != V::Null {
            let computation = H::map(&n["computation"])?;
            let ok = H::string_is(&computation["status"], "ok");
            available &= ok;
            if ok {
                crate::reasoning_values::validate(&computation["value"])?;
                value = computation["value"].clone();
            }
        } else if let Some(V::Text(verdict)) = H::map(&n["body"])
            .ok()
            .and_then(|body| body.get("verdict").or_else(|| body.get("title")))
        {
            value = object([
                ("type", V::Text("text".into())),
                ("value", V::Text(verdict.clone())),
            ]);
        } else {
            available = false;
        }
        result.values.insert(
            id.clone(),
            reading(value, available, findings.clone(), evidence(node)?)?,
        );
        let mut reasons = BTreeSet::new();
        for action in list(&n["attention"])? {
            for reason in list(&H::map(action)?["reasons"])? {
                reasons.insert(H::text(&H::map(reason)?["code"])?.to_owned());
            }
        }
        let mut reasons = reasons.into_iter().collect::<Vec<_>>();
        if !available {
            reasons.push("reading_unavailable".into());
        }
        if !reasons.is_empty() || H::string_is(&H::map(&n["support"])?["status"], "reserved") {
            if reasons.is_empty() {
                reasons.push("support_reserved".into());
            }
            result.maintenance.push(json!({"id":id,"reasons":reasons,"findings":crate::followup_store::typed_json(&findings)?,"snapshot_id":snapshot_id,"findings_revision":findings_revision}));
        }
    }
    for (id, subject) in H::map(&report["history_subjects"])? {
        if !result.values.contains_key(id) {
            result.values.insert(
                id.clone(),
                reading(
                    V::Null,
                    false,
                    subject.clone(),
                    object([("subject", subject.clone())]),
                )?,
            );
            result.maintenance.push(json!({"id":id,"reasons":["reading_unavailable"],"findings":crate::followup_store::typed_json(subject)?,"snapshot_id":snapshot_id,"findings_revision":findings_revision}));
        }
    }
    Ok(result)
}

/// Called only for values tagged by the captured graph or core_baseline marker.
pub(crate) fn envelope(value: &J) -> Result<&J> {
    let root = value
        .as_object()
        .ok_or_else(|| H::error("invalid core followup reading"))?;
    require(
        root.len() == 1 && root.contains_key("core"),
        "invalid core followup reading",
    )?;
    let core = &root["core"];
    let item = core
        .as_object()
        .ok_or_else(|| H::error("invalid core followup reading"))?;
    require(
        item.len() == 5
            && ["version", "available", "value", "evidence", "findings"]
                .iter()
                .all(|key| item.contains_key(*key))
            && item["version"].as_u64() == Some(1)
            && item["available"].is_boolean()
            && item["findings"].is_object(),
        "invalid core followup reading",
    )?;
    let retained = item["evidence"]
        .as_object()
        .ok_or_else(|| H::error("invalid core followup evidence"))?;
    require(
        retained.len() == 3
            && ["encoding", "payload", "digest"]
                .iter()
                .all(|key| retained.contains_key(*key))
            && retained["encoding"] == "typed-json/v1",
        "invalid core followup evidence",
    )?;
    let decoded = V::from_tagged(&retained["payload"])?;
    require(
        decoded.to_tagged()? == retained["payload"]
            && retained["digest"].as_str() == Some(decoded.digest()?.as_str()),
        "noncanonical core followup evidence",
    )?;
    let reading = V::from_json(
        &json!({"available":item["available"],"value":item["value"],"findings":item["findings"]}),
    )?;
    let decoded_reading = H::map(&decoded)?
        .get("reading")
        .ok_or_else(|| H::error("core followup reading does not match retained evidence"))?;
    require(
        decoded_reading.digest()? == reading.digest()?,
        "core followup reading does not match retained evidence",
    )?;
    if item["available"] == true {
        crate::reasoning_values::validate(&V::from_json(&item["value"])?)?;
    }
    Ok(core)
}
