//! Lossless display compaction for attributes in experimental v2 row packets.
use crate::{Error, Result, reasoning_projection, value::TypedValue as V};
use serde_json::{Map, Value as J, json};
use std::collections::BTreeMap;

pub const ENCODING_SCHEMA: &str = "kpopper.attribute-compaction/v1";
pub const PROJECTED_STATUS_RENDERER: &str = "reasoning-projection/render_projected_status/v1";
pub const VALUE_RENDERER: &str = "reasoning-projection/render_value/v1";
const V2_SCHEMA: &str = "kpopper.canonical-graph-view/v2";
const POOL_MIN_STATUS_BYTES: usize = 128;
const ALIAS_MIN_BYTES: usize = 24;

fn attrs_mut(row: &mut J) -> Result<&mut Map<String, J>> {
    row.get_mut(2)
        .and_then(J::as_object_mut)
        .ok_or_else(|| Error("invalid compact node attributes".into()))
}

fn attrs(row: &J) -> Result<&Map<String, J>> {
    row.get(2)
        .and_then(J::as_object)
        .ok_or_else(|| Error("invalid compact node attributes".into()))
}

fn typed_body(row: &J) -> Result<V> {
    let body = row
        .get(1)
        .ok_or_else(|| Error("missing compact node body".into()))?;
    V::from_tagged(body)
}

fn wrapped(kind: &str, value: V) -> V {
    V::Map(BTreeMap::from([
        ("type".into(), V::Text(kind.into())),
        ("value".into(), value),
    ]))
}

fn to_reasoning_value(value: &V) -> Result<V> {
    if reasoning_projection::render_value(value).is_ok() {
        return Ok(value.clone());
    }
    Ok(match value {
        V::Null => V::Map(BTreeMap::from([("type".into(), V::Text("null".into()))])),
        V::Bool(value) => wrapped("boolean", V::Bool(*value)),
        V::Integer(value) => V::Map(BTreeMap::from([
            ("denominator".into(), V::Text("1".into())),
            ("numerator".into(), V::Text(value.as_str().into())),
            ("type".into(), V::Text("number".into())),
        ])),
        V::Text(value) => wrapped("text", V::Text(value.clone())),
        V::Date(value) => wrapped("text", V::Text(value.as_str().into())),
        V::DateTime(value) => wrapped("text", V::Text(value.as_str().into())),
        V::Float(_) => {
            return Err(Error(
                "float body field has no exact display adapter".into(),
            ));
        }
        V::List(values) => V::Map(BTreeMap::from([
            (
                "items".into(),
                V::List(
                    values
                        .iter()
                        .map(to_reasoning_value)
                        .collect::<Result<Vec<_>>>()?,
                ),
            ),
            ("type".into(), V::Text("list".into())),
        ])),
        V::Map(values) => V::Map(BTreeMap::from([
            (
                "fields".into(),
                V::Map(
                    values
                        .iter()
                        .map(|(key, value)| Ok((key.clone(), to_reasoning_value(value)?)))
                        .collect::<Result<BTreeMap<_, _>>>()?,
                ),
            ),
            ("type".into(), V::Text("record".into())),
        ])),
    })
}

fn render_body_value(value: &V) -> Result<String> {
    reasoning_projection::render_value(&to_reasoning_value(value)?)
}

fn node_ref(row: &J) -> Result<&str> {
    row.get(0)
        .and_then(J::as_str)
        .ok_or_else(|| Error("invalid compact node reference".into()))
}

fn attributes_encoding_rules() -> J {
    json!({
        "schema":ENCODING_SCHEMA,
        "manifest":"attribute_compaction.nodes[ref]",
        "status_pool":{"field":"status_ref","dictionary":"shared_assessments","exact_json":true},
        "status_text":{
            "renderer":PROJECTED_STATUS_RENDERER,
            "modes":{"status":"render_projected_status","uncertainty":"exact_array_alias"},
            "derived":true,
            "display_only":true
        },
        "value_text":{
            "reference":"attributes.status.computation.value_text_ref",
            "renderer":VALUE_RENDERER,
            "body_field_candidates":["v","quoted","value"],
            "display_only":true
        },
        "node_rule_values":{"status":"shared assessment alias","status_text":"status_text mode","value_text":"body field key"},
        "restore":"view_attributes::restore_attributes"
    })
}

fn projected_status_text(attributes: &Map<String, J>) -> Option<String> {
    let status = attributes.get("status")?;
    let value = V::from_json(status).ok()?;
    reasoning_projection::render_projected_status(&value).ok()
}

fn compact_row_displays(row: &mut J, rules: &mut Map<String, J>) -> Result<()> {
    let reference = node_ref(row)?.to_owned();
    let body = typed_body(row)?;
    let body_fields = match body {
        V::Map(fields) => fields,
        _ => BTreeMap::new(),
    };
    let attributes = attrs_mut(row)?;
    let mut rule = Map::new();

    if let Some(status_text) = attributes.get("status_text").cloned() {
        if let Some(text) = status_text.as_str() {
            if let Some(rendered) = projected_status_text(attributes)
                && rendered == text
            {
                attributes.remove("status_text");
                rule.insert("status_text".into(), json!("status"));
            }
        } else if status_text.is_array()
            && attributes
                .get("uncertainty")
                .is_some_and(|uncertainty| uncertainty.is_array() && uncertainty == &status_text)
            && serde_json::to_vec(&status_text)?.len() >= ALIAS_MIN_BYTES
        {
            attributes.remove("status_text");
            rule.insert("status_text".into(), json!("uncertainty"));
        }
    }

    if let Some(status) = attributes.get_mut("status")
        && let Some(computation) = status
            .as_object_mut()
            .and_then(|status| status.get_mut("computation"))
            .and_then(J::as_object_mut)
        && let Some(value_text) = computation.get("value_text").and_then(J::as_str)
        && !computation.contains_key("value_text_ref")
    {
        for field in ["v", "quoted", "value"] {
            let Some(value) = body_fields.get(field) else {
                continue;
            };
            let Ok(rendered) = render_body_value(value) else {
                continue;
            };
            if rendered == value_text {
                computation.remove("value_text");
                computation.insert("value_text_ref".into(), json!(field));
                rule.insert("value_text".into(), json!(field));
                break;
            }
        }
    }

    if !rule.is_empty() {
        rules.insert(reference, J::Object(rule));
    }
    Ok(())
}

fn normalized_status_groups(packet: &J) -> Result<BTreeMap<String, Vec<String>>> {
    let mut groups = BTreeMap::<String, Vec<String>>::new();
    for row in packet["nodes"]
        .as_array()
        .ok_or_else(|| Error("invalid compact node rows".into()))?
    {
        let reference = node_ref(row)?.to_owned();
        let attributes = attrs(row)?;
        if attributes.contains_key("status_ref") {
            continue;
        }
        let Some(status) = attributes.get("status").filter(|status| status.is_object()) else {
            continue;
        };
        let serialized = serde_json::to_string(status)?;
        groups.entry(serialized).or_default().push(reference);
    }
    Ok(groups)
}

fn pool_is_worthwhile(status: &J, alias: &str, row_count: usize) -> Result<bool> {
    if row_count < 2
        || status.is_null()
        || serde_json::to_vec(status)?.len() < POOL_MIN_STATUS_BYTES
    {
        return Ok(false);
    }
    let inline = row_count * serde_json::to_vec(&json!({"status":status}))?.len();
    let pool_entry = J::Object(Map::from_iter([(alias.to_owned(), status.clone())]));
    let pooled = serde_json::to_vec(&pool_entry)?.len()
        + row_count * serde_json::to_vec(&json!({"status_ref":alias}))?.len()
        + row_count * serde_json::to_vec(&json!({"status":alias}))?.len();
    Ok(inline > pooled.saturating_add(16))
}

fn compact_status_pool(packet: &mut J, rules: &mut Map<String, J>) -> Result<()> {
    if packet.get("shared_assessments").is_some() {
        return Ok(());
    }
    let groups = normalized_status_groups(packet)?;
    let mut pool = Map::new();
    let mut plans = Vec::<(String, String, J)>::new();
    for (serialized, refs) in groups {
        let status: J = serde_json::from_str(&serialized)?;
        let alias = format!("a{}", pool.len());
        if !pool_is_worthwhile(&status, &alias, refs.len())? {
            continue;
        }
        pool.insert(alias.clone(), status.clone());
        for reference in refs {
            plans.push((reference, alias.clone(), status.clone()));
        }
    }
    if pool.is_empty() {
        return Ok(());
    }
    for row in packet["nodes"]
        .as_array_mut()
        .ok_or_else(|| Error("invalid compact node rows".into()))?
    {
        let reference = node_ref(row)?.to_owned();
        if let Some((_, alias, _)) = plans
            .iter()
            .find(|(candidate, _, _)| candidate == &reference)
        {
            let attributes = attrs_mut(row)?;
            if attributes.contains_key("status_ref") {
                return Err(Error(
                    "status_ref collision during assessment pooling".into(),
                ));
            }
            attributes.remove("status");
            attributes.insert("status_ref".into(), json!(alias));
            let node_rules = rules.entry(reference).or_insert_with(|| json!({}));
            node_rules["status"] = json!(alias);
        }
    }
    packet["shared_assessments"] = J::Object(pool);
    Ok(())
}

/// Compact losslessly derivable display attributes in a canonical v2 row packet.
/// Mutation is transactional: invalid schemas and reserved-name collisions leave
/// the packet unchanged.
pub fn compact(packet: &mut J) -> Result<()> {
    if packet["schema"] != V2_SCHEMA {
        return Err(Error(
            "attribute compaction requires canonical graph view v2".into(),
        ));
    }
    if packet.get("attribute_compaction").is_some() {
        return Err(Error("attribute compaction manifest already exists".into()));
    }
    let mut working = packet.clone();
    let fields = working
        .get_mut("fields")
        .and_then(J::as_object_mut)
        .ok_or_else(|| Error("invalid canonical row fields".into()))?;
    if fields.get("node") != Some(&json!(["ref", "body", "attributes"])) {
        return Err(Error("unsupported canonical node row schema".into()));
    }
    if fields.contains_key("attribute_compaction") {
        return Err(Error(
            "attribute compaction field schema already exists".into(),
        ));
    }
    let valid_node_refs: std::collections::BTreeSet<String> = working["dictionary"]
        .as_object()
        .ok_or_else(|| Error("invalid compact view dictionary".into()))?
        .iter()
        .filter(|(_, entry)| entry["kind"] == "node")
        .map(|(reference, _)| reference.clone())
        .collect();
    let rows = working
        .get_mut("nodes")
        .and_then(J::as_array_mut)
        .ok_or_else(|| Error("invalid compact node rows".into()))?;
    let mut node_rules = Map::new();
    let mut seen = std::collections::BTreeSet::new();
    for row in rows {
        if !row.is_array() || row.as_array().unwrap().len() != 3 {
            return Err(Error("invalid compact node row arity".into()));
        }
        let reference = node_ref(row)?;
        if !seen.insert(reference.to_owned()) {
            return Err(Error("duplicate compact node reference".into()));
        }
        if !valid_node_refs.contains(reference) {
            return Err(Error(
                "compact node reference is absent from dictionary".into(),
            ));
        }
        attrs(row)?;
        compact_row_displays(row, &mut node_rules)?;
    }
    compact_status_pool(&mut working, &mut node_rules)?;
    let compacted_nodes: Vec<_> = node_rules.keys().cloned().collect();
    let references_with_rule = |key: &str| -> Vec<String> {
        node_rules
            .iter()
            .filter(|(_, rule)| rule.get(key).is_some())
            .map(|(reference, _)| reference.clone())
            .collect()
    };
    working["attribute_compaction"] = json!({
        "schema":ENCODING_SCHEMA,
        "compacted_nodes":compacted_nodes,
        "status_pool_nodes":node_rules.iter().filter(|(_, rule)| rule["status"].is_string()).map(|(reference, _)| reference).collect::<Vec<_>>(),
        "status_text_nodes":references_with_rule("status_text"),
        "value_text_nodes":references_with_rule("value_text"),
        "nodes":node_rules,
    });
    let fields = working["fields"].as_object_mut().unwrap();
    fields.insert("attribute_compaction".into(), attributes_encoding_rules());
    *packet = working;
    Ok(())
}

fn verify_node_reference(packet: &J, row: &J) -> Result<String> {
    let reference = node_ref(row)?.to_owned();
    if packet["dictionary"][&reference]["kind"] != "node" {
        return Err(Error("unknown or non-node compact reference".into()));
    }
    Ok(reference)
}

fn restore_status_pool(packet: &J, attributes: &mut Map<String, J>, rule: &J) -> Result<()> {
    if rule.is_null() {
        return Ok(());
    }
    let alias = rule
        .as_str()
        .ok_or_else(|| Error("invalid assessment reference rule".into()))?;
    if attributes.get("status").is_some()
        || attributes.get("status_ref").and_then(J::as_str) != Some(alias)
    {
        return Err(Error("missing or mismatched status_ref".into()));
    }
    let status = packet["shared_assessments"]
        .get(alias)
        .filter(|value| value.is_object())
        .cloned()
        .ok_or_else(|| Error("unknown shared_assessments reference".into()))?;
    attributes.remove("status_ref");
    attributes.insert("status".into(), status);
    Ok(())
}

fn restore_value_text(
    row: &J,
    attributes: &mut Map<String, J>,
    rule: &J,
    schema: &J,
) -> Result<()> {
    if rule.is_null() {
        return Ok(());
    }
    let field = rule
        .as_str()
        .filter(|field| ["v", "quoted", "value"].contains(field))
        .ok_or_else(|| Error("invalid body-field display reference".into()))?;
    if schema["renderer"] != VALUE_RENDERER
        || schema["reference"] != "attributes.status.computation.value_text_ref"
        || !schema["body_field_candidates"]
            .as_array()
            .is_some_and(|fields| {
                fields
                    .iter()
                    .any(|candidate| candidate.as_str() == Some(field))
            })
    {
        return Err(Error("unsupported body-field display reference".into()));
    }
    let status = attributes
        .get_mut("status")
        .and_then(J::as_object_mut)
        .ok_or_else(|| Error("missing status for value_text restoration".into()))?;
    let computation = status
        .get_mut("computation")
        .and_then(J::as_object_mut)
        .ok_or_else(|| Error("missing computation for value_text restoration".into()))?;
    let reference = computation
        .get("value_text_ref")
        .ok_or_else(|| Error("missing value_text_ref".into()))?;
    if reference.as_str() != Some(field) || computation.contains_key("value_text") {
        return Err(Error("mismatched value_text_ref".into()));
    }
    let body = typed_body(row)?;
    let V::Map(fields) = body else {
        return Err(Error("compact node body must be a typed map".into()));
    };
    let value = fields
        .get(field)
        .ok_or_else(|| Error("value_text body field is missing".into()))?;
    let rendered = render_body_value(value)?;
    computation.remove("value_text_ref");
    computation.insert("value_text".into(), json!(rendered));
    Ok(())
}

fn restore_status_text(attributes: &mut Map<String, J>, rule: &J, schema: &J) -> Result<()> {
    if rule.is_null() {
        return Ok(());
    }
    if attributes.contains_key("status_text") {
        return Err(Error(
            "status_text derivation collides with an existing value".into(),
        ));
    }
    if schema["renderer"] != PROJECTED_STATUS_RENDERER
        || schema["derived"] != true
        || schema["display_only"] != true
    {
        return Err(Error("unsupported status_text rendering schema".into()));
    }
    let restored = match rule.as_str() {
        Some("status") => {
            let status = attributes.get("status").ok_or_else(|| {
                Error("missing structured status for status_text restoration".into())
            })?;
            let status = V::from_json(status)?;
            J::String(reasoning_projection::render_projected_status(&status)?)
        }
        Some("uncertainty") => attributes
            .get("uncertainty")
            .filter(|value| value.is_array())
            .cloned()
            .ok_or_else(|| Error("missing uncertainty array for exact alias".into()))?,
        _ => return Err(Error("unsupported status_text derivation".into())),
    };
    attributes.insert("status_text".into(), restored);
    Ok(())
}

/// Restore a row's exact original attributes. References, types and derivation
/// rules are checked before any value is reconstructed.
pub fn restore_attributes(packet: &J, row: &J) -> Result<J> {
    if ![V2_SCHEMA, "kpopper.canonical-graph-view/v3"]
        .contains(&packet["schema"].as_str().unwrap_or(""))
    {
        return Err(Error("unsupported compact view schema".into()));
    }
    let reference = verify_node_reference(packet, row)?;
    let mut attributes = attrs(row)?.clone();
    let compaction = packet.get("attribute_compaction");
    if compaction.is_none() {
        if packet["fields"].get("attribute_compaction").is_some() {
            return Err(Error("missing attribute_compaction manifest".into()));
        }
        return Ok(J::Object(attributes));
    }
    let compaction = compaction.unwrap();
    if compaction["schema"] != ENCODING_SCHEMA
        || packet["fields"]["attribute_compaction"] != attributes_encoding_rules()
    {
        return Err(Error("unsupported attribute_compaction manifest".into()));
    }
    let rule = compaction["nodes"]
        .get(&reference)
        .cloned()
        .unwrap_or(J::Null);
    let compacted_nodes = compaction["compacted_nodes"]
        .as_array()
        .ok_or_else(|| Error("missing compacted_nodes manifest".into()))?;
    let listed = compacted_nodes
        .iter()
        .any(|value| value.as_str() == Some(&reference));
    if listed && rule.is_null() || !listed && !rule.is_null() {
        return Err(Error("incomplete per-node compaction manifest".into()));
    }
    for (manifest_key, rule_key) in [
        ("status_pool_nodes", "status"),
        ("status_text_nodes", "status_text"),
        ("value_text_nodes", "value_text"),
    ] {
        let nodes = compaction[manifest_key]
            .as_array()
            .ok_or_else(|| Error(format!("missing {manifest_key} manifest")))?;
        let expected = nodes.iter().any(|value| value.as_str() == Some(&reference));
        let actual = !rule[rule_key].is_null();
        if expected != actual {
            return Err(Error(format!("incomplete {rule_key} reference metadata")));
        }
    }
    let schema = &packet["fields"]["attribute_compaction"];
    restore_status_pool(packet, &mut attributes, &rule["status"])?;
    restore_value_text(
        row,
        &mut attributes,
        &rule["value_text"],
        &schema["value_text"],
    )?;
    restore_status_text(
        &mut attributes,
        &rule["status_text"],
        &schema["status_text"],
    )?;
    Ok(J::Object(attributes))
}
