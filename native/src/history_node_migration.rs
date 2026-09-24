//! Copy-only conversion of a checked legacy history into node-local storage.
//! The original directory is never used as a publication target.
use crate::{
    history_contract::*,
    history_node_archive::Archive,
    history_node_capture as N, history_node_codec as C,
    history_node_current::Original,
    history_node_frame as Frame,
    history_node_observation::ObservationNode,
    history_node_publication as P,
    history_node_receipt::{Nodes, Receipt},
    history_transaction_fs as FS,
    history_view::{list, map_mut},
    history_yaml as Y,
    identity::sha256,
    require,
    value::TypedValue as V,
    Result,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

fn s(value: &str) -> V {
    V::Text(value.into())
}
fn unsupported(code: &str) -> crate::Error {
    error(code)
}

/// A complete, independently checked copy. `files` contains no source paths.
pub struct Plan {
    source: crate::history_capture::Capture,
    source_files: BTreeMap<String, Vec<u8>>,
    files: BTreeMap<String, Vec<u8>>,
    summary: V,
}
impl Plan {
    pub fn prepare(entry: &Path) -> Result<Self> {
        let source = crate::history_store::Store::new(entry)?.capture()?;
        require(
            source.layout.entry == "GROUNDING.yaml",
            "node_migration_entry_unsupported",
        )?;
        require(
            !source.commits.is_empty(),
            "node_migration_ordinary_unsupported",
        )?;
        let source_files = source_files(&source)?;
        let archive = Archive::new(
            source_files.clone(),
            V::Map(Map::from([("entry".into(), s(&source.layout.entry))])),
        )?;
        let archive_bytes = archive.encode()?;
        let archive_hash = sha256(&archive_bytes);
        let archive_path = format!("evidence/legacy/{archive_hash}.zip");
        let marker = map(&source.marker)?;
        let authority = V::Map(Map::from([
            ("version".into(), V::from_json(&serde_json::json!(3))?),
            ("profile".into(), s(C::FORMAT)),
            ("authority".into(), s("history")),
            ("record_id".into(), marker["record_id"].clone()),
            ("generation".into(), marker["generation"].clone()),
            ("requires".into(), V::List(vec![s(C::FORMAT)])),
        ]));
        P::validate_authority(&authority)?;
        let authority_raw = Y::encode_document(&authority)?;
        let authority_hash = sha256(&authority_raw);
        let mut files = BTreeMap::from([(".kpopper/history.yaml".into(), authority_raw)]);
        let manifests = source
            .commits
            .iter()
            .map(|(op, raw)| {
                let decoded = Y::decode_document(raw)?;
                crate::history_authority::validate_commit(&decoded)?;
                Ok((op.clone(), decoded))
            })
            .collect::<Result<BTreeMap<_, V>>>()?;
        let order = causal_order(&manifests)?;
        let mut digests = BTreeMap::<String, String>::new();
        let mut frontiers = BTreeMap::<String, BTreeMap<String, BTreeSet<String>>>::new();
        let mut versions = BTreeMap::<String, C::Version>::new();
        let mut event_ancestors = BTreeMap::<String, BTreeSet<String>>::new();
        let mut saw_by_id = BTreeMap::<String, BTreeSet<String>>::new();
        let mut event_for_id = BTreeMap::<String, String>::new();
        let mut frames = BTreeMap::<String, Vec<u8>>::new();
        let mut object_count = 0usize;
        for op in &order {
            let m = map(&manifests[op])?;
            let old_raw = &source.commits[op];
            let mut parents = BTreeMap::new();
            let mut frontier = BTreeMap::<String, BTreeSet<String>>::new();
            for parent in map(&m["parents"])?.keys() {
                parents.insert(parent.clone(), digests[parent].clone());
                for (subject, ids) in &frontiers[parent] {
                    frontier
                        .entry(subject.clone())
                        .or_default()
                        .extend(ids.iter().cloned());
                }
            }
            for ids in frontier.values_mut() {
                prune(ids, &event_ancestors);
            }
            let parent_closure = frontier
                .values()
                .flat_map(|ids| ids.iter())
                .flat_map(|id| {
                    std::iter::once(id.clone()).chain(event_ancestors[id].iter().cloned())
                })
                .collect::<BTreeSet<_>>();
            let references = list(&m["objects"])?;
            let mut remaining = references
                .iter()
                .map(|r| {
                    let r = map(r)?;
                    Ok(text(&r["id"])?.to_owned())
                })
                .collect::<Result<BTreeSet<_>>>()?;
            // A retained or adopted legacy manifest may inventory an object that an
            // earlier transaction already introduced. Its original semantic ID has
            // one storage creation, regardless of how many manifests retain it.
            // A parallel owner is ambiguous: this encoder cannot make that object
            // causally visible in both branches without an explicit alias event.
            for id in &remaining {
                if let Some(event) = event_for_id.get(id) {
                    require(
                        parent_closure.contains(event),
                        "node_migration_parallel_identity_unsupported",
                    )?;
                }
            }
            remaining.retain(|id| !event_for_id.contains_key(id));
            let mut touches = Vec::new();
            while !remaining.is_empty() {
                let next = remaining
                    .iter()
                    .find(|id| {
                        let object = &source.objects[*id];
                        let saw = list(&map(object).expect("checked object")["saw"])
                            .expect("checked saw");
                        !saw.iter()
                            .any(|v| text(v).ok().is_some_and(|id| remaining.contains(id)))
                    })
                    .cloned()
                    .ok_or_else(|| error("node_migration_object_cycle"))?;
                remaining.remove(&next);
                let object = &source.objects[&next];
                let fields = map(object)?;
                let subject = text(&fields["subject"])?.to_owned();
                let saw = list(&fields["saw"])?
                    .iter()
                    .map(|v| text(v).map(str::to_owned))
                    .collect::<Result<BTreeSet<_>>>()?;
                let base_observation = saw
                    .iter()
                    .filter_map(|id| saw_by_id.get(id).map(|old| (id, old)))
                    .min_by_key(|(id, old)| {
                        (old.symmetric_difference(&saw).count(), (*id).clone())
                    });
                let observation = if let Some((base, previous)) = base_observation {
                    ObservationNode::between(&next, base, previous, &saw)?
                } else {
                    ObservationNode::root(&next, &saw)?
                };
                let raw = source
                    .object_bytes
                    .get(&(subject.clone(), next.clone()))
                    .ok_or_else(|| error("node_migration_missing_object"))?;
                let payload = N::payload_from_source(raw, &observation)?;
                let mut predecessors = frontier.get(&subject).cloned().unwrap_or_default();
                // Within one atomic operation, only objects actually observed by this
                // object become storage parents. Unobserved siblings remain concurrent.
                predecessors.retain(|event| {
                    versions[event].operation() != op
                        || event_for_id
                            .iter()
                            .find(|(_, value)| *value == event)
                            .is_some_and(|(id, _)| saw.contains(id))
                });
                prune(&mut predecessors, &event_ancestors);
                let parent_ids = predecessors.iter().cloned().collect::<Vec<_>>();
                let base = parent_ids.first().and_then(|id| versions.get(id));
                let event = if parent_ids.len() > 1 {
                    C::Event::create_merge(
                        &subject,
                        op,
                        parent_ids.clone(),
                        base.unwrap(),
                        Some(payload),
                    )?
                } else {
                    C::Event::create(&subject, op, parent_ids.clone(), base, Some(payload))?
                };
                let version = event.reconstruct(base)?;
                let mut ancestors = predecessors.clone();
                for id in &predecessors {
                    ancestors.extend(event_ancestors[id].iter().cloned());
                }
                event_ancestors.insert(event.id().into(), ancestors);
                frontier
                    .entry(subject.clone())
                    .or_default()
                    .insert(event.id().into());
                prune(frontier.get_mut(&subject).unwrap(), &event_ancestors);
                let frame = event.encode()?;
                touches.push((subject.clone(), event.id().into(), sha256(&frame)));
                frames.entry(subject).or_default().extend(frame);
                saw_by_id.insert(next.clone(), saw);
                event_for_id.insert(next.clone(), event.id().into());
                versions.insert(event.id().into(), version);
                object_count += 1;
            }
            let context = V::Map(Map::from([
                ("format".into(), s(crate::history_node_legacy::FORMAT)),
                (
                    "options".into(),
                    V::Map(Map::from([(
                        "original_manifest_sha256".into(),
                        s(&sha256(old_raw)),
                    )])),
                ),
                ("header".into(), V::Null),
                ("before".into(), V::Null),
                ("after".into(), V::Null),
            ]));
            let raw = P::migration_manifest(
                &authority_hash,
                op,
                parents,
                touches,
                None,
                &context,
                BTreeMap::new(),
            )?;
            digests.insert(op.clone(), manifest_digest(&raw)?);
            files.insert(format!(".kpopper/history-commits/{op}.json"), raw);
            frontiers.insert(op.clone(), frontier);
        }
        let checkpoint = format!("migration-{}", &archive_hash[..32]);
        require(
            !digests.contains_key(&checkpoint),
            "node_migration_operation_collision",
        )?;
        let checkpoint_parents = order
            .iter()
            .filter(|op| {
                !manifests.values().any(|v| {
                    map(v)
                        .ok()
                        .and_then(|m| map(&m["parents"]).ok())
                        .is_some_and(|p| p.contains_key(*op))
                })
            })
            .map(|op| (op.clone(), digests[op].clone()))
            .collect::<BTreeMap<_, _>>();
        let mut frontier = BTreeMap::<String, BTreeSet<String>>::new();
        for parent in checkpoint_parents.keys() {
            for (subject, ids) in &frontiers[parent] {
                frontier
                    .entry(subject.clone())
                    .or_default()
                    .extend(ids.iter().cloned());
            }
        }
        for ids in frontier.values_mut() {
            prune(ids, &event_ancestors);
        }
        let mut document = source.document.clone();
        map_mut(
            map_mut(&mut document)?
                .get_mut("meta")
                .ok_or_else(|| error("invalid_document"))?,
        )?
        .remove("history");
        let capabilities = crate::reasoning_fields::capabilities(&document, None)?;
        let profile = text(field(map(&capabilities)?, "profile")?)?;
        let sides = V::Map(Map::from([("document".into(), document.clone())]));
        let receipt = Receipt::pack(&crate::history_transaction::semantic_receipt(
            profile,
            &capabilities,
            &sides,
            &sides,
        )?)?;
        let mut nodes = Nodes::new();
        let before_changes = nodes.prepare(receipt.before())?;
        nodes.apply(&before_changes)?;
        let after_changes = nodes.prepare(receipt.after())?;
        let mut checkpoint_touches = Vec::new();
        let mut checkpoint_tail = BTreeMap::<String, Vec<u8>>::new();
        for (subject, parents) in &frontier {
            let base_id = parents
                .iter()
                .next()
                .ok_or_else(|| error("node_migration_missing_frontier"))?;
            let base = &versions[base_id];
            let semantic = Frame::decode(
                base.state()
                    .ok_or_else(|| error("node_semantic_absent_unsupported"))?,
            )?
            .semantic;
            let piece = nodes.values().get(subject);
            let needs_join = parents.len() > 1;
            if !needs_join && piece.is_none() {
                continue;
            }
            let value = Frame::encode(&semantic, "evidence", "before", piece)?;
            let ids = parents.iter().cloned().collect::<Vec<_>>();
            let event = if needs_join {
                C::Event::create_merge(subject, &checkpoint, ids, base, Some(value))?
            } else {
                C::Event::create(subject, &checkpoint, ids, Some(base), Some(value))?
            };
            let frame = event.encode()?;
            checkpoint_touches.push((subject.clone(), event.id().into(), sha256(&frame)));
            checkpoint_tail.insert(subject.clone(), frame.clone());
            frames.entry(subject.clone()).or_default().extend(frame);
        }
        require(after_changes.is_empty(), "node_migration_receipt_mismatch")?;
        let mut originals = Map::new();
        let mut tails = Map::new();
        let mut lazy = BTreeSet::new();
        for (subject, ids) in &frontier {
            if ids.len() != 1 {
                continue;
            }
            let id = ids.iter().next().unwrap();
            let version = &versions[id];
            if !version.parents().is_empty() {
                continue;
            }
            let Some((semantic_id, _)) = event_for_id.iter().find(|(_, event)| *event == id) else {
                continue;
            };
            let object = &source.objects[semantic_id];
            if string_is(&map(object)?["kind"], "act") {
                continue;
            }
            let payload = Frame::decode(
                version
                    .state()
                    .ok_or_else(|| error("node_semantic_absent_unsupported"))?,
            )?
            .semantic;
            let payload = map(&payload)?;
            let binding = Original::create(
                subject,
                text(&payload["collection"])?,
                version.operation(),
                payload["body"].clone(),
                map(&payload["context"])?.clone(),
            )?;
            let Ok(initial) = binding.from_document(&document) else {
                continue;
            };
            if initial.id() != id {
                continue;
            }
            let Some(tail) = checkpoint_tail.get(subject) else {
                continue;
            };
            originals.insert(subject.clone(), binding.encode()?);
            tails.insert(subject.clone(), s(&STANDARD.encode(tail)));
            lazy.insert(subject.clone());
        }
        map_mut(map_mut(&mut document)?.get_mut("meta").unwrap())?.insert(
            "node_history".into(),
            V::Map(Map::from([
                ("version".into(), V::from_json(&serde_json::json!(2))?),
                ("originals".into(), V::Map(originals)),
                ("tails".into(), V::Map(tails)),
            ])),
        );
        for (subject, bytes) in frames {
            if lazy.contains(&subject) {
                continue;
            }
            files.insert(
                format!(".kpopper/history/{}", C::subject_path(&subject)?),
                bytes,
            );
        }
        // The exact archive is the one source of old receipts and inactive generations.
        files.insert(archive_path.clone(), archive_bytes);
        let id_map = event_for_id
            .iter()
            .map(|(id, event)| {
                (
                    id.clone(),
                    serde_json::json!({"event":event,"status":"converted"}),
                )
            })
            .chain(
                source
                    .inactive_generations
                    .values()
                    .flat_map(|g| g.objects.keys())
                    .map(|id| (id.clone(), serde_json::json!({"status":"archived-only"}))),
            )
            .collect::<BTreeMap<_, _>>();
        let mapping = serde_json::json!({"format":"node-migration-map/v1","archive_sha256":archive_hash,
            "semantic_ids":id_map,"operations":digests});
        let mapping_raw = serde_json::to_vec(&mapping)?;
        let mapping_path = format!("evidence/migration/{checkpoint}.json");
        files.insert(mapping_path.clone(), mapping_raw);
        let mut evidence = BTreeMap::new();
        for path in [&archive_path, &mapping_path] {
            evidence.insert(path.clone(), sha256(&files[path]));
        }
        let retained_prefix = format!(
            "{}/",
            crate::history_transaction::Layout::for_entry(&source.layout.entry)?.retained
        );
        for (path, raw) in source_files
            .iter()
            .filter(|(path, _)| path.starts_with(&retained_prefix))
        {
            P::evidence_path(path)?;
            evidence.insert(path.clone(), sha256(raw));
            files.insert(path.clone(), raw.clone());
        }
        for (path, raw) in crate::history_hypothesis_authoring::physical_files(
            &crate::history_store::Store::new(entry)?,
        )? {
            evidence.insert(path.clone(), sha256(&raw));
            files.insert(path, raw);
        }
        let context = V::Map(Map::from([
            ("format".into(), s(crate::history_node_legacy::CHECKPOINT)),
            (
                "options".into(),
                V::Map(Map::from([(
                    "evidence".into(),
                    V::Map(evidence.iter().map(|(p, h)| (p.clone(), s(h))).collect()),
                )])),
            ),
            ("header".into(), receipt.header().clone()),
            ("before".into(), receipt.before().context().clone()),
            ("after".into(), receipt.after().context().clone()),
        ]));
        let after = P::bind_view(&Y::encode_document(&document)?, &checkpoint)?;
        let manifest = P::migration_manifest(
            &authority_hash,
            &checkpoint,
            checkpoint_parents,
            checkpoint_touches,
            Some(sha256(&after)),
            &context,
            evidence,
        )?;
        files.insert(
            format!(".kpopper/history-commits/{checkpoint}.json"),
            manifest,
        );
        files.insert("GROUNDING.yaml".into(), after);
        let summary = V::Map(Map::from([
            (
                "operations".into(),
                V::from_json(&serde_json::json!(order.len()))?,
            ),
            (
                "objects".into(),
                V::from_json(&serde_json::json!(object_count))?,
            ),
            ("archive_sha256".into(), s(&archive_hash)),
            ("checkpoint".into(), s(&checkpoint)),
        ]));
        Ok(Self {
            source,
            source_files,
            files,
            summary,
        })
    }

    pub fn summary(&self) -> &V {
        &self.summary
    }
    pub fn files(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.files
    }

    /// Publish into a new sibling directory only after both sides are checked.
    pub fn publish(&self, destination: &Path) -> Result<V> {
        require(!destination.exists(), "node_migration_destination_exists")?;
        self.source.verify_current()?;
        require(
            source_files(&self.source)? == self.source_files,
            "snapshot_changed",
        )?;
        let parent = destination.parent().ok_or_else(|| error("invalid_path"))?;
        let temporary = tempfile::Builder::new()
            .prefix(".node-migration-")
            .tempdir_in(parent)?;
        for (path, bytes) in &self.files {
            FS::publish_immutable(temporary.path(), path, bytes)?;
        }
        crate::history_node_capture::Capture::read(temporary.path())?;
        crate::history_node_physical::finish_copy(temporary.path())?;
        crate::history_node_capture::Capture::read(temporary.path())?;
        P::export(temporary.path())?;
        self.source.verify_current()?;
        require(
            source_files(&self.source)? == self.source_files,
            "snapshot_changed",
        )?;
        require(!destination.exists(), "node_migration_destination_exists")?;
        let held: PathBuf = temporary.keep();
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        let moved = rustix::fs::renameat_with(
            rustix::fs::CWD,
            &held,
            rustix::fs::CWD,
            destination,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(|e| crate::Error(format!("io: {e}")));
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let moved = fs::rename(&held, destination).map_err(crate::Error::from);
        if let Err(e) = moved {
            let _ = fs::remove_dir_all(&held);
            return Err(e);
        }
        Ok(self.summary.clone())
    }
}

fn manifest_digest(raw: &[u8]) -> Result<String> {
    Ok(serde_json::from_slice::<serde_json::Value>(raw)?["digest"]
        .as_str()
        .ok_or_else(|| error("node_migration_manifest_digest"))?
        .into())
}
fn prune(ids: &mut BTreeSet<String>, ancestors: &BTreeMap<String, BTreeSet<String>>) {
    let old = ids.clone();
    ids.retain(|id| {
        !old.iter()
            .any(|other| other != id && ancestors.get(other).is_some_and(|a| a.contains(id)))
    });
}
fn causal_order(manifests: &BTreeMap<String, V>) -> Result<Vec<String>> {
    let mut pending = manifests.keys().cloned().collect::<BTreeSet<_>>();
    let mut done = BTreeSet::new();
    let mut order = Vec::new();
    while !pending.is_empty() {
        let next = pending
            .iter()
            .find(|op| {
                map(&manifests[*op])
                    .ok()
                    .and_then(|m| map(&m["parents"]).ok())
                    .is_some_and(|p| p.keys().all(|parent| done.contains(parent)))
            })
            .cloned()
            .ok_or_else(|| error("node_migration_causal_cycle"))?;
        pending.remove(&next);
        done.insert(next.clone());
        order.push(next);
    }
    Ok(order)
}
fn source_files(source: &crate::history_capture::Capture) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut files = BTreeMap::new();
    files.insert(source.layout.entry.clone(), source.entry_bytes.clone());
    files.insert(
        source.layout.authority.clone(),
        source.authority_bytes.clone(),
    );
    for (op, raw) in &source.commits {
        files.insert(
            format!("{}/{}.yaml", source.layout.commits, op),
            raw.clone(),
        );
    }
    for generation in source.inactive_generations.values() {
        for (op, raw) in &generation.commits {
            files.insert(
                format!("{}/{}.yaml", source.layout.commits, op),
                raw.clone(),
            );
        }
    }
    for (path, raw) in &source.storage_bytes {
        files.insert(format!("{}/{}", source.layout.objects, path), raw.clone());
    }
    for (name, raw) in &source.cancellation_bytes {
        files.insert(
            format!("{}/{}", source.layout.cancellations, name),
            raw.clone(),
        );
    }
    let layout = crate::history_transaction::Layout::for_entry(&source.layout.entry)?;
    for path in [&layout.view, &layout.replaced] {
        if let Some(raw) = FS::read(&FS::target(&source.root, path)?)? {
            files.insert(path.clone(), raw);
        }
    }
    for (path, raw) in crate::history_hypothesis_authoring::physical_files(
        &crate::history_store::Store::new(&source.root.join(&source.layout.entry))?,
    )? {
        files.insert(path, raw);
    }
    collect_files(&source.root, &layout.retained, &mut files)?;
    Ok(files)
}
fn collect_files(root: &Path, relative: &str, files: &mut BTreeMap<String, Vec<u8>>) -> Result<()> {
    let path = FS::target(root, relative)?;
    if !path.exists() {
        return Ok(());
    }
    require(path.is_dir(), "node_migration_retained_path")?;
    for member in fs::read_dir(path)? {
        let member = member?;
        let name = member
            .file_name()
            .into_string()
            .map_err(|_| error("invalid_path"))?;
        let child = format!("{relative}/{name}");
        let kind = member.file_type()?;
        require(!kind.is_symlink(), "symlink_path")?;
        if kind.is_dir() {
            collect_files(root, &child, files)?;
        } else if kind.is_file() {
            files.insert(child, fs::read(member.path())?);
        } else {
            return Err(unsupported("node_migration_retained_path"));
        }
    }
    Ok(())
}
