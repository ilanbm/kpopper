//! Small retained commitments for compact temporal transactions.
//! Snapshot bodies and assessment nodes remain in verified semantic history.
use crate::{
    Result,
    history_contract::*,
    history_transaction as T, reasoning_history_assessment as H,
    reasoning_runtime::OperationalBounds,
    reasoning_snapshot::{Snapshot, digest},
    reasoning_temporal, require,
    value::TypedValue as V,
};
use std::collections::BTreeMap;

fn s(value: &str) -> V {
    V::Text(value.into())
}
fn hash(value: &V) -> bool {
    text(value).is_ok_and(|s| s.len() == 64 && crate::history_paths::object_id(s))
}
fn parse_bounds(value: &V) -> Result<OperationalBounds> {
    let limits = schema(
        value,
        &[
            "timeout_seconds",
            "batch_requests",
            "input_bytes",
            "output_bytes",
        ],
        &[],
    )?;
    let number = |key: &str| -> Result<usize> {
        let V::Integer(value) = &limits[key] else {
            return Err(error("node_temporal_recipe_limits"));
        };
        value
            .as_str()
            .parse()
            .map_err(|_| error("node_temporal_recipe_limits"))
    };
    let timeout = u64::try_from(number("timeout_seconds")?)
        .map_err(|_| error("node_temporal_recipe_limits"))?;
    let bounds = OperationalBounds {
        timeout: std::time::Duration::from_secs(timeout),
        batch_requests: number("batch_requests")?,
        input_bytes: number("input_bytes")?,
        output_bytes: number("output_bytes")?,
    };
    bounds.validate()?;
    Ok(bounds)
}

fn side(value: &V) -> Result<V> {
    let side = map(value)?;
    let Some(replay) = side.get("temporal_replay").filter(|v| **v != V::Null) else {
        return Ok(V::Null);
    };
    let replay = schema(replay, &["version", "snapshot", "claims"], &[])?;
    require(is_int(&replay["version"], "1"), "node_temporal_recipe")?;
    let snapshot = Snapshot::from_json(text(&replay["snapshot"])?.as_bytes())?;
    let claims = map(&replay["claims"])?;
    require(!claims.is_empty(), "node_temporal_recipe_empty")?;
    let assessment = side
        .get("assessment")
        .filter(|v| **v != V::Null)
        .ok_or_else(|| error("node_temporal_recipe_assessment_required"))?;
    let assessment = H::validate_v2(&snapshot, assessment)?;
    let report = map(&assessment)?;
    let nodes = map(&report["nodes"])?;
    let mut semantic = Map::new();
    for (subject, claim) in claims {
        require(hash(claim), "node_temporal_recipe_claim")?;
        let node = nodes
            .get(subject)
            .ok_or_else(|| error("node_temporal_recipe_node_missing"))?;
        semantic.insert(
            subject.clone(),
            s(&digest(&reasoning_temporal::semantic_node(node)?)?),
        );
    }
    let recipe = V::Map(Map::from([
        ("version".into(), crate::history_authoring::n("1")),
        ("snapshot_id".into(), s(snapshot.snapshot_id())),
        (
            "attention_policy".into(),
            report["attention_policy"].clone(),
        ),
        (
            "operational_limits".into(),
            report["operational_limits"].clone(),
        ),
        ("semantic_digest".into(), s(&digest(&V::Map(semantic))?)),
    ]));
    validate_side(&recipe)?;
    Ok(recipe)
}

pub(crate) fn encode(receipt: &V) -> Result<V> {
    T::validate_receipt(receipt)?;
    let receipt = map(receipt)?;
    let before = side(&receipt["before"])?;
    let after = side(&receipt["after"])?;
    if before == V::Null && after == V::Null {
        return Ok(V::Null);
    }
    let recipe = V::Map(BTreeMap::from([
        ("before".into(), before),
        ("after".into(), after),
    ]));
    validate(&recipe)?;
    Ok(recipe)
}

pub(crate) fn validate_side(value: &V) -> Result<()> {
    let recipe = schema(
        value,
        &[
            "version",
            "snapshot_id",
            "attention_policy",
            "operational_limits",
            "semantic_digest",
        ],
        &[],
    )?;
    require(
        is_int(&recipe["version"], "1")
            && hash(&recipe["snapshot_id"])
            && hash(&recipe["semantic_digest"]),
        "node_temporal_recipe",
    )?;
    require(
        ["focused-review/v1", "falsifiers-only/v1"]
            .iter()
            .any(|p| string_is(&recipe["attention_policy"], p)),
        "node_temporal_recipe_policy",
    )?;
    parse_bounds(&recipe["operational_limits"])?;
    require(
        value.canonical_bytes()?.len() <= 64 * 1024,
        "node_temporal_recipe_limit",
    )
}
pub(crate) fn settings(value: &V) -> Result<(&str, OperationalBounds)> {
    validate_side(value)?;
    let recipe = map(value)?;
    Ok((
        text(&recipe["attention_policy"])?,
        parse_bounds(&recipe["operational_limits"])?,
    ))
}

pub(crate) fn validate(value: &V) -> Result<()> {
    if *value == V::Null {
        return Ok(());
    }
    let sides = schema(value, &["before", "after"], &[])?;
    require(
        sides.values().any(|v| *v != V::Null),
        "node_temporal_recipe_empty",
    )?;
    for side in sides.values() {
        if *side != V::Null {
            validate_side(side)?;
        }
    }
    require(
        value.canonical_bytes()?.len() <= 128 * 1024,
        "node_temporal_recipe_limit",
    )
}

pub(crate) fn phase(value: &V, phase: &str) -> Result<Option<V>> {
    validate(value)?;
    if *value == V::Null {
        return Ok(None);
    }
    let side = field(map(value)?, phase)?;
    Ok((side != &V::Null).then(|| side.clone()))
}
