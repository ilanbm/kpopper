//! Compact per-node semantic storage for one ordinary action.
//!
//! Slots retain authored prototypes while `emit` identifies only the objects written by
//! this action. Observation membership is stored separately in the supplied sparse node.
use crate::{
    Result,
    history_contract::{self, Map, error, field, map, schema, string_is, text},
    history_node_observation::ObservationNode,
    history_view::map_mut,
    require,
    value::TypedValue as V,
};

const FORMAT: &str = "node-ledger/v1";
const LEGACY: &str = "node-semantic-object/v1";
const MAX_SLOTS: usize = history_contract::MAX_OBJECTS;

pub(crate) fn is_ledger(state: &V) -> bool {
    map(state)
        .ok()
        .and_then(|p| p.get("context"))
        .and_then(|c| map(c).ok())
        .is_some_and(|c| c.get("format").is_some_and(|f| string_is(f, FORMAT)))
}

/// Reject mutations to retained prototypes outside the slots emitted by this action.
pub(crate) fn validate_transition(previous: Option<&V>, next: &V) -> Result<()> {
    require(is_ledger(next), "node_ledger_transition")?;
    unpack(next)?;
    let new_slots = expanded_slots(next)?;
    let emitted = emit_names(next)?;
    let old_state = previous.map(normalize_ledger).transpose()?;
    let old_slots = old_state
        .as_ref()
        .map(expanded_slots)
        .transpose()?
        .unwrap_or_default();
    if previous.is_none() {
        require(
            new_slots.iter().all(|(name, _, _)| emitted.contains(name)),
            "node_ledger_hidden_slot",
        )?;
        return Ok(());
    }
    let old_by_name = old_slots
        .iter()
        .map(|(name, slot, _)| (name.as_str(), slot))
        .collect::<std::collections::BTreeMap<_, _>>();
    let new_by_name = new_slots
        .iter()
        .map(|(name, slot, _)| (name.as_str(), slot))
        .collect::<std::collections::BTreeMap<_, _>>();
    for (name, slot, _) in &new_slots {
        if !emitted.contains(name) {
            require(
                old_by_name
                    .get(name.as_str())
                    .is_some_and(|old| *old == slot),
                "node_ledger_hidden_slot",
            )?;
        }
    }
    for (name, old_slot, _) in &old_slots {
        if !emitted.contains(name) {
            require(
                new_by_name
                    .get(name.as_str())
                    .is_some_and(|new| *new == old_slot),
                "node_ledger_prototype_removed",
            )?;
        }
    }
    if emitted.is_empty() {
        let old_context = map(field(map(old_state.as_ref().unwrap())?, "context")?)?;
        let new_context = map(field(map(next)?, "context")?)?;
        require(
            old_context["tip"] == new_context["tip"],
            "node_ledger_tip_changed",
        )?;
    }
    if !emitted.contains("claim0") {
        let old = old_state.as_ref().unwrap();
        let old_payload = schema(old, &["collection", "body", "context"], &[])?;
        let new_payload = schema(next, &["collection", "body", "context"], &[])?;
        require(
            old_payload["collection"] == new_payload["collection"]
                && old_payload["body"] == new_payload["body"],
            "node_ledger_primary_changed",
        )?;
    }
    Ok(())
}

/// Deterministic observation parent for the next join, including a prior empty emit.
pub(crate) fn observation_base(state: &V) -> Result<Option<String>> {
    let normalized = if is_ledger(state) {
        state.clone()
    } else {
        pack(Some(state), &[])?
    };
    let slots = unpack_slots(&normalized)?;
    let p = schema(&normalized, &["collection", "body", "context"], &[])?;
    let c = schema(
        &p["context"],
        &["format", "common", "slots", "emit", "tip"],
        &[],
    )?;
    let Some(tip) = c.get("tip").filter(|v| **v != V::Null) else {
        return Ok(None);
    };
    let tip = text(tip)?;
    let (_, slot, _) = slots
        .iter()
        .find(|(name, _, _)| name == tip)
        .ok_or_else(|| error("node_ledger_tip"))?;
    let obs = observation(field(map(&slot)?, "observation")?)?;
    Ok(Some(obs.id))
}

/// Merge a bounded action into its subject-local slot ledger.
pub(crate) fn pack(previous: Option<&V>, objects: &[(V, ObservationNode)]) -> Result<V> {
    let mut collection = V::Null;
    let mut body = V::Null;
    let mut slots = Map::new();
    let mut subject: Option<String> = None;
    let mut tip: Option<String> = None;
    if let Some(previous) = previous {
        if is_ledger(previous) {
            let old = unpack_slots(previous)?;
            let p = schema(previous, &["collection", "body", "context"], &[])?;
            collection = p["collection"].clone();
            body = p["body"].clone();
            let previous_context = map(&p["context"])?;
            tip = previous_context
                .get("tip")
                .filter(|v| **v != V::Null)
                .map(text)
                .transpose()?
                .map(str::to_owned);
            subject = old.first().map(|(_, _, s)| s.clone());
            let common = ledger_common(previous)?;
            for (name, mut slot, _) in old {
                expand_common(&mut slot, &common)?;
                slots.insert(name, slot);
            }
        } else {
            // Lift exactly the one legacy compact object. Unknown envelopes are rejected.
            let legacy = crate::history_node_frame::decode(previous)
                .map(|decoded| decoded.semantic)
                .unwrap_or_else(|_| previous.clone());
            let p = schema(&legacy, &["collection", "body", "context"], &[])?;
            let c = schema(
                &p["context"],
                &["format", "header", "observation"],
                &["source_order"],
            )?;
            require(string_is(&c["format"], LEGACY), "node_ledger_previous")?;
            let obs = observation(&c["observation"])?;
            let mut header = map(&c["header"])?.clone();
            require(text(field(&header, "id")?)? == obs.id, "node_ledger_id")?;
            header.remove("id");
            let subj = text(field(&header, "subject")?)?.to_owned();
            subject = Some(subj);
            let is_act = string_is(field(&header, "kind")?, "act");
            let name = format!("{}0", if is_act { "act" } else { "claim" });
            tip = Some(name.clone());
            let mut slot = Map::from([
                ("header".into(), V::Map(header)),
                (
                    "observation".into(),
                    V::from_json(&serde_json::to_value(obs)?)?,
                ),
            ]);
            if name != "claim0" {
                slot.insert("body".into(), p["body"].clone());
            } else {
                body = p["body"].clone();
                collection = p["collection"].clone();
            }
            slots.insert(name, V::Map(slot));
        }
    }

    require(slots.len() <= MAX_SLOTS, "node_ledger_limit")?;
    let mut emitted = Vec::with_capacity(objects.len());
    let mut action_claim = 0usize;
    let mut action_act = 0usize;
    for (object, observation) in objects {
        history_contract::validate_object(object)?;
        observation.validate()?;
        let om = map(object)?;
        require(text(field(om, "id")?)? == observation.id, "node_ledger_id")?;
        let current_subject = text(field(om, "subject")?)?.to_owned();
        if let Some(expected) = &subject {
            require(expected == &current_subject, "node_ledger_subject")?;
        } else {
            subject = Some(current_subject);
        }
        let is_act = string_is(field(om, "kind")?, "act");
        let prefix = if is_act { "act" } else { "claim" };
        let index = if is_act {
            let index = action_act;
            action_act += 1;
            index
        } else {
            let index = action_claim;
            action_claim += 1;
            index
        };
        let name = format!("{prefix}{index}");
        let mut header = om.clone();
        let object_body = header
            .remove("body")
            .ok_or_else(|| error("invalid_schema"))?;
        header.remove("saw");
        header.remove("id");
        let mut slot = Map::from([
            ("header".into(), V::Map(header)),
            (
                "observation".into(),
                V::from_json(&serde_json::to_value(observation)?)?,
            ),
        ]);
        if name == "claim0" {
            collection = V::Text(text(field(map(field(om, "authored")?)?, "collection")?)?.into());
            body = object_body;
        } else {
            slot.insert("body".into(), object_body);
        }
        slots.insert(name.clone(), V::Map(slot));
        emitted.push(V::Text(name));
        require(slots.len() <= MAX_SLOTS, "node_ledger_limit")?;
    }
    if let Some(V::Text(last)) = emitted.last() {
        tip = Some(last.clone());
    }
    if collection == V::Null && action_claim == 0 {
        collection = V::Text("acts".into());
    }
    compact_slots(&mut slots)?;
    let common = lift_common(&mut slots)?;
    let payload = V::Map(Map::from([
        ("collection".into(), collection),
        ("body".into(), body),
        (
            "context".into(),
            V::Map(Map::from([
                ("format".into(), V::Text(FORMAT.into())),
                ("common".into(), V::Map(common)),
                ("slots".into(), V::Map(slots)),
                ("emit".into(), V::List(emitted)),
                ("tip".into(), tip.map(V::Text).unwrap_or(V::Null)),
            ])),
        ),
    ]));
    // Validate our own output at the boundary used by the reader.
    unpack(&payload)?;
    Ok(payload)
}

/// Return only this action's emitted objects. `saw` is intentionally reconstructed elsewhere.
pub(crate) fn unpack(state: &V) -> Result<Vec<(V, ObservationNode)>> {
    let p = schema(state, &["collection", "body", "context"], &[])?;
    let c = schema(
        &p["context"],
        &["format", "common", "slots", "emit", "tip"],
        &[],
    )?;
    require(string_is(&c["format"], FORMAT), "node_ledger_format")?;
    let slots = map(&c["slots"])?;
    let anchors = slot_anchors(slots)?;
    let common = map(&c["common"])?;
    require(
        common
            .keys()
            .all(|k| ["subject", "by", "on", "op"].contains(&k.as_str())),
        "node_ledger_common",
    )?;
    require(
        slots.is_empty() || common.contains_key("subject"),
        "node_ledger_common",
    )?;
    require(slots.len() <= MAX_SLOTS, "node_ledger_limit")?;
    let mut decoded = Vec::new();
    let mut indices = std::collections::BTreeMap::<&str, std::collections::BTreeSet<usize>>::new();
    for (name, value) in slots {
        let (prefix, index) = parse_slot_name(name)?;
        indices.entry(prefix).or_default().insert(index);
        let s = schema(value, &["header", "observation"], &["body"])?;
        require(
            (prefix == "claim" && index == 0) == !s.contains_key("body"),
            "node_ledger_slot",
        )?;
        let obs = decode_observation(&s["observation"], &anchors)?;
        let header = map(&s["header"])?;
        require(
            !["id", "body", "saw"]
                .iter()
                .any(|k| header.contains_key(*k)),
            "node_ledger_header",
        )?;
        require(
            common.keys().all(|k| !header.contains_key(k)),
            "node_ledger_common",
        )?;
        require(
            string_is(
                field(header, "kind")?,
                if prefix == "act" { "act" } else { "reading" },
            ) || (prefix == "claim" && string_is(field(header, "kind")?, "judgment")),
            "node_ledger_slot",
        )?;
        let mut object = header.clone();
        for (key, value) in common {
            object.insert(key.clone(), value.clone());
        }
        object.insert("id".into(), V::Text(obs.id.clone()));
        let mut object_body = if prefix == "claim" && index == 0 {
            p["body"].clone()
        } else {
            s["body"].clone()
        };
        if prefix == "act" {
            expand_act_body(&mut object_body, &anchors)?;
        }
        object.insert("body".into(), object_body);
        let subj = text(field(&object, "subject")?)?.to_owned();
        require(!subj.is_empty(), "node_ledger_subject")?;
        if prefix == "claim" && index == 0 {
            require(
                field(map(field(&object, "authored")?)?, "collection")? == &p["collection"],
                "node_ledger_collection",
            )?;
        }
        decoded.push((name.clone(), V::Map(object), obs, subj));
    }
    for set in indices.values() {
        require(set.iter().copied().eq(0..set.len()), "node_ledger_slot")?;
    }
    if let Some(first) = decoded.first() {
        require(
            decoded.iter().all(|(_, _, _, s)| s == &first.3),
            "node_ledger_subject",
        )?;
    }
    let emits = match &c["emit"] {
        V::List(v) => v,
        _ => return Err(error("node_ledger_emit")),
    };
    require(emits.len() <= slots.len(), "node_ledger_emit")?;
    let mut seen = std::collections::BTreeSet::new();
    let mut output = Vec::with_capacity(emits.len());
    for item in emits {
        let name = text(item)?;
        require(seen.insert(name), "node_ledger_emit")?;
        let item = decoded
            .iter()
            .find(|(n, _, _, _)| n == name)
            .ok_or_else(|| error("node_ledger_emit"))?;
        output.push((item.1.clone(), item.2.clone()));
    }
    let tip = match &c["tip"] {
        V::Null => None,
        value => Some(text(value)?),
    };
    if let Some(last) = emits.last() {
        require(tip == Some(text(last)?), "node_ledger_tip")?;
    } else if !slots.is_empty() {
        require(
            tip.is_some_and(|name| slots.contains_key(name)),
            "node_ledger_tip",
        )?;
    } else {
        require(tip.is_none(), "node_ledger_tip")?;
    }
    Ok(output)
}

fn unpack_slots(state: &V) -> Result<Vec<(String, V, String)>> {
    let p = schema(state, &["collection", "body", "context"], &[])?;
    let c = schema(
        &p["context"],
        &["format", "common", "slots", "emit", "tip"],
        &[],
    )?;
    require(string_is(&c["format"], FORMAT), "node_ledger_format")?;
    let slots = map(&c["slots"])?;
    let common = map(&c["common"])?;
    require(
        common
            .keys()
            .all(|k| ["subject", "by", "on", "op"].contains(&k.as_str())),
        "node_ledger_common",
    )?;
    require(slots.len() <= MAX_SLOTS, "node_ledger_limit")?;
    let anchors = slot_anchors(slots)?;
    let mut out = Vec::new();
    for (name, value) in slots {
        let (prefix, index) = parse_slot_name(name)?;
        let s = schema(value, &["header", "observation"], &["body"])?;
        require(
            (prefix == "claim" && index == 0) == !s.contains_key("body"),
            "node_ledger_slot",
        )?;
        let obs = decode_observation(&s["observation"], &anchors)?;
        let header = map(&s["header"])?;
        require(
            !["id", "body", "saw"]
                .iter()
                .any(|k| header.contains_key(*k)),
            "node_ledger_header",
        )?;
        require(
            common.keys().all(|k| !header.contains_key(k)),
            "node_ledger_common",
        )?;
        let subj = text(field(common, "subject")?)?.to_owned();
        let mut expanded = value.clone();
        let expanded_map = map_mut(&mut expanded)?;
        expanded_map.insert("observation".into(), observation_value(&obs)?);
        if prefix == "act" {
            let body = expanded_map
                .get_mut("body")
                .ok_or_else(|| error("node_ledger_slot"))?;
            expand_act_body(body, &anchors)?;
        }
        out.push((name.clone(), expanded, subj));
    }
    // Verify emit references and outer slot bindings before carrying state forward.
    unpack(state)?;
    Ok(out)
}

fn normalize_ledger(state: &V) -> Result<V> {
    if is_ledger(state) {
        Ok(state.clone())
    } else {
        pack(Some(state), &[])
    }
}

fn expanded_slots(state: &V) -> Result<Vec<(String, V, String)>> {
    let mut slots = unpack_slots(state)?;
    let common = ledger_common(state)?;
    for (_, slot, _) in &mut slots {
        expand_common(slot, &common)?;
    }
    Ok(slots)
}

fn emit_names(state: &V) -> Result<std::collections::BTreeSet<String>> {
    let p = schema(state, &["collection", "body", "context"], &[])?;
    let c = schema(
        &p["context"],
        &["format", "common", "slots", "emit", "tip"],
        &[],
    )?;
    let values = match &c["emit"] {
        V::List(values) => values,
        _ => return Err(error("node_ledger_emit")),
    };
    values.iter().map(|v| Ok(text(v)?.to_owned())).collect()
}

fn ledger_common(state: &V) -> Result<Map> {
    let p = schema(state, &["collection", "body", "context"], &[])?;
    let c = schema(
        &p["context"],
        &["format", "common", "slots", "emit", "tip"],
        &[],
    )?;
    let common = map(&c["common"])?;
    require(
        common
            .keys()
            .all(|k| ["subject", "by", "on", "op"].contains(&k.as_str())),
        "node_ledger_common",
    )?;
    Ok(common.clone())
}

fn expand_common(slot: &mut V, common: &Map) -> Result<()> {
    let slot = map_mut(slot)?;
    let header = map_mut(
        slot.get_mut("header")
            .ok_or_else(|| error("node_ledger_header"))?,
    )?;
    for (key, value) in common {
        require(!header.contains_key(key), "node_ledger_common")?;
        header.insert(key.clone(), value.clone());
    }
    Ok(())
}

fn lift_common(slots: &mut Map) -> Result<Map> {
    let mut common = Map::new();
    for key in ["subject", "by", "on", "op"] {
        let mut values = slots.values().map(|slot| {
            map(slot)
                .and_then(|s| map(field(s, "header")?))
                .and_then(|h| field(h, key))
        });
        if let Some(Ok(first)) = values.next() {
            if values.all(|v| v.is_ok_and(|value| value == first)) {
                common.insert(key.into(), first.clone());
            }
        }
    }
    if !common.is_empty() {
        for slot in slots.values_mut() {
            let header = map_mut(map_mut(slot)?.get_mut("header").unwrap())?;
            for key in common.keys() {
                header.remove(key);
            }
        }
    }
    Ok(common)
}

/// Alias only the exact IDs in the node's membership transition and act reference fields.
fn compact_slots(slots: &mut Map) -> Result<()> {
    let mut anchors = std::collections::BTreeMap::<String, String>::new();
    for (name, value) in slots.iter() {
        let slot = map(value)?;
        let obs = observation(field(slot, "observation")?)?;
        require(
            anchors.insert(obs.id, name.clone()).is_none(),
            "node_ledger_anchor_duplicate",
        )?;
    }
    for (name, value) in slots.iter_mut() {
        let slot = map_mut(value)?;
        let obs = observation(field(slot, "observation")?)?;
        slot.insert("observation".into(), compact_observation(&obs, &anchors)?);
        let header = map(field(slot, "header")?)?;
        if string_is(field(header, "kind")?, "act") {
            compact_act_body(
                slot.get_mut("body")
                    .ok_or_else(|| error("node_ledger_slot"))?,
                &anchors,
            )?;
        }
        let _ = name;
    }
    Ok(())
}

fn slot_anchors(slots: &Map) -> Result<std::collections::BTreeMap<String, String>> {
    let mut anchors = std::collections::BTreeMap::new();
    for (name, value) in slots {
        parse_slot_name(name)?;
        let slot = schema(value, &["header", "observation"], &["body"])?;
        let compact = schema(&slot["observation"], &["i", "b", "a", "r", "n", "h"], &[])?;
        let id = text(&compact["i"])?;
        require(
            crate::history_paths::object_id(id),
            "node_ledger_observation",
        )?;
        require(
            anchors.insert(id.to_owned(), name.clone()).is_none(),
            "node_ledger_anchor_duplicate",
        )?;
    }
    Ok(anchors)
}

fn compact_observation(
    node: &ObservationNode,
    anchors: &std::collections::BTreeMap<String, String>,
) -> Result<V> {
    node.validate()?;
    let base = node
        .base
        .as_ref()
        .map(|id| alias(id, anchors))
        .transpose()?
        .unwrap_or(V::Null);
    let delta = |ids: &[String]| -> Result<V> {
        Ok(V::List(
            ids.iter()
                .map(|id| {
                    if node.base.as_deref() == Some(id.as_str()) {
                        Ok(V::Text("^".into()))
                    } else {
                        alias(id, anchors)
                    }
                })
                .collect::<Result<Vec<_>>>()?,
        ))
    };
    Ok(V::Map(Map::from([
        ("i".into(), V::Text(node.id.clone())),
        ("b".into(), base),
        ("a".into(), delta(&node.added)?),
        ("r".into(), delta(&node.removed)?),
        (
            "n".into(),
            V::from_json(&serde_json::json!(node.cardinality))?,
        ),
        ("h".into(), V::Text(node.saw_digest.clone())),
    ])))
}

fn decode_observation(
    value: &V,
    anchors: &std::collections::BTreeMap<String, String>,
) -> Result<ObservationNode> {
    let m = schema(value, &["i", "b", "a", "r", "n", "h"], &[])?;
    let id = text(&m["i"])?.to_owned();
    require(
        crate::history_paths::object_id(&id),
        "node_ledger_observation",
    )?;
    let base = match &m["b"] {
        V::Null => None,
        v => Some(resolve_alias(text(v)?, anchors, false)?),
    };
    let delta = |v: &V| -> Result<Vec<String>> {
        let values = match v {
            V::List(values) => values,
            _ => return Err(error("node_ledger_observation")),
        };
        values
            .iter()
            .map(|v| {
                let token = text(v)?;
                if token == "^" {
                    base.clone().ok_or_else(|| error("node_ledger_alias"))
                } else {
                    resolve_alias(token, anchors, false)
                }
            })
            .collect()
    };
    let n = match &m["n"] {
        V::Integer(value) => value
            .as_str()
            .parse()
            .map_err(|_| error("node_ledger_observation"))?,
        _ => return Err(error("node_ledger_observation")),
    };
    let node = ObservationNode {
        format: crate::history_node_observation::FORMAT.into(),
        id,
        base: base.clone(),
        added: delta(&m["a"])?,
        removed: delta(&m["r"])?,
        cardinality: n,
        saw_digest: text(&m["h"])?.into(),
    };
    node.validate()?;
    Ok(node)
}

fn alias(id: &str, anchors: &std::collections::BTreeMap<String, String>) -> Result<V> {
    if let Some(name) = anchors.get(id) {
        Ok(V::Text(format!("@{name}")))
    } else {
        require(crate::history_paths::object_id(id), "node_ledger_alias")?;
        Ok(V::Text(id.into()))
    }
}

fn resolve_alias(
    token: &str,
    anchors: &std::collections::BTreeMap<String, String>,
    caret: bool,
) -> Result<String> {
    if token.starts_with('@') {
        let name = &token[1..];
        parse_slot_name(name)?;
        let id = anchors
            .iter()
            .find_map(|(id, slot)| (slot == name).then_some(id.clone()))
            .ok_or_else(|| error("node_ledger_alias"))?;
        Ok(id)
    } else {
        require(token != "^" || caret, "node_ledger_alias")?;
        require(crate::history_paths::object_id(token), "node_ledger_alias")?;
        require(
            !anchors.contains_key(token),
            "node_ledger_alias_noncanonical",
        )?;
        Ok(token.into())
    }
}

fn compact_act_body(
    body: &mut V,
    anchors: &std::collections::BTreeMap<String, String>,
) -> Result<()> {
    let m = map_mut(body)?;
    if let Some(of) = m.get_mut("of") {
        *of = alias(text(of)?, anchors)?;
    }
    if let Some(over) = m.get_mut("over") {
        let ids = match over {
            V::List(ids) => ids,
            _ => return Err(error("node_ledger_alias")),
        };
        for id in ids {
            *id = alias(text(id)?, anchors)?;
        }
    }
    if let Some(read) = m.get_mut("read") {
        for id in map_mut(read)?.values_mut() {
            *id = alias(text(id)?, anchors)?;
        }
    }
    Ok(())
}

fn expand_act_body(
    body: &mut V,
    anchors: &std::collections::BTreeMap<String, String>,
) -> Result<()> {
    let m = map_mut(body)?;
    if let Some(of) = m.get_mut("of") {
        *of = V::Text(resolve_alias(text(of)?, anchors, false)?);
    }
    if let Some(over) = m.get_mut("over") {
        let ids = match over {
            V::List(ids) => ids,
            _ => return Err(error("node_ledger_alias")),
        };
        for id in ids {
            *id = V::Text(resolve_alias(text(id)?, anchors, false)?);
        }
    }
    if let Some(read) = m.get_mut("read") {
        for id in map_mut(read)?.values_mut() {
            *id = V::Text(resolve_alias(text(id)?, anchors, false)?);
        }
    }
    Ok(())
}

fn observation_value(node: &ObservationNode) -> Result<V> {
    V::from_json(&serde_json::to_value(node)?)
}

fn observation(value: &V) -> Result<ObservationNode> {
    let node: ObservationNode = serde_json::from_value(value.to_json()?)?;
    node.validate()?;
    Ok(node)
}

fn parse_slot_name(name: &str) -> Result<(&str, usize)> {
    let (prefix, digits) = if let Some(n) = name.strip_prefix("claim") {
        ("claim", n)
    } else if let Some(n) = name.strip_prefix("act") {
        ("act", n)
    } else {
        return Err(error("node_ledger_slot"));
    };
    require(
        !digits.is_empty()
            && (digits == "0" || !digits.starts_with('0'))
            && digits.bytes().all(|b| b.is_ascii_digit()),
        "node_ledger_slot",
    )?;
    let index = digits
        .parse::<usize>()
        .map_err(|_| error("node_ledger_slot"))?;
    require(index < MAX_SLOTS, "node_ledger_slot")?;
    Ok((prefix, index))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{history_node_observation::ObservationNode, history_view::map_mut};
    use serde_json::json;
    use std::collections::BTreeSet;

    fn v(value: serde_json::Value) -> V {
        V::from_json(&value).unwrap()
    }

    fn claim(
        collection: &str,
        op: &str,
        by: &str,
        on: &str,
        body_value: usize,
    ) -> (V, ObservationNode) {
        let saw = BTreeSet::from(["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned()]);
        let mut object = v(json!({
            "schema_version":2,"id_scheme":"typed-history/v2","kind":"reading",
            "subject":"p.ledger","by":by,"on":on,"op":op,"body":{"value":body_value},
            "saw":saw.iter().cloned().collect::<Vec<_>>(),"pins":{},"pin_gaps":{},
            "authored":{"collection":collection,"profile":"ordinary-reader/v1","fields":{"value":"value"}}
        }));
        let id = crate::identity::typed_object_identity(&object).unwrap();
        map_mut(&mut object)
            .unwrap()
            .insert("id".into(), V::Text(id.clone()));
        history_contract::validate_object(&object).unwrap();
        let observation = ObservationNode::root(&id, &saw).unwrap();
        (object, observation)
    }

    fn act(op: &str, by: &str, on: &str, of: &str) -> (V, ObservationNode) {
        let saw = vec![of.to_owned()];
        let mut object = v(json!({
            "schema_version":2,"id_scheme":"typed-history/v2","kind":"act",
            "subject":"p.ledger","by":by,"on":on,"op":op,
            "body":{"act":"accept","of":of,"over":[],"because":"because"},"saw":saw
        }));
        let id = crate::identity::typed_object_identity(&object).unwrap();
        map_mut(&mut object)
            .unwrap()
            .insert("id".into(), V::Text(id.clone()));
        history_contract::validate_object(&object).unwrap();
        let observation = ObservationNode::root(&id, &BTreeSet::from([of.to_owned()])).unwrap();
        (object, observation)
    }

    fn restore_saw(object: &mut V, observation: &ObservationNode) {
        let saw = ObservationNode::root(&observation.id, &BTreeSet::new()).unwrap();
        // Tests that need exact membership use the same root membership embedded in input.
        let _ = saw;
        let node = observation;
        let values = if node.added.is_empty() {
            vec![]
        } else {
            node.added.clone()
        };
        map_mut(object).unwrap().insert(
            "saw".into(),
            V::List(values.into_iter().map(V::Text).collect()),
        );
    }

    #[test]
    fn compact_slots_roundtrip_objects_and_sparse_membership() {
        let first = claim("known", "one", "alice", "2026-09-24", 1);
        let second = claim("known", "two", "bob", "2026-09-25", 2);
        let packed = pack(None, &[first.clone(), second.clone()]).unwrap();
        let unpacked = unpack(&packed).unwrap();
        assert_eq!(unpacked.len(), 2);
        for ((mut actual, observation), (expected, expected_observation)) in
            unpacked.into_iter().zip([first, second])
        {
            restore_saw(&mut actual, &observation);
            assert_eq!(actual, expected);
            assert_eq!(observation, expected_observation);
        }
        let context = map(field(map(&packed).unwrap(), "context").unwrap()).unwrap();
        let slots = map(&context["slots"]).unwrap();
        assert!(
            !map(&slots["claim0"]).unwrap()["header"]
                .to_json()
                .unwrap()
                .to_string()
                .contains("aaaaaaaa")
        );
        assert!(
            !map(&slots["claim1"]).unwrap()["header"]
                .to_json()
                .unwrap()
                .to_string()
                .contains("aaaaaaaa")
        );
    }

    #[test]
    fn act_only_keeps_claim_prototype_and_slots_are_reused_across_actions() {
        let initial_claim = claim("known", "claim", "alice", "today", 1);
        let first = pack(None, &[initial_claim.clone()]).unwrap();
        let first_act = act(
            "act",
            "bob",
            "tomorrow",
            text(field(map(&initial_claim.0).unwrap(), "id").unwrap()).unwrap(),
        );
        let act_only = pack(Some(&first), &[first_act]).unwrap();
        let first_context = map(field(map(&first).unwrap(), "context").unwrap()).unwrap();
        let first_slots = map(&first_context["slots"]).unwrap();
        let mut before = first_slots["claim0"].clone();
        expand_common(&mut before, &ledger_common(&first).unwrap()).unwrap();
        let act_context = map(field(map(&act_only).unwrap(), "context").unwrap()).unwrap();
        let act_slots = map(&act_context["slots"]).unwrap();
        let mut after = act_slots["claim0"].clone();
        expand_common(&mut after, &ledger_common(&act_only).unwrap()).unwrap();
        assert_eq!(after, before);
        assert_eq!(unpack(&act_only).unwrap().len(), 1);
        let next = pack(
            Some(&act_only),
            &[claim("other", "next", "carol", "later", 3)],
        )
        .unwrap();
        let context = map(field(map(&next).unwrap(), "context").unwrap()).unwrap();
        assert_eq!(context["emit"], V::List(vec![V::Text("claim0".into())]));
        assert_eq!(map(&context["slots"]).unwrap().len(), 2);
        let next_emitted = unpack(&next).unwrap();
        let new_claim_id = text(field(map(&next_emitted[0].0).unwrap(), "id").unwrap()).unwrap();
        let retained_act = map(&map(&context["slots"]).unwrap()["act0"]).unwrap();
        let old_act_body = map(&retained_act["body"]).unwrap();
        assert_eq!(
            text(&old_act_body["of"]).unwrap(),
            text(field(map(&initial_claim.0).unwrap(), "id").unwrap()).unwrap()
        );
        assert_ne!(text(&old_act_body["of"]).unwrap(), new_claim_id);

        let mut current = None;
        for n in 0..100 {
            let c = claim("known", &format!("repeat{n}"), "alice", "today", n);
            let a = act(
                &format!("act{n}"),
                "bob",
                "today",
                text(field(map(&c.0).unwrap(), "id").unwrap()).unwrap(),
            );
            current = Some(pack(current.as_ref(), &[c, a]).unwrap());
        }
        let state = current.unwrap();
        let context = map(field(map(&state).unwrap(), "context").unwrap()).unwrap();
        assert_eq!(map(&context["slots"]).unwrap().len(), 2);
        assert_eq!(
            context["emit"],
            V::List(vec![V::Text("claim0".into()), V::Text("act0".into())])
        );
    }

    #[test]
    fn metadata_and_batch_order_are_per_object() {
        let a = claim("one", "op-a", "alice", "date-a", 1);
        let b = claim("two", "op-b", "bob", "date-b", 2);
        let state = pack(None, &[a.clone(), b.clone()]).unwrap();
        let out = unpack(&state).unwrap();
        assert_eq!(out[0].0, {
            let mut x = a.0.clone();
            map_mut(&mut x).unwrap().remove("saw");
            x
        });
        assert_eq!(out[1].0, {
            let mut x = b.0.clone();
            map_mut(&mut x).unwrap().remove("saw");
            x
        });
        assert_ne!(out[0].0, out[1].0);
    }

    #[test]
    fn identical_headers_are_lifted_once_and_mixed_values_stay_per_slot() {
        let a = claim("one", "same", "alice", "today", 1);
        let b = claim("two", "same", "alice", "today", 2);
        let packed = pack(None, &[a.clone(), b.clone()]).unwrap();
        let context = map(field(map(&packed).unwrap(), "context").unwrap()).unwrap();
        let common = map(&context["common"]).unwrap();
        assert_eq!(common.len(), 4);
        let slots = map(&context["slots"]).unwrap();
        let first_slot = map(&slots["claim0"]).unwrap();
        let first_header = map(&first_slot["header"]).unwrap();
        assert!(!first_header.contains_key("by"));
        for actual in unpack(&packed)
            .unwrap()
            .into_iter()
            .map(|(mut object, observation)| {
                restore_saw(&mut object, &observation);
                object
            })
        {
            assert!(actual == a.0 || actual == b.0);
        }
    }

    #[test]
    fn input_observation_must_bind_to_the_object_id_and_schema_is_not_duplicated() {
        let c = claim("known", "one", "alice", "today", 1);
        let wrong =
            ObservationNode::root("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", &BTreeSet::new())
                .unwrap();
        assert!(pack(None, &[(c.0.clone(), wrong)]).is_err());
        let large = claim("known", "large", "alice", "today", 1);
        let first = pack(None, &[large.clone()]).unwrap();
        let mut state = first;
        for i in 0..20 {
            let act = act(
                &format!("act{i}"),
                "bob",
                "today",
                text(field(map(&large.0).unwrap(), "id").unwrap()).unwrap(),
            );
            state = pack(Some(&state), &[act]).unwrap();
        }
        let context = map(field(map(&state).unwrap(), "context").unwrap()).unwrap();
        let slots = map(&context["slots"]).unwrap();
        assert_eq!(slots.len(), 2);
        let authored = map(field(map(&slots["claim0"]).unwrap(), "header").unwrap()).unwrap();
        assert_eq!(authored["authored"], map(&large.0).unwrap()["authored"]);
        assert!(!authored.contains_key("saw"));
        assert!(!authored.contains_key("id"));
    }

    #[test]
    fn malformed_emit_slots_subject_and_observation_binding_are_rejected() {
        let c = claim("known", "one", "alice", "today", 1);
        let packed = pack(None, &[c]).unwrap();
        let mut duplicate = packed.clone();
        map_mut(map_mut(&mut duplicate).unwrap().get_mut("context").unwrap())
            .unwrap()
            .insert(
                "emit".into(),
                V::List(vec![V::Text("claim0".into()), V::Text("claim0".into())]),
            );
        assert!(unpack(&duplicate).is_err());

        let mut header_id = packed.clone();
        let context =
            map_mut(map_mut(&mut header_id).unwrap().get_mut("context").unwrap()).unwrap();
        let slot = map_mut(
            map_mut(context.get_mut("slots").unwrap())
                .unwrap()
                .get_mut("claim0")
                .unwrap(),
        )
        .unwrap();
        map_mut(slot.get_mut("header").unwrap()).unwrap().insert(
            "id".into(),
            V::Text("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into()),
        );
        assert!(unpack(&header_id).is_err());

        let mut wrong_subject = packed.clone();
        let context = map_mut(
            map_mut(&mut wrong_subject)
                .unwrap()
                .get_mut("context")
                .unwrap(),
        )
        .unwrap();
        let slot = map_mut(
            map_mut(context.get_mut("slots").unwrap())
                .unwrap()
                .get_mut("claim0")
                .unwrap(),
        )
        .unwrap();
        map_mut(slot.get_mut("header").unwrap())
            .unwrap()
            .insert("subject".into(), V::Text("p.other".into()));
        assert!(unpack(&wrong_subject).is_err());

        let mut wrong_observation = packed;
        let context = map_mut(
            map_mut(&mut wrong_observation)
                .unwrap()
                .get_mut("context")
                .unwrap(),
        )
        .unwrap();
        let slot = map_mut(
            map_mut(context.get_mut("slots").unwrap())
                .unwrap()
                .get_mut("claim0")
                .unwrap(),
        )
        .unwrap();
        map_mut(slot.get_mut("observation").unwrap())
            .unwrap()
            .insert("format".into(), V::Text("bad-format".into()));
        assert!(unpack(&wrong_observation).is_err());

        let claim = claim("known", "base", "alice", "today", 2);
        let id = text(field(map(&claim.0).unwrap(), "id").unwrap())
            .unwrap()
            .to_owned();
        let action = act("action", "bob", "tomorrow", &id);
        let with_alias = pack(None, &[claim, action]).unwrap();
        let mut tampered = with_alias;
        let context = map_mut(map_mut(&mut tampered).unwrap().get_mut("context").unwrap()).unwrap();
        let slot = map_mut(
            map_mut(context.get_mut("slots").unwrap())
                .unwrap()
                .get_mut("act0")
                .unwrap(),
        )
        .unwrap();
        map_mut(slot.get_mut("body").unwrap())
            .unwrap()
            .insert("of".into(), V::Text("@missing0".into()));
        assert!(unpack(&tampered).is_err());
    }

    #[test]
    fn aliases_expand_for_act_references_and_sparse_base_delta() {
        let claim = claim("known", "claim", "alice", "today", 1);
        let claim_id = text(field(map(&claim.0).unwrap(), "id").unwrap())
            .unwrap()
            .to_owned();
        let prior_id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned();
        let before = BTreeSet::from([prior_id.clone()]);
        let after = BTreeSet::from([prior_id, claim_id.clone()]);
        let mut action = v(json!({
            "schema_version":2,"id_scheme":"typed-history/v2","kind":"act",
            "subject":"p.ledger","by":"bob","on":"tomorrow","op":"action",
            "body":{"act":"accept","of":claim_id,"over":[],"because":"because"},
            "saw":after.iter().cloned().collect::<Vec<_>>()
        }));
        let action_id = crate::identity::typed_object_identity(&action).unwrap();
        map_mut(&mut action)
            .unwrap()
            .insert("id".into(), V::Text(action_id.clone()));
        history_contract::validate_object(&action).unwrap();
        let action_observation =
            ObservationNode::between(&action_id, &claim_id, &before, &after).unwrap();
        let input = vec![claim, (action, action_observation)];
        let packed = pack(None, &input).unwrap();
        let context = map(field(map(&packed).unwrap(), "context").unwrap()).unwrap();
        let slots = map(&context["slots"]).unwrap();
        let act_slot = map(&slots["act0"]).unwrap();
        let obs = map(&act_slot["observation"]).unwrap();
        assert_eq!(obs["b"], V::Text("@claim0".into()));
        assert_eq!(obs["a"], V::List(vec![V::Text("^".into())]));
        assert_eq!(
            map(&act_slot["body"]).unwrap()["of"],
            V::Text("@claim0".into())
        );
        for ((expected, expected_obs), (mut actual, actual_obs)) in
            input.iter().zip(unpack(&packed).unwrap())
        {
            map_mut(&mut actual)
                .unwrap()
                .insert("saw".into(), map(expected).unwrap()["saw"].clone());
            assert_eq!(actual, *expected);
            assert_eq!(actual_obs, *expected_obs);
        }
    }

    #[test]
    fn transition_preserves_unemitted_prototypes_and_explicit_tip() {
        let initial_claim = claim("known", "claim", "alice", "today", 1);
        let first_id = text(field(map(&initial_claim.0).unwrap(), "id").unwrap())
            .unwrap()
            .to_owned();
        let initial = pack(None, &[initial_claim]).unwrap();
        validate_transition(None, &initial).unwrap();
        let act_only = pack(Some(&initial), &[act("act-one", "bob", "today", &first_id)]).unwrap();
        validate_transition(Some(&initial), &act_only).unwrap();

        let replacement = claim("known", "replacement", "alice", "tomorrow", 2);
        let replacement_state = pack(Some(&act_only), &[replacement]).unwrap();
        validate_transition(Some(&act_only), &replacement_state).unwrap();
        let replacement_id = text(
            field(
                map(&unpack(&replacement_state).unwrap()[0].0).unwrap(),
                "id",
            )
            .unwrap(),
        )
        .unwrap()
        .to_owned();
        let slots = map(
            &map(field(map(&replacement_state).unwrap(), "context").unwrap()).unwrap()["slots"],
        )
        .unwrap();
        let held_act = map(&slots["act0"]).unwrap();
        assert_eq!(
            text(&map(&held_act["body"]).unwrap()["of"]).unwrap(),
            first_id
        );

        let empty = pack(Some(&replacement_state), &[]).unwrap();
        validate_transition(Some(&replacement_state), &empty).unwrap();
        assert!(unpack(&empty).unwrap().is_empty());
        assert_eq!(
            observation_base(&empty).unwrap(),
            Some(replacement_id.clone())
        );

        let mut header_tampered = empty.clone();
        let context = map_mut(
            map_mut(&mut header_tampered)
                .unwrap()
                .get_mut("context")
                .unwrap(),
        )
        .unwrap();
        let slot = map_mut(
            map_mut(context.get_mut("slots").unwrap())
                .unwrap()
                .get_mut("claim0")
                .unwrap(),
        )
        .unwrap();
        map_mut(slot.get_mut("header").unwrap())
            .unwrap()
            .insert("op".into(), V::Text("tampered".into()));
        assert!(validate_transition(Some(&replacement_state), &header_tampered).is_err());

        let mut body_tampered = empty.clone();
        map_mut(&mut body_tampered)
            .unwrap()
            .insert("body".into(), V::Map(Map::new()));
        assert!(validate_transition(Some(&replacement_state), &body_tampered).is_err());

        let mut observation_tampered = empty.clone();
        let context = map_mut(
            map_mut(&mut observation_tampered)
                .unwrap()
                .get_mut("context")
                .unwrap(),
        )
        .unwrap();
        let slot = map_mut(
            map_mut(context.get_mut("slots").unwrap())
                .unwrap()
                .get_mut("claim0")
                .unwrap(),
        )
        .unwrap();
        map_mut(slot.get_mut("observation").unwrap())
            .unwrap()
            .insert("h".into(), V::Text("b".repeat(64)));
        assert!(validate_transition(Some(&replacement_state), &observation_tampered).is_err());

        let mut alias_tampered = replacement_state.clone();
        let context = map_mut(
            map_mut(&mut alias_tampered)
                .unwrap()
                .get_mut("context")
                .unwrap(),
        )
        .unwrap();
        let slot = map_mut(
            map_mut(context.get_mut("slots").unwrap())
                .unwrap()
                .get_mut("act0")
                .unwrap(),
        )
        .unwrap();
        map_mut(slot.get_mut("body").unwrap())
            .unwrap()
            .insert("of".into(), V::Text("@claim0".into()));
        assert!(validate_transition(Some(&act_only), &alias_tampered).is_err());

        let set_claim = claim("known", "set", "carol", "later", 3);
        let set_id = text(field(map(&set_claim.0).unwrap(), "id").unwrap())
            .unwrap()
            .to_owned();
        let set_pair = pack(
            Some(&empty),
            &[set_claim, act("set-act", "carol", "later", &set_id)],
        )
        .unwrap();
        validate_transition(Some(&empty), &set_pair).unwrap();
        assert_eq!(
            emit_names(&set_pair).unwrap(),
            std::collections::BTreeSet::from(["act0".into(), "claim0".into()])
        );
    }
}
