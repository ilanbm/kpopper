//! Sealed-copy validation and inverse export; no live-record authority is granted.
use crate::{
    Result, history_adapter as Adapter,
    history_authoring::{empty, obj, s},
    history_authority as A,
    history_contract::*,
    history_migration::{ARTIFACTS, entry_identity, hypothesis_identity},
    history_migration_source as S,
    history_store::Store,
    history_transaction as T, history_transaction_fs as F,
    history_view::list,
    history_yaml as Y,
    identity::sha256,
    reasoning_snapshot::{Snapshot, history_bytes},
    require,
    source_capture::{ReadMode, capture_source},
    source_inventory::{absolute, name},
    value::TypedValue as V,
};
use std::{
    collections::BTreeSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
};
pub(crate) fn receipt_path(path: &str) -> bool {
    path == format!("{ARTIFACTS}/receipt.json")
        || path.starts_with(&format!("{ARTIFACTS}/generations/"))
            && path.matches('/').count() == 3
            && path.ends_with("/receipt.json")
}
fn hash(v: &V) -> Result<()> {
    let t = text(v)?;
    require(
        t.len() == 64
            && t.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid_migration_receipt",
    )
}
fn artifact_root(receipt: &V) -> Result<&str> {
    map(receipt)?
        .get("artifact_root")
        .map(text)
        .transpose()
        .map(|v| v.unwrap_or(ARTIFACTS))
}
pub(crate) fn copy_receipt(raw: &[u8]) -> Result<V> {
    let value = T::parse_journal(raw, "invalid_migration_receipt")?;
    require(value.canonical_bytes()? == raw, "invalid_migration_receipt")?;
    let m = schema(
        &value,
        &[
            "version",
            "kind",
            "operation",
            "record",
            "record_id",
            "complete",
            "problems",
            "source_snapshot_id",
            "candidate_snapshot_id",
            "locators",
            "originals",
            "destination",
        ],
        &["topology", "observation", "inverse", "artifact_root"],
    )?;
    let version = if is_int(&m["version"], "1") {
        1
    } else if is_int(&m["version"], "2") {
        2
    } else {
        return Err(error("incomplete_history_import"));
    };
    require(
        string_is(&m["kind"], &format!("history-import/v{version}"))
            && m["complete"] == V::Bool(true)
            && m["problems"] == V::List(vec![]),
        "incomplete_history_import",
    )?;
    require(
        !text(&m["operation"])?.is_empty() && !text(&m["record_id"])?.is_empty(),
        "invalid_migration_receipt",
    )?;
    let record = text(&m["record"])?;
    A::relative_path(record)?;
    require(!record.contains('/'), "invalid_migration_receipt")?;
    let dest = map(&m["destination"])?;
    let originals = map(&m["originals"])?;
    require(originals.contains_key(record), "invalid_migration_receipt")?;
    for (path, digest) in dest {
        A::relative_path(path)?;
        hash(digest)?;
    }
    let artifacts = artifact_root(&value)?;
    A::relative_path(artifacts)?;
    let generation = artifacts.strip_prefix(&format!("{ARTIFACTS}/generations/"));
    require(
        artifacts == ARTIFACTS
            || generation.is_some_and(|v| {
                !v.is_empty()
                    && v.bytes().all(|b| b.is_ascii_digit())
                    && v.parse::<u64>().is_ok_and(|v| v > 1)
            }),
        "invalid_migration_receipt",
    )?;
    require(
        !dest.contains_key(&format!("{artifacts}/receipt.json")),
        "invalid_migration_receipt",
    )?;
    for (name, v) in originals {
        A::relative_path(name)?;
        let item = if version == 1 {
            schema(v, &["path", "sha256"], &[])?
        } else {
            schema(v, &["path", "sha256", "storage"], &[])?
        };
        let storage = item.get("storage").map(text).transpose()?.unwrap_or("copy");
        require(
            storage == "copy" && string_is(&item["path"], &format!("{artifacts}/originals/{name}"))
                || storage == "retained"
                    && version == 2
                    && string_is(&item["path"], name)
                    && S::immutable(name, record, artifacts)?,
            "invalid_original_mapping",
        )?;
        hash(&item["sha256"])?;
        require(
            dest.get(text(&item["path"])?) == Some(&item["sha256"]),
            "invalid_original_mapping",
        )?;
    }
    if let Some(topology) = m.get("topology").filter(|v| **v != V::Null) {
        let t = schema(topology, &["version", "entry", "originals"], &[])?;
        let paths = map(&t["originals"])?;
        require(
            is_int(&t["version"], "1") && paths.keys().eq(originals.keys()),
            "invalid_original_topology",
        )?;
        A::relative_path(text(&t["entry"])?)?;
        let mut seen = BTreeSet::new();
        for path in paths.values() {
            let path = text(path)?;
            A::relative_path(path)?;
            require(seen.insert(path), "invalid_original_topology")?;
        }
        for path in &seen {
            for (index, _) in path.match_indices('/') {
                require(!seen.contains(&path[..index]), "invalid_original_topology")?;
            }
        }
        require(
            paths.get(record) == Some(&t["entry"]),
            "invalid_original_topology",
        )?;
    }
    if let Some(inverse) = m.get("inverse").filter(|v| **v != V::Null) {
        let i = schema(inverse, &["representable", "reason", "members"], &[])?;
        let members = list(&i["members"])?;
        let names = members.iter().map(text).collect::<Result<Vec<_>>>()?;
        require(
            i["representable"] == V::Bool(false)
                && string_is(&i["reason"], "absolute_original_pointer")
                && !names.is_empty()
                && names.windows(2).all(|p| p[0] < p[1])
                && names.iter().all(|p| originals.contains_key(*p)),
            "invalid_inverse_constraint",
        )?;
    }
    if let Some(observation) = m.get("observation").filter(|v| **v != V::Null) {
        let o = schema(
            observation,
            &["read_mode", "path", "snapshot_id", "sha256"],
            &[],
        )?;
        require(
            string_is(&o["read_mode"], "live")
                && string_is(&o["path"], &format!("{artifacts}/live.json")),
            "invalid_migration_observation",
        )?;
        hash(&o["snapshot_id"])?;
        hash(&o["sha256"])?;
        require(
            dest.get(text(&o["path"])?) == Some(&o["sha256"]),
            "invalid_migration_observation",
        )?;
    }
    Ok(value)
}
pub(crate) fn inventory(root: &Path) -> Result<A::Files> {
    S::no_link(root)?;
    let root = root.canonicalize()?;
    require(root.is_dir(), "migration_copy_unavailable")?;
    let mut result = A::Files::new();
    let mut pending = vec![root.clone()];
    let mut total = 0usize;
    let mut visits = 0;
    while let Some(dir) = pending.pop() {
        for item in fs::read_dir(dir)? {
            let path = item?.path();
            visits += 1;
            require(visits <= MAX_OBJECTS, "history_limit")?;
            S::no_link(&path)?;
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            require(
                path.is_file() && result.len() < MAX_OBJECTS,
                "history_limit",
            )?;
            let relative = S::posix(path.strip_prefix(&root).unwrap())?;
            require(
                F::target(&root, &relative)? == path,
                "invalid_migration_path",
            )?;
            let file = fs::File::open(&path)?;
            require(
                file.metadata()?.len() <= T::MAX_TRANSACTION_BYTES as u64,
                "history_limit",
            )?;
            let mut raw = vec![];
            file.take((T::MAX_TRANSACTION_BYTES + 1) as u64)
                .read_to_end(&mut raw)?;
            total = total.saturating_add(raw.len());
            require(total <= T::MAX_TRANSACTION_BYTES, "history_limit")?;
            result.insert(relative, raw);
        }
    }
    Ok(result)
}
fn copy_artifacts(files: &A::Files) -> Result<String> {
    let mut candidates = vec![];
    for (path, raw) in files {
        if !receipt_path(path) {
            continue;
        }
        let receipt = copy_receipt(raw)?;
        let entry = text(&map(&receipt)?["record"])?;
        let layout = T::Layout::for_entry(entry)?;
        if let Some(raw) = files.get(&layout.authority) {
            let marker = Y::decode_document(raw)?;
            A::validate_authority(&marker)?;
            let m = map(&marker)?;
            let generation = match &m["generation"] {
                V::Integer(v) => v.as_str(),
                _ => return Err(error("invalid_authority")),
            };
            let expected = if generation == "1" {
                ARTIFACTS.into()
            } else {
                format!("{ARTIFACTS}/generations/{generation}")
            };
            if string_is(&m["authority"], "history")
                && *path == format!("{expected}/receipt.json")
                && artifact_root(&receipt)? == expected
            {
                candidates.push(expected);
            }
        }
    }
    require(candidates.len() == 1, "missing_migration_receipt")?;
    Ok(candidates.remove(0))
}
pub(crate) fn publish_tree(
    destination: &Path,
    files: &A::Files,
    verify: &mut dyn FnMut(&Path) -> Result<()>,
) -> Result<PathBuf> {
    let destination = absolute(destination)?;
    require(
        !destination.exists() && destination.symlink_metadata().is_err(),
        "migration_destination_exists",
    )?;
    let parent = destination.parent().ok_or_else(|| error("invalid_path"))?;
    fs::create_dir_all(parent)?;
    let parent = parent.canonicalize()?;
    let destination = parent.join(
        destination
            .file_name()
            .ok_or_else(|| error("invalid_path"))?,
    );
    let temp = tempfile::Builder::new()
        .prefix(".kpopper-import-")
        .tempdir_in(&parent)?;
    for (name, raw) in files {
        let target = F::target(temp.path(), name)?;
        fs::create_dir_all(target.parent().unwrap())?;
        fs::write(target, raw)?;
    }
    verify(temp.path())?;
    rename_absent(temp.path(), &destination)?;
    Ok(destination)
}
fn rename_absent(source: &Path, destination: &Path) -> Result<()> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        rustix::fs::renameat_with(
            rustix::fs::CWD,
            source,
            rustix::fs::CWD,
            destination,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(std::io::Error::from)?;
        Ok(())
    }
    #[cfg(windows)]
    {
        require(
            destination.symlink_metadata().is_err(),
            "migration_destination_exists",
        )?;
        fs::rename(source, destination)?;
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        let _ = (source, destination);
        Err(error("atomic_absent_publication_unavailable"))
    }
}
pub(crate) fn validate_pending(snapshot: &Snapshot) -> Result<()> {
    let data = snapshot.to_data();
    let context = map(&map(&data)?["context"])?;
    let pending = context.get("pending").cloned().unwrap_or_else(empty);
    let p = map(&pending)?;
    let bundles = p.get("bundles").cloned().unwrap_or_else(empty);
    let bundles = map(&bundles)?;
    let events = p.get("events").cloned().unwrap_or(V::List(vec![]));
    let events = list(&events)?;
    let mut revisions = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for event in events {
        let m = map(event).map_err(|_| error("invalid_pending_observation"))?;
        let revision = m
            .get("revision")
            .and_then(|v| text(v).ok())
            .ok_or_else(|| error("invalid_pending_observation"))?;
        let id = m
            .get("event_id")
            .and_then(|v| text(v).ok())
            .ok_or_else(|| error("invalid_pending_observation"))?;
        require(
            bundles.contains_key(revision) && ids.insert(id),
            "invalid_pending_observation",
        )?;
        revisions.insert(revision);
    }
    require(
        revisions
            .iter()
            .copied()
            .eq(bundles.keys().map(String::as_str)),
        "invalid_pending_observation",
    )?;
    for (revision, portable) in bundles {
        let m = map(portable)?;
        require(
            m.get("revision") == Some(&s(revision)),
            "invalid_pending_revision",
        )?;
        let files = history_bytes(field(m, "files")?)?;
        crate::pending_bundle::validate(
            &obj([
                ("revision", m["revision"].clone()),
                ("manifest", field(m, "manifest")?.clone()),
            ]),
            &files,
        )?;
    }
    Ok(())
}
struct Copy {
    files: A::Files,
    artifacts: String,
    receipt: V,
    candidate: Snapshot,
    original: Snapshot,
}
impl Copy {
    fn open(root: &Path) -> Result<Self> {
        let files = inventory(root)?;
        let artifacts = copy_artifacts(&files)?;
        let receipt_name = format!("{artifacts}/receipt.json");
        let receipt = copy_receipt(&files[&receipt_name])?;
        let r = map(&receipt)?;
        let expected = map(&r["destination"])?;
        require(
            files.len() == expected.len() + 1 && expected.keys().all(|k| files.contains_key(k)),
            "migration_inventory_changed",
        )?;
        for (path, digest) in expected {
            require(
                text(digest)? == sha256(&files[path]),
                "migration_bytes_changed",
            )?;
        }
        let store = Store::new(&root.join(text(&r["record"])?))?;
        let capture = store.capture()?;
        require(
            capture.commits.len() == 1
                && capture.commits.contains_key(text(&r["operation"])?)
                && map(&capture.marker)?["record_id"] == r["record_id"],
            "newer_history_not_representable",
        )?;
        let commit = Y::decode_document(&capture.commits[text(&r["operation"])?])?;
        let c = map(&commit)?;
        require(
            c["parents"] == empty() && store.render(&capture)? == files[text(&r["record"])?],
            "migration_view_mismatch",
        )?;
        let import = T::validate_receipt(&c["receipt"])?;
        let im = map(&import)?;
        let source = im["before"].clone();
        let b = map(&source)?;
        let a = map(&im["after"])?;
        let originals = map(&r["originals"])?;
        let sources = V::Map(
            originals
                .iter()
                .map(|(n, v)| Ok((n.clone(), map(v)?["sha256"].clone())))
                .collect::<Result<_>>()?,
        );
        require(
            b.get("kind") == Some(&s("legacy-import/v1")) && b.get("source") == Some(&sources),
            "invalid_original_mapping",
        )?;
        if is_int(&r["version"], "2") {
            require(
                b.get("originals_storage") == Some(&r["originals"]),
                "invalid_original_mapping",
            )?;
        }
        require(
            a.get("locators") == Some(&r["locators"])
                && a.get("operation") == Some(&r["operation"]),
            "invalid_migration_receipt",
        )?;
        for key in ["topology", "observation", "inverse"] {
            require(b.get(key) == r.get(key), "invalid_original_mapping")?;
        }
        require(
            b.get("artifact_root")
                .map(text)
                .transpose()?
                .unwrap_or(ARTIFACTS)
                == artifacts,
            "invalid_original_mapping",
        )?;
        let original = Snapshot::from_json(
            files
                .get(&format!("{artifacts}/original.json"))
                .ok_or_else(|| error("missing_original_snapshot"))?,
        )?;
        let candidate = Snapshot::from_json(
            files
                .get(&format!("{artifacts}/candidate.json"))
                .ok_or_else(|| error("missing_candidate_snapshot"))?,
        )?;
        require(
            r["source_snapshot_id"] == s(original.snapshot_id())
                && b.get("snapshot_id") == Some(&s(original.snapshot_id()))
                && r["candidate_snapshot_id"] == s(candidate.snapshot_id()),
            "migration_replay_mismatch",
        )?;
        let adapted = Adapter::from_store_capture(&capture)?;
        require(
            adapted.document().digest()? == map(&candidate.to_data())?["document"].digest()?
                && entry_identity(adapted.document())?
                    == entry_identity(&map(&original.to_data())?["document"])?,
            "migration_body_mismatch",
        )?;
        let template = map(field(c, "view_template")?)?;
        let meta = map(field(template, "meta")?)?;
        let import = map(field(meta, "history_import")?)?;
        let members = list(field(import, "members")?)?;
        let binding = members
            .iter()
            .filter_map(|v| map(v).ok())
            .filter(|m| m.get("role") == Some(&s("retained_original")))
            .map(|m| {
                Ok((
                    text(field(m, "path")?)?.to_string(),
                    field(m, "sha256")?.clone(),
                ))
            })
            .collect::<Result<Map>>()?;
        let original_path = format!("{artifacts}/original.json");
        require(
            binding.get(&original_path) == Some(&s(&sha256(&files[&original_path])))
                && originals.values().all(|v| {
                    map(v).is_ok_and(|m| {
                        text(&m["path"]).is_ok_and(|p| binding.get(p) == Some(&m["sha256"]))
                    })
                }),
            "invalid_original_mapping",
        )?;
        require(inventory(root)? == files, "migration_copy_changed")?;
        Ok(Self {
            files,
            artifacts,
            receipt,
            candidate,
            original,
        })
    }
}
pub fn restore_from_copy(copied: &Path, destination: &Path) -> Result<V> {
    let copied = copied.canonicalize()?;
    require(
        !destination.exists() && destination.symlink_metadata().is_err(),
        "migration_destination_exists",
    )?;
    let _guard = F::DirectoryGuard::acquire(&copied, false)?;
    let copy = Copy::open(&copied)?;
    let r = map(&copy.receipt)?;
    require(!r.contains_key("inverse"), "nonrepresentable_inverse")?;
    let topology = r.get("topology").map(map).transpose()?;
    let originals = map(&r["originals"])?;
    let paths = topology.map(|t| map(&t["originals"])).transpose()?;
    let restored_entry = topology
        .map(|t| text(&t["entry"]))
        .transpose()?
        .unwrap_or(text(&r["record"])?);
    let files = originals
        .iter()
        .map(|(n, v)| {
            let path = text(&map(v)?["path"])?;
            let dest = paths.map(|p| text(&p[n])).transpose()?.unwrap_or(n);
            Ok((dest.into(), copy.files[path].clone()))
        })
        .collect::<Result<A::Files>>()?;
    let as_of = map(&copy.original.to_data())?["as_of"].clone();
    let publish = || {
        publish_tree(destination, &files, &mut |root| {
            require(
                inventory(&copied)? == copy.files && inventory(root)? == files,
                "migration_copy_changed",
            )?;
            // Check original topology before the source reader can follow pointers.
            let mut pending = vec![root.join(restored_entry)];
            let mut visited = BTreeSet::new();
            while let Some(path) = pending.pop() {
                let path = absolute(&path)?;
                require(path.starts_with(root), "nonrelocatable_original_pointer")?;
                if !visited.insert(path.clone()) {
                    continue;
                }
                require(visited.len() <= MAX_OBJECTS, "history_limit")?;
                let raw = fs::read(&path)?;
                for pointer in S::pointer_values(&S::parse(&raw)?) {
                    pending.push(path.parent().unwrap().join(pointer));
                }
            }
            let capture = capture_source(
                &[root.join(restored_entry)],
                root,
                ReadMode::Frozen,
                Some(as_of.clone()),
            )?;
            require(
                entry_identity(&capture.strict_document()?)?
                    == entry_identity(&map(&copy.original.to_data())?["document"])?
                    && hypothesis_identity(&map(&capture.snapshot()?.to_data())?["hypotheses"])?
                        == hypothesis_identity(&map(&copy.original.to_data())?["hypotheses"])?,
                "migration_replay_mismatch",
            )?;
            require(inventory(&copied)? == copy.files, "migration_copy_changed")
        })
    };
    let root = std::thread::scope(|scope| scope.spawn(publish).join())
        .map_err(|_| error("migration_copy_worker_failed"))??;
    Ok(obj([
        ("state", s("restored_copy")),
        ("record", s(name(&root.join(restored_entry))?)),
        ("source_snapshot_id", s(copy.original.snapshot_id())),
        (
            "copy_receipt_sha256",
            s(&sha256(
                &copy.files[&format!("{}/receipt.json", copy.artifacts)],
            )),
        ),
    ]))
}
pub fn replay_from_copy(copied: &Path) -> Result<Snapshot> {
    let copied = copied.canonicalize()?;
    let _guard = F::DirectoryGuard::acquire(&copied, false)?;
    let copy = Copy::open(&copied)?;
    validate_pending(&copy.candidate)?;
    if let Some(observation) = map(&copy.receipt)?.get("observation") {
        let o = map(observation)?;
        let observed = Snapshot::from_json(&copy.files[text(&o["path"])?])?;
        validate_pending(&observed)?;
        require(
            o["snapshot_id"] == s(observed.snapshot_id()),
            "migration_replay_mismatch",
        )?;
        let actual = copy.candidate.to_data();
        let expected = observed.to_data();
        let a = map(&actual)?;
        let e = map(&expected)?;
        require(
            hypothesis_identity(&a["hypotheses"])? == hypothesis_identity(&e["hypotheses"])?,
            "migration_context_mismatch",
        )?;
        let ac = map(&a["context"])?;
        let ec = map(&e["context"])?;
        for key in [
            "pending",
            "target",
            "project",
            "history_contributions",
            "conflicts",
        ] {
            require(ac.get(key) == ec.get(key), "migration_context_mismatch")?;
        }
    }
    require(inventory(&copied)? == copy.files, "migration_copy_changed")?;
    Ok(copy.candidate)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sealed_inventory_uses_portable_relative_names() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("nested")).unwrap();
        fs::write(temp.path().join("nested").join("reading.yaml"), b"known: {}\n").unwrap();
        let files = inventory(temp.path()).unwrap();
        assert_eq!(files, A::Files::from([("nested/reading.yaml".into(), b"known: {}\n".to_vec())]));
    }

    #[test]
    fn publication_never_replaces_a_destination_created_during_validation() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("copy");
        let files = A::Files::from([("original.txt".into(), b"source".to_vec())]);
        let result = publish_tree(&destination, &files, &mut |_| {
            fs::create_dir(&destination)?;
            Ok(())
        });
        assert!(result.is_err());
        assert!(destination.is_dir());
        assert!(fs::read_dir(&destination).unwrap().next().is_none());
    }
}
