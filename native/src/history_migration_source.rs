//! Additional bounded observations required by a lossless history import.
use crate::{
    Result, history_authority as A,
    history_contract::*,
    history_migration_copy as Copy, history_transaction as T,
    history_view::truth,
    history_yaml as Y,
    identity::sha256,
    project_modes::Project,
    require,
    source_capture::CapturedSource,
    source_inventory::{Inventory, Observation, absolute, escaped, name},
    value::TypedValue as V,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
pub(crate) const ARTIFACTS: &str = ".kpopper-history-migration";

pub(crate) fn adjunct(entry: &str, role: &str) -> String {
    let layout = T::Layout::for_entry(entry).expect("validated entry");
    match role {
        "view" => layout.view,
        "hypotheses" => layout.hypotheses,
        "replaced" => layout.replaced,
        "measure" | "session" => format!(
            "{}{}.{}",
            if entry == "GROUNDING.yaml" {
                ".kpopper/"
            } else {
                "PROVENANCE."
            },
            role,
            if role == "session" { "json" } else { "yaml" }
        ),
        _ => role.into(),
    }
}
pub(crate) fn posix(path: &Path) -> Result<String> {
    path.iter()
        .map(|part| name(Path::new(part)).map(str::to_string))
        .collect::<Result<Vec<_>>>()
        .map(|parts| parts.join("/"))
}
pub(crate) fn portable(record: &Path, path: &Path) -> Result<String> {
    let path = absolute(path)?;
    Ok(match path.strip_prefix(record.parent().unwrap()) {
        Ok(relative) => posix(relative)?,
        Err(_) => format!(
            "_external/{}/{}",
            &sha256(name(&path)?.as_bytes())[..24],
            name(Path::new(
                path.file_name().ok_or_else(|| error("invalid_path"))?
            ))?
        ),
    })
}
pub(crate) fn parse(raw: &[u8]) -> Result<V> {
    let value = Y::decode_source_value(raw)?.typed();
    let value = if truth(&value) {
        value
    } else {
        V::Map(Map::new())
    };
    map(&value).map_err(|_| error("invalid_migration_record"))?;
    Ok(value)
}
pub(crate) fn no_link(path: &Path) -> Result<()> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        require(!meta.file_type().is_symlink(), "migration_symlink")?;
    }
    Ok(())
}
fn tree(inventory: &mut Inventory, root: &Path, hidden: bool, flat: bool) -> Result<()> {
    let mut pending = vec![root.to_path_buf()];
    let mut visits = 0;
    while let Some(directory) = pending.pop() {
        no_link(&directory)?;
        require(inventory.directory(&directory)?, "invalid_history_path")?;
        let mut found = inventory.glob(&escaped(&directory)?.join("*"))?;
        if hidden {
            found.extend(inventory.glob(&escaped(&directory)?.join(".*"))?);
        }
        found.sort();
        found.dedup();
        for path in found {
            visits += 1;
            require(visits <= MAX_OBJECTS, "history_limit")?;
            no_link(&path)?;
            if inventory.directory(&path)? {
                require(!flat, "cancellation_membership_mismatch")?;
                pending.push(path);
            } else {
                require(path.is_file(), "invalid_history_path")?;
                inventory.read(&path)?;
            }
        }
    }
    Ok(())
}
fn references(value: &V, out: &mut BTreeSet<String>) {
    match value {
        V::Map(m) => {
            if let Some(V::Text(file)) = m.get("file") {
                out.insert(file.clone());
            }
            for v in m.values() {
                references(v, out);
            }
        }
        V::List(a) => {
            for v in a {
                references(v, out);
            }
        }
        _ => {}
    }
}
/// Record-local evidence files named by `file:` in the record or a physical hypothesis.
/// Absolute, remote and parent-relative references are not record members.
pub(crate) fn referenced_evidence(source: &CapturedSource) -> Result<BTreeSet<String>> {
    let mut evidence = BTreeSet::new();
    references(&source.strict_document()?, &mut evidence);
    for hyp in map(source.hypotheses())?.values() {
        let h = map(hyp)?;
        if !h.get("kind").is_some_and(|v| string_is(v, "contribution"))
            && let Some(doc) = h.get("document").or_else(|| h.get("doc"))
        {
            references(doc, &mut evidence);
        }
    }
    evidence.retain(|file| {
        let p = Path::new(file);
        !(p.is_absolute()
            || file.contains("://")
            || p.components().any(|v| v == std::path::Component::ParentDir))
    });
    Ok(evidence)
}
pub(crate) fn extend(
    source: &CapturedSource,
    record: &Path,
    project: &Project,
) -> Result<Inventory> {
    let mut inventory = source.inventory().clone();
    let entry = name(Path::new(
        record.file_name().ok_or_else(|| error("invalid_path"))?,
    ))?;
    let root = record.parent().unwrap();
    let layout = T::Layout::for_entry(entry)?;
    let other = if entry == "GROUNDING.yaml" {
        "PROVENANCE.yaml"
    } else {
        "GROUNDING.yaml"
    };
    let alternate = T::Layout::for_entry(other)?;
    let optional: BTreeSet<_> = source
        .files()
        .keys()
        .chain(std::iter::once(&record.to_path_buf()))
        .map(|p| {
            let e = p.file_name().and_then(|v| v.to_str()).unwrap_or("");
            T::Layout::for_entry(e).map(|l| p.parent().unwrap().join(l.authority))
        })
        .collect::<Result<_>>()?;
    for ((kind, path), value) in &inventory.events {
        require(
            !(kind == "exists" && *value == Observation::Exists(false) && !optional.contains(path)),
            "missing_migration_source",
        )?;
    }
    for role in ["view", "measure", "session", "replaced", "hypotheses"] {
        let other_path = root.join(adjunct(other, role));
        let exists = inventory.exists(&other_path)?;
        if role == "hypotheses" {
            for extension in ["*.yaml", "*.yml"] {
                require(
                    inventory
                        .glob(&escaped(&other_path)?.join(extension))?
                        .is_empty(),
                    "half_moved_record_layout",
                )?;
                inventory.glob(&escaped(&root.join(adjunct(entry, role)))?.join(extension))?;
            }
        } else {
            require(!exists, "half_moved_record_layout")?;
            let path = root.join(adjunct(entry, role));
            if inventory.exists(&path)? {
                inventory.read(&path)?;
            }
        }
    }
    for file in referenced_evidence(source)? {
        let p = Path::new(&file);
        let relative = p
            .components()
            .filter(|c| *c != std::path::Component::CurDir)
            .collect::<PathBuf>();
        crate::history_branch::portable_path(&posix(&relative)?)?;
        require(
            !p.components().any(|v| v.as_os_str() == ".git")
                && !file.starts_with(".kpopper-core-migration/"),
            "invalid_evidence_locator",
        )?;
        let path = root.join(p);
        require(
            inventory.exists(&path)? && path.is_file(),
            "missing_referenced_evidence",
        )?;
        require(path.canonicalize()?.starts_with(root), "evidence_escape")?;
        inventory.read(&path)?;
    }
    let mut admin = vec![
        project.config_path.clone(),
        project.state.join("publication.json"),
    ];
    if let Some(common) = &project.common {
        admin.push(common.join("kpopper-record"));
    }
    for path in admin {
        if inventory.exists(&path)? {
            inventory.observe_bytes(&path)?;
        }
    }
    for (role, other_role, is_authority, is_cancel) in [
        (&layout.objects, &alternate.objects, false, false),
        (&layout.commits, &alternate.commits, false, false),
        (&layout.cancellations, &alternate.cancellations, false, true),
        (&layout.authority, &alternate.authority, true, false),
    ] {
        require(
            !inventory.exists(&root.join(other_role))?,
            "half_moved_history_layout",
        )?;
        let path = root.join(role);
        if !inventory.exists(&path)? {
            continue;
        }
        if is_authority {
            let marker = Y::decode_document(&inventory.read(&path)?)?;
            A::validate_authority(&marker)?;
            require(
                string_is(&map(&marker)?["authority"], "legacy"),
                "already_active_history",
            )?;
        } else {
            if is_cancel {
                require(
                    inventory.glob(&escaped(&path)?.join(".*"))?.is_empty(),
                    "cancellation_membership_mismatch",
                )?;
            }
            tree(&mut inventory, &path, false, is_cancel)?;
        }
    }
    let files = relative_files(&inventory.files, root)?;
    let (commits, storage, cancellations) = retained(&files, entry)?;
    if !commits.is_empty() {
        let marker = Y::decode_document(
            files
                .get(&layout.authority)
                .ok_or_else(|| error("missing_retained_history_authority"))?,
        )?;
        let (objects, _) = A::objects_from_storage(&commits, &storage)?;
        A::committed_generations(&marker, &commits, &objects, &cancellations)?;
    } else {
        require(cancellations.is_empty(), "cancellation_membership_mismatch")?;
    }
    let artifacts = root.join(ARTIFACTS);
    if inventory.exists(&artifacts)? {
        let marker = Y::decode_document(
            files
                .get(&layout.authority)
                .ok_or_else(|| error("migration_artifact_collision"))?,
        )?;
        require(
            string_is(&map(&marker)?["authority"], "legacy"),
            "migration_artifact_collision",
        )?;
        tree(&mut inventory, &artifacts, true, false)?;
        let files = relative_files(&inventory.files, root)?;
        let retained: BTreeMap<_, _> = files
            .iter()
            .filter(|(p, _)| p.starts_with(&format!("{ARTIFACTS}/")))
            .collect();
        let mut known = BTreeSet::new();
        for (path, raw) in &retained {
            if !Copy::receipt_path(path) {
                continue;
            }
            let receipt = Copy::copy_receipt(raw)?;
            known.insert((*path).clone());
            for (member, digest) in map(&map(&receipt)?["destination"])? {
                if member.starts_with(&format!("{ARTIFACTS}/")) {
                    require(
                        retained
                            .get(member)
                            .is_some_and(|raw| text(digest).ok() == Some(&sha256(raw))),
                        "retained_artifact_mismatch",
                    )?;
                    known.insert(member.clone());
                }
            }
        }
        require(
            retained.keys().all(|k| known.contains(*k)),
            "unknown_retained_artifact",
        )?;
    }
    inventory.verify()?;
    source.verify()?;
    Ok(inventory)
}
pub(crate) fn relative_files(files: &BTreeMap<PathBuf, Vec<u8>>, root: &Path) -> Result<A::Files> {
    files
        .iter()
        .filter_map(|(p, b)| {
            p.strip_prefix(root)
                .ok()
                .map(|p| posix(p).map(|n| (n, b.clone())))
        })
        .collect()
}
pub(crate) fn retained(files: &A::Files, entry: &str) -> Result<(A::Files, A::Files, A::Files)> {
    let l = T::Layout::for_entry(entry)?;
    let select = |prefix: &str, stem: bool| -> Result<A::Files> {
        files
            .iter()
            .filter_map(|(p, b)| {
                p.strip_prefix(&format!("{prefix}/")).map(|n| {
                    let n = if stem {
                        Path::new(n)
                            .file_stem()
                            .and_then(|n| n.to_str())
                            .ok_or_else(|| error("invalid_path"))?
                    } else {
                        n
                    };
                    Ok((n.into(), b.clone()))
                })
            })
            .collect()
    };
    Ok((
        select(&l.commits, true)?,
        select(&l.objects, false)?,
        select(&l.cancellations, false)?,
    ))
}
pub(crate) fn immutable(name: &str, entry: &str, artifacts: &str) -> Result<bool> {
    let l = T::Layout::for_entry(entry)?;
    Ok(
        name.starts_with(&format!("{ARTIFACTS}/")) && !name.starts_with(&format!("{artifacts}/"))
            || [&l.objects, &l.commits, &l.cancellations]
                .iter()
                .any(|p| name.starts_with(&format!("{p}/"))),
    )
}
pub(crate) fn pointer_values(value: &V) -> Vec<String> {
    let mut out = vec![];
    fn visit(v: &V, out: &mut Vec<String>) {
        match v {
            V::Text(t) if t.ends_with(".yaml") || t.ends_with(".yml") => out.push(t.clone()),
            V::Map(m) => {
                for v in m.values() {
                    visit(v, out)
                }
            }
            V::List(a) => {
                for v in a {
                    visit(v, out)
                }
            }
            _ => {}
        }
    }
    if let Ok(m) = map(value) {
        for k in ["record", "also"] {
            if let Some(v) = m.get(k) {
                visit(v, &mut out)
            }
        }
    }
    out
}
pub(crate) fn pointers(
    document: &V,
    source: &Path,
    mapping: &BTreeMap<PathBuf, String>,
) -> Result<V> {
    fn rewrite(v: &mut V, source: &Path, mapping: &BTreeMap<PathBuf, String>) -> Result<()> {
        match v {
            V::Text(t) if t.ends_with(".yaml") || t.ends_with(".yml") => {
                let target = absolute(&source.parent().unwrap().join(&*t))?;
                let mapped = mapping
                    .get(&target)
                    .ok_or_else(|| error("unresolved_original_pointer"))?;
                let from = Path::new(&mapping[source]).parent().unwrap();
                let a = from.components().collect::<Vec<_>>();
                let b = Path::new(mapped).components().collect::<Vec<_>>();
                let common = a.iter().zip(&b).take_while(|(a, b)| a == b).count();
                let mut relative = PathBuf::new();
                for _ in common..a.len() {
                    relative.push("..");
                }
                for c in &b[common..] {
                    relative.push(c);
                }
                *t = posix(&relative)?;
            }
            V::Map(m) => {
                for v in m.values_mut() {
                    rewrite(v, source, mapping)?
                }
            }
            V::List(a) => {
                for v in a {
                    rewrite(v, source, mapping)?
                }
            }
            _ => {}
        }
        Ok(())
    }
    let mut result = document.clone();
    for k in ["record", "also"] {
        if let Some(v) = crate::history_view::map_mut(&mut result)?.get_mut(k) {
            rewrite(v, source, mapping)?
        }
    }
    Ok(result)
}
