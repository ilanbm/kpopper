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
/// Retained receipt components for one node. Component absence in a side's record
/// context is different from deletion of that component for a particular subject.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub subject: String,
    pub before: Option<V>,
    pub after: Option<V>,
}
/// An in-memory preparation view of node-local evidence; never a manifest payload.
#[derive(Clone, Debug, Default)]
pub struct Nodes {
    values: Map,
}
fn s(value: &str) -> V {
    V::Text(value.into())
}
fn empty() -> V {
    V::Map(Map::new())
}
/// Assessment scope documents are projections, not ordinary record documents: every
/// nonreserved map is a collection, including collections whose bodies are lists.
fn scoped_document_collections(document: &V) -> Result<Map> {
    let document = map(document)?;
    document
        .iter()
        .filter(|(name, _)| !["meta", "schema", "record", "also"].contains(&name.as_str()))
        .map(|(name, value)| Ok((name.clone(), V::Map(map(value)?.clone()))))
        .collect()
}
/// Report intent carries exact hashes and routing identity, never the report or graph itself.
pub(crate) fn report_context(value: &V) -> Result<()> {
    if map(value)?.is_empty() {
        return Ok(());
    }
    let c = schema(
        value,
        &[
            "kind",
            "event_id",
            "source_sha256",
            "envelope_sha256",
            "record",
            "state_dir",
            "policy_sha256",
            "routing_sha256",
        ],
        &["target_sha256"],
    )?;
    require(
        string_is(&c["kind"], "source-report-node/v1"),
        "node_receipt_batch_context_unsupported",
    )?;
    let event = text(&c["event_id"])?;
    require(
        event.len() == 32
            && event
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "node_receipt_report_event",
    )?;
    for key in [
        "source_sha256",
        "envelope_sha256",
        "policy_sha256",
        "routing_sha256",
        "target_sha256",
    ] {
        if let Some(hash) = c.get(key) {
            require(
                crate::history_paths::object_id(text(hash)?),
                "node_receipt_report_hash",
            )?;
        }
    }
    for key in ["record", "state_dir"] {
        require(text(&c[key])?.len() <= 4096, "node_receipt_report_path")?;
    }
    Ok(())
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
            "names",
            "because",
            "take",
            "drops",
            "assessment_version",
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
            "body",
            "collection",
            "because",
            "hypothesis",
            "proposal",
            "actions",
            "context",
            "evidence",
            "steps",
            "validation_world",
            "final_dependency_pins",
        ][..]
    };
    let a = schema(value, &[], allowed)?;
    if hypothesis {
        if let Some(kind) = a.get("kind") {
            require(
                ["edit", "fold", "refute"]
                    .iter()
                    .any(|k| string_is(kind, k)),
                "node_receipt_action_unsupported",
            )?;
        }
        if let Some(names) = a.get("names") {
            for name in list(names)? {
                text(name)?;
            }
        }
        if let Some(name) = a.get("name") {
            text(name)?;
        }
        if let Some(value) = a.get("because") {
            text(value)?;
        }
        if let Some(take) = a.get("take") {
            for name in list(take)? {
                text(name)?;
            }
        }
        if let Some(drops) = a.get("drops") {
            for (id, reason) in map(drops)? {
                text(&s(id))?;
                text(reason)?;
            }
        }
        if let Some(version) = a.get("assessment_version") {
            require(
                is_int(version, "0") || is_int(version, "1"),
                "node_receipt_action_unsupported",
            )?;
        }
    }
    if let Some(actions) = a.get("actions") {
        require(
            a.get("kind").is_some_and(|v| string_is(v, "batch")),
            "node_receipt_action_unsupported",
        )?;
        let actions = list(actions)?;
        require(!actions.is_empty() && actions.len() <= 64, "invalid_batch")?;
        for action in actions {
            // Reuse the narrow direct intent schema; no nested batch/report payload.
            authoring(
                &V::Map(Map::from([("action".into(), action.clone())])),
                false,
            )?;
        }
    }
    if let Some(context) = a.get("context") {
        report_context(context)?;
    }
    if let Some(evidence) = a.get("evidence") {
        require(map(evidence)?.len() <= 64, "node_receipt_evidence_limit")?;
        for (path, hash) in map(evidence)? {
            crate::history_node_publication::evidence_path(path)?;
            require(
                crate::history_paths::object_id(text(hash)?),
                "node_receipt_evidence_hash",
            )?;
        }
    }
    if let Some(steps) = a.get("steps") {
        let steps = list(steps)?;
        require(!steps.is_empty() && steps.len() <= 64, "invalid_batch")?;
        for (index, step) in steps.iter().enumerate() {
            let step = schema(
                step,
                &[
                    "index",
                    "subject",
                    "action_digest",
                    "prior_body_digest",
                    "objects",
                    "notes",
                ],
                &[],
            )?;
            require(
                is_int(&step["index"], &index.to_string()),
                "node_receipt_batch_step",
            )?;
            text(&step["subject"])?;
            for key in ["action_digest", "prior_body_digest"] {
                if step[key] != V::Null {
                    require(
                        crate::history_paths::object_id(text(&step[key])?),
                        "node_receipt_batch_step",
                    )?;
                }
            }
            ids(&step["objects"], true)?;
            for note in list(&step["notes"])? {
                text(note)?;
            }
        }
    }
    if let Some(value) = a.get("validation_world") {
        require(
            string_is(value, "final"),
            "node_receipt_batch_context_unsupported",
        )?;
    }
    if let Some(value) = a.get("final_dependency_pins") {
        require(
            *value == V::Bool(true),
            "node_receipt_batch_context_unsupported",
        )?;
    }
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
    for name in [
        "kind",
        "subject",
        "acceptance",
        "collection",
        "because",
        "proposal",
    ] {
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
                map(&h["document"])?;
                text(&h["kind"])?;
                text(&h["status"])?;
            }
        }
        if let Some(context) = scope.get("context") {
            if let Some(conflicts) = map(context)?.get("conflicts") {
                map(conflicts)?;
            }
        }
    }
    Ok(())
}
fn identity_authoring(value: &V) -> Result<()> {
    let a = schema(
        value,
        &[],
        &[
            "version",
            "kind",
            "a",
            "b",
            "by",
            "operation",
            "recorded_at",
            "baseline",
            "archive",
            "physical",
            "view_sha256",
            "keep",
            "because",
            "as_of",
            "objects",
        ],
    )?;
    for key in ["a", "b", "operation", "recorded_at", "because", "as_of"] {
        if let Some(v) = a.get(key) {
            text(v)?;
        }
    }
    if let Some(v) = a.get("keep").filter(|v| **v != V::Null) {
        text(v)?;
    }
    if let Some(v) = a.get("kind") {
        require(
            ["same", "distinct"].iter().any(|k| string_is(v, k)),
            "node_receipt_identity",
        )?;
    }
    if let Some(v) = a.get("version") {
        require(is_int(v, "1"), "node_receipt_identity")?;
    }
    if let Some(v) = a.get("objects") {
        ids(v, true)?;
    }
    if let Some(v) = a.get("view_sha256") {
        require(*v == V::Null, "node_receipt_physical_evidence_unsupported")?;
    }
    if let Some(v) = a.get("physical") {
        require(
            map(v)?.is_empty(),
            "node_receipt_physical_evidence_unsupported",
        )?;
    }
    if let Some(v) = a.get("baseline") {
        authoring(&V::Map(Map::from([("baseline".into(), v.clone())])), false)?;
    }
    Ok(())
}
fn piece<'a>(nodes: &'a mut Map, subject: &str) -> Result<&'a mut Map> {
    map_mut(nodes.entry(subject.into()).or_insert_with(empty))
}
fn baseline(side: &mut V) -> Result<Option<&mut Map>> {
    let m = map_mut(side)?;
    let keys = ["authoring", "hypothesis_authoring", "identity_authoring"]
        .into_iter()
        .filter(|k| m.contains_key(*k))
        .collect::<Vec<_>>();
    require(keys.len() <= 1, "node_receipt_authoring_unsupported")?;
    let key = keys.first().copied().unwrap_or("authoring");
    let Some(authoring) = m.get_mut(key) else {
        return Ok(None);
    };
    let Some(baseline) = map_mut(authoring)?.get_mut("baseline") else {
        return Ok(None);
    };
    Ok(Some(map_mut(baseline)?))
}
fn components(context: &V) -> Result<BTreeSet<&'static str>> {
    let context = schema(
        context,
        &["format", "literal", "selection_from_nodes"],
        &["proposal"],
    )?;
    let mut literal = context["literal"].clone();
    let side = map(&literal)?;
    let mut result = BTreeSet::new();
    if context.contains_key("proposal") {
        result.insert("proposal");
    }
    if side.contains_key("document") {
        result.insert("document");
    }
    if side.contains_key("assessment") {
        result.insert("assessment");
        if let Some(report) = side.get("assessment") {
            if let Some(scope) = map(report).ok().and_then(|m| m.get("scope")) {
                if map(scope)
                    .ok()
                    .is_some_and(|m| m.contains_key("hypotheses"))
                {
                    result.insert("hypotheses");
                }
                if map(scope)
                    .ok()
                    .and_then(|m| m.get("context"))
                    .and_then(|v| map(v).ok())
                    .is_some_and(|m| m.contains_key("conflicts"))
                {
                    result.insert("conflicts");
                }
            }
        }
    }
    if let Some(baseline) = baseline(&mut literal)? {
        for field in ["heads", "open_acts"] {
            if baseline.contains_key(field) {
                result.insert(field);
            }
        }
    }
    Ok(result)
}
fn scoped_hypothesis_context(literal: &V) -> bool {
    map(literal)
        .ok()
        .and_then(|m| m.get("assessment"))
        .and_then(|r| map(r).ok())
        .and_then(|r| r.get("scope"))
        .and_then(|s| map(s).ok())
        .is_some_and(|s| {
            s.get("hypotheses")
                .and_then(|hs| map(hs).ok())
                .is_some_and(|hs| {
                    hs.values().any(|h| {
                        map(h)
                            .ok()
                            .and_then(|h| h.get("document"))
                            .and_then(|d| scoped_document_collections(d).ok())
                            .is_some_and(|cs| {
                                cs.values().any(|m| map(m).is_ok_and(|m| !m.is_empty()))
                            })
                    })
                })
                || s.get("context")
                    .and_then(|c| map(c).ok())
                    .and_then(|c| c.get("conflicts"))
                    .and_then(|c| map(c).ok())
                    .is_some_and(|c| !c.is_empty())
        })
}
impl Nodes {
    pub fn new() -> Self {
        Self::default()
    }
    /// Import independently verified per-node payloads, never an authoritative global index.
    pub fn from_values(values: Map) -> Result<Self> {
        require(values.len() <= MAX_OBJECTS, "node_receipt_limit")?;
        for node in values.values() {
            let node = schema(
                node,
                &[],
                &[
                    "document",
                    "assessment",
                    "heads",
                    "open_acts",
                    "proposal",
                    "hypotheses",
                    "conflicts",
                ],
            )?;
            require(!node.is_empty(), "node_receipt_empty_piece")?;
        }
        Ok(Self { values })
    }
    pub fn values(&self) -> &Map {
        &self.values
    }
    /// Prepare atomic before/after images for changed subjects only. Inactive components
    /// remain available for later sides; active components remove genuinely absent nodes.
    pub fn prepare(&self, side: &Side) -> Result<Vec<Change>> {
        side.restore()?;
        let active = components(side.context())?;
        let subjects = self
            .values
            .keys()
            .chain(side.nodes.keys())
            .collect::<BTreeSet<_>>();
        let mut changes = Vec::new();
        for subject in subjects {
            let before = self.values.get(subject).cloned();
            let mut after = before
                .as_ref()
                .map(map)
                .transpose()?
                .cloned()
                .unwrap_or_default();
            let wanted = side.nodes.get(subject).map(map).transpose()?;
            for component in &active {
                if let Some(value) = wanted.and_then(|m| m.get(*component)) {
                    after.insert((*component).into(), value.clone());
                } else {
                    after.remove(*component);
                }
            }
            let after = (!after.is_empty()).then_some(V::Map(after));
            if before != after {
                changes.push(Change {
                    subject: subject.clone(),
                    before,
                    after,
                });
            }
        }
        Ok(changes)
    }
    /// Apply only after every before-image and resulting payload has passed validation.
    pub fn apply(&mut self, changes: &[Change]) -> Result<()> {
        let mut seen = BTreeSet::new();
        let mut candidate = self.values.clone();
        for change in changes {
            require(seen.insert(&change.subject), "node_receipt_duplicate")?;
            require(
                self.values.get(&change.subject) == change.before.as_ref(),
                "node_receipt_stale_piece",
            )?;
            if let Some(after) = &change.after {
                candidate.insert(change.subject.clone(), after.clone());
            } else {
                candidate.remove(&change.subject);
            }
        }
        *self = Self::from_values(candidate)?;
        Ok(())
    }
    /// Reconstruct only the components declared by this side's context.
    pub fn side(&self, context: &V) -> Result<Side> {
        let active = components(context)?;
        let mut nodes = Map::new();
        for (subject, value) in &self.values {
            let node = map(value)?
                .iter()
                .filter(|(k, _)| active.contains(k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<Map>();
            if !node.is_empty() {
                nodes.insert(subject.clone(), V::Map(node));
            }
        }
        Side::from_parts(context.clone(), nodes)
    }
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
                "proposal",
                "identity_authoring",
            ],
        )?;
        if let Some(value) = map(side)?.get("authoring") {
            authoring(value, false)?;
        }
        if let Some(value) = map(side)?.get("identity_authoring") {
            identity_authoring(value)?;
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
        let scoped_hypotheses = scoped_hypothesis_context(&literal);
        let mut nodes = Map::new();
        let proposal = map_mut(&mut literal)?
            .remove("proposal")
            .map(|value| {
                // Exactly one hypothetical world; recursive/nested authoring remains unsupported.
                schema(&value, &[], &["kind", "document", "assessment"])?;
                let side = Self::pack(&value)?;
                for (subject, value) in side.nodes {
                    piece(&mut nodes, &subject)?.insert("proposal".into(), value);
                }
                Ok::<V, crate::Error>(side.context)
            })
            .transpose()?;
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
            if let Some(scope) = report.get_mut("scope") {
                let scope = map_mut(scope)?;
                if let Some(hypotheses) = scope.get_mut("hypotheses") {
                    for (name, h) in map_mut(hypotheses)? {
                        let h = map_mut(h)?;
                        let document = h
                            .get_mut("document")
                            .ok_or_else(|| error("node_receipt_scope_unsupported"))?;
                        let mut shell = document.clone();
                        for (collection, members) in scoped_document_collections(document)? {
                            let members = map(&members)?.clone();
                            for (subject, body) in members {
                                let worlds = map_mut(
                                    piece(&mut nodes, &subject)?
                                        .entry("hypotheses".into())
                                        .or_insert_with(empty),
                                )?;
                                let world =
                                    map_mut(worlds.entry(name.clone()).or_insert_with(empty))?;
                                require(
                                    world.insert(collection.clone(), body).is_none(),
                                    "node_receipt_duplicate",
                                )?;
                            }
                            map_mut(&mut shell)?.insert(collection, empty());
                        }
                        *document = shell;
                    }
                }
                if let Some(context) = scope.get_mut("context") {
                    if let Some(conflicts) = map_mut(context)?.get_mut("conflicts") {
                        let entries = map(conflicts)?.clone();
                        for (subject, variants) in entries {
                            piece(&mut nodes, &subject)?.insert("conflicts".into(), variants);
                        }
                        *conflicts = empty();
                    }
                }
            }
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
                let value = Evidence::pack(&evidence, &snapshot, &declared)?.to_value()?;
                piece(&mut nodes, &subject)?.insert("assessment".into(), value);
            }
            report.insert("nodes".into(), empty());
        }
        let mut context = V::Map(Map::from([
            (
                "format".into(),
                s(if scoped_hypotheses {
                    "node-receipt-side/v3"
                } else if proposal.is_some() {
                    "node-receipt-side/v2"
                } else {
                    "node-receipt-side/v1"
                }),
            ),
            ("literal".into(), literal),
            ("selection_from_nodes".into(), V::Bool(selection)),
        ]));
        if let Some(proposal) = proposal {
            map_mut(&mut context)?.insert("proposal".into(), proposal);
        }
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
            &["proposal"],
        )?;
        let has_scoped_parts = self.nodes.values().any(|node| {
            map(node)
                .ok()
                .is_some_and(|m| m.contains_key("hypotheses") || m.contains_key("conflicts"))
        });
        let expected = if has_scoped_parts {
            "node-receipt-side/v3"
        } else if context.contains_key("proposal") {
            "node-receipt-side/v2"
        } else {
            "node-receipt-side/v1"
        };
        require(
            string_is(&context["format"], expected),
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
                "identity_authoring",
            ],
        )?;
        for (name, hypothesis) in [("authoring", false), ("hypothesis_authoring", true)] {
            if let Some(value) = map(&literal)?.get(name) {
                authoring(value, hypothesis)?;
            }
        }
        if let Some(value) = map(&literal)?.get("identity_authoring") {
            identity_authoring(value)?;
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
            if let Some(scope) = map(report)?.get("scope") {
                if let Some(hypotheses) = map(scope)?.get("hypotheses") {
                    for h in map(hypotheses)?.values() {
                        for members in scoped_document_collections(&map(h)?["document"])?.values() {
                            require(map(members)?.is_empty(), "node_receipt_embedded_hypothesis")?;
                        }
                    }
                }
                if let Some(conflicts) = map(scope)?
                    .get("context")
                    .and_then(|c| map(c).ok())
                    .and_then(|c| c.get("conflicts"))
                {
                    require(
                        map(conflicts)?.is_empty(),
                        "node_receipt_embedded_conflicts",
                    )?;
                }
            }
        }
        require(self.nodes.len() <= MAX_OBJECTS, "node_receipt_limit")?;
        let mut selected = Vec::new();
        let mut proposal_nodes = Map::new();
        for (subject, node) in &self.nodes {
            let node = schema(
                node,
                &[],
                &[
                    "document",
                    "assessment",
                    "heads",
                    "open_acts",
                    "proposal",
                    "hypotheses",
                    "conflicts",
                ],
            )?;
            require(!node.is_empty(), "node_receipt_empty_piece")?;
            if let Some(value) = node.get("proposal") {
                require(context.contains_key("proposal"), "node_receipt_proposal")?;
                // Nested payloads are node-local and cannot themselves contain another proposal.
                schema(value, &[], &["document", "assessment"])?;
                proposal_nodes.insert(subject.clone(), value.clone());
            }
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
            if let Some(worlds) = node.get("hypotheses") {
                let worlds = map(worlds)?;
                let report = map_mut(
                    map_mut(&mut literal)?
                        .get_mut("assessment")
                        .ok_or_else(|| error("node_receipt_assessment"))?,
                )?;
                let scope = map_mut(
                    report
                        .get_mut("scope")
                        .ok_or_else(|| error("node_receipt_scope_unsupported"))?,
                )?;
                let declarations = map_mut(
                    scope
                        .get_mut("hypotheses")
                        .ok_or_else(|| error("node_receipt_scope_unsupported"))?,
                )?;
                for (name, pieces) in worlds {
                    let h = declarations
                        .get_mut(name)
                        .ok_or_else(|| error("node_receipt_hypothesis_missing"))?;
                    let document = map_mut(
                        map_mut(h)?
                            .get_mut("document")
                            .ok_or_else(|| error("node_receipt_scope_unsupported"))?,
                    )?;
                    for (collection, body) in map(pieces)? {
                        let entries = document
                            .get_mut(collection)
                            .ok_or_else(|| error("node_receipt_hypothesis_collection_missing"))?;
                        require(
                            map_mut(entries)?
                                .insert(subject.clone(), body.clone())
                                .is_none(),
                            "node_receipt_duplicate",
                        )?;
                    }
                }
            }
            if let Some(conflicts) = node.get("conflicts") {
                let report = map_mut(
                    map_mut(&mut literal)?
                        .get_mut("assessment")
                        .ok_or_else(|| error("node_receipt_assessment"))?,
                )?;
                let scope = map_mut(
                    report
                        .get_mut("scope")
                        .ok_or_else(|| error("node_receipt_scope_unsupported"))?,
                )?;
                let context = map_mut(
                    scope
                        .get_mut("context")
                        .ok_or_else(|| error("node_receipt_scope_unsupported"))?,
                )?;
                let entries = map_mut(
                    context
                        .get_mut("conflicts")
                        .ok_or_else(|| error("node_receipt_scope_unsupported"))?,
                )?;
                require(
                    entries.insert(subject.clone(), conflicts.clone()).is_none(),
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
                let restored = Evidence::from_value(evidence)?.restore(snapshot)?;
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
        if let Some(proposal) = context.get("proposal") {
            schema(
                proposal,
                &["format", "literal", "selection_from_nodes"],
                &[],
            )?;
            let restored = Self::from_parts(proposal.clone(), proposal_nodes)?.restore()?;
            schema(&restored, &[], &["kind", "document", "assessment"])?;
            map_mut(&mut literal)?.insert("proposal".into(), restored);
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
