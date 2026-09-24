//! Exact receipt decomposition. Publication must store changed node pieces, not this full set.
use crate::{
    Result,
    history_contract::*,
    history_node_evidence::Evidence,
    history_transaction as T,
    history_view::{list, map_mut},
    reasoning_fields as F, require,
    value::TypedValue as V,
};
use std::collections::BTreeSet;

#[derive(Clone, Debug)]
pub struct Side {
    context: V,
    nodes: Map,
}
#[derive(Clone, Debug)]
pub struct Receipt {
    header: V,
    before: Side,
    after: Side,
}
fn s(value: &str) -> V {
    V::Text(value.into())
}
fn empty() -> V {
    V::Map(Map::new())
}
fn authoring(value: &V, hypothesis: bool) -> Result<()> {
    let allowed = if hypothesis {
        &[
            "version",
            "action",
            "archive",
            "baseline",
            "by",
            "head",
            "kind",
            "name",
            "operation",
            "physical",
            "recorded_at",
            "objects",
        ][..]
    } else {
        &[
            "version",
            "action",
            "archive",
            "baseline",
            "by",
            "recorded_at",
            "objects",
            "notes",
            "kind",
            "subject",
            "acceptance",
            "bootstrap",
        ][..]
    };
    let a = schema(value, &[], allowed)?;
    if let Some(physical) = a.get("physical") {
        require(
            !crate::history_view::truth(physical),
            "node_receipt_physical_evidence_unsupported",
        )?;
    }
    if let Some(bootstrap) = a.get("bootstrap") {
        let b = schema(bootstrap, &["kind", "version"], &[])?;
        require(
            string_is(&b["kind"], "new-record-bootstrap/v1") && is_int(&b["version"], "1"),
            "node_receipt_bootstrap_unsupported",
        )?;
    }
    if let Some(objects) = a.get("objects") {
        // Only touched semantic references, never object bodies or a record inventory.
        ids(objects, true)?;
    }
    for name in ["kind", "subject", "acceptance"] {
        if let Some(value) = a.get(name) {
            text(value)?;
        }
    }
    if let Some(notes) = a.get("notes") {
        for note in list(notes)? {
            text(note)?;
        }
    }
    if let Some(action) = a.get("action") {
        let action = schema(
            action,
            &[],
            &[
                "kind",
                "id",
                "body",
                "value",
                "source",
                "at",
                "as_of",
                "into",
                "why",
                "because",
                "of",
                "over",
                "read",
                "amend",
                "answer_by",
                "hypothesis",
                "section",
                "profile",
                "expected_record_sha256",
                "_record_scope",
            ],
        )?;
        require(
            action.get("kind").is_some_and(|v| {
                [
                    "add", "set", "review", "act", "accept", "correct", "refute", "propose",
                    "retire",
                ]
                .iter()
                .any(|k| string_is(v, k))
            }),
            "node_receipt_action_unsupported",
        )?;
        require(
            !["actions", "steps", "objects", "document", "assessment"]
                .iter()
                .any(|k| action.contains_key(*k)),
            "node_receipt_action_unsupported",
        )?;
    }
    if let Some(baseline) = a.get("baseline") {
        let b = schema(
            baseline,
            &[],
            &[
                "version",
                "record_id",
                "authority_generation",
                "authority_digest",
                "committed_set_digest",
                "heads",
                "open_acts",
                "format",
                "publication",
            ],
        )?;
        for name in ["heads", "open_acts"] {
            if let Some(values) = b.get(name) {
                for refs in map(values)?.values() {
                    ids(refs, true)?;
                }
            }
        }
    }
    Ok(())
}
fn assessment(value: &V) -> Result<()> {
    let report = schema(
        value,
        &["nodes", "snapshot_id"],
        &[
            "as_of",
            "assessment_profile",
            "assessment_revision",
            "attention_policy",
            "operational_limits",
            "schema_version",
            "scope",
            "selection",
        ],
    )?;
    if let Some(scope) = report.get("scope") {
        let scope = map(scope)?;
        if let Some(hypotheses) = scope.get("hypotheses") {
            for value in map(hypotheses)?.values() {
                let h = schema(value, &["kind", "status", "document"], &[])?;
                require(
                    map(&h["document"])?.is_empty(),
                    "node_receipt_scope_unsupported",
                )?;
                text(&h["kind"])?;
                text(&h["status"])?;
            }
        }
        if let Some(context) = scope.get("context") {
            require(
                map(context)?
                    .get("conflicts")
                    .is_none_or(|v| !crate::history_view::truth(v)),
                "node_receipt_scope_unsupported",
            )?;
        }
    }
    Ok(())
}
fn piece<'a>(nodes: &'a mut Map, subject: &str) -> Result<&'a mut Map> {
    map_mut(nodes.entry(subject.into()).or_insert_with(empty))
}
fn baseline(side: &mut V) -> Result<Option<&mut Map>> {
    let m = map_mut(side)?;
    require(
        !(m.contains_key("authoring") && m.contains_key("hypothesis_authoring")),
        "node_receipt_authoring_unsupported",
    )?;
    let key = if m.contains_key("hypothesis_authoring") {
        "hypothesis_authoring"
    } else {
        "authoring"
    };
    let Some(authoring) = m.get_mut(key) else {
        return Ok(None);
    };
    let Some(baseline) = map_mut(authoring)?.get_mut("baseline") else {
        return Ok(None);
    };
    Ok(Some(map_mut(baseline)?))
}
impl Side {
    fn pack(side: &V) -> Result<Self> {
        schema(
            side,
            &[],
            &[
                "kind",
                "document",
                "assessment",
                "authoring",
                "hypothesis_authoring",
            ],
        )?;
        if let Some(value) = map(side)?.get("authoring") {
            authoring(value, false)?;
        }
        if let Some(authoring) = map(side)?.get("hypothesis_authoring") {
            self::authoring(authoring, true)?;
            if let Some(physical) = map(authoring)?.get("physical") {
                require(
                    !crate::history_view::truth(physical),
                    "node_receipt_physical_evidence_unsupported",
                )?;
            }
        }
        let mut literal = side.clone();
        let mut nodes = Map::new();
        let mut declarations = BTreeSet::<Vec<String>>::new();
        if let Some(document) = map_mut(&mut literal)?.get_mut("document") {
            let fields = F::snapshot_fields(document)?;
            let deps = text(&fields["deps"])?;
            for (collection, members) in F::collections(document)? {
                for (subject, body) in members {
                    if let Some(V::List(ids)) = map(&body).ok().and_then(|m| m.get(deps)) {
                        if let Ok(mut ids) = ids
                            .iter()
                            .map(|v| text(v).map(str::to_owned))
                            .collect::<Result<Vec<_>>>()
                        {
                            ids.sort();
                            ids.dedup();
                            declarations.insert(ids);
                        }
                    }
                    piece(&mut nodes, &subject)?
                        .insert("document".into(), V::List(vec![s(&collection), body]));
                }
                map_mut(document)?.insert(collection, empty());
            }
        }
        if let Some(baseline) = baseline(&mut literal)? {
            for field in ["heads", "open_acts"] {
                if let Some(values) = baseline.get_mut(field) {
                    for (subject, value) in map(values)? {
                        piece(&mut nodes, subject)?.insert(field.into(), value.clone());
                    }
                    *values = empty();
                }
            }
        }
        let mut selection = false;
        if let Some(assessment) = map_mut(&mut literal)?.get_mut("assessment") {
            self::assessment(assessment)?;
            let report = map_mut(assessment)?;
            let subjects = map(field(report, "nodes")?)?.clone();
            let snapshot = text(field(report, "snapshot_id")?)?.to_owned();
            declarations.extend(subjects.keys().map(|id| vec![id.clone()]));
            let declared = declarations.into_iter().collect::<Vec<_>>();
            if let Some(selected) = report.get_mut("selection") {
                let exact = V::List(subjects.keys().map(|id| s(id)).collect());
                require(*selected == exact, "node_receipt_selection_unsupported")?;
                *selected = V::Null;
                selection = true;
            }
            for (subject, evidence) in subjects {
                let encoded = Evidence::pack(&evidence, &snapshot, &declared)?.encode()?;
                let value = V::from_json(&serde_json::from_slice(&encoded)?)?;
                piece(&mut nodes, &subject)?.insert("assessment".into(), value);
            }
            report.insert("nodes".into(), empty());
        }
        let context = V::Map(Map::from([
            ("format".into(), s("node-receipt-side/v1")),
            ("literal".into(), literal),
            ("selection_from_nodes".into(), V::Bool(selection)),
        ]));
        let parts = Self { context, nodes };
        require(parts.restore()? == *side, "node_receipt_roundtrip")?;
        Ok(parts)
    }
    pub fn context(&self) -> &V {
        &self.context
    }
    pub fn nodes(&self) -> &Map {
        &self.nodes
    }
    /// Transport pieces are not trusted. Full receipt restoration checks the original digest.
    pub fn from_parts(context: V, nodes: Map) -> Result<Self> {
        let side = Self { context, nodes };
        side.restore()?;
        Ok(side)
    }
    pub fn restore(&self) -> Result<V> {
        let context = schema(
            &self.context,
            &["format", "literal", "selection_from_nodes"],
            &[],
        )?;
        require(
            string_is(&context["format"], "node-receipt-side/v1"),
            "node_receipt_format",
        )?;
        let V::Bool(selection) = context["selection_from_nodes"] else {
            return Err(error("node_receipt_context"));
        };
        let mut literal = context["literal"].clone();
        schema(
            &literal,
            &[],
            &[
                "kind",
                "document",
                "assessment",
                "authoring",
                "hypothesis_authoring",
            ],
        )?;
        for (name, hypothesis) in [("authoring", false), ("hypothesis_authoring", true)] {
            if let Some(value) = map(&literal)?.get(name) {
                authoring(value, hypothesis)?;
            }
        }
        if let Some(document) = map(&literal)?.get("document") {
            require(
                F::collections(document)?.is_empty(),
                "node_receipt_embedded_document",
            )?;
        }
        if let Some(b) = baseline(&mut literal)? {
            for field in ["heads", "open_acts"] {
                if let Some(value) = b.get(field) {
                    require(map(value)?.is_empty(), "node_receipt_embedded_baseline")?;
                }
            }
        }
        if let Some(report) = map(&literal)?.get("assessment") {
            assessment(report)?;
            require(
                map(field(map(report)?, "nodes")?)?.is_empty(),
                "node_receipt_embedded_assessment",
            )?;
        }
        require(self.nodes.len() <= MAX_OBJECTS, "node_receipt_limit")?;
        let mut selected = Vec::new();
        for (subject, node) in &self.nodes {
            let node = schema(node, &[], &["document", "assessment", "heads", "open_acts"])?;
            require(!node.is_empty(), "node_receipt_empty_piece")?;
            if let Some(document) = node.get("document") {
                let pair = list(document)?;
                require(pair.len() == 2, "node_receipt_document")?;
                let collection = text(&pair[0])?;
                let doc = map_mut(&mut literal)?
                    .get_mut("document")
                    .ok_or_else(|| error("node_receipt_document"))?;
                let entries = map_mut(
                    map_mut(doc)?
                        .get_mut(collection)
                        .ok_or_else(|| error("node_receipt_document"))?,
                )?;
                require(
                    entries.insert(subject.clone(), pair[1].clone()).is_none(),
                    "node_receipt_duplicate",
                )?;
            }
            for name in ["heads", "open_acts"] {
                if let Some(value) = node.get(name) {
                    let b =
                        baseline(&mut literal)?.ok_or_else(|| error("node_receipt_baseline"))?;
                    let entries = map_mut(
                        b.get_mut(name)
                            .ok_or_else(|| error("node_receipt_baseline"))?,
                    )?;
                    require(
                        entries.insert(subject.clone(), value.clone()).is_none(),
                        "node_receipt_duplicate",
                    )?;
                }
            }
            if let Some(evidence) = node.get("assessment") {
                let report = map_mut(
                    map_mut(&mut literal)?
                        .get_mut("assessment")
                        .ok_or_else(|| error("node_receipt_assessment"))?,
                )?;
                let snapshot = text(field(report, "snapshot_id")?)?;
                let raw = serde_json::to_vec(&evidence.to_json()?)?;
                // Evidence::decode checks canonical struct ordering; typed maps are ordered
                // by key, so reconstruct the typed transport before canonical encoding.
                let evidence: Evidence = serde_json::from_slice(&raw)?;
                let restored = Evidence::decode(&evidence.encode()?)?.restore(snapshot)?;
                let entries = map_mut(
                    report
                        .get_mut("nodes")
                        .ok_or_else(|| error("node_receipt_assessment"))?,
                )?;
                require(
                    entries.insert(subject.clone(), restored).is_none(),
                    "node_receipt_duplicate",
                )?;
                selected.push(s(subject));
            }
        }
        if selection {
            let report = map_mut(
                map_mut(&mut literal)?
                    .get_mut("assessment")
                    .ok_or_else(|| error("node_receipt_assessment"))?,
            )?;
            require(
                report.get("selection") == Some(&V::Null),
                "node_receipt_selection",
            )?;
            report.insert("selection".into(), V::List(selected));
        }
        Ok(literal)
    }
}
impl Receipt {
    pub fn pack(receipt: &V) -> Result<Self> {
        T::validate_receipt(receipt)?;
        let mut header = map(receipt)?.clone();
        let before = Side::pack(&header.remove("before").unwrap())?;
        let after = Side::pack(&header.remove("after").unwrap())?;
        let packed = Self {
            header: V::Map(header),
            before,
            after,
        };
        require(packed.restore()? == *receipt, "node_receipt_roundtrip")?;
        Ok(packed)
    }
    pub fn header(&self) -> &V {
        &self.header
    }
    pub fn before(&self) -> &Side {
        &self.before
    }
    pub fn after(&self) -> &Side {
        &self.after
    }
    pub fn from_parts(header: V, before: Side, after: Side) -> Result<Self> {
        let receipt = Self {
            header,
            before,
            after,
        };
        receipt.restore()?;
        Ok(receipt)
    }
    pub fn restore(&self) -> Result<V> {
        let mut header = map(&self.header)?.clone();
        require(
            !header.contains_key("before") && !header.contains_key("after"),
            "node_receipt_header",
        )?;
        header.insert("before".into(), self.before.restore()?);
        header.insert("after".into(), self.after.restore()?);
        let value = V::Map(header);
        T::validate_receipt(&value)?;
        Ok(value)
    }
}
