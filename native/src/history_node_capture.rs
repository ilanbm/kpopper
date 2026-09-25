//! Semantic capture before legacy object/receipt expansion. Public routing is gated separately.
use crate::{
    Result, history_authoring_core::Input, history_contract::*, history_node_codec as C,
    history_node_current::Original, history_node_observation::ObservationNode,
    history_node_publication as P, history_node_semantics::History, history_view::map_mut,
    history_yaml as Y, require, value::TypedValue as V,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

const FORMAT: &str = "node-semantic-object/v1";

#[derive(Clone, Debug)]
pub(crate) struct ReplacedArchive {
    pub(crate) source: &'static str,
    pub(crate) path: String,
    pub(crate) member_sha256: String,
    pub(crate) bytes: Vec<u8>,
}

/// Compact immutable payload. The original object hash is checked when its exact saw is restored.
pub fn payload(object: &V, observation: &ObservationNode) -> Result<V> {
    validate_object(object)?;
    observation.validate()?;
    let mut header = map(object)?.clone();
    require(
        text(&header["id"])? == observation.id,
        "node_semantic_identity",
    )?;
    let body = header.remove("body").unwrap();
    header.remove("saw");
    let collection = if string_is(&header["kind"], "act") {
        "acts"
    } else {
        text(field(map(&header["authored"])?, "collection")?)?
    }
    .to_owned();
    Ok(V::Map(Map::from([
        ("collection".into(), V::Text(collection)),
        ("body".into(), body),
        (
            "context".into(),
            V::Map(Map::from([
                ("format".into(), V::Text(FORMAT.into())),
                ("header".into(), V::Map(header)),
                (
                    "observation".into(),
                    V::from_json(&serde_json::to_value(observation)?)?,
                ),
            ])),
        ),
    ])))
}

/// Preserve original source mapping order alongside unchanged typed semantic identity.
/// Used when importing retained original objects, never to invent an ordering from a hash.
pub fn payload_from_source(raw: &[u8], observation: &ObservationNode) -> Result<V> {
    let source = Y::decode_source_document(raw)?;
    let mut value = payload(&source.typed(), observation)?;
    let order = crate::history_node_source_order::encode(
        &crate::history_node_source_order::without_saw(source)?,
    );
    if order != V::Null {
        map_mut(map_mut(&mut value)?.get_mut("context").unwrap())?
            .insert("source_order".into(), order);
    }
    Ok(value)
}

/// Retain the first claim alongside its readable body, without creating a stream.
pub fn original(object: &V, observation: &ObservationNode) -> Result<Original> {
    let o = map(object)?;
    require(!string_is(&o["kind"], "act"), "node_semantic_original_act")?;
    let value = payload(object, observation)?;
    let value = map(&value)?;
    Original::create(
        text(&o["subject"])?,
        text(&value["collection"])?,
        text(&o["op"])?,
        value["body"].clone(),
        map(&value["context"])?.clone(),
    )
}

fn unpack(
    subject: &str,
    version: &C::Version,
    snapshot: &P::Snapshot,
) -> Result<(V, ObservationNode, Option<V>)> {
    let value = version
        .state()
        .ok_or_else(|| error("node_semantic_absent_unsupported"))?;
    let decoded = crate::history_node_frame::decode(value)?;
    let p = schema(&decoded.semantic, &["collection", "body", "context"], &[])?;
    let context = schema(
        &p["context"],
        &["format", "header", "observation"],
        &["source_order"],
    )?;
    require(
        string_is(&context["format"], FORMAT),
        "node_semantic_format",
    )?;
    let observation: ObservationNode = serde_json::from_value(context["observation"].to_json()?)?;
    let mut object = map(&context["header"])?.clone();
    require(
        !object.contains_key("body") && !object.contains_key("saw"),
        "node_semantic_header",
    )?;
    object.insert("body".into(), p["body"].clone());
    require(
        string_is(field(&object, "subject")?, subject)
            && string_is(field(&object, "id")?, &observation.id)
            && crate::history_node_writer::operation_member(
                snapshot,
                version.operation(),
                text(field(&object, "op")?)?,
            )?,
        "node_semantic_binding",
    )?;
    if !string_is(field(&object, "kind")?, "act") {
        require(
            field(map(field(&object, "authored")?)?, "collection")? == &p["collection"],
            "node_semantic_collection",
        )?;
    } else {
        require(
            string_is(&p["collection"], "acts"),
            "node_semantic_collection",
        )?;
    }
    Ok((
        V::Map(object),
        observation,
        context.get("source_order").cloned(),
    ))
}

#[derive(Clone, Debug)]
pub struct Capture {
    pub(crate) snapshot: P::Snapshot,
    pub(crate) history: History,
    pub(crate) state: V,
    pub(crate) clocks: crate::history_source_ancestry::Graph,
    pub(crate) temporal: Option<V>,
    document: V,
    baseline: V,
    pub(crate) semantic_events: BTreeMap<String, String>,
}

/// Exact semantic identities and current state after complete compact-history
/// validation. This is a read receipt, not an export of the retained raw archive.
pub(crate) fn public_evidence(entry: &Path) -> Result<V> {
    let root = entry.parent().ok_or_else(|| error("invalid_path"))?;
    let captured = Capture::read(root)?;
    require(!captured.is_unborn(), "node_record_not_born")?;
    let objects = captured
        .history
        .objects()
        .iter()
        .map(|(id, object)| {
            Ok((
                id.clone(),
                captured.object(text(&map(object)?["subject"])?, id)?,
            ))
        })
        .collect::<Result<Map>>()?;
    let text_value = |value: &str| V::Text(value.into());
    let evidence = V::Map(Map::from([
        ("format".into(), text_value("node-history-capture/v1")),
        ("profile".into(), text_value(C::FORMAT)),
        ("revision".into(), text_value(captured.revision())),
        (
            "entry_sha256".into(),
            text_value(&crate::identity::sha256(captured.entry_bytes())),
        ),
        ("authority".into(), captured.snapshot.authority.clone()),
        (
            "document".into(),
            Y::decode_document(captured.entry_bytes())?,
        ),
        ("baseline".into(), captured.baseline.clone()),
        ("state".into(), captured.state.clone()),
        ("objects".into(), V::Map(objects)),
        (
            "commits".into(),
            V::Map(
                captured
                    .snapshot
                    .transactions
                    .iter()
                    .map(|(id, transaction)| (id.clone(), text_value(&transaction.digest)))
                    .collect(),
            ),
        ),
    ]));
    captured.verify_current(root)?;
    Ok(evidence)
}

impl Capture {
    /// Compact retained objects and verified storage-event bindings for display only.
    /// This never expands accumulated `saw` ancestry or changes semantic pins.
    pub(crate) fn historical_versions(&self, prefixes: &[String]) -> Result<Vec<V>> {
        let event_versions = self
            .snapshot
            .versions
            .values()
            .flat_map(|versions| versions.values().map(|version| (version.id(), version)))
            .collect::<BTreeMap<_, _>>();
        let mut rows = Vec::new();
        for (id, object) in self.history.objects() {
            let fields = map(object)?;
            let subject = text(field(fields, "subject")?)?;
            if !prefixes.is_empty()
                && !prefixes
                    .iter()
                    .any(|p| subject == p || subject.starts_with(&format!("{p}.")))
            {
                continue;
            }
            let event_id = self.storage_event(id)?;
            let version = event_versions
                .get(event_id)
                .copied()
                .ok_or_else(|| error("missing_object"))?;
            let mut row = fields
                .iter()
                .filter(|(key, _)| key.as_str() != "saw")
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<Map>();
            // `saw` accumulates ancestors on every revision. It is already verified
            // capture input, but expanding or echoing it makes display quadratic.
            row.insert("storage_event_id".into(), V::Text(version.id().into()));
            row.insert("operation_id".into(), V::Text(version.operation().into()));
            row.insert(
                "parents".into(),
                V::List(version.parents().iter().cloned().map(V::Text).collect()),
            );
            rows.push((
                version.id().to_owned(),
                text(field(fields, "id")?)?.to_owned(),
                V::Map(row),
            ));
        }
        rows.sort_by_key(|(event, object, _)| (event.clone(), object.clone()));
        Ok(rows.into_iter().map(|(_, _, row)| row).collect())
    }
    /// Complete verified event ancestry, including storage events with no selected row.
    pub(crate) fn historical_event_parents(&self) -> BTreeMap<String, Vec<String>> {
        self.snapshot
            .versions
            .values()
            .flat_map(|versions| versions.values())
            .map(|version| (version.id().to_owned(), version.parents().to_vec()))
            .collect()
    }
    /// Exact replacement evidence retained in a verified copy archive, never a new act.
    pub(crate) fn archived_replaced_yaml(&self) -> Result<Option<ReplacedArchive>> {
        let mut found: Option<ReplacedArchive> = None;
        for transaction in self.snapshot.transactions.values() {
            let Some(context) = transaction.context.as_ref() else { continue };
            let context = map(context)?;
            if let Some(archived) =
                crate::history_node_import::retained_replaced(&self.snapshot, transaction)?
            {
                if let Some(previous) = &found {
                    require(previous.bytes == archived.bytes, "history_archive_ambiguous")?;
                } else {
                    found = Some(archived);
                }
                continue;
            }
            let (paths, source) = if context.get("format")
                .is_some_and(|v| string_is(v, crate::history_node_bootstrap::FORMAT)) {
                let options = map(field(context, "options")?)?;
                (vec![text(field(options, "archive")?)?], "verified_bootstrap_archive")
            } else if context.get("format")
                .is_some_and(|v| string_is(v, crate::history_node_legacy::CHECKPOINT)) {
                (transaction.evidence.keys().filter(|p| p.starts_with(crate::history_node_legacy::PREFIX)
                    && p.ends_with(".zip")).map(String::as_str).collect(), "verified_legacy_archive")
            } else { continue };
            for path in paths {
                let raw = self.snapshot.raw_evidence.get(path)
                    .ok_or_else(|| error("history_archive_missing"))?;
                require(transaction.evidence.get(path)
                    .is_some_and(|hash| hash == &crate::identity::sha256(raw.as_slice())),
                    "history_archive_hash")?;
                let archive = crate::history_node_archive::Archive::decode(raw)?;
                let Some(replaced) = archive.files().get(".kpopper/replaced.yaml") else { continue };
                if source == "verified_legacy_archive"
                    && !legacy_import_binds_replaced(&archive, replaced)
                {
                    continue;
                }
                if let Some(previous) = &found {
                    require(previous.bytes == *replaced, "history_archive_ambiguous")?;
                } else {
                    found = Some(ReplacedArchive {
                        source,
                        path: ".kpopper/replaced.yaml".into(),
                        member_sha256: crate::identity::sha256(replaced),
                        bytes: replaced.clone(),
                    });
                }
            }
        }
        Ok(found)
    }
    pub fn read(root: &Path) -> Result<Self> {
        Self::from_snapshot(P::capture_snapshot(root)?)
    }
    pub fn revision(&self) -> &str {
        &self.snapshot.revision
    }
    pub fn entry_bytes(&self) -> &[u8] {
        self.snapshot.current.as_deref().unwrap_or_default()
    }
    pub(crate) fn is_unborn(&self) -> bool {
        self.snapshot.current.is_none()
    }
    pub fn verify_current(&self, root: &Path) -> Result<()> {
        require(
            P::capture_snapshot(root)?.revision == self.snapshot.revision,
            "snapshot_changed",
        )
    }
    pub(crate) fn from_snapshot(snapshot: P::Snapshot) -> Result<Self> {
        Self::from_snapshot_inner(snapshot, false)
    }
    pub(crate) fn from_union(snapshot: P::Snapshot) -> Result<Self> {
        Self::from_snapshot_inner(snapshot, true)
    }
    fn from_snapshot_inner(snapshot: P::Snapshot, union: bool) -> Result<Self> {
        let mut document = if let Some(raw) = snapshot.current.as_deref() {
            Y::decode_document(raw)?
        } else {
            require(
                snapshot.transactions.is_empty()
                    && snapshot.versions.is_empty()
                    && snapshot.operations.is_empty(),
                "node_semantic_missing_view",
            )?;
            // A marker reserves identity, but is not yet a record. This fixed
            // planning seed is never published as an unbound empty view.
            crate::reasoning_authoring::declare_document(&V::Map(Map::from([(
                "meta".into(),
                V::Map(Map::from([("updated".into(), V::Null)])),
            )])))?
        };
        let mut objects = Vec::new();
        let mut source_orders = BTreeMap::new();
        let mut semantic_events = BTreeMap::new();
        for (subject, versions) in &snapshot.versions {
            for version in versions.values() {
                require(
                    snapshot.operations.get(version.id()).map(String::as_str)
                        == Some(version.operation()),
                    "node_semantic_operation",
                )?;
                let state = version
                    .state()
                    .ok_or_else(|| error("node_semantic_absent_unsupported"))?;
                if crate::history_node_ledger::is_ledger(state) {
                    let inherited = if version.parents().is_empty() {
                        crate::history_node_ledger::validate_transition(None, state).is_ok()
                    } else {
                        version.parents().iter().any(|id| {
                            versions
                                .get(id)
                                .and_then(|v| v.state())
                                .is_some_and(|previous| {
                                    crate::history_node_ledger::validate_transition(
                                        Some(previous),
                                        state,
                                    )
                                    .is_ok()
                                })
                        })
                    };
                    require(inherited, "node_ledger_transition")?;
                    for (object, observation) in crate::history_node_ledger::unpack(state)? {
                        let o = map(&object)?;
                        require(
                            string_is(&o["subject"], subject)
                                && crate::history_node_writer::operation_member(
                                    &snapshot,
                                    version.operation(),
                                    text(&o["op"])?,
                                )?,
                            "node_semantic_binding",
                        )?;
                        require(
                            semantic_events
                                .insert(observation.id.clone(), version.id().into())
                                .is_none(),
                            "node_semantic_duplicate",
                        )?;
                        objects.push((object, observation));
                    }
                    continue;
                }
                let payload = crate::history_node_frame::decode(
                    version
                        .state()
                        .ok_or_else(|| error("node_semantic_absent_unsupported"))?,
                )?;
                crate::history_node_frame::validate_evidence(version, versions, &payload)?;
                if !payload.is_semantic {
                    continue;
                }
                let (object, observation, source_order) = unpack(subject, version, &snapshot)?;
                if let Some(order) = source_order {
                    source_orders.insert(observation.id.clone(), order);
                }
                require(
                    semantic_events
                        .insert(observation.id.clone(), version.id().into())
                        .is_none(),
                    "node_semantic_duplicate",
                )?;
                objects.push((object, observation));
            }
        }
        let history = History::from_ordered(objects, source_orders)?;
        crate::history_node_legacy::validate(&snapshot, &history, &semantic_events)?;
        crate::history_node_bootstrap::validate(&snapshot, &history, &semantic_events)?;
        crate::history_node_import::validate(&snapshot, &history, &semantic_events)?;
        crate::history_node_contribution::validate_imports(&snapshot, &history, &semantic_events)?;
        let clocks = snapshot.source_clocks.select(
            snapshot
                .transactions
                .values()
                .flat_map(|tx| tx.evidence.keys()),
        );
        let state = clocks.with_ancestry(|ancestry| history.reduce(None, Some(ancestry)))?;
        let templates = crate::history_node_writer::verify_receipts(&snapshot)?;
        let meta = map_mut(
            map_mut(&mut document)?
                .get_mut("meta")
                .ok_or_else(|| error("node_semantic_missing_meta"))?,
        )?;
        let publication = meta.remove("node_publication").unwrap_or(V::Null);
        meta.remove("node_history");
        require(
            !meta.contains_key("history"),
            "node_semantic_legacy_context",
        )?;
        let baseline = V::Map(Map::from([
            ("format".into(), V::Text("node-semantic-baseline/v1".into())),
            ("publication".into(), publication),
        ]));
        let rendered = render(&document, history.objects(), &state, false)?;
        if !union {
            require(rendered == document, "node_semantic_view_mismatch")?;
        }
        {
            let expected = crate::history_authority::document_template(&document)?;
            let parents = snapshot
                .transactions
                .values()
                .flat_map(|t| t.parents.iter())
                .collect::<BTreeSet<_>>();
            for (op, _) in snapshot
                .transactions
                .iter()
                .filter(|(op, _)| !parents.contains(op))
            {
                if let Some(template) = templates.get(op) {
                    require(*template == expected, "node_template_view_mismatch")?;
                } else if union {
                    let template = crate::history_node_transaction::after_template(&snapshot, op)?;
                    require(template == expected, "divergent_templates")?;
                }
            }
        }
        let document = render(&document, history.objects(), &state, true)?;
        let mut captured = Self {
            snapshot,
            history,
            state,
            clocks,
            document,
            baseline,
            semantic_events,
            temporal: None,
        };
        captured.temporal = crate::history_node_temporal::capture(&captured)?;
        Ok(captured)
    }
    /// Original semantic identity lookup; storage-event IDs cannot silently replace pins.
    pub fn object(&self, subject: &str, id: &str) -> Result<V> {
        let object = match self.history.object(id) {
            Ok(object) => object,
            Err(e) if e.0 == "missing_object" => {
                return crate::history_node_legacy::archived_object(&self.snapshot, subject, id);
            }
            Err(e) => return Err(e),
        };
        require(
            string_is(field(map(&object)?, "subject")?, subject),
            "reference_mismatch",
        )?;
        Ok(object)
    }
    pub fn document(&self) -> &V {
        &self.document
    }
    pub fn state(&self) -> &V {
        &self.state
    }
    pub fn object_count(&self) -> usize {
        self.history.objects().len()
    }
    /// Reduce a prospective object set without materializing legacy receipts or files.
    /// The returned input is for authoring evidence only; it is not a published capture.
    pub(crate) fn candidate(&self, new: &[V]) -> Result<Self> {
        self.candidate_with_template(new, &self.document)
    }
    pub(crate) fn candidate_with_template(&self, new: &[V], template: &V) -> Result<Self> {
        let mut objects = self
            .history
            .objects()
            .iter()
            .map(|(id, object)| {
                Ok((
                    object.clone(),
                    self.history
                        .observations()
                        .get(id)
                        .ok_or_else(|| error("incomplete_closure"))?
                        .clone(),
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        for object in new {
            validate_object(object)?;
            let mut compact = map(object)?.clone();
            let saw = crate::history_view::list(&compact.remove("saw").unwrap())?
                .iter()
                .map(|v| text(v).map(str::to_owned))
                .collect::<Result<BTreeSet<_>>>()?;
            let observation = ObservationNode::root(text(&compact["id"])?, &saw)?;
            objects.push((V::Map(compact), observation));
        }
        let history = History::from_ordered(objects, self.history.source_orders().clone())?;
        let state = self.clocks.with_ancestry(|ancestry| {
            history.reduce(Some(map(&map(&self.state)?["rules"])?), Some(ancestry))
        })?;
        let document = crate::history_authoring::destination(&render(
            template,
            history.objects(),
            &state,
            true,
        )?)?;
        Ok(Self {
            history,
            state,
            document,
            ..self.clone()
        })
    }
    pub(crate) fn with_clocks(
        &self,
        clocks: crate::history_source_ancestry::Graph,
    ) -> Result<Self> {
        let state = clocks.with_ancestry(|ancestry| {
            self.history
                .reduce(Some(map(&map(&self.state)?["rules"])?), Some(ancestry))
        })?;
        let document = crate::history_authoring::destination(&render(
            &self.document,
            self.history.objects(),
            &state,
            true,
        )?)?;
        Ok(Self {
            clocks,
            state,
            document,
            ..self.clone()
        })
    }
    pub fn storage_event(&self, semantic_id: &str) -> Result<&str> {
        self.semantic_events
            .get(semantic_id)
            .map(String::as_str)
            .ok_or_else(|| error("missing_object"))
    }
    /// Pure add/set/review preparation. This does not grant publication authority or
    /// persist a receipt; the eventual publisher must replay intent and retain its evidence.
    pub fn prepare_claims(
        &self,
        action: &V,
        options: &crate::history_authoring::Options,
        runtime: Option<&crate::reasoning_runtime::Runtime>,
    ) -> Result<(Vec<V>, V)> {
        self.check_expected(action)?;
        let plan = crate::history_authoring_core::prepare(self, action, options, runtime)?;
        Ok((plan.objects, plan.document))
    }
    pub(crate) fn check_expected(&self, action: &V) -> Result<()> {
        if let Some(expected) = map(action)?.get("expected_record_sha256") {
            let expected = text(expected)?;
            require(
                expected.len() == 64 && expected.bytes().all(|b| b.is_ascii_hexdigit()),
                "invalid_expected_record_sha256",
            )?;
            require(
                self.snapshot
                    .current
                    .as_deref()
                    .map(crate::identity::sha256)
                    .as_deref()
                    == Some(expected),
                "record changed since the caller read it",
            )?;
        }
        Ok(())
    }
}

fn legacy_import_binds_replaced(
    archive: &crate::history_node_archive::Archive,
    replaced: &[u8],
) -> bool {
    let Some(entry) = archive.files().get("GROUNDING.yaml") else {
        return false;
    };
    let Ok(document) = crate::history_yaml::decode_document(entry) else {
        return false;
    };
    let Ok(document) = map(&document) else {
        return false;
    };
    let Some(meta) = document.get("meta") else {
        return false;
    };
    let Ok(meta) = map(meta) else {
        return false;
    };
    let Some(import) = meta.get("history_import") else {
        return false;
    };
    let Ok(import) = map(import) else {
        return false;
    };
    let Some(members) = import.get("members") else {
        return false;
    };
    let Ok(members) = crate::history_view::list(members) else {
        return false;
    };
    let replaced_hash = crate::identity::sha256(replaced);
    members.iter().any(|member| {
        let Ok(member) = map(member) else { return false };
        member.get("role").is_some_and(|role| string_is(role, "replaced"))
            && member.get("path").is_some_and(|path| string_is(path, ".kpopper/replaced.yaml"))
            && member
                .get("sha256")
                .is_some_and(|hash| string_is(hash, replaced_hash.as_str()))
    })
}
impl Input for Capture {
    fn document(&self) -> Result<V> {
        Ok(self.document.clone())
    }
    fn state(&self) -> &V {
        &self.state
    }
    fn baseline(&self) -> &V {
        &self.baseline
    }
    fn object(&self, id: &str) -> Result<&V> {
        self.history
            .objects()
            .get(id)
            .ok_or_else(|| error("incomplete_closure"))
    }
    fn saw(&self, subject: &str) -> Result<Vec<String>> {
        self.history
            .objects()
            .iter()
            .filter_map(|(id, o)| match map(o) {
                Ok(o) if o.get("subject").is_some_and(|v| string_is(v, subject)) => {
                    Some(Ok(id.clone()))
                }
                Ok(_) => None,
                Err(e) => Some(Err(e)),
            })
            .collect()
    }
}

/// Reuse authored field and body interpretation after complete semantic closure validation.
pub(crate) fn render(template: &V, objects: &Map, state: &V, adapt: bool) -> Result<V> {
    let mut doc = template.clone();
    let mut collections = crate::reasoning_fields::collections(template)?
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    for object in objects.values() {
        let o = map(object)?;
        if !string_is(&o["kind"], "act") {
            collections
                .insert(text(&crate::history_adapter::authored(object)?["collection"])?.into());
        }
    }
    for collection in collections {
        map_mut(&mut doc)?.insert(collection, V::Map(Map::new()));
    }
    let mut profile = None;
    let mut roles = map(template)?
        .get("schema")
        .cloned()
        .unwrap_or(V::Map(Map::new()));
    for (subject, state) in map(&map(state)?["subjects"])? {
        let state = map(state)?;
        if !string_is(&state["acceptance"], "accepted") {
            continue;
        }
        let head = text(&state["head"])?;
        let object = &objects[head];
        require_interpretable_claim(object)?;
        let authored = crate::history_adapter::authored(object)?;
        let selected = text(&authored["profile"])?;
        require(
            profile.is_none_or(|p| p == selected),
            "incompatible_authored_profiles",
        )?;
        profile = Some(selected);
        for (role, field) in map(&authored["fields"])? {
            require(
                map(&roles)?.get(role).is_none_or(|old| old == field),
                "incompatible_field_roles",
            )?;
            map_mut(&mut roles)?.insert(role.clone(), field.clone());
        }
        let collection = text(&authored["collection"])?;
        map_mut(map_mut(&mut doc)?.get_mut(collection).unwrap())?.insert(
            subject.clone(),
            if adapt {
                crate::history_adapter::adapt_body(object)?
            } else {
                map(object)?["body"].clone()
            },
        );
    }
    if adapt {
        map_mut(&mut doc)?.insert("schema".into(), roles);
    }
    if let Some(selected) = profile {
        let cap = crate::reasoning_fields::capabilities(&doc, Some(selected))?;
        require(
            string_is(&map(&cap)?["profile"], selected),
            "incompatible_authored_profiles",
        )?;
        if selected == "core/v1" {
            crate::history_view::core_declaration(&doc, &V::Null)?;
        }
    }
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{history_authoring as A, history_store::Store, history_view::list};

    #[test]
    fn provider_prepares_same_core_claims_and_pins_as_legacy_capture() {
        let data: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/history-authoring-candidate.json"
        ))
        .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let runtime = A::tests::runtime(&temp.path().join("runtime"));
        let mut actions = BTreeSet::new();
        for case in data["cases"].as_array().unwrap() {
            let root = tempfile::tempdir().unwrap();
            for (name, raw) in case["files"].as_object().unwrap() {
                let path = root.path().join(name);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, raw.as_str().unwrap()).unwrap();
            }
            let store = Store::new(&root.path().join("GROUNDING.yaml")).unwrap();
            let legacy = store.capture().unwrap();
            let mut versions = BTreeMap::<String, BTreeMap<String, C::Version>>::new();
            let mut operations = BTreeMap::new();
            for object in legacy.objects.values() {
                let o = map(object).unwrap();
                let saw = list(&o["saw"])
                    .unwrap()
                    .iter()
                    .map(|v| text(v).unwrap().to_owned())
                    .collect();
                let observation = ObservationNode::root(text(&o["id"]).unwrap(), &saw).unwrap();
                let subject = text(&o["subject"]).unwrap();
                let operation = text(&o["op"]).unwrap();
                let event = C::Event::create(
                    subject,
                    operation,
                    vec![],
                    None,
                    Some(payload(object, &observation).unwrap()),
                )
                .unwrap();
                operations.insert(event.id().into(), operation.into());
                versions
                    .entry(subject.into())
                    .or_default()
                    .insert(event.id().into(), event.reconstruct(None).unwrap());
            }
            let mut doc = legacy.document.clone();
            map_mut(map_mut(&mut doc).unwrap().get_mut("meta").unwrap())
                .unwrap()
                .remove("history");
            let node = Capture::from_snapshot(P::Snapshot {
                source_clocks: Default::default(),
                legacy: Default::default(),
                raw_evidence: Default::default(),
                authority: V::Null,
                revision: String::new(),
                current: Some(Y::encode_document(&doc).unwrap()),
                versions,
                operations,
                transactions: BTreeMap::new(),
            })
            .unwrap_or_else(|e| panic!("{}: {e}", case["name"]));
            let action = V::from_tagged(&case["action"]).unwrap();
            actions.insert(text(&map(&action).unwrap()["kind"]).unwrap().to_owned());
            let options = A::Options {
                operation: "write-1".into(),
                recorded_at: "2026-09-19T09:30:00+00:00".into(),
                recording_day: "2026-09-19".into(),
                by: V::Text("writer".into()),
                strict: case["strict"].as_bool().unwrap(),
                paths: crate::history_paths::Scheme::Hashed,
                receipt_version: None,
            };
            let expected =
                crate::history_authoring_core::prepare(&legacy, &action, &options, Some(&runtime));
            let actual = node.prepare_claims(&action, &options, Some(&runtime));
            match (expected, actual) {
                (Ok(expected), Ok((objects, document))) => {
                    assert_eq!(objects, expected.objects, "{} objects", case["name"]);
                    assert_eq!(document, expected.document, "{} document", case["name"]);
                }
                (Err(expected), Err(actual)) => {
                    assert_eq!(actual.0, expected.0, "{}", case["name"])
                }
                _ => panic!("{} provider acceptance differs", case["name"]),
            }
        }
        assert_eq!(
            actions,
            BTreeSet::from(["add".into(), "set".into(), "review".into()])
        );
    }
}
