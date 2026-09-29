//! Pure, same-snapshot continuation of canonical V3 views.
//!
//! A materialized value proves decoding, not host acknowledgment or retention.
//! The caller must gate its use on a live context receipt. This module owns no
//! session, receipt, cache, selector, graph, or history state.
use crate::{Error, Result, identity::sha256, require, value::TypedValue};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as J, json};
use std::collections::{BTreeMap, BTreeSet};

pub const SCHEMA: &str = "kpopper.selected-view-delta/v1";
pub const HEADER: &str = "# Checked graph delta (kpopper.selected-view-delta/v1)";
pub const RULE: &str = "Evidence additions remain received; focus.fold only folds active rows. References name original identities. Group coverage is not a body read.";

/// SHA-256 of compact, sorted-key UTF-8 JSON. After serde_json normalizes token
/// spelling, numbers are not coerced through a floating-point type. See the
/// Python golden checker.
pub fn digest(value: &J) -> Result<String> {
    Ok(sha256(&serde_json::to_vec(value)?))
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub source_id: String,
    pub body_sha256: String,
    pub body: J,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub source_id: String,
    pub body_sha256: String,
    pub attributes: J,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MetadataChanges {
    pub set: BTreeMap<String, J>,
    pub unset: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FocusChanges {
    pub upsert: Vec<Node>,
    /// Removal from the selected target, never deletion of received evidence.
    pub fold: Vec<String>,
    /// Exact target row order, using original IDs, including retained rows.
    pub order: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Delta {
    pub schema: String,
    pub revision: String,
    pub scope: String,
    pub project_sha256: String,
    pub base_sha256: String,
    pub target_sha256: String,
    pub base_evidence_sha256: String,
    pub target_evidence_sha256: String,
    pub evidence_additions: Vec<Evidence>,
    pub metadata: MetadataChanges,
    pub focus: FocusChanges,
}

#[derive(Clone, Debug, PartialEq)]
struct Projection {
    metadata: BTreeMap<String, J>,
    nodes: BTreeMap<String, Node>,
    order: Vec<String>,
}

/// Decoded selected view and additive bodies. The fields are private so callers
/// cannot assert a materialization without validating its contents.
#[derive(Clone, Debug)]
pub struct MaterializedView {
    packet: J,
    projection: Projection,
    evidence: BTreeMap<String, Evidence>,
}

impl MaterializedView {
    pub fn from_full(packet: &J) -> Result<Self> {
        let (projection, evidence) = project(packet)?;
        Ok(Self {
            packet: packet.clone(),
            projection,
            evidence,
        })
    }
    pub fn packet(&self) -> &J {
        &self.packet
    }
    pub fn sha256(&self) -> Result<String> {
        digest(&self.packet)
    }
    pub fn evidence_sha256(&self) -> Result<String> {
        digest(&serde_json::to_value(&self.evidence)?)
    }
    pub fn received(&self) -> &BTreeMap<String, Evidence> {
        &self.evidence
    }
    /// Resolve only an original ID. A dictionary-only member is not received.
    pub fn resolve(&self, source_id: &str) -> Option<&Evidence> {
        self.evidence.get(source_id)
    }
}

fn text(value: &J) -> Result<&str> {
    value
        .as_str()
        .ok_or_else(|| Error("delta: expected string".into()))
}
fn object(value: &J) -> Result<&Map<String, J>> {
    value
        .as_object()
        .ok_or_else(|| Error("delta: expected object".into()))
}
fn array(value: &J) -> Result<&Vec<J>> {
    value
        .as_array()
        .ok_or_else(|| Error("delta: expected array".into()))
}
fn row(value: &J, length: usize) -> Result<()> {
    require(array(value)?.len() == length, "delta: wrong row arity")
}
fn count(value: &J) -> Result<()> {
    require(value.as_u64().is_some(), "delta: invalid row count")
}
fn hash(value: &str) -> Result<()> {
    require(
        value.len() == 64
            && value
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
        "delta: invalid SHA-256",
    )
}
fn unique_sorted<'a>(values: impl IntoIterator<Item = &'a str>) -> Result<()> {
    let mut last = None;
    for value in values {
        require(
            last.is_none_or(|previous| previous < value),
            "delta: duplicate or noncanonical order",
        )?;
        last = Some(value);
    }
    Ok(())
}

// Alias slots describe only the target encoding. Cross-read references below
// carry the complete original dictionary identity, never a recycled alias.
fn slot(alias: &str, families: &[&str]) -> Result<J> {
    let (family, number) = alias
        .split_at_checked(1)
        .ok_or_else(|| Error("delta: empty alias".into()))?;
    let index = number
        .parse::<u64>()
        .map_err(|_| Error("delta: invalid alias".into()))?;
    require(
        families.contains(&family) && alias == format!("{family}{index}"),
        "delta: unsupported alias",
    )?;
    Ok(json!([family, index]))
}
fn alias(value: &J, families: &[&str]) -> Result<String> {
    row(value, 2)?;
    let family = text(&value[0])?;
    let index = value[1]
        .as_u64()
        .ok_or_else(|| Error("delta: invalid slot".into()))?;
    require(families.contains(&family), "delta: unsupported slot")?;
    Ok(format!("{family}{index}"))
}

/// Translate only structural reference positions. Source bodies, literal
/// attributes, exact original edges and unknown metadata are never searched
/// and replaced: a source may legitimately contain the text "n3".
fn translate(packet: &mut J, dictionary: &Map<String, J>, forward: bool) -> Result<()> {
    object(packet)?;
    for name in ["nodes", "groups", "links", "expanded_edges"] {
        array(&packet[name])?;
    }
    object(&packet["navigation_facets"])?;
    for name in ["nodes", "groups"] {
        array(&packet["navigation_facets"][name])?;
    }
    let mut reverse = BTreeMap::new();
    if !forward {
        for (alias, identity) in dictionary {
            require(
                reverse
                    .insert(serde_json::to_string(identity)?, alias)
                    .is_none(),
                "delta: ambiguous original identity",
            )?;
        }
    }
    let original = |value: &J| -> Result<String> {
        let identity = dictionary
            .get(text(value)?)
            .ok_or_else(|| Error("delta: unresolved alias".into()))?;
        Ok(text(&identity["original"])?.to_owned())
    };
    let reference = |value: &mut J, kind: Option<&str>| -> Result<()> {
        if forward {
            let entry = dictionary
                .get(text(value)?)
                .ok_or_else(|| Error("delta: unresolved alias".into()))?;
            require(
                kind.is_none_or(|k| entry["kind"] == k),
                "delta: wrong reference kind",
            )?;
            *value = entry.clone();
        } else {
            require(
                kind.is_none_or(|k| value["kind"] == k),
                "delta: wrong reference kind",
            )?;
            let alias = reverse
                .get(&serde_json::to_string(value)?)
                .ok_or_else(|| Error("delta: unresolved original identity".into()))?;
            *value = json!(alias);
        }
        Ok(())
    };
    let pool = |value: &mut J| -> Result<()> {
        *value = if forward {
            slot(text(value)?, &["a"])?
        } else {
            json!(alias(value, &["a"])?)
        };
        Ok(())
    };
    let view_id = text(&packet["view_id"])?.to_owned();
    let edge = |value: &mut J, qualified: bool| -> Result<()> {
        if forward {
            let raw = text(value)?;
            let raw = if qualified {
                raw.strip_prefix(&format!("edgeset:{view_id}:"))
                    .ok_or_else(|| Error("delta: stale edge handle".into()))?
            } else {
                raw
            };
            *value = slot(raw, &["e"])?[1].clone();
        } else {
            let index = value
                .as_u64()
                .ok_or_else(|| Error("delta: invalid edge ordinal".into()))?;
            *value = json!(if qualified {
                format!("edgeset:{view_id}:e{index}")
            } else {
                format!("e{index}")
            });
        }
        Ok(())
    };
    // Translate manifest first so each node can distinguish generated pooling
    // from an unrelated source attribute with the same reserved-looking name.
    let mut pooled = BTreeSet::new();
    if let Some(manifest) = packet.get_mut("attribute_compaction") {
        let manifest_object = manifest
            .as_object_mut()
            .ok_or_else(|| Error("delta: invalid compaction manifest".into()))?;
        let rules = manifest_object
            .get("nodes")
            .ok_or_else(|| Error("delta: missing compaction rules".into()))?;
        let mut translated = Vec::new();
        let mut restored = Map::new();
        let entries = if forward {
            object(rules)?
                .iter()
                .map(|(key, value)| json!([key, value]))
                .collect::<Vec<_>>()
        } else {
            array(rules)?.clone()
        };
        for pair in entries {
            row(&pair, 2)?;
            let mut key = pair[0].clone();
            let mut rule = pair[1].clone();
            object(&rule)?;
            if let Some(status) = rule.get_mut("status") {
                pool(status)?;
                pooled.insert(if forward {
                    original(&key)?
                } else {
                    text(&key["original"])?.to_owned()
                });
            }
            reference(&mut key, Some("node"))?;
            if forward {
                translated.push(json!([key, rule]));
            } else {
                require(
                    restored.insert(text(&key)?.into(), rule).is_none(),
                    "delta: duplicate compaction rule",
                )?;
            }
        }
        manifest_object.insert(
            "nodes".into(),
            if forward {
                json!(translated)
            } else {
                J::Object(restored)
            },
        );
        for name in [
            "compacted_nodes",
            "status_pool_nodes",
            "status_text_nodes",
            "value_text_nodes",
        ] {
            let values = manifest_object
                .get_mut(name)
                .and_then(J::as_array_mut)
                .ok_or_else(|| Error("delta: missing compaction list".into()))?;
            for value in values {
                reference(value, Some("node"))?;
            }
        }
    }
    for value in packet["nodes"]
        .as_array_mut()
        .ok_or_else(|| Error("delta: missing nodes".into()))?
    {
        row(value, 3)?;
        let source_id = if forward {
            original(&value[0])?
        } else {
            text(&value[0]["original"])?.to_owned()
        };
        if pooled.contains(&source_id) {
            let status = value[2]
                .get_mut("status_ref")
                .ok_or_else(|| Error("delta: missing pooled assessment".into()))?;
            pool(status)?;
        }
        reference(&mut value[0], Some("node"))?;
    }
    for value in packet["groups"]
        .as_array_mut()
        .ok_or_else(|| Error("delta: missing groups".into()))?
    {
        row(value, 4)?;
        count(&value[1])?;
        reference(&mut value[0], Some("group"))?;
    }
    for value in packet["links"]
        .as_array_mut()
        .ok_or_else(|| Error("delta: missing links".into()))?
    {
        row(value, 5)?;
        count(&value[3])?;
        reference(&mut value[0], None)?;
        reference(&mut value[2], None)?;
        edge(&mut value[4], false)?;
    }
    for value in packet["expanded_edges"]
        .as_array_mut()
        .ok_or_else(|| Error("delta: missing expanded edges".into()))?
    {
        row(value, 2)?;
        for original_edge in array(&value[1])? {
            object(original_edge)?;
        }
        edge(&mut value[0], true)?;
    }
    for (section, length) in [("groups", 2), ("nodes", 2)] {
        let rows = packet["navigation_facets"][section]
            .as_array_mut()
            .ok_or_else(|| Error("delta: missing navigation facet".into()))?;
        for value in rows {
            row(value, length)?;
            if section == "groups" {
                count(&value[1])?;
            }
            reference(
                &mut value[0],
                Some(if section == "groups" { "group" } else { "node" }),
            )?;
            if section == "nodes" {
                for group in value[1]
                    .as_array_mut()
                    .ok_or_else(|| Error("delta: invalid navigation groups".into()))?
                {
                    reference(group, Some("group"))?;
                }
            }
        }
    }
    // A preexisting source field with this name prevents canonical assessment
    // pooling. Without manifest references it is literal metadata, of any type.
    if !pooled.is_empty()
        && let Some(assessments) = packet.get_mut("shared_assessments")
    {
        *assessments = if forward {
            json!(
                object(assessments)?
                    .iter()
                    .map(|(key, value)| Ok(json!([slot(key, &["a"])?[1], value])))
                    .collect::<Result<Vec<_>>>()?
            )
        } else {
            let mut result = Map::new();
            for pair in array(assessments)? {
                row(pair, 2)?;
                let name = alias(&json!(["a", pair[0]]), &["a"])?;
                require(
                    result.insert(name, pair[1].clone()).is_none(),
                    "delta: duplicate assessment slot",
                )?;
            }
            J::Object(result)
        };
    }
    Ok(())
}

fn project(packet: &J) -> Result<(Projection, BTreeMap<String, Evidence>)> {
    require(
        packet["schema"] == crate::canonical_view::COMPACT_SCHEMA,
        "delta: requires canonical V3",
    )?;
    for key in [
        "revision",
        "scope",
        "project",
        "mode",
        "rules",
        "edge_set_handle_template",
        "group_expansion_route",
    ] {
        text(&packet[key])?;
    }
    for key in ["fields", "coverage", "navigation_facets", "descriptions"] {
        object(&packet[key])?;
    }
    hash(text(&packet["view_id"])?)?;
    TypedValue::from_tagged(&packet["project_identity"])?;
    let dictionary = object(&packet["dictionary"])?;
    let mut identities = BTreeSet::new();
    let mut slots = Vec::new();
    for (key, entry) in dictionary {
        let kind = text(&entry["kind"])?;
        require(
            ["node", "group", "external"].contains(&kind),
            "delta: invalid identity kind",
        )?;
        if kind == "external" {
            require(
                entry["original_present"].is_boolean() && entry.get("original").is_some(),
                "delta: invalid external identity",
            )?;
        } else {
            text(&entry["original"])?;
        }
        require(
            identities.insert(serde_json::to_string(&json!([
                kind,
                entry.get("original_present"),
                entry["original"]
            ]))?),
            "delta: ambiguous dictionary identity",
        )?;
        slots.push(json!({"slot":slot(key,&["n","g","f","m","x"])?,"identity":entry}));
    }
    for value in array(&packet["nodes"])? {
        row(value, 3)?;
        TypedValue::from_tagged(&value[1])?;
        crate::view_attributes::restore_attributes(packet, value)?;
    }
    let mut normalized = packet.clone();
    translate(&mut normalized, dictionary, true)?;
    normalized["dictionary"] = json!(slots);
    let mut nodes = BTreeMap::new();
    let mut evidence = BTreeMap::new();
    let mut order = Vec::new();
    for value in array(&normalized["nodes"])? {
        let id = text(&value[0]["original"])?.to_owned();
        object(&value[2])?;
        let body_sha256 = digest(&value[1])?;
        let node = Node {
            source_id: id.clone(),
            body_sha256: body_sha256.clone(),
            attributes: value[2].clone(),
        };
        require(
            nodes.insert(id.clone(), node).is_none(),
            "delta: duplicate source row",
        )?;
        evidence.insert(
            id.clone(),
            Evidence {
                source_id: id.clone(),
                body_sha256,
                body: value[1].clone(),
            },
        );
        order.push(id);
    }
    let mut metadata = object(&normalized)?.clone();
    metadata.remove("nodes");
    Ok((
        Projection {
            metadata: metadata.into_iter().collect(),
            nodes,
            order,
        },
        evidence,
    ))
}

fn restore(projection: &Projection, evidence: &BTreeMap<String, Evidence>) -> Result<J> {
    let mut packet = json!(projection.metadata);
    let mut dictionary = Map::new();
    for binding in array(&packet["dictionary"])? {
        require(
            object(binding)?.len() == 2,
            "delta: invalid dictionary binding",
        )?;
        let name = alias(&binding["slot"], &["n", "g", "f", "m", "x"])?;
        require(
            dictionary
                .insert(name, binding["identity"].clone())
                .is_none(),
            "delta: duplicate dictionary slot",
        )?;
    }
    let mut seen = BTreeSet::new();
    let mut rows = Vec::new();
    let mut node_identities = BTreeMap::new();
    for identity in dictionary.values().filter(|value| value["kind"] == "node") {
        require(
            node_identities
                .insert(text(&identity["original"])?, identity)
                .is_none(),
            "delta: ambiguous source identity",
        )?;
    }
    for id in &projection.order {
        require(seen.insert(id), "delta: duplicate focus order")?;
        let node = projection
            .nodes
            .get(id)
            .ok_or_else(|| Error("delta: missing selected row".into()))?;
        let body = evidence
            .get(id)
            .ok_or_else(|| Error("delta: missing received body".into()))?;
        require(
            body.body_sha256 == node.body_sha256,
            "delta: body reference mismatch",
        )?;
        let identity = node_identities
            .get(id.as_str())
            .ok_or_else(|| Error("delta: selected identity missing from dictionary".into()))?;
        rows.push(json!([identity, body.body, node.attributes]));
    }
    require(
        seen.len() == projection.nodes.len(),
        "delta: incomplete focus order",
    )?;
    packet["nodes"] = json!(rows);
    translate(&mut packet, &dictionary, false)?;
    packet["dictionary"] = J::Object(dictionary);
    // The inverse must itself be a supported canonical view, including its
    // typed bodies and attribute compaction. Unsupported forms fail closed.
    let (canonical, _) = project(&packet)?;
    require(canonical == *projection, "delta: noncanonical projection")?;
    Ok(packet)
}

fn same_snapshot(base: &J, target: &J) -> Result<()> {
    require(
        base["revision"] == target["revision"],
        "delta: revision mismatch; full checkpoint required",
    )?;
    require(
        base["scope"] == target["scope"],
        "delta: scope mismatch; full checkpoint required",
    )?;
    require(
        base["project"] == target["project"]
            && base["project_identity"] == target["project_identity"],
        "delta: project mismatch; full checkpoint required",
    )
}

/// Compare a usable materialization with the independently selected full target.
/// No selector budget is reinvested. A body cannot change within one revision.
pub fn between(base: &MaterializedView, target: &J) -> Result<Delta> {
    same_snapshot(&base.packet, target)?;
    let (projection, bodies) = project(target)?;
    let mut received = base.evidence.clone();
    let mut additions = Vec::new();
    for (id, evidence) in bodies {
        if let Some(old) = received.get(&id) {
            require(
                old == &evidence,
                "delta: source body changed within revision",
            )?;
        } else {
            additions.push(evidence.clone());
            received.insert(id, evidence);
        }
    }
    let metadata = MetadataChanges {
        set: projection
            .metadata
            .iter()
            .filter(|(key, value)| base.projection.metadata.get(*key) != Some(value))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        unset: base
            .projection
            .metadata
            .keys()
            .filter(|key| !projection.metadata.contains_key(*key))
            .cloned()
            .collect(),
    };
    let focus = FocusChanges {
        upsert: projection
            .nodes
            .iter()
            .filter(|(id, node)| base.projection.nodes.get(*id) != Some(node))
            .map(|(_, node)| node.clone())
            .collect(),
        fold: base
            .projection
            .nodes
            .keys()
            .filter(|id| !projection.nodes.contains_key(*id))
            .cloned()
            .collect(),
        order: projection.order.clone(),
    };
    let delta = Delta {
        schema: SCHEMA.into(),
        revision: text(&target["revision"])?.into(),
        scope: text(&target["scope"])?.into(),
        project_sha256: digest(&json!([target["project"], target["project_identity"]]))?,
        base_sha256: base.sha256()?,
        target_sha256: digest(target)?,
        base_evidence_sha256: base.evidence_sha256()?,
        target_evidence_sha256: digest(&json!(received))?,
        evidence_additions: additions,
        metadata,
        focus,
    };
    require(
        apply(base, &delta)?.packet == *target,
        "delta: producer failed exact target reconstruction",
    )?;
    Ok(delta)
}

/// Transactional apply: all checks finish before a new materialization returns.
/// Receipt replay/sibling-head checks belong to the caller; a pure no-op delta
/// can legitimately have the same base/target content digest.
pub fn apply(base: &MaterializedView, delta: &Delta) -> Result<MaterializedView> {
    require(delta.schema == SCHEMA, "delta: unsupported schema")?;
    for value in [
        &delta.project_sha256,
        &delta.base_sha256,
        &delta.target_sha256,
        &delta.base_evidence_sha256,
        &delta.target_evidence_sha256,
    ] {
        hash(value)?;
    }
    require(
        base.packet["revision"] == delta.revision && base.packet["scope"] == delta.scope,
        "delta: revision/scope mismatch",
    )?;
    require(
        digest(&json!([
            base.packet["project"],
            base.packet["project_identity"]
        ]))? == delta.project_sha256,
        "delta: project mismatch",
    )?;
    require(
        base.sha256()? == delta.base_sha256
            && base.evidence_sha256()? == delta.base_evidence_sha256,
        "delta: base mismatch",
    )?;
    unique_sorted(
        delta
            .evidence_additions
            .iter()
            .map(|e| e.source_id.as_str()),
    )?;
    unique_sorted(delta.focus.upsert.iter().map(|e| e.source_id.as_str()))?;
    unique_sorted(delta.focus.fold.iter().map(String::as_str))?;
    unique_sorted(delta.metadata.unset.iter().map(String::as_str))?;
    let upserts = delta
        .focus
        .upsert
        .iter()
        .map(|node| (node.source_id.as_str(), node.body_sha256.as_str()))
        .collect::<BTreeMap<_, _>>();
    let mut received = base.evidence.clone();
    for evidence in &delta.evidence_additions {
        TypedValue::from_tagged(&evidence.body)?;
        require(
            digest(&evidence.body)? == evidence.body_sha256,
            "delta: evidence digest mismatch",
        )?;
        require(
            received
                .insert(evidence.source_id.clone(), evidence.clone())
                .is_none(),
            "delta: evidence is additive",
        )?;
        require(
            upserts.get(evidence.source_id.as_str()) == Some(&evidence.body_sha256.as_str()),
            "delta: unselected evidence addition",
        )?;
    }
    let mut projection = base.projection.clone();
    for key in &delta.metadata.unset {
        require(
            !delta.metadata.set.contains_key(key) && projection.metadata.remove(key).is_some(),
            "delta: invalid metadata removal",
        )?;
    }
    for (key, value) in &delta.metadata.set {
        require(key != "nodes", "delta: nodes belong to focus")?;
        projection.metadata.insert(key.clone(), value.clone());
    }
    for id in &delta.focus.fold {
        require(
            !upserts.contains_key(id.as_str()) && projection.nodes.remove(id).is_some(),
            "delta: invalid focus fold",
        )?;
    }
    for node in &delta.focus.upsert {
        hash(&node.body_sha256)?;
        object(&node.attributes)?;
        projection
            .nodes
            .insert(node.source_id.clone(), node.clone());
    }
    projection.order = delta.focus.order.clone();
    let packet = restore(&projection, &received)?;
    same_snapshot(&base.packet, &packet)?;
    require(
        digest(&packet)? == delta.target_sha256,
        "delta: target digest mismatch",
    )?;
    require(
        digest(&json!(received))? == delta.target_evidence_sha256,
        "delta: evidence state digest mismatch",
    )?;
    Ok(MaterializedView {
        packet,
        projection,
        evidence: received,
    })
}

pub fn render(delta: &Delta) -> Result<String> {
    let payload = serde_json::to_string(delta)?;
    bounded_json(&payload)?;
    Ok(format!("{HEADER}\n{RULE}\n{payload}\n"))
}
// Typed maps/lists use two JSON array levels per value level. Reserve room for
// the envelope without admitting unbounded untrusted recursion.
pub const MAX_JSON_DEPTH: usize = 2 * crate::value::MAX_DEPTH + 32;
fn bounded_json(payload: &str) -> Result<J> {
    Ok(crate::json_ingress::parse_slice_bounded(
        payload.as_bytes(),
        crate::json_ingress::DuplicateKeys::Reject,
        MAX_JSON_DEPTH,
    )?)
}
pub fn decode(wire: &str) -> Result<Delta> {
    let prefix = format!("{HEADER}\n{RULE}\n");
    let payload = wire
        .strip_prefix(&prefix)
        .ok_or_else(|| Error("delta: invalid wire header".into()))?;
    let delta: Delta = serde_json::from_value(bounded_json(payload)?)?;
    // A single canonical encoding rejects duplicate JSON keys, truncated data,
    // reordered arrays/fields and ambiguous numeric re-encodings at receipt.
    require(
        render(&delta)? == wire,
        "delta: noncanonical or incomplete wire",
    )?;
    Ok(delta)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FullReason {
    MissingBase,
    IncompatibleBase,
    DeltaNotSmaller,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeliveryKind {
    Delta,
    Full(FullReason),
}
#[derive(Clone, Debug)]
pub struct Delivery {
    pub kind: DeliveryKind,
    /// Exactly the model-visible bytes, including the supplied host wrapper.
    pub wire: String,
    pub wire_sha256: String,
    pub target_sha256: String,
    pub full_bytes: usize,
    pub delta_bytes: Option<usize>,
}

/// Choose after encoding BOTH complete candidates with the SAME wrapper. The
/// wrapper must be a pure host serializer, include all model-visible metadata,
/// and identify `full_checkpoint`/`delta` explicitly. No hidden full target is
/// sent with a selected delta. A caller lacking retention proof passes None.
pub fn choose<F>(
    base: Option<&MaterializedView>,
    target: &J,
    format: crate::view_format::ViewFormat,
    wrap: F,
) -> Result<Delivery>
where
    F: Fn(&str, &str) -> Result<String>,
{
    MaterializedView::from_full(target)?;
    let full = wrap(
        "full_checkpoint",
        &crate::view_format::render(target, format)?,
    )?;
    let full_bytes = full.len();
    let (kind, wire, delta_bytes) = if let Some(base) = base {
        if same_snapshot(&base.packet, target).is_err() {
            (DeliveryKind::Full(FullReason::IncompatibleBase), full, None)
        } else {
            let delta = wrap("delta", &render(&between(base, target)?)?)?;
            let length = delta.len();
            if length < full_bytes {
                (DeliveryKind::Delta, delta, Some(length))
            } else {
                (
                    DeliveryKind::Full(FullReason::DeltaNotSmaller),
                    full,
                    Some(length),
                )
            }
        }
    } else {
        (DeliveryKind::Full(FullReason::MissingBase), full, None)
    };
    Ok(Delivery {
        kind,
        wire_sha256: sha256(wire.as_bytes()),
        wire,
        target_sha256: digest(target)?,
        full_bytes,
        delta_bytes,
    })
}
