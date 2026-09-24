//! Bounded temporal claims retain their exact immutable witnesses.
use crate::{Result, history_authority as A, history_contract::*, require, value::TypedValue as V};
use std::collections::BTreeSet;

fn list(value: &V) -> Result<&[V]> {
    match value {
        V::List(items) => Ok(items),
        _ => Err(error("invalid_temporal_projection")),
    }
}
fn hash(value: &V) -> Result<()> {
    require(
        text(value).is_ok_and(|s| s.len() == 64 && crate::history_paths::object_id(s)),
        "invalid_identifier",
    )
}
fn applicability(value: &V) -> bool {
    text(value).is_ok_and(|s| ["current", "anchored", "general"].contains(&s))
}
pub(crate) fn validate_projection(projection: &Map) -> Result<()> {
    let temporal = projection.get("temporal").filter(|v| **v != V::Null);
    let required = matches!(projection.get("requires"), Some(V::List(v)) if v.iter().any(|v| string_is(v,A::TEMPORAL_APPLICABILITY)));
    require(
        temporal.is_some() == required,
        "temporal_capability_mismatch",
    )?;
    let Some(temporal) = temporal else {
        return Ok(());
    };
    let t = schema(
        temporal,
        &["version", "complete", "observations", "findings"],
        &[],
    )?;
    // Python's projection envelope deliberately accepts numeric equality here;
    // immutable claim metadata above requires an actual integer version.
    require(
        (is_int(&t["version"], "1")
            || t["version"] == V::Bool(true)
            || matches!(&t["version"], V::Float(v) if v.get() == 1.0))
            && matches!(t["complete"], V::Bool(_)),
        "invalid_temporal_projection",
    )?;
    let observations = list(&t["observations"])?;
    require(observations.len() <= 64, "invalid_temporal_projection")?;
    let mut keys = BTreeSet::new();
    for observation in observations {
        let o = schema(
            observation,
            &[
                "operation",
                "phase",
                "evidence_kind",
                "snapshot",
                "assessment",
                "claims",
            ],
            &["recipe"],
        )?;
        token(&o["operation"])?;
        let recorded = string_is(&o["evidence_kind"], "recorded_receipt");
        let compact = string_is(&o["evidence_kind"], "retained_compact_recipe");
        require(
            o.contains_key("recipe") == compact,
            "invalid_temporal_projection",
        )?;
        if compact {
            crate::history_node_temporal_recipe::validate_side(&o["recipe"])?;
        }

        require(
            text(&o["phase"]).is_ok_and(|v| ["before", "after"].contains(&v))
                && (recorded
                    || compact
                    || string_is(&o["evidence_kind"], "reconstructed_committed_world"))
                && matches!(o["snapshot"], V::Text(_))
                && (if recorded {
                    matches!(o["assessment"], V::Map(_))
                } else {
                    o["assessment"] == V::Null
                }),
            "invalid_temporal_projection",
        )?;
        require(
            keys.insert((text(&o["operation"])?, text(&o["phase"])?)),
            "invalid_temporal_projection",
        )?;
        let mut claim_keys = Vec::new();
        for claim in list(&o["claims"])? {
            let c = schema(
                claim,
                &[
                    "subject",
                    "claim_id",
                    "applicability",
                    "predicate_digest",
                    "anchors",
                ],
                &[],
            )?;
            subject(&c["subject"])?;
            id(&c["claim_id"])?;
            hash(&c["predicate_digest"])?;
            require(
                applicability(&c["applicability"])
                    && map(&c["anchors"])
                        .is_ok_and(|m| m.keys().map(String::as_str).eq(["applies", "at", "on"])),
                "invalid_temporal_projection",
            )?;
            claim_keys.push((text(&c["subject"])?, text(&c["claim_id"])?));
        }
        require(
            claim_keys.windows(2).all(|w| w[0] < w[1]),
            "invalid_temporal_projection",
        )?;
    }
    let findings = list(&t["findings"])?;
    for finding in findings {
        let f = schema(finding, &["code", "subject", "object_id", "detail"], &[])?;
        require(
            f.values().all(|v| matches!(v, V::Text(_))),
            "invalid_temporal_projection",
        )?;
    }
    require(
        t["complete"] == V::Bool(findings.is_empty()),
        "invalid_temporal_projection",
    )
}
pub(crate) fn validate_witnesses(projection: &Map, pins: &Map) -> Result<()> {
    let Some(t) = projection.get("temporal").filter(|v| **v != V::Null) else {
        return Ok(());
    };
    for observation in list(&map(t)?["observations"])? {
        for claim in list(&map(observation)?["claims"])? {
            let c = map(claim)?;
            let witness = pins
                .get(text(&c["claim_id"])?)
                .and_then(|v| map(v).ok())
                .ok_or_else(|| error("temporal_claim_mismatch"))?;
            require(
                string_is(&witness["status"], "recorded") && witness["subject"] == c["subject"],
                "temporal_claim_mismatch",
            )?;
            let o = map(&witness["object"])?;
            let body = map(&o["body"]).map_err(|_| error("temporal_claim_mismatch"))?;
            let metadata = body
                .get("temporal")
                .and_then(|v| map(v).ok())
                .ok_or_else(|| error("temporal_claim_mismatch"))?;
            let predicate = o
                .get("authored")
                .and_then(|a| map(a).ok())
                .and_then(|a| a.get("fields"))
                .and_then(|f| map(f).ok())
                .and_then(|f| f.get("predicate"))
                .and_then(|p| text(p).ok())
                .ok_or_else(|| error("temporal_claim_mismatch"))?;
            require(
                metadata.get("applicability") == Some(&c["applicability"])
                    && string_is(
                        &c["predicate_digest"],
                        &body.get(predicate).unwrap_or(&V::Null).digest()?,
                    )
                    && ["on", "at", "applies"].iter().all(|k| {
                        map(&c["anchors"]).unwrap().get(*k) == Some(o.get(*k).unwrap_or(&V::Null))
                    }),
                "temporal_claim_mismatch",
            )?;
        }
    }
    Ok(())
}
