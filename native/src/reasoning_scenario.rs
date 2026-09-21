//! Computational scenarios derive only captured alternatives. They carry no
//! history admission or acceptance authority and replay their retained sources.
use crate::history_authoring::{empty, n, obj, s, strings};
use crate::{
    Result,
    history_contract::*,
    history_view::{list, map_mut, truth},
    history_yaml::compact_json_size,
    reasoning_authoring as Authoring,
    reasoning_authoring_preparation::promotion_blockers,
    reasoning_context::CapturedAssessment,
    reasoning_evaluate::Evaluator,
    reasoning_fields as F, reasoning_language as L, reasoning_operations,
    reasoning_runtime::{OperationalBounds, Runtime},
    reasoning_snapshot::{CaptureOptions, MAX_REQUEST_BYTES, Snapshot, digest, entries},
    require,
    value::TypedValue as V,
};
use std::collections::BTreeSet;

pub const KIND: &str = "computational-scenario/v1";

#[cfg(test)]
#[path = "../tests/support/reasoning_scenario.rs"]
mod tests;
fn check(condition: bool, detail: &str) -> Result<()> {
    require(
        condition,
        &format!("invalid_computational_scenario: {detail}"),
    )
}
fn budget(value: &V) -> Result<()> {
    value.validate_bounded(MAX_REQUEST_BYTES / 8)?;
    require(
        compact_json_size(value) <= MAX_REQUEST_BYTES,
        "scenario_limit",
    )
}
fn source(value: &V) -> Result<Snapshot> {
    value.validate_bounded(MAX_REQUEST_BYTES / 8)?;
    let context = map(value)
        .ok()
        .and_then(|m| m.get("context"))
        .and_then(|v| map(v).ok());
    check(context.is_some(), "missing source")?;
    check(
        !context.unwrap().contains_key("scenario"),
        "nested scenarios are not supported",
    )?;
    Snapshot::from_snapshot(value)
}
fn core(document: &V) -> Result<bool> {
    Ok(string_is(
        field(map(&F::capabilities(document, None)?)?, "profile")?,
        "core/v1",
    ))
}
fn required(document: &V) -> Result<BTreeSet<String>> {
    list(field(map(&F::capabilities(document, None)?)?, "requires")?)?
        .iter()
        .map(|v| text(v).map(str::to_owned))
        .collect()
}
fn complete(data: &Map, label: &str) -> Result<()> {
    if let Some(history) = map(&data["context"])?.get("history") {
        let h = map(history)?;
        check(
            truth(field(map(field(h, "coverage")?)?, "complete")?)
                && truth(field(map(field(h, "integrity")?)?, "complete")?),
            label,
        )?;
    }
    Ok(())
}
fn legacy_literals(document: &V, label: &str) -> Result<()> {
    let mut semantic = F::snapshot_fields(document)?
        .values()
        .map(|v| text(v).map(str::to_owned))
        .collect::<Result<BTreeSet<_>>>()?;
    semantic.extend(
        [
            "rests_on",
            "seen",
            "wrong_if",
            "verdict",
            "rule",
            "blocked_on",
            "reopened_by",
        ]
        .map(str::to_owned),
    );
    for (identifier, (_, body)) in entries(document)? {
        if let V::Map(m) = body {
            check(
                ["v", "quoted", "asked", "url", "file", "read"]
                    .iter()
                    .any(|k| m.contains_key(*k))
                    && !m.keys().any(|k| semantic.contains(k)),
                &format!("{label} is not a literal reading or source: {identifier}"),
            )?;
        } else {
            check(
                matches!(
                    body,
                    V::Null | V::Text(_) | V::Bool(_) | V::Integer(_) | V::Float(_)
                ),
                &format!("{label} is not a literal reading: {identifier}"),
            )?;
        }
    }
    Ok(())
}
#[derive(Default)]
struct Overlays {
    chosen: Map,
    items: Vec<V>,
    collisions: BTreeSet<String>,
}
impl Overlays {
    fn add(
        &mut self,
        origin: &str,
        id: &str,
        collection: &str,
        body: &V,
        versions: &[V],
    ) -> Result<()> {
        let value = V::List(vec![s(collection), body.clone()]);
        if let Some(prior) = self.chosen.get(id)
            && digest(prior)? != digest(&value)?
        {
            self.collisions.insert(id.into());
            return Ok(());
        }
        self.chosen.insert(id.into(), value.clone());
        let mut versions = versions
            .iter()
            .map(|v| text(v).map(str::to_owned))
            .collect::<Result<Vec<_>>>()?;
        versions.sort();
        self.items.push(obj([
            ("origin", s(origin)),
            ("id", s(id)),
            ("digest", s(&digest(&value)?)),
            ("versions", strings(versions)),
        ]));
        Ok(())
    }
}
struct Derived {
    document: V,
    overlays: V,
    collisions: V,
    heads: V,
}
fn derive(source: &Snapshot, selection: &V, shared: Option<&Snapshot>) -> Result<Derived> {
    let data = map(source.data())?;
    check(core(&data["document"])?, "source profile is not core/v1")?;
    complete(data, "source history is incomplete")?;
    check(
        matches!(selection, V::Map(_)),
        "selection must name captured hypotheses",
    )?;
    let mut document = data["document"].clone();
    if let Some(meta) = map_mut(&mut document)?.get_mut("meta") {
        map_mut(meta)?.remove("history");
    }
    let known = entries(&document)?;
    let mut overlays = Overlays::default();
    let mut heads = Map::new();
    let mut requires = required(&document)?;
    let fields = F::snapshot_fields(&document)?;
    if let Some(shared) = shared {
        let other = map(shared.data())?;
        let other_core = core(&other["document"])?;
        if !other_core {
            check(
                promotion_blockers(&other["document"], Some(&document))?.is_empty(),
                "shared legacy executable fields need an explicit core/v1 interpretation",
            )?;
            legacy_literals(&other["document"], "shared legacy entry")?;
        } else {
            check(
                F::snapshot_fields(&other["document"])? == fields,
                "incompatible shared field roles",
            )?;
        }
        check(
            other["as_of"] == data["as_of"],
            "shared temporal basis differs",
        )?;
        complete(other, "shared history is incomplete")?;
        if other_core {
            requires.extend(required(&other["document"])?);
        }
        for (id, (collection, body)) in entries(&other["document"])? {
            if let Some((old_collection, old_body)) = known.get(&id) {
                if digest(&V::List(vec![s(old_collection), old_body.clone()]))?
                    != digest(&V::List(vec![s(&collection), body.clone()]))?
                {
                    overlays.collisions.insert(id);
                }
            } else {
                overlays.add("shared", &id, &collection, &body, &[])?;
            }
        }
    }
    for (name, identifiers) in map(selection)? {
        let hypotheses = map(&data["hypotheses"])?;
        check(
            hypotheses.contains_key(name),
            &format!("unknown hypothesis {name}"),
        )?;
        let identifiers = list(identifiers)
            .ok()
            .filter(|ids| ids.iter().all(|v| matches!(v, V::Text(_))));
        check(identifiers.is_some(), "noncanonical selection")?;
        let ids = identifiers
            .unwrap()
            .iter()
            .map(|v| text(v).unwrap())
            .collect::<Vec<_>>();
        check(
            ids.windows(2).all(|pair| pair[0] < pair[1]),
            "noncanonical selection",
        )?;
        let hypothesis = map(&hypotheses[name])?;
        check(
            !truth(field(hypothesis, "error")?),
            &format!("unreadable hypothesis {name}"),
        )?;
        check(
            !hypothesis
                .get("kind")
                .is_some_and(|v| string_is(v, "contribution")),
            "pending contribution is not a selected hypothesis",
        )?;
        let hyp_doc = field(hypothesis, "document")?;
        let source_entries = entries(hyp_doc)?;
        check(
            ids.iter().all(|id| source_entries.contains_key(*id)),
            &format!("selection outside hypothesis {name}"),
        )?;
        let declared = map(hyp_doc)?
            .get("meta")
            .and_then(|v| map(v).ok())
            .and_then(|m| m.get("reasoning"))
            .filter(|v| **v != V::Null);
        let head = field(hypothesis, "head")?;
        if declared.is_none() {
            check(
                promotion_blockers(hyp_doc, Some(&document))?.is_empty()
                    && !map(head)?.get("wrong_if").is_some_and(truth),
                &format!("legacy hypothesis needs an explicit core/v1 interpretation: {name}"),
            )?;
            legacy_literals(hyp_doc, "legacy hypothesis entry")?;
        } else {
            check(
                core(hyp_doc)?,
                &format!("incompatible hypothesis profile {name}"),
            )?;
            requires.extend(required(hyp_doc)?);
        }
        let schema = map(hyp_doc)?.get("schema");
        check(
            schema.is_none_or(|v| {
                map(v).is_ok_and(|m| {
                    m.iter()
                        .all(|(role, field)| fields.get(role).is_none_or(|v| v == field))
                })
            }),
            &format!("incompatible hypothesis field roles {name}"),
        )?;
        heads.insert(name.clone(), head.clone());
        let versions = map(&data["context"])?
            .get("history_hypotheses")
            .map(map)
            .transpose()?
            .and_then(|m| m.get("groups"))
            .map(map)
            .transpose()?
            .and_then(|m| m.get(name))
            .map(map)
            .transpose()?;
        for id in ids {
            let (collection, body) = &source_entries[id];
            if let V::Map(m) = body {
                check(
                    !["rule", text(&fields["predicate"])?]
                        .iter()
                        .any(|f| matches!(m.get(*f),Some(V::Text(v)) if !v.is_empty())),
                    &format!("uninterpreted hypothetical expression {id}"),
                )?;
            }
            let pins = versions
                .and_then(|m| m.get(id))
                .map(list)
                .transpose()?
                .unwrap_or(&[]);
            overlays.add(name, id, collection, body, pins)?;
        }
    }
    for (id, value) in &overlays.chosen {
        if overlays.collisions.contains(id) {
            continue;
        }
        let value = list(value)?;
        let collection = text(&value[0])?;
        let doc = map_mut(&mut document)?;
        for (name, members) in doc.iter_mut() {
            if name != collection
                && let V::Map(m) = members
            {
                m.remove(id);
            }
        }
        let members = doc.entry(collection.into()).or_insert_with(empty);
        if !matches!(members, V::Map(_)) {
            *members = empty();
        }
        map_mut(members)?.insert(id.clone(), value[1].clone());
    }
    document = Authoring::declare_document(&document)?;
    requires.extend(required(&document)?);
    map_mut(
        map_mut(map_mut(&mut document)?.get_mut("meta").unwrap())?
            .get_mut("reasoning")
            .unwrap(),
    )?
    .insert("requires".into(), strings(requires));
    Ok(Derived {
        document,
        overlays: V::List(overlays.items),
        collisions: strings(overlays.collisions),
        heads: V::Map(heads),
    })
}

/// A selection names captured bodies; it cannot supply replacement text.
pub fn build(observed: &Snapshot, selection: &V, shared: Option<&Snapshot>) -> Result<Snapshot> {
    let observed = source(observed.data())?;
    let shared = shared.map(|s| source(s.data())).transpose()?;
    let derived = derive(&observed, selection, shared.as_ref())?;
    let evidence = obj([
        ("version", n("1")),
        ("kind", s(KIND)),
        ("source", observed.to_data()),
        ("source_snapshot_id", s(observed.snapshot_id())),
        ("selection", selection.clone()),
        ("shared", shared.as_ref().map_or(V::Null, Snapshot::to_data)),
        (
            "shared_snapshot_id",
            shared.as_ref().map_or(V::Null, |v| s(v.snapshot_id())),
        ),
        ("overlays", derived.overlays),
        ("collisions", derived.collisions),
        ("heads", derived.heads),
    ]);
    budget(&evidence)?;
    let result = Snapshot::from_data(
        &derived.document,
        CaptureOptions {
            context: Some(obj([("read_mode", s("supplied")), ("scenario", evidence)])),
            as_of: Some(map(observed.data())?["as_of"].clone()),
            ..Default::default()
        },
    )?;
    result.to_json()?;
    Ok(result)
}

pub(crate) fn validate(
    document: &V,
    context: &V,
    hypotheses: &V,
    as_of: &V,
    snapshot_data: Option<&V>,
) -> Result<()> {
    let Some(evidence) = map(context)?.get("scenario") else {
        return Ok(());
    };
    obj([
        ("document", document.clone()),
        ("context", context.clone()),
        ("hypotheses", hypotheses.clone()),
    ])
    .validate_bounded(MAX_REQUEST_BYTES / 8)?;
    budget(evidence)?;
    if let Some(data) = snapshot_data {
        budget(&V::from_json_bounded(
            &data.to_tagged_bounded(MAX_REQUEST_BYTES / 8)?,
            MAX_REQUEST_BYTES / 8,
        )?)?;
    }
    let expected = [
        "version",
        "kind",
        "source",
        "source_snapshot_id",
        "selection",
        "shared",
        "shared_snapshot_id",
        "overlays",
        "collisions",
        "heads",
    ];
    let ev = map(evidence)?;
    check(
        ev.len() == expected.len()
            && expected.iter().all(|k| ev.contains_key(*k))
            && ev.get("version").is_some_and(|v| is_int(v, "1"))
            && ev.get("kind").is_some_and(|v| string_is(v, KIND)),
        "invalid evidence envelope",
    )?;
    check(
        *context == obj([("read_mode", s("supplied")), ("scenario", evidence.clone())])
            && map(hypotheses)?.is_empty(),
        "scenario has independent authority or hypotheses",
    )?;
    let source = source(&ev["source"])?;
    let shared = if ev["shared"] == V::Null {
        None
    } else {
        Some(self::source(&ev["shared"])?)
    };
    check(
        ev["source_snapshot_id"] == s(source.snapshot_id())
            && ev["shared_snapshot_id"] == shared.as_ref().map_or(V::Null, |v| s(v.snapshot_id())),
        "source identity mismatch",
    )?;
    check(
        *as_of == map(source.data())?["as_of"],
        "temporal basis changed",
    )?;
    let derived = derive(&source, &ev["selection"], shared.as_ref())?;
    check(
        digest(document)? == digest(&derived.document)?
            && ev["overlays"] == derived.overlays
            && ev["collisions"] == derived.collisions
            && ev["heads"] == derived.heads,
        "derived computation differs from retained sources",
    )
}

/// Return only computational findings and explicitly evaluated hypothesis heads.
pub fn assess(
    snapshot: &Snapshot,
    runtime: Option<&Runtime>,
    bounds: OperationalBounds,
) -> Result<V> {
    let snapshot = Snapshot::from_snapshot(snapshot.data())?;
    let data = map(snapshot.data())?;
    let context = map(&data["context"])?;
    check(
        context.contains_key("scenario"),
        "scenario snapshot required",
    )?;
    let report = CapturedAssessment::from_snapshot(
        snapshot.clone(),
        None,
        "focused-review/v1",
        runtime,
        bounds.clone(),
        None,
    )?;
    let projected = reasoning_operations::findings(&report)?;
    let projected = map(&projected)?;
    let findings = V::Map(
        ["falsified", "holes", "moved"]
            .into_iter()
            .map(|k| (k.into(), projected[k].clone()))
            .collect(),
    );
    let ev = map(&context["scenario"])?;
    let mut heads = vec![];
    for (name, head) in map(&ev["heads"])? {
        if let Some(expression) = map(head)?.get("wrong_if").filter(|v| **v != V::Null) {
            let (truth, computation) = if matches!(expression, V::Map(_)) {
                let tree = L::lower(expression)?;
                let refs = L::references(&tree);
                let result = Evaluator::new(&snapshot, runtime, None, bounds.clone())?
                    .evaluate(V::from_json(&tree)?, refs)?;
                let truth = if result["status"] == "ok" && result["value"]["type"] == "boolean" {
                    result["value"]["value"].as_bool().map_or(V::Null, V::Bool)
                } else {
                    V::Null
                };
                (truth, V::from_json(&result)?)
            } else {
                (V::Null, V::Null)
            };
            heads.push(obj([
                ("name", s(name)),
                ("truth", truth),
                ("computation", computation),
            ]));
        }
    }
    Ok(obj([
        ("findings", findings),
        ("heads", V::List(heads)),
        ("collisions", ev["collisions"].clone()),
        ("source_snapshot_id", ev["source_snapshot_id"].clone()),
        ("scenario_id", s(snapshot.snapshot_id())),
        ("snapshot", s(&snapshot.to_json()?)),
    ]))
}
