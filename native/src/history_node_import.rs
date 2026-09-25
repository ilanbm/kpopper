//! Copy-only import of an ordinary record into compact node history. Its claims
//! and acts are exactly those of the established history import; the source
//! closure is one hash-bound archive from which every read replays them.
//! No legacy store, receipt, or active copy of the original layout is written.
use crate::{
    Result,
    history_authoring::{n, obj, s, strings},
    history_contract::*,
    history_migration::{self as M, ImportPlan, entry_identity, original_provenance},
    history_migration_source as S,
    history_node_archive::Archive,
    history_node_capture as N, history_node_codec as C, history_node_frame as Frame,
    history_node_observation::ObservationNode,
    history_node_publication as P,
    history_node_receipt::{Nodes, Receipt},
    history_node_semantics::History,
    history_transaction as T,
    history_view::{list, map_mut},
    history_yaml as Y,
    identity::sha256,
    reasoning_fields as Fields,
    reasoning_runtime::Runtime,
    reasoning_snapshot::Snapshot,
    require,
    source_capture::{ReadMode, capture_source},
    source_inventory::{absolute, name},
    value::TypedValue as V,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

pub(crate) const FORMAT: &str = "node-history-import/v1";
const MAP_FORMAT: &str = "node-history-import-map/v1";
const ENTRY: &str = "GROUNDING.yaml";
/// Values the readable view may spend on lazy original bindings. Each binding
/// repeats a tagged object header, and the view is one bounded document; every
/// other subject keeps the identical initial event in its stream instead.
const LAZY_VALUES: usize = crate::value::MAX_VALUES / 4;

fn values(value: &V) -> usize {
    1 + match value {
        V::List(items) => items.iter().map(values).sum(),
        V::Map(items) => items.values().map(values).sum(),
        _ => 0,
    }
}

pub struct Plan {
    import: ImportPlan,
    options: M::Options,
    files: BTreeMap<String, Vec<u8>>,
    mapping: V,
    archive_hash: String,
    archived_only: Vec<String>,
    materialized: Vec<String>,
    hypotheses: Vec<String>,
}

/// One transaction's semantic objects in their exact creation order. Within a
/// subject every import object observes all earlier ones, so `saw` orders it.
fn chains(objects: &Map) -> Result<BTreeMap<String, Vec<V>>> {
    let mut chains = BTreeMap::<String, Vec<V>>::new();
    for object in objects.values() {
        chains
            .entry(text(&map(object)?["subject"])?.into())
            .or_default()
            .push(object.clone());
    }
    for chain in chains.values_mut() {
        chain.sort_by_key(|o| list(&map(o).unwrap()["saw"]).unwrap().len());
        for pair in chain.windows(2) {
            let earlier = map(&pair[0])?;
            let later = saw(&pair[1])?;
            require(
                later.contains(text(&earlier["id"])?) && saw(&pair[0])?.is_subset(&later),
                "node_import_chain",
            )?;
        }
    }
    Ok(chains)
}

fn saw(object: &V) -> Result<BTreeSet<String>> {
    list(&map(object)?["saw"])?
        .iter()
        .map(|v| text(v).map(str::to_owned))
        .collect()
}

struct Layout {
    topology: Option<Map>,
    entry: String,
    absolute: Map,
}

fn members(archive: &Archive) -> Result<Layout> {
    let metadata = schema(
        archive.metadata(),
        &["format", "entry", "topology", "absolute_pointers"],
        &[],
    )?;
    require(
        string_is(&metadata["format"], FORMAT),
        "node_import_archive",
    )?;
    let topology = match &metadata["topology"] {
        V::Null => None,
        topology => Some(map(topology)?.clone()),
    };
    Ok(Layout {
        topology,
        entry: text(&metadata["entry"])?.to_owned(),
        absolute: map(&metadata["absolute_pointers"])?.clone(),
    })
}

/// The original absolute pointers now name reconstructed members. Only replay
/// reads these bytes; locators and the archive keep the exact originals.
fn relocate(raw: &[u8], targets: &Map, place: &BTreeMap<String, PathBuf>) -> Result<Vec<u8>> {
    fn visit(value: &mut V, targets: &Map, place: &BTreeMap<String, PathBuf>) -> Result<()> {
        match value {
            V::Text(pointer) if targets.contains_key(pointer.as_str()) => {
                let member = text(&targets[pointer.as_str()])?;
                let path = place
                    .get(member)
                    .ok_or_else(|| error("node_import_archive_pointer"))?;
                *pointer = path
                    .to_str()
                    .ok_or_else(|| error("invalid_path"))?
                    .to_owned();
            }
            V::Map(items) => {
                for item in items.values_mut() {
                    visit(item, targets, place)?;
                }
            }
            V::List(items) => {
                for item in items {
                    visit(item, targets, place)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    let mut document = S::parse(raw)?;
    for key in ["record", "also"] {
        if let Some(value) = map_mut(&mut document)?.get_mut(key) {
            visit(value, targets, place)?;
        }
    }
    Y::encode_document(&document)
}

/// Reconstruct the exact original layout, relative to its recorded common root.
/// Project discovery stops at this directory: a Git repository or project
/// configuration above the temporary directory is not part of the archived source.
fn reconstruct(
    archive: &Archive,
) -> Result<(
    tempfile::TempDir,
    PathBuf,
    BTreeMap<PathBuf, String>,
    BTreeMap<PathBuf, Vec<u8>>,
)> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().canonicalize()?;
    let Layout {
        topology,
        entry,
        absolute,
    } = members(archive)?;
    let (record, originals) = match &topology {
        Some(t) => (
            text(field(t, "entry")?)?.to_owned(),
            Some(map(field(t, "originals")?)?),
        ),
        None => (entry.clone(), None),
    };
    let mut place = BTreeMap::new();
    for member in archive.files().keys() {
        let relative = match originals {
            Some(originals) => text(
                originals
                    .get(member)
                    .ok_or_else(|| error("node_import_archive_topology"))?,
            )?,
            None => member.as_str(),
        };
        crate::history_branch::portable_path(relative)?;
        place.insert(
            member.clone(),
            crate::history_transaction_fs::target(&root, relative)?,
        );
    }
    if let Some(originals) = originals {
        require(
            originals.len() == archive.files().len(),
            "node_import_archive_topology",
        )?;
    }
    let mut names = BTreeMap::new();
    let mut exact = BTreeMap::new();
    for (member, bytes) in archive.files() {
        let path = place[member].clone();
        let replayed = match absolute.get(member) {
            Some(targets) => relocate(bytes, map(targets)?, &place)?,
            None => bytes.clone(),
        };
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, replayed)?;
        exact.insert(path.clone(), bytes.clone());
        require(
            names.insert(path, member.clone()).is_none(),
            "node_import_archive_topology",
        )?;
    }
    require(
        absolute
            .keys()
            .all(|member| archive.files().contains_key(member)),
        "node_import_archive_pointer",
    )?;
    // An unusable gitfile ends Git discovery here, and one simple project names
    // the reconstructed record; neither is an archive member.
    let boundary = root.join(".git");
    let config = root.join(".kpopper/project.json");
    require(
        !boundary.exists() && !config.exists(),
        "node_import_archive_topology",
    )?;
    std::fs::write(&boundary, b"gitdir: .kpopper-replay-no-repository\n")?;
    std::fs::create_dir_all(config.parent().unwrap())?;
    std::fs::write(
        &config,
        serde_json::to_vec(&serde_json::json!({
            "version": 1, "mode": "simple", "generation": 0,
            "record": record, "publication": null,
        }))?,
    )?;
    Ok((temp, root.join(record), names, exact))
}

fn options(operation: &str, recorded_at: &str) -> M::Options {
    M::Options {
        operation: operation.into(),
        recorded_at: recorded_at.into(),
        record_id: None,
        read_mode: ReadMode::Frozen,
        route: false,
        as_of: None,
    }
}

/// The derived half of the import map; replay recomputes it exactly.
fn derived_mapping(
    import: &ImportPlan,
    operation: &str,
    archive_path: &str,
    events: &BTreeMap<String, String>,
) -> Result<Map> {
    let physical = import
        .physical
        .iter()
        .map(|h| {
            obj([
                ("name", s(&h.name)),
                ("source", s(&h.path)),
                ("sha256", s(&sha256(&h.bytes))),
            ])
        })
        .collect();
    let objects = import
        .objects
        .keys()
        .map(|id| {
            Ok(obj([
                ("semantic_id", s(id)),
                (
                    "storage_event",
                    s(events.get(id).ok_or_else(|| error("node_import_event"))?),
                ),
            ]))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Map::from([
        ("format".into(), s(MAP_FORMAT)),
        ("operation".into(), s(operation)),
        ("archive".into(), s(archive_path)),
        ("source_entry".into(), s(&import.entry)),
        ("destination_entry".into(), s(ENTRY)),
        (
            "topology".into(),
            import.topology.clone().unwrap_or(V::Null),
        ),
        ("original_provenance".into(), original_provenance()),
        ("locators".into(), V::List(import.locators.clone())),
        ("objects".into(), V::List(objects)),
        ("physical_hypotheses".into(), V::List(physical)),
        ("inverse".into(), import.inverse.clone().unwrap_or(V::Null)),
        ("prior_authority".into(), prior_authority(import)?),
    ]))
}

/// A deactivated earlier history stays archived evidence; this names its marker.
fn prior_authority(import: &ImportPlan) -> Result<V> {
    let Some(previous) = &import.previous else {
        return Ok(V::Null);
    };
    let path = import
        .record
        .parent()
        .unwrap()
        .join(&import.layout.authority);
    let member = import
        .mapping
        .get(&path)
        .ok_or_else(|| error("missing_retained_history_authority"))?;
    Ok(obj([
        ("member", s(member)),
        ("sha256", s(&sha256(&import.inventory.files[&path]))),
        ("record_id", map(previous)?["record_id"].clone()),
        ("generation", map(previous)?["generation"].clone()),
    ]))
}

/// Members whose original bytes hold an absolute pointer, with each pointer's target member.
fn absolute_pointers(import: &ImportPlan) -> Result<Map> {
    let mut found = Map::new();
    for path in &import.record_files {
        let mut targets = Map::new();
        for pointer in S::pointer_values(&S::parse(&import.inventory.files[path])?) {
            if Path::new(&pointer).is_absolute() {
                let member = import
                    .mapping
                    .get(&absolute(Path::new(&pointer))?)
                    .ok_or_else(|| error("unresolved_original_pointer"))?;
                targets.insert(pointer, s(member));
            }
        }
        if !targets.is_empty() {
            found.insert(import.mapping[path].clone(), V::Map(targets));
        }
    }
    Ok(found)
}

impl Plan {
    pub fn prepare(
        record: &Path,
        cwd: &Path,
        options: M::Options,
        runtime: Option<&Runtime>,
    ) -> Result<Self> {
        let import = ImportPlan::extract(record, cwd, &options, runtime, None)?;
        let operation = options.operation.clone();
        let recorded_at = options.recorded_at.clone();
        // A deactivated earlier history keeps its record identity and the next
        // generation, as the established import does; its files stay archived.
        let record_id = match &import.previous {
            Some(previous) => {
                let id = text(&map(previous)?["record_id"])?;
                require(
                    options.record_id.as_deref().is_none_or(|r| r == id),
                    "authority_mismatch",
                )?;
                id.to_owned()
            }
            None => options
                .record_id
                .clone()
                .ok_or_else(|| error("missing_record_id"))?,
        };
        let root = import.record.parent().unwrap().to_path_buf();
        let source_files = &import.inventory.files;
        let archive_files = import
            .mapping
            .iter()
            .map(|(path, member)| (member.clone(), source_files[path].clone()))
            .collect::<BTreeMap<_, _>>();
        let archive = Archive::new(
            archive_files,
            obj([
                ("format", s(FORMAT)),
                ("entry", s(&import.entry)),
                ("topology", import.topology.clone().unwrap_or(V::Null)),
                ("absolute_pointers", V::Map(absolute_pointers(&import)?)),
            ]),
        )?;
        let archive_raw = archive.encode()?;
        let archive_hash = sha256(&archive_raw);
        let archive_path = format!("evidence/bootstrap/{archive_hash}.zip");
        let mapping_path = format!("evidence/bootstrap/{operation}.json");

        let mut view = import.document.clone();
        map_mut(&mut view)?.remove("record");
        map_mut(&mut view)?.remove("also");
        for object in import.objects.values() {
            let m = map(object)?;
            if !string_is(&m["kind"], "act") {
                map_mut(&mut view)?
                    .entry(text(&map(&m["authored"])?["collection"])?.into())
                    .or_insert_with(|| obj([]));
            }
        }
        let mut before = view.clone();
        for collection in Fields::collections(&view)?.keys() {
            map_mut(&mut before)?.insert(collection.clone(), obj([]));
        }
        let receipt = Receipt::pack(&T::semantic_receipt(
            text(&import.profile)?,
            &import.capabilities,
            &obj([("document", before)]),
            &obj([("document", view.clone())]),
        )?)?;
        let mut receipt_nodes = Nodes::new();
        let changes = receipt_nodes.prepare(receipt.after())?;
        let changed = changes
            .iter()
            .map(|c| (c.subject.clone(), c))
            .collect::<BTreeMap<_, _>>();
        require(
            changed.len() == changes.len(),
            "node_import_receipt_subject",
        )?;

        let mut files = BTreeMap::new();
        let mut events = BTreeMap::new();
        let mut touches = Vec::new();
        let mut originals = Map::new();
        let mut tails = Map::new();
        let chains = chains(&import.objects)?;
        let mut lazy_values = 0;
        for subject in changed.keys() {
            require(chains.contains_key(subject), "node_import_receipt_subject")?;
        }
        for (subject, chain) in &chains {
            let mut frames = Vec::new();
            let mut last: Option<(C::Version, V)> = None;
            let mut previous: Option<(String, BTreeSet<String>)> = None;
            let lazy = if chain.len() == 1
                && !string_is(&map(&chain[0])?["kind"], "act")
                && changed.contains_key(subject)
            {
                let id = text(&map(&chain[0])?["id"])?;
                let observation = ObservationNode::root(id, &saw(&chain[0])?)?;
                // The binding, its subject key, and one tail string.
                let cost = values(&N::original(&chain[0], &observation)?.encode()?) + 2;
                lazy_values + cost <= LAZY_VALUES && {
                    lazy_values += cost;
                    true
                }
            } else {
                false
            };
            for object in chain {
                let id = text(&map(object)?["id"])?.to_owned();
                let observed = saw(object)?;
                let observation = match &previous {
                    Some((base, prior)) => ObservationNode::between(&id, base, prior, &observed)?,
                    None => ObservationNode::root(&id, &observed)?,
                };
                let semantic = N::payload(object, &observation)?;
                let event = if lazy {
                    let original = N::original(object, &observation)?;
                    originals.insert(subject.clone(), original.encode()?);
                    original.restore(map(object)?["body"].clone())?
                } else {
                    C::Event::create(
                        subject,
                        &operation,
                        last.iter().map(|(v, _)| v.id().to_owned()).collect(),
                        last.as_ref().map(|(v, _)| v),
                        Some(semantic.clone()),
                    )?
                };
                let frame = event.encode()?;
                touches.push((subject.clone(), event.id().to_owned(), sha256(&frame)));
                events.insert(id.clone(), event.id().to_owned());
                frames.extend(frame);
                last = Some((event.reconstruct(last.as_ref().map(|(v, _)| v))?, semantic));
                previous = Some((id, observed));
            }
            if let Some(change) = changed.get(subject) {
                let (base, semantic) = last.as_ref().unwrap();
                let value = Frame::encode(semantic, "evidence", "after", change.after.as_ref())?;
                let tail = C::Event::create(
                    subject,
                    &operation,
                    vec![base.id().to_owned()],
                    Some(base),
                    Some(value),
                )?;
                let frame = tail.encode()?;
                touches.push((subject.clone(), tail.id().to_owned(), sha256(&frame)));
                if lazy {
                    tails.insert(subject.clone(), s(&STANDARD.encode(&frame)));
                } else {
                    frames.extend(frame);
                }
            }
            if !lazy {
                files.insert(
                    format!(".kpopper/history/{}", C::subject_path(subject)?),
                    frames,
                );
            }
        }
        receipt_nodes.apply(&changes)?;
        map_mut(
            map_mut(&mut view)?
                .entry("meta".into())
                .or_insert_with(|| obj([])),
        )?
        .insert(
            "node_history".into(),
            obj([
                ("version", n("2")),
                ("originals", V::Map(originals)),
                ("tails", V::Map(tails)),
            ]),
        );
        let rendered = P::bind_view(&Y::encode_document(&view)?, &operation)?;

        let mut mapping = derived_mapping(&import, &operation, &archive_path, &events)?;
        mapping.insert(
            "source_snapshot_id".into(),
            s(import.original.snapshot_id()),
        );
        let mut evidence = BTreeMap::from([(archive_path.clone(), archive_hash.clone())]);
        let mut observation = V::Null;
        if let Some(observed) = &import.observed {
            let path = format!("evidence/bootstrap/{operation}-live.json");
            let bytes = observed.snapshot()?.to_json()?.into_bytes();
            observation = obj([
                ("read_mode", s("live")),
                ("path", s(&path)),
                ("snapshot_id", s(observed.snapshot()?.snapshot_id())),
                ("sha256", s(&sha256(&bytes))),
            ]);
            evidence.insert(path.clone(), sha256(&bytes));
            files.insert(path, bytes);
        }
        mapping.insert("observation".into(), observation);
        let mapping = V::Map(mapping);
        let mapping_raw = mapping.canonical_bytes()?;
        evidence.insert(mapping_path.clone(), sha256(&mapping_raw));

        let context = obj([
            ("format", s(FORMAT)),
            (
                "options",
                obj([
                    ("operation", s(&operation)),
                    ("recorded_at", s(&recorded_at)),
                    ("archive", s(&archive_path)),
                    ("mapping", s(&mapping_path)),
                    (
                        "evidence",
                        V::Map(evidence.iter().map(|(p, h)| (p.clone(), s(h))).collect()),
                    ),
                ]),
            ),
            ("header", receipt.header().clone()),
            ("before", receipt.before().context().clone()),
            ("after", receipt.after().context().clone()),
        ]);
        let authority_raw = Y::encode_document(&obj([
            ("version", n("3")),
            ("profile", s(C::FORMAT)),
            ("authority", s("history")),
            ("record_id", s(&record_id)),
            ("generation", n(&import.generation.to_string())),
            ("requires", strings(vec![C::FORMAT.into()])),
        ]))?;
        let manifest = P::migration_manifest(
            &sha256(&authority_raw),
            &operation,
            BTreeMap::new(),
            touches,
            Some(sha256(&rendered)),
            &context,
            evidence,
        )?;
        files.insert(ENTRY.into(), rendered);
        files.insert(
            ".gitattributes".into(),
            P::GIT_ATTRIBUTES.as_bytes().to_vec(),
        );
        files.insert(".kpopper/history.yaml".into(), authority_raw);
        files.insert(
            format!(".kpopper/history-commits/{operation}.json"),
            manifest,
        );
        files.insert(archive_path, archive_raw);
        files.insert(mapping_path, mapping_raw);

        // Physical hypotheses enter through the established physical import after
        // this transaction; their exact bytes are also in the archive.
        let mut hypotheses = Vec::new();
        let mut placed = BTreeSet::new();
        for h in &import.physical {
            let file = Path::new(&h.path)
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| error("missing_imported_hypothesis"))?;
            let path = format!(".kpopper/hypotheses/{file}");
            require(
                files.insert(path.clone(), h.bytes.clone()).is_none(),
                "node_import_member_collision",
            )?;
            placed.insert(h.path.clone());
            hypotheses.push(h.name.clone());
        }
        // Files referenced by the record stay at their relative paths so its
        // locators still resolve. Every other member is retained in the archive.
        let referenced = S::referenced_evidence(&import.source)?
            .iter()
            .map(|file| absolute(&root.join(file)))
            .collect::<Result<BTreeSet<_>>>()?;
        let mut archived_only = Vec::new();
        let mut materialized = Vec::new();
        for (path, member) in &import.mapping {
            if placed.contains(member) {
                continue;
            }
            if !referenced.contains(path) {
                archived_only.push(member.clone());
                continue;
            }
            require(
                !member.starts_with(".kpopper/")
                    && !member.starts_with("evidence/bootstrap/")
                    && files
                        .insert(member.clone(), source_files[path].clone())
                        .is_none(),
                "node_import_member_collision",
            )?;
            materialized.push(member.clone());
        }
        let plan = Self {
            import,
            options,
            files,
            mapping,
            archive_hash,
            archived_only,
            materialized,
            hypotheses,
        };
        plan.verify_source()?;
        Ok(plan)
    }

    pub fn verify_source(&self) -> Result<()> {
        M::verify_sources(
            &self.import.source,
            &self.import.inventory,
            self.import.observed.as_ref(),
        )
    }

    pub fn files(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.files
    }

    /// Complete staged checks. The original directory is only read.
    fn verify_staged(&self, staging: &Path) -> Result<Snapshot> {
        self.verify_source()?;
        crate::history_node_physical::finish_copy(staging)?;
        let captured = N::Capture::read(staging)?;
        let context = crate::history_node_hypothesis::context(&captured)?;
        for name in &self.hypotheses {
            require(
                map(&context.groups)?.contains_key(name),
                "node_import_hypothesis_missing",
            )?;
        }
        P::export(staging)?;
        let copied = capture_source(
            &[staging.join(ENTRY)],
            staging,
            ReadMode::Frozen,
            self.options.as_of.clone(),
        )?;
        require(
            entry_identity(&copied.strict_document()?)?
                == entry_identity(&self.import.source.strict_document()?)?,
            "migration_body_mismatch",
        )?;
        let snapshot = copied.snapshot()?.clone();
        captured.verify_current(staging)?;
        self.verify_source()?;
        Ok(snapshot)
    }

    fn report(&self, candidate: Option<&Snapshot>) -> Result<V> {
        let data = candidate.map(Snapshot::to_data);
        let complete = data
            .as_ref()
            .and_then(|d| map(d).ok())
            .and_then(|d| d.get("context"))
            .and_then(|c| map(c).ok())
            .and_then(|c| c.get("history"))
            .and_then(|h| map(h).ok())
            .and_then(|h| h.get("coverage"))
            .and_then(|c| map(c).ok())
            .and_then(|c| c.get("complete"))
            .cloned()
            .unwrap_or(V::Null);
        let problems = &self.import.problems;
        Ok(obj([
            (
                "state",
                s(if problems.is_empty() {
                    "preview"
                } else {
                    "blocked"
                }),
            ),
            ("complete", V::Bool(problems.is_empty())),
            ("historical_support_complete", complete),
            ("record", s(name(&self.import.record)?)),
            ("operation", s(&self.options.operation)),
            ("problems", strings(problems.clone())),
            (
                "source_read_mode",
                s(if self.options.read_mode == ReadMode::Live {
                    "live"
                } else {
                    "frozen"
                }),
            ),
            ("copied_read_mode", s("frozen")),
            (
                "captured_pending_context",
                V::Bool(self.import.observed.is_some()),
            ),
            (
                "inverse",
                self.import
                    .inverse
                    .clone()
                    .unwrap_or(obj([("representable", V::Bool(true))])),
            ),
            (
                "prior_history",
                map(&self.mapping)?["prior_authority"].clone(),
            ),
            ("original_provenance", original_provenance()),
            ("history_format", s(C::FORMAT)),
            ("source_entry", s(&self.import.entry)),
            ("destination_entry", s(ENTRY)),
            ("entry_renamed", V::Bool(self.import.entry != ENTRY)),
            ("archive_sha256", s(&self.archive_hash)),
            ("materialized_members", strings(self.materialized.clone())),
            ("archived_only_members", strings(self.archived_only.clone())),
            ("hypotheses", strings(self.hypotheses.clone())),
            (
                "candidate_snapshot_id",
                candidate.map_or(V::Null, |c| s(c.snapshot_id())),
            ),
            ("manifest", self.mapping.clone()),
        ]))
    }

    /// Builds and fully checks the copy in a private temporary directory, then discards it.
    /// A blocked import is reported without staging: replay refuses incomplete imports.
    pub fn summary(&self) -> Result<V> {
        if !self.import.problems.is_empty() {
            return self.report(None);
        }
        let staging = tempfile::tempdir()?;
        for (relative, raw) in &self.files {
            let target = crate::history_transaction_fs::target(staging.path(), relative)?;
            std::fs::create_dir_all(target.parent().unwrap())?;
            std::fs::write(target, raw)?;
        }
        let candidate = self.verify_staged(staging.path())?;
        self.report(Some(&candidate))
    }

    pub fn publish(&self, destination: &Path) -> Result<V> {
        require(self.import.problems.is_empty(), "incomplete_history_import")?;
        self.verify_source()?;
        let mut candidate = None;
        let root = crate::history_migration_copy::publish_tree(
            destination,
            &self.files,
            &mut |staging| {
                candidate = Some(self.verify_staged(staging)?);
                Ok(())
            },
        )?;
        let mut result = self.report(candidate.as_ref())?;
        map_mut(&mut result)?.extend(Map::from([
            ("state".into(), s("materialized")),
            ("record".into(), s(name(&root.join(ENTRY))?)),
            ("read_mode".into(), s("frozen")),
        ]));
        Ok(result)
    }
}

/// Restore the exact original files of a compact import copy into an absent
/// directory. Like the history/v1 copy restore, it refuses a copy with later
/// history and an original layout that named absolute locations.
pub fn restore(copied: &Path, destination: &Path) -> Result<V> {
    let copied = copied.canonicalize()?;
    require(
        !destination.exists() && destination.symlink_metadata().is_err(),
        "migration_destination_exists",
    )?;
    // The source reader takes its own lock; the captured revision is rechecked
    // before publication instead of holding a directory guard across it.
    let captured = N::Capture::read(&copied)?;
    let snapshot = &captured.snapshot;
    let mut import = None;
    let mut physical = Vec::new();
    for (operation, transaction) in &snapshot.transactions {
        let format = transaction
            .context
            .as_ref()
            .and_then(|c| map(c).ok())
            .and_then(|c| c.get("format"))
            .and_then(|f| text(f).ok());
        match format {
            Some(FORMAT) if import.is_none() => import = Some((operation, transaction)),
            Some(crate::history_node_physical::FORMAT) => physical.push(transaction),
            _ => return Err(error("newer_history_not_representable")),
        }
    }
    let (operation, transaction) = import.ok_or_else(|| error("node_import_missing"))?;
    let options = map(field(
        map(transaction.context.as_ref().unwrap())?,
        "options",
    )?)?;
    let archive = Archive::decode(bound(
        snapshot,
        transaction,
        text(field(options, "archive")?)?,
    )?)?;
    // A physical layer is part of this migration only when it is the one batch
    // of original hypotheses, directly following the import. Later proposals
    // must not disappear just because the base entries still match the archive.
    let mapping_raw = bound(snapshot, transaction, text(field(options, "mapping")?)?)?;
    let mapping = V::from_tagged(&serde_json::from_slice(mapping_raw)?)?;
    let mut hypotheses = BTreeMap::new();
    for item in list(field(map(&mapping)?, "physical_hypotheses")?)? {
        let item = map(item)?;
        let member = text(field(item, "source")?)?;
        let raw = archive
            .files()
            .get(member)
            .ok_or_else(|| error("node_import_archive_topology"))?;
        require(
            string_is(field(item, "sha256")?, &sha256(raw)),
            "node_import_archive_topology",
        )?;
        let filename = Path::new(member)
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| error("node_import_archive_topology"))?;
        require(
            hypotheses
                .insert(format!(".kpopper/hypotheses/{filename}"), raw)
                .is_none(),
            "node_import_archive_topology",
        )?;
    }
    require(
        physical.len() == usize::from(!hypotheses.is_empty()),
        "newer_history_not_representable",
    )?;
    if let Some(layer) = physical.first() {
        require(
            layer.parents.as_slice() == [operation.as_str()]
                && layer.evidence.keys().eq(hypotheses.keys()),
            "newer_history_not_representable",
        )?;
        for (path, raw) in hypotheses {
            require(
                bound(snapshot, layer, &path)? == raw.as_slice(),
                "newer_history_not_representable",
            )?;
        }
    }
    let layout = members(&archive)?;
    require(layout.absolute.is_empty(), "nonrepresentable_inverse")?;
    let originals = layout
        .topology
        .as_ref()
        .map(|t| map(field(t, "originals")?).cloned())
        .transpose()?;
    let restored_entry = match &layout.topology {
        Some(t) => text(field(t, "entry")?)?.to_owned(),
        None => layout.entry.clone(),
    };
    let files = archive
        .files()
        .iter()
        .map(|(member, raw)| {
            let path = match &originals {
                Some(o) => text(
                    o.get(member)
                        .ok_or_else(|| error("node_import_archive_topology"))?,
                )?,
                None => member.as_str(),
            };
            Ok((path.to_owned(), raw.clone()))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let expected = capture_source(&[copied.join(ENTRY)], &copied, ReadMode::Frozen, None)?
        .strict_document()?;
    let root = crate::history_migration_copy::publish_tree(destination, &files, &mut |root| {
        require(
            crate::history_migration_copy::inventory(root)? == files,
            "migration_copy_changed",
        )?;
        // Check original topology before the source reader can follow pointers.
        let mut pending = vec![root.join(&restored_entry)];
        let mut visited = BTreeSet::new();
        while let Some(path) = pending.pop() {
            let path = absolute(&path)?;
            require(path.starts_with(root), "nonrelocatable_original_pointer")?;
            if !visited.insert(path.clone()) {
                continue;
            }
            require(visited.len() <= MAX_OBJECTS, "history_limit")?;
            for pointer in S::pointer_values(&S::parse(&std::fs::read(&path)?)?) {
                pending.push(path.parent().unwrap().join(pointer));
            }
        }
        let restored = capture_source(&[root.join(&restored_entry)], root, ReadMode::Frozen, None)?;
        require(
            entry_identity(&restored.strict_document()?)? == entry_identity(&expected)?,
            "migration_replay_mismatch",
        )?;
        captured.verify_current(&copied)
    })?;
    let mapping = bound(snapshot, transaction, text(field(options, "mapping")?)?)?;
    Ok(obj([
        ("state", s("restored_copy")),
        ("record", s(name(&root.join(&restored_entry))?)),
        ("operation", s(operation)),
        ("archive_sha256", s(&sha256(&Archive::encode(&archive)?))),
        ("map_sha256", s(&sha256(mapping))),
    ]))
}

/// The exact replaced-version sidecar retained by an import archive, for display.
/// Its versions are also semantic claims; this is the byte-bound source evidence.
pub(crate) fn retained_replaced(
    snapshot: &P::Snapshot,
    transaction: &P::Transaction,
) -> Result<Option<N::ReplacedArchive>> {
    let Some(context) = transaction.context.as_ref() else {
        return Ok(None);
    };
    let context = map(context)?;
    if !context.get("format").is_some_and(|v| string_is(v, FORMAT)) {
        return Ok(None);
    }
    let options = map(field(context, "options")?)?;
    let archive = Archive::decode(bound(
        snapshot,
        transaction,
        text(field(options, "archive")?)?,
    )?)?;
    let path = S::adjunct(&members(&archive)?.entry, "replaced");
    Ok(archive.files().get(&path).map(|bytes| N::ReplacedArchive {
        source: "verified_import_archive",
        member_sha256: sha256(bytes),
        path,
        bytes: bytes.clone(),
    }))
}

/// Replay every compact import from its exact retained archive. A mismatch in
/// any semantic object, storage binding, or derived map field refuses the record.
pub(crate) fn validate(
    snapshot: &P::Snapshot,
    history: &History,
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
        let replayed = std::thread::scope(|scope| {
            scope
                .spawn(|| replay(snapshot, history, semantic_events, operation, transaction))
                .join()
                .map_err(|_| error("node_import_replay_failed"))?
        });
        replayed?;
    }
    Ok(())
}

fn bound<'a>(
    snapshot: &'a P::Snapshot,
    transaction: &P::Transaction,
    path: &str,
) -> Result<&'a [u8]> {
    let raw = snapshot
        .raw_evidence
        .get(path)
        .ok_or_else(|| error("node_import_evidence_missing"))?;
    require(
        transaction.evidence.get(path) == Some(&sha256(raw)),
        "node_import_evidence_hash",
    )?;
    Ok(raw.as_slice())
}

fn replay(
    snapshot: &P::Snapshot,
    history: &History,
    semantic_events: &BTreeMap<String, String>,
    operation: &str,
    transaction: &P::Transaction,
) -> Result<()> {
    let context = map(transaction.context.as_ref().unwrap())?;
    let o = schema(
        field(context, "options")?,
        &["operation", "recorded_at", "archive", "mapping", "evidence"],
        &[],
    )?;
    require(
        string_is(&o["operation"], operation),
        "node_import_operation",
    )?;
    let recorded_at = text(&o["recorded_at"])?;
    let archive_path = text(&o["archive"])?;
    let archive = Archive::decode(bound(snapshot, transaction, archive_path)?)?;
    let mapping_raw = bound(snapshot, transaction, text(&o["mapping"])?)?;
    let stored = V::from_tagged(
        &crate::json_ingress::parse_slice_bounded(
            mapping_raw,
            crate::json_ingress::DuplicateKeys::Reject,
            crate::value::MAX_DEPTH * 2 + 8,
        )
        .map_err(|_| error("node_import_mapping_invalid"))?,
    )?;
    require(
        stored.canonical_bytes()?.as_slice() == mapping_raw,
        "node_import_mapping_noncanonical",
    )?;
    let (_temp, record, names, originals) = reconstruct(&archive)?;
    let root = record
        .parent()
        .ok_or_else(|| error("invalid_path"))?
        .to_path_buf();
    let import = ImportPlan::extract(
        &record,
        &root,
        &options(operation, recorded_at),
        None,
        Some(&M::Replay {
            names: &names,
            originals: &originals,
        }),
    )?;
    require(import.problems.is_empty(), "node_import_replay_incomplete")?;
    require(
        import.entry == members(&archive)?.entry,
        "node_import_archive_entry",
    )?;
    let own = semantic_events
        .iter()
        .filter(|(_, event)| {
            snapshot
                .operations
                .get(*event)
                .is_some_and(|op| op == operation)
        })
        .map(|(id, event)| (id.clone(), event.clone()))
        .collect::<BTreeMap<_, _>>();
    require(
        own.keys().eq(import.objects.keys()),
        "node_import_semantic_mismatch",
    )?;
    for (id, expected) in &import.objects {
        require(
            history.object(id)? == *expected,
            "node_import_semantic_mismatch",
        )?;
    }
    let mut expected = derived_mapping(&import, operation, archive_path, &own)?;
    let s_map = map(&stored)?;
    let recorded_snapshot = field(s_map, "source_snapshot_id")?;
    require(
        text(recorded_snapshot)?.len() == 64,
        "node_import_mapping_mismatch",
    )?;
    expected.insert("source_snapshot_id".into(), recorded_snapshot.clone());
    let observation = field(s_map, "observation")?;
    if *observation != V::Null {
        let ob = schema(
            observation,
            &["read_mode", "path", "snapshot_id", "sha256"],
            &[],
        )?;
        let path = text(&ob["path"])?;
        require(
            string_is(&ob["read_mode"], "live")
                && path == format!("evidence/bootstrap/{operation}-live.json"),
            "invalid_migration_observation",
        )?;
        let raw = bound(snapshot, transaction, path)?;
        require(
            string_is(&ob["sha256"], &sha256(raw)),
            "invalid_migration_observation",
        )?;
        let observed = Snapshot::from_json(raw)?;
        crate::history_migration_copy::validate_pending(&observed)?;
        require(
            string_is(&ob["snapshot_id"], observed.snapshot_id()),
            "invalid_migration_observation",
        )?;
    }
    expected.insert("observation".into(), observation.clone());
    require(V::Map(expected) == stored, "node_import_mapping_mismatch")?;
    let mut evidence = BTreeSet::from([archive_path.to_owned(), text(&o["mapping"])?.to_owned()]);
    if let V::Map(ob) = observation {
        evidence.insert(text(&ob["path"])?.to_owned());
    }
    require(
        transaction
            .evidence
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>()
            == evidence,
        "node_import_evidence_set",
    )
}

#[cfg(test)]
mod restore_tests {
    use super::*;

    #[test]
    fn restore_refuses_physical_hypotheses_added_after_the_original_copy() {
        for original_hypothesis in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let source = temp.path().join("source");
            std::fs::create_dir_all(&source).unwrap();
            std::fs::write(source.join(ENTRY), "known: {p.value: {v: 1}}\n").unwrap();
            if original_hypothesis {
                std::fs::create_dir_all(source.join(".kpopper/hypotheses")).unwrap();
                std::fs::write(
                    source.join(".kpopper/hypotheses/original.yaml"),
                    "hypothesis: {claim: Original alternative}\nknown: {p.original: {v: 2}}\n",
                )
                .unwrap();
            }
            let mut configuration = options("restore-import", "2026-09-25T00:00:00+00:00");
            configuration.record_id = Some("restore-fixture".into());
            let copied = temp.path().join("copy");
            Plan::prepare(&source.join(ENTRY), &source, configuration, None)
                .unwrap()
                .publish(&copied)
                .unwrap();
            std::fs::create_dir_all(copied.join(".kpopper/hypotheses")).unwrap();
            std::fs::write(
                copied.join(".kpopper/hypotheses/later.yaml"),
                "hypothesis: {claim: Later alternative}\nknown: {p.later: {v: 3}}\n",
            )
            .unwrap();
            crate::history_node_physical::finish_copy(&copied).unwrap();
            let before = P::export(&copied).unwrap().files().unwrap();
            let output = temp.path().join("restored");
            assert_eq!(
                restore(&copied, &output)
                    .err()
                    .expect("later proposal was silently discarded")
                    .0,
                "newer_history_not_representable"
            );
            assert!(!output.exists());
            assert_eq!(P::export(&copied).unwrap().files().unwrap(), before);
        }
    }
}
