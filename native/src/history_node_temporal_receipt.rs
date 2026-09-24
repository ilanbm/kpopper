//! Exact temporal Snapshot recipes, partitioned through the same node-local world codec.
use crate::{
    Result,
    history_contract::*,
    history_node_receipt::Side,
    history_view::map_mut,
    reasoning_snapshot::{CaptureOptions, Snapshot},
    require,
    value::TypedValue as V,
};

fn empty() -> V {
    V::Map(Map::new())
}
fn s(v: &str) -> V {
    V::Text(v.into())
}

pub(crate) fn pack(value: &V, side_document: Option<&V>) -> Result<(V, Map)> {
    let replay = schema(value, &["version", "snapshot", "claims"], &[])?;
    let raw = text(&replay["snapshot"])?;
    let snapshot = Snapshot::from_json(raw.as_bytes())?;
    // Generated native snapshots have one exact encoding. Other lexemes need a raw archive.
    require(
        snapshot.to_json()? == raw,
        "node_temporal_snapshot_encoding",
    )?;
    let mut data = map(&snapshot.to_data())?.clone();
    // Captured history/scenarios may contain a complete graph; they require their own codec.
    schema(
        &data["context"],
        &[],
        &["read_mode", "source_collection", "conflicts"],
    )?;
    let document = data.remove("document").unwrap();
    let reuse_document = side_document == Some(&document);
    let mut world = Map::from([(
        "assessment".into(),
        V::Map(Map::from([
            ("snapshot_id".into(), data["snapshot_id"].clone()),
            ("nodes".into(), empty()),
            (
                "scope".into(),
                V::Map(Map::from([
                    ("context".into(), data.remove("context").unwrap()),
                    ("hypotheses".into(), data.remove("hypotheses").unwrap()),
                ])),
            ),
        ])),
    )]);
    if !reuse_document {
        world.insert("document".into(), document);
    }
    let world = Side::pack(&V::Map(world))?;
    // Snapshot nodes are a checked deterministic projection of its document.
    data.remove("nodes");
    let mut nodes = world
        .nodes()
        .iter()
        .map(|(subject, piece)| {
            (
                subject.clone(),
                V::Map(Map::from([("world".into(), piece.clone())])),
            )
        })
        .collect::<Map>();
    for (subject, claim) in map(&replay["claims"])? {
        crate::history_paths::subject(subject)?;
        id(claim)?;
        map_mut(nodes.entry(subject.clone()).or_insert_with(empty))?
            .insert("claim".into(), claim.clone());
    }
    let context = V::Map(Map::from([
        ("format".into(), s("node-temporal-replay/v1")),
        ("version".into(), replay["version"].clone()),
        ("snapshot".into(), V::Map(data)),
        ("world".into(), world.context().clone()),
        ("document_from_side".into(), V::Bool(reuse_document)),
    ]));
    require(
        restore(&context, &nodes, side_document)? == *value,
        "node_temporal_roundtrip",
    )?;
    Ok((context, nodes))
}

pub(crate) fn restore(context: &V, nodes: &Map, side_document: Option<&V>) -> Result<V> {
    let c = schema(
        context,
        &[
            "format",
            "version",
            "snapshot",
            "world",
            "document_from_side",
        ],
        &[],
    )?;
    let V::Bool(reuse_document) = c["document_from_side"] else {
        return Err(error("node_temporal_document"));
    };
    require(
        string_is(&c["format"], "node-temporal-replay/v1"),
        "node_temporal_format",
    )?;
    let header = schema(
        &c["snapshot"],
        &[
            "schema_version",
            "snapshot_id",
            "as_of",
            "authored_revision",
        ],
        &[],
    )?;
    let mut worlds = Map::new();
    let mut claims = Map::new();
    for (subject, value) in nodes {
        let piece = schema(value, &[], &["world", "claim"])?;
        require(!piece.is_empty(), "node_receipt_empty_piece")?;
        if let Some(world) = piece.get("world") {
            schema(world, &[], &["document", "hypotheses", "conflicts"])?;
            worlds.insert(subject.clone(), world.clone());
        }
        if let Some(claim) = piece.get("claim") {
            crate::history_paths::subject(subject)?;
            id(claim)?;
            claims.insert(subject.clone(), claim.clone());
        }
    }
    let world = Side::from_parts(c["world"].clone(), worlds)?.restore()?;
    let world = schema(&world, &["assessment"], &["document"])?;
    require(
        world.contains_key("document") != reuse_document,
        "node_temporal_document",
    )?;
    let document = if reuse_document {
        side_document.ok_or_else(|| error("node_temporal_document"))?
    } else {
        &world["document"]
    };
    let report = schema(
        &world["assessment"],
        &["snapshot_id", "nodes", "scope"],
        &[],
    )?;
    require(
        map(&report["nodes"])?.is_empty() && report["snapshot_id"] == header["snapshot_id"],
        "node_temporal_snapshot_context",
    )?;
    let scope = schema(&report["scope"], &["context", "hypotheses"], &[])?;
    schema(
        &scope["context"],
        &[],
        &["read_mode", "source_collection", "conflicts"],
    )?;
    let snapshot = Snapshot::from_data(
        document,
        CaptureOptions {
            as_of: Some(header["as_of"].clone()),
            authored_revision: Some(header["authored_revision"].clone()),
            context: Some(scope["context"].clone()),
            hypotheses: Some(scope["hypotheses"].clone()),
        },
    )?;
    let data = snapshot.to_data();
    require(
        header
            .iter()
            .all(|(k, v)| map(&data).unwrap().get(k) == Some(v)),
        "node_temporal_snapshot_context",
    )?;
    Ok(V::Map(Map::from([
        ("version".into(), c["version"].clone()),
        ("snapshot".into(), s(&snapshot.to_json()?)),
        ("claims".into(), V::Map(claims)),
    ])))
}
