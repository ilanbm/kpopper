//! Versioned node-local receipt components. Evidence events never introduce semantic claims.
use crate::{
    Result, history_contract::*, history_node_codec::Version, history_node_receipt::Nodes,
    history_view::map_mut, require, value::TypedValue as V,
};
use std::collections::{BTreeMap, BTreeSet};

const FORMAT: &str = "node-semantic-evidence/v1";
const SEMANTIC: &str = "node-semantic-object/v1";

pub(crate) struct Payload {
    pub semantic: V,
    pub is_semantic: bool,
    pub phase: String,
    pub receipt: Option<V>,
}

pub(crate) fn decode(value: &V) -> Result<Payload> {
    let p = schema(value, &["collection", "body", "context"], &[])?;
    let context = map(&p["context"])?;
    if string_is(field(context, "format")?, SEMANTIC) {
        schema(&p["context"], &["format", "header", "observation"], &[])?;
        return Ok(Payload {
            semantic: value.clone(),
            is_semantic: true,
            phase: "semantic".into(),
            receipt: None,
        });
    }
    let context = schema(
        &p["context"],
        &[
            "format",
            "header",
            "observation",
            "event_kind",
            "phase",
            "receipt",
        ],
        &[],
    )?;
    require(
        string_is(&context["format"], FORMAT),
        "node_semantic_format",
    )?;
    let kind = text(&context["event_kind"])?;
    let phase = text(&context["phase"])?;
    require(
        ["semantic", "evidence"].contains(&kind)
            && ["before", "semantic", "after"].contains(&phase),
        "node_evidence_variant",
    )?;
    require(
        kind != "evidence" || phase != "semantic",
        "node_evidence_variant",
    )?;
    let receipt = (context["receipt"] != V::Null).then(|| context["receipt"].clone());
    if let Some(receipt) = &receipt {
        Nodes::from_values(Map::from([("node".into(), receipt.clone())]))?;
    }
    let mut semantic = value.clone();
    let c = map_mut(map_mut(&mut semantic)?.get_mut("context").unwrap())?;
    for name in ["event_kind", "phase", "receipt"] {
        c.remove(name);
    }
    c.insert("format".into(), V::Text(SEMANTIC.into()));
    Ok(Payload {
        semantic,
        is_semantic: kind == "semantic",
        phase: phase.into(),
        receipt,
    })
}

pub(crate) fn encode(semantic: &V, kind: &str, phase: &str, receipt: Option<&V>) -> Result<V> {
    let mut value = decode(semantic)?.semantic;
    let c = map_mut(map_mut(&mut value)?.get_mut("context").unwrap())?;
    c.insert("format".into(), V::Text(FORMAT.into()));
    c.insert("event_kind".into(), V::Text(kind.into()));
    c.insert("phase".into(), V::Text(phase.into()));
    c.insert("receipt".into(), receipt.cloned().unwrap_or(V::Null));
    decode(&value)?;
    Ok(value)
}

pub(crate) fn tip(versions: &BTreeMap<String, Version>) -> Result<Option<&Version>> {
    let parents = versions
        .values()
        .flat_map(|v| v.parents())
        .collect::<BTreeSet<_>>();
    let tips = versions
        .iter()
        .filter(|(id, _)| !parents.contains(id))
        .map(|(_, v)| v)
        .collect::<Vec<_>>();
    require(tips.len() <= 1, "node_semantic_merge_required")?;
    Ok(tips.first().copied())
}

pub(crate) fn validate_evidence(
    version: &Version,
    versions: &BTreeMap<String, Version>,
    payload: &Payload,
) -> Result<()> {
    if !payload.is_semantic {
        require(!version.parents().is_empty(), "node_evidence_parent")?;
        // A storage merge may reuse an existing semantic payload; it never creates a claim.
        let parent = version
            .parents()
            .iter()
            .filter_map(|id| versions.get(id))
            .find(|v| {
                v.state()
                    .and_then(|s| decode(s).ok())
                    .is_some_and(|p| p.semantic == payload.semantic)
            })
            .ok_or_else(|| error("node_evidence_parent"))?;
        let previous = decode(
            parent
                .state()
                .ok_or_else(|| error("node_evidence_parent"))?,
        )?;
        require(
            previous.semantic == payload.semantic,
            "node_evidence_changed_semantics",
        )?;
    }
    Ok(())
}
