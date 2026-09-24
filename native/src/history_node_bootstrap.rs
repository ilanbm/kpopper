//! Copy-only first observation of an ordinary record into node history.
//! The archive binds the original bytes; no historical author, operation, or pin is inferred.
use crate::{
    Result,
    history_authoring::{n, obj, s, strings},
    history_contract::*,
    history_migration::original_provenance,
    history_node_archive::Archive,
    history_node_capture as N, history_node_codec as C, history_node_frame as Frame,
    history_node_observation::ObservationNode,
    history_node_publication as P,
    history_node_receipt::{Nodes, Receipt},
    history_transaction as T,
    history_view::map_mut,
    history_yaml as Y,
    identity::{sha256, typed_object_identity},
    reasoning_fields as Fields,
    reasoning_snapshot::entries,
    require,
    source_capture::{ReadMode, capture_source},
    source_inventory::{absolute, name},
    value::TypedValue as V,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

pub(crate) const FORMAT: &str = "node-original-bootstrap/v1";
const ENTRY: &str = "GROUNDING.yaml";

struct Replayed {
    document: V,
    entries: Vec<(String, String, V, String, String)>,
}

fn pointer_member(base: &str, pointer: &str) -> Result<String> {
    let pointer = Path::new(pointer);
    require(!pointer.is_absolute(), "bootstrap_external_pointer")?;
    let mut parts = base.split('/').map(str::to_owned).collect::<Vec<_>>();
    parts.pop();
    for component in pointer.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(part) => parts.push(name(Path::new(part))?.into()),
            std::path::Component::ParentDir => {
                require(parts.pop().is_some(), "bootstrap_external_pointer")?;
            }
            _ => return Err(error("bootstrap_external_pointer")),
        }
    }
    let result = parts.join("/");
    crate::history_branch::portable_path(&result)?;
    Ok(result)
}

fn preflight_pointers(files: &BTreeMap<String, Vec<u8>>) -> Result<()> {
    let mut pending = vec![ENTRY.to_owned()];
    let mut seen = BTreeSet::new();
    while let Some(path) = pending.pop() {
        if !seen.insert(path.clone()) {
            continue;
        }
        require(seen.len() <= 100_000, "bootstrap_source_limit")?;
        let Some(raw) = files.get(&path) else {
            continue;
        };
        let parsed = crate::history_migration_source::parse(raw)?;
        let doc = map(&parsed)?;
        for key in ["record", "also"] {
            let Some(value) = doc.get(key) else { continue };
            let candidates = match value {
                V::Text(value) => vec![value],
                V::List(values) => values
                    .iter()
                    .filter_map(|v| match v {
                        V::Text(s) => Some(s),
                        _ => None,
                    })
                    .collect(),
                V::Map(values) => values
                    .values()
                    .filter_map(|v| match v {
                        V::Text(s) => Some(s),
                        _ => None,
                    })
                    .collect(),
                _ => vec![],
            };
            for candidate in candidates {
                if candidate.ends_with(".yaml") || candidate.ends_with(".yml") {
                    pending.push(pointer_member(&path, candidate)?);
                }
            }
        }
    }
    Ok(())
}

fn replay_archive(archive: &Archive) -> Result<Replayed> {
    preflight_pointers(archive.files())?;
    let metadata = map(archive.metadata())?;
    let entry = text(field(metadata, "entry")?)?;
    require(entry == ENTRY, "bootstrap_entry_unsupported")?;
    let reconstructed = archive.reconstruct()?;
    let root = reconstructed.path();
    let archived_config = archive.files().get(".kpopper/project.json").cloned();
    if archived_config.is_some() {
        fs::remove_file(root.join(".kpopper/project.json"))?;
    }
    let source = capture_source(&[root.join(entry)], root, ReadMode::Frozen, None)?;
    let document = source.strict_document()?;
    require(
        field(metadata, "source_document_sha256")? == &s(&document.digest()?)
            && field(metadata, "source_profile")? == &Fields::capabilities(&document, None)?,
        "bootstrap_archive_meaning",
    )?;
    let (mut files, paths, _) = source_files(root, &source)?;
    if let Some(config) = archived_config {
        files.insert(".kpopper/project.json".into(), config);
    }
    require(&files == archive.files(), "bootstrap_archive_files")?;
    let mut entries_out = Vec::new();
    for (subject, (collection, body)) in entries(&document)? {
        let origin = source
            .origins()
            .get(&collection)
            .and_then(|m| m.get(&subject))
            .ok_or_else(|| error("unresolved_import_origin"))?;
        let path = paths
            .get(origin)
            .ok_or_else(|| error("unresolved_import_origin"))?;
        let raw = files
            .get(path)
            .ok_or_else(|| error("unresolved_import_origin"))?;
        entries_out.push((subject, collection, body, path.clone(), sha256(raw)));
    }
    Ok(Replayed {
        document,
        entries: entries_out,
    })
}

pub struct Plan {
    source: crate::source_capture::CapturedSource,
    extra_files: BTreeMap<PathBuf, Vec<u8>>,
    entry: PathBuf,
    files: BTreeMap<String, Vec<u8>>,
    operation: String,
    archive_hash: String,
    subjects: usize,
}

fn source_files(
    root: &Path,
    source: &crate::source_capture::CapturedSource,
) -> Result<(
    BTreeMap<String, Vec<u8>>,
    BTreeMap<PathBuf, String>,
    BTreeMap<PathBuf, Vec<u8>>,
)> {
    let root = crate::project_modes::resolved(root)?;
    let mut files = BTreeMap::new();
    let mut paths = BTreeMap::new();
    let mut extra = BTreeMap::new();
    for (path, raw) in source.files() {
        let resolved = crate::project_modes::resolved(path)?;
        let relative = resolved
            .strip_prefix(&root)
            .map_err(|_| error("bootstrap_external_source"))?;
        let relative = name(relative)?.replace(std::path::MAIN_SEPARATOR, "/");
        crate::history_branch::portable_path(&relative)?;
        require(
            files.insert(relative.clone(), raw.clone()).is_none(),
            "bootstrap_source_collision",
        )?;
        paths.insert(path.clone(), relative);
    }
    for relative in [
        ".kpopper/project.json",
        ".kpopper/replaced.yaml",
        ".kpopper/view.yaml",
    ] {
        let path = root.join(relative);
        if path.symlink_metadata().is_ok() {
            require(path.is_file(), "bootstrap_extra_file")?;
            let raw = fs::read(&path)?;
            require(raw.len() <= 16 * 1024 * 1024, "bootstrap_extra_limit")?;
            if let Some(old) = files.insert(relative.into(), raw.clone()) {
                require(old == raw, "bootstrap_source_changed")?;
            }
            extra.insert(path, raw);
        }
    }
    Ok((files, paths, extra))
}

fn claim(
    subject: &str,
    collection: &str,
    body: &V,
    fields: &V,
    profile: &V,
    locator: V,
    operation: &str,
    recorded_at: &str,
    known: &BTreeSet<String>,
) -> Result<V> {
    let deps_name = text(&map(fields)?["deps"])?;
    let deps = map(body).ok().and_then(|m| m.get(deps_name));
    let gaps = deps
        .map(|v| match v {
            V::List(values) => values
                .iter()
                .map(|v| text(v).map(str::to_owned))
                .collect::<Result<Vec<_>>>(),
            V::Map(values) => {
                require(
                    values.values().all(
                        |v| !matches!(v, V::Text(hash) if crate::history_paths::object_id(hash)),
                    ),
                    "bootstrap_historical_pin_unresolved",
                )?;
                Ok(values.keys().cloned().collect())
            }
            _ => Err(error("unsupported_bootstrap_dependency_mapping")),
        })
        .transpose()?
        .map(|values| {
            values
                .iter()
                .map(|id| {
                    Ok((
                        id.clone(),
                        s(if known.contains(id) {
                            "not_recorded"
                        } else {
                            "unavailable"
                        }),
                    ))
                })
                .collect::<Result<Map>>()
        })
        .transpose()?
        .unwrap_or_default();
    let mut value = obj([
        ("schema_version", n("2")),
        ("id_scheme", s("typed-history/v2")),
        ("subject", s(subject)),
        (
            "kind",
            s(if deps.is_some() {
                "judgment"
            } else {
                "reading"
            }),
        ),
        ("by", V::Null),
        ("on", s(recorded_at)),
        ("op", s(operation)),
        ("body", body.clone()),
        ("saw", strings(Vec::new())),
        (
            "authored",
            obj([
                ("collection", s(collection)),
                ("fields", fields.clone()),
                ("profile", profile.clone()),
                ("locator", locator),
            ]),
        ),
        ("pins", obj([])),
        ("pin_gaps", V::Map(gaps)),
    ]);
    let id = typed_object_identity(&value)?;
    map_mut(&mut value)?.insert("id".into(), s(&id));
    validate_object(&value)?;
    Ok(value)
}

impl Plan {
    pub fn prepare(entry: &Path) -> Result<Self> {
        let entry = absolute(entry)?;
        require(
            entry.file_name().is_some_and(|n| n == ENTRY),
            "bootstrap_entry_unsupported",
        )?;
        let root = entry
            .parent()
            .ok_or_else(|| error("invalid_path"))?
            .canonicalize()?;
        require(!P::selected(&entry)?, "bootstrap_existing_history")?;
        require(
            !root.join(".kpopper/history.yaml").exists(),
            "bootstrap_existing_history",
        )?;
        let source = capture_source(&[entry.clone()], &root, ReadMode::Frozen, None)?;
        require(
            source.history_capture().is_none() && source.node_history_capture().is_none(),
            "bootstrap_existing_history",
        )?;
        let source_doc = source.strict_document()?;
        let source_entries = entries(&source_doc)?;
        require(!source_entries.is_empty(), "bootstrap_empty_record")?;
        map(source.hypotheses())?;
        let (original_files, path_names, extra_files) = source_files(&root, &source)?;
        let archive = Archive::new(
            original_files.clone(),
            obj([
                ("entry", s(ENTRY)),
                ("source_document_sha256", s(&source_doc.digest()?)),
                ("source_profile", Fields::capabilities(&source_doc, None)?),
            ]),
        )?;
        let archive_raw = archive.encode()?;
        let archive_hash = sha256(&archive_raw);
        let operation = format!("bootstrap-{}", uuid::Uuid::new_v4());
        let recorded_at = chrono::Utc::now().to_rfc3339();
        let fields = V::Map(Fields::snapshot_fields(&source_doc)?);
        let capabilities = Fields::capabilities(&source_doc, None)?;
        let profile = map(&capabilities)?["profile"].clone();
        let known = source_entries.keys().cloned().collect::<BTreeSet<_>>();
        let mut view = source_doc.clone();
        let view_map = map_mut(&mut view)?;
        view_map.remove("record");
        view_map.remove("also");
        let mut before = view.clone();
        for collection in Fields::collections(&view)?.keys() {
            map_mut(&mut before)?.insert(collection.clone(), obj([]));
        }
        let mut originals = Map::new();
        let mut mappings = Vec::new();
        let mut base_events = BTreeMap::new();
        for (subject, (collection, body)) in &source_entries {
            let origin = source
                .origins()
                .get(collection)
                .and_then(|m| m.get(subject))
                .ok_or_else(|| error("unresolved_import_origin"))?;
            let path = path_names
                .get(origin)
                .ok_or_else(|| error("unresolved_import_origin"))?;
            let raw = original_files
                .get(path)
                .ok_or_else(|| error("unresolved_import_origin"))?;
            let locator = obj([
                ("version", n("1")),
                ("kind", s("ordinary_bootstrap")),
                ("path", s(path)),
                ("sha256", s(&sha256(raw))),
                ("collection", s(collection)),
                ("subject", s(subject)),
                ("original", original_provenance()),
                (
                    "import",
                    obj([
                        ("operation", s(&operation)),
                        ("recorded_at", s(&recorded_at)),
                    ]),
                ),
            ]);
            let object = claim(
                subject,
                collection,
                body,
                &fields,
                &profile,
                locator,
                &operation,
                &recorded_at,
                &known,
            )?;
            let id = text(&map(&object)?["id"])?.to_owned();
            let observation = ObservationNode::root(&id, &BTreeSet::new())?;
            let semantic = N::payload(&object, &observation)?;
            let original = N::original(&object, &observation)?;
            let event = original.restore(body.clone())?;
            originals.insert(subject.clone(), original.encode()?);
            mappings.push(obj([
                (
                    "source",
                    obj([
                        ("collection", s(collection)),
                        ("id", s(subject)),
                        ("path", s(path)),
                        ("sha256", s(&sha256(raw))),
                        ("historical_object_id", V::Null),
                    ]),
                ),
                ("semantic_id", s(&id)),
                ("storage_event", s(event.id())),
            ]));
            base_events.insert(subject.clone(), (event, semantic));
        }
        let after_receipt = obj([("document", view.clone())]);
        let before_receipt = obj([("document", before)]);
        let receipt = Receipt::pack(&T::semantic_receipt(
            text(&profile)?,
            &capabilities,
            &before_receipt,
            &after_receipt,
        )?)?;
        let mut receipt_nodes = Nodes::new();
        let changes = receipt_nodes.prepare(receipt.after())?;
        let mut tails = Map::new();
        let mut touches = Vec::new();
        for change in &changes {
            let (initial, semantic) = base_events
                .get(&change.subject)
                .ok_or_else(|| error("bootstrap_receipt_subject"))?;
            let value = Frame::encode(semantic, "evidence", "after", change.after.as_ref())?;
            let base = initial.reconstruct(None)?;
            let tail = C::Event::create(
                &change.subject,
                &operation,
                vec![initial.id().into()],
                Some(&base),
                Some(value),
            )?;
            tails.insert(change.subject.clone(), s(&STANDARD.encode(tail.encode()?)));
            touches.push((
                change.subject.clone(),
                initial.id().into(),
                sha256(&initial.encode()?),
            ));
            touches.push((
                change.subject.clone(),
                tail.id().into(),
                sha256(&tail.encode()?),
            ));
        }
        receipt_nodes.apply(&changes)?;
        let meta = map_mut(
            map_mut(&mut view)?
                .entry("meta".into())
                .or_insert_with(|| obj([])),
        )?;
        meta.insert(
            "node_history".into(),
            obj([
                ("version", n("2")),
                ("originals", V::Map(originals)),
                ("tails", V::Map(tails)),
            ]),
        );
        let rendered = P::bind_view(&Y::encode_document(&view)?, &operation)?;
        let authority = obj([
            ("version", n("3")),
            ("profile", s(C::FORMAT)),
            ("authority", s("history")),
            ("record_id", s(&format!("record-{}", uuid::Uuid::new_v4()))),
            ("generation", n("1")),
            ("requires", V::List(vec![s(C::FORMAT)])),
        ]);
        let authority_raw = Y::encode_document(&authority)?;
        let archive_path = format!("evidence/bootstrap/{archive_hash}.zip");
        let mapping_path = format!("evidence/bootstrap/{operation}.json");
        let mapping = obj([
            ("format", s(FORMAT)),
            ("operation", s(&operation)),
            ("archive", s(&archive_path)),
            (
                "source_ids",
                s("ordinary source keys, not historical object IDs"),
            ),
            ("entries", V::List(mappings)),
        ]);
        let mapping_raw = mapping.canonical_bytes()?;
        let evidence = BTreeMap::from([
            (archive_path.clone(), sha256(&archive_raw)),
            (mapping_path.clone(), sha256(&mapping_raw)),
        ]);
        let context = obj([
            ("format", s(FORMAT)),
            (
                "options",
                obj([
                    ("operation", s(&operation)),
                    ("recorded_at", s(&recorded_at)),
                    ("archive", s(&archive_path)),
                    (
                        "evidence",
                        V::Map(
                            evidence
                                .iter()
                                .map(|(path, hash)| (path.clone(), s(hash)))
                                .collect(),
                        ),
                    ),
                ]),
            ),
            ("header", receipt.header().clone()),
            ("before", receipt.before().context().clone()),
            ("after", receipt.after().context().clone()),
        ]);
        let manifest = P::migration_manifest(
            &sha256(&authority_raw),
            &operation,
            BTreeMap::new(),
            touches,
            Some(sha256(&rendered)),
            &context,
            evidence,
        )?;
        let mut files = BTreeMap::new();
        files.insert(ENTRY.into(), rendered);
        files.insert(".kpopper/history.yaml".into(), authority_raw);
        files.insert(
            format!(".kpopper/history-commits/{operation}.json"),
            manifest,
        );
        files.insert(archive_path, archive_raw);
        files.insert(mapping_path, mapping_raw);
        for (path, raw) in &original_files {
            if path.starts_with(".kpopper/hypotheses/") {
                require(
                    files.insert(path.clone(), raw.clone()).is_none(),
                    "bootstrap_source_collision",
                )?;
            }
        }
        source.verify()?;
        for (path, expected) in &extra_files {
            require(fs::read(path)? == *expected, "bootstrap_source_changed")?;
        }
        Ok(Self {
            source,
            extra_files,
            entry,
            files,
            operation,
            archive_hash,
            subjects: source_entries.len(),
        })
    }

    pub fn files(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.files
    }

    pub fn summary(&self) -> Result<V> {
        Ok(obj([
            ("state", s("prepared")),
            ("operation", s(&self.operation)),
            ("source", s(name(&self.entry)?)),
            ("archive_sha256", s(&self.archive_hash)),
            ("subjects", n(&self.subjects.to_string())),
        ]))
    }

    pub fn publish(&self, destination: &Path) -> Result<V> {
        self.source.verify()?;
        self.verify_extras()?;
        let root = crate::history_migration_copy::publish_tree(
            destination,
            &self.files,
            &mut |staging| {
                self.source.verify()?;
                self.verify_extras()?;
                crate::history_node_physical::finish_copy(staging)?;
                let archive = Archive::decode(
                    self.files
                        .values()
                        .find(|raw| sha256(raw) == self.archive_hash)
                        .ok_or_else(|| error("bootstrap_archive_missing"))?,
                )?;
                require(archive.metadata() != &V::Null, "bootstrap_archive_missing")?;
                let snapshot = P::capture_snapshot(staging)?;
                require(
                    snapshot.transactions.contains_key(&self.operation),
                    "bootstrap_publication_invalid",
                )?;
                N::Capture::read(staging)?;
                Ok(())
            },
        )?;
        Ok(obj([
            ("state", s("published_copy")),
            ("destination", s(name(&root)?)),
            ("operation", s(&self.operation)),
            ("archive_sha256", s(&self.archive_hash)),
        ]))
    }

    fn verify_extras(&self) -> Result<()> {
        for (path, expected) in &self.extra_files {
            require(fs::read(path)? == *expected, "bootstrap_source_changed")?;
        }
        Ok(())
    }
}

/// Recompute each newly observed object from the exact archived source closure.
/// The caller must first bind `archive_raw` to the transaction evidence hash.
pub(crate) fn validate(
    snapshot: &P::Snapshot,
    history: &crate::history_node_semantics::History,
    semantic_events: &BTreeMap<String, String>,
) -> Result<()> {
    for (operation, transaction) in &snapshot.transactions {
        let Some(context) = transaction.context.as_ref() else {
            continue;
        };
        if !map(context)
            .ok()
            .and_then(|m| m.get("format"))
            .is_some_and(|v| string_is(v, FORMAT))
        {
            continue;
        }
        let options = map(field(map(context)?, "options")?)?;
        let archive_path = text(field(options, "archive")?)?;
        let archive_raw = snapshot
            .raw_evidence
            .get(archive_path)
            .ok_or_else(|| error("bootstrap_archive_missing"))?;
        validate_archive(snapshot, history, semantic_events, operation, archive_raw)?;
    }
    Ok(())
}

fn validate_archive(
    snapshot: &P::Snapshot,
    history: &crate::history_node_semantics::History,
    semantic_events: &BTreeMap<String, String>,
    operation: &str,
    archive_raw: &[u8],
) -> Result<()> {
    let transaction = snapshot
        .transactions
        .get(operation)
        .ok_or_else(|| error("bootstrap_missing_transaction"))?;
    let context = transaction
        .context
        .as_ref()
        .ok_or_else(|| error("bootstrap_missing_context"))?;
    let context = map(context)?;
    require(
        context.get("format").is_some_and(|v| string_is(v, FORMAT)),
        "bootstrap_context",
    )?;
    let options = map(field(context, "options")?)?;
    let archive_path = text(field(options, "archive")?)?;
    require(
        transaction
            .evidence
            .get(archive_path)
            .is_some_and(|hash| hash == &sha256(archive_raw)),
        "bootstrap_archive_hash",
    )?;
    let recorded_at = text(field(options, "recorded_at")?)?;
    let archive = Archive::decode(archive_raw)?;
    let replayed = std::thread::scope(|scope| {
        scope
            .spawn(|| replay_archive(&archive))
            .join()
            .map_err(|_| error("bootstrap_replay_failed"))?
    })?;
    let known = replayed
        .entries
        .iter()
        .map(|(subject, _, _, _, _)| subject.clone())
        .collect::<BTreeSet<_>>();
    let fields = V::Map(Fields::snapshot_fields(&replayed.document)?);
    let profile = map(&Fields::capabilities(&replayed.document, None)?)?["profile"].clone();
    let mut expected_view = replayed.document.clone();
    map_mut(&mut expected_view)?.remove("record");
    map_mut(&mut expected_view)?.remove("also");
    let receipt = crate::history_node_writer::receipt(snapshot, operation)?;
    require(
        field(map(&map(&receipt)?["after"])?, "document")? == &expected_view,
        "bootstrap_view_meaning",
    )?;
    let mut expected = BTreeSet::new();
    let mut expected_mappings = Vec::new();
    for (subject, collection, body, path, raw_sha) in replayed.entries {
        let locator = obj([
            ("version", n("1")),
            ("kind", s("ordinary_bootstrap")),
            ("path", s(&path)),
            ("sha256", s(&raw_sha)),
            ("collection", s(&collection)),
            ("subject", s(&subject)),
            ("original", original_provenance()),
            (
                "import",
                obj([("operation", s(operation)), ("recorded_at", s(recorded_at))]),
            ),
        ]);
        let expected_object = claim(
            &subject,
            &collection,
            &body,
            &fields,
            &profile,
            locator,
            operation,
            recorded_at,
            &known,
        )?;
        let id = text(&map(&expected_object)?["id"])?.to_owned();
        require(
            semantic_events.contains_key(&id) && history.object(&id)? == expected_object,
            "bootstrap_semantic_mismatch",
        )?;
        expected_mappings.push(obj([
            (
                "source",
                obj([
                    ("collection", s(&collection)),
                    ("id", s(&subject)),
                    ("path", s(&path)),
                    ("sha256", s(&raw_sha)),
                    ("historical_object_id", V::Null),
                ]),
            ),
            ("semantic_id", s(&id)),
            ("storage_event", s(&semantic_events[&id])),
        ]));
        expected.insert(id);
    }
    let actual = semantic_events
        .keys()
        .filter(|id| {
            snapshot
                .operations
                .get(semantic_events[*id].as_str())
                .is_some_and(|op| op == operation)
        })
        .cloned()
        .collect::<BTreeSet<_>>();
    require(actual == expected, "bootstrap_semantic_mismatch")?;
    let mapping_path = format!("evidence/bootstrap/{operation}.json");
    let mapping_raw = snapshot
        .raw_evidence
        .get(&mapping_path)
        .ok_or_else(|| error("bootstrap_mapping_missing"))?;
    require(
        transaction
            .evidence
            .get(&mapping_path)
            .is_some_and(|hash| hash == &sha256(mapping_raw)),
        "bootstrap_mapping_hash",
    )?;
    let expected_mapping = obj([
        ("format", s(FORMAT)),
        ("operation", s(operation)),
        ("archive", s(archive_path)),
        (
            "source_ids",
            s("ordinary source keys, not historical object IDs"),
        ),
        ("entries", V::List(expected_mappings)),
    ]);
    require(
        expected_mapping.canonical_bytes()?.as_slice() == mapping_raw.as_slice(),
        "bootstrap_mapping_mismatch",
    )
}
