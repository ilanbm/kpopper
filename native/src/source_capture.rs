//! Bracketed source capture. Portable meaning and private byte observations stay
//! separate; neither grants a write or publication permission.
use crate::{
    Result,
    history_contract::*,
    history_view::map_mut,
    identity::sha256,
    project_modes as M,
    reasoning_snapshot::{self as S, CaptureOptions, Snapshot},
    require,
    source_document::{self as D, Document},
    source_inventory::{Inventory, Observation, absolute, name},
    value::TypedValue as V,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadMode {
    Frozen,
    Live,
}
impl ReadMode {
    fn name(self) -> &'static str {
        match self {
            Self::Frozen => "frozen",
            Self::Live => "live",
        }
    }
}
fn s(v: &str) -> V {
    V::Text(v.into())
}
fn empty() -> V {
    V::Map(Map::new())
}
fn object(items: impl IntoIterator<Item = (&'static str, V)>) -> V {
    V::Map(items.into_iter().map(|(k, v)| (k.into(), v)).collect())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Routing {
    config: V,
    config_bytes: Option<Vec<u8>>,
    root: PathBuf,
    record: PathBuf,
    selected: Vec<PathBuf>,
    pending: Option<crate::pending_state::Observation>,
}
fn policy_bytes(path: &Path) -> Result<Option<Vec<u8>>> {
    use std::io::Read;
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    require(file.metadata()?.is_file(), "invalid_project_config")?;
    let mut raw = Vec::new();
    file.take(1024 * 1024 + 1).read_to_end(&mut raw)?;
    require(raw.len() <= 1024 * 1024, "project_config_limit")?;
    Ok(Some(raw))
}
fn observation(paths: &[PathBuf], cwd: &Path, mode: ReadMode) -> Result<Routing> {
    let project = M::project_for(paths, cwd)?;
    let config_bytes = policy_bytes(&project.config_path)?;
    let config = project.config()?;
    let pending = if mode == ReadMode::Live
        && project.is_git()
        && string_is(&map(&config)?["mode"], "advanced")
    {
        Some(crate::pending_state::Observation::capture(
            &project, &config,
        )?)
    } else {
        None
    };
    let record = project.record(Some(&config))?;
    let selected = if mode == ReadMode::Live {
        M::write_paths(paths, cwd)?
    } else {
        paths.to_vec()
    };
    require(
        config_bytes == policy_bytes(&project.config_path)?,
        "snapshot_changed",
    )?;
    Ok(Routing {
        config,
        config_bytes,
        root: project.root,
        record,
        selected,
        pending,
    })
}
fn origin(base: &Path, path: &Path) -> Result<String> {
    let path = absolute(path)?;
    Ok(match path.strip_prefix(base) {
        Ok(relative) => format!("origin:{}", name(relative)?),
        Err(_) => format!(
            "external:{}/{}",
            &sha256(name(&path)?.as_bytes())[..24],
            name(Path::new(
                path.file_name().ok_or_else(|| error("invalid_path"))?
            ))?
        ),
    })
}
fn portable(value: &V, base: &Path, authored: bool) -> Result<V> {
    Ok(match value {
        V::Map(m) => V::Map(
            m.iter()
                .filter(|(k, _)| authored || *k != "standing_permission")
                .map(|(k, v)| {
                    Ok((
                        k.clone(),
                        portable(
                            v,
                            base,
                            authored || ["doc", "document", "manifest"].contains(&k.as_str()),
                        )?,
                    ))
                })
                .collect::<Result<Map>>()?,
        ),
        V::List(a) => V::List(
            a.iter()
                .map(|v| portable(v, base, authored))
                .collect::<Result<Vec<_>>>()?,
        ),
        V::Text(v) if !authored && Path::new(v).is_absolute() => s(&origin(base, Path::new(v))?),
        _ => value.clone(),
    })
}
fn publication_context(value: &V) -> Result<V> {
    let mut value = value.clone();
    let m = map_mut(&mut value)?;
    for key in ["retry_at", "failures"] {
        m.remove(key);
    }
    if let Some(V::Map(verified)) = m.get_mut("last_verified") {
        verified.remove("at");
    }
    Ok(value)
}
fn snapshot(
    doc: &Document,
    inventory: &Inventory,
    initial: &Routing,
    paths: &[PathBuf],
    mode: ReadMode,
    as_of: Option<V>,
) -> Result<Snapshot> {
    let base = absolute(&paths[0])?
        .parent()
        .ok_or_else(|| error("invalid_path"))?
        .to_owned();
    let config = map(&initial.config)?;
    let publication_identity =
        if let Some(publication) = config.get("publication").filter(|v| **v != V::Null) {
            let p = map(publication)?;
            let fields = ["remote", "repository", "target", "branch"]
                .iter()
                .map(|k| ((*k).into(), p[*k].clone()))
                .collect::<Map>();
            s(&sha256(&serde_json::to_vec(&V::Map(fields).to_json()?)?))
        } else {
            V::Null
        };
    let mut context = object([
        (
            "read_mode",
            s(if mode == ReadMode::Live {
                "captured-live"
            } else {
                "frozen"
            }),
        ),
        ("original_read_mode", s(mode.name())),
        (
            "project",
            object([
                ("version", config["version"].clone()),
                ("mode", config["mode"].clone()),
                ("generation", config["generation"].clone()),
                ("routing_identity", s(&origin(&base, &initial.record)?)),
                ("publication_identity", publication_identity),
            ]),
        ),
        (
            "pending",
            object([
                ("ref", V::Null),
                ("bundles", empty()),
                ("events", V::List(vec![])),
                ("observations", empty()),
                ("contributions", V::List(vec![])),
            ]),
        ),
        ("conflicts", empty()),
        (
            "target",
            object([
                ("status", s("unassessed")),
                ("reason", V::Null),
                ("ref", V::Null),
                ("revision", V::Null),
            ]),
        ),
    ]);
    if let Some(history) = &doc.history_projection {
        map_mut(&mut context)?.insert("history".into(), history.clone());
        map_mut(&mut context)?.insert("history_view".into(), doc.history_view.clone().unwrap());
    }
    if let Some(overlay) = &doc.overlay {
        let mut pending = portable(&overlay.pending, &base, false)?;
        map_mut(&mut pending)?.insert(
            "observations".into(),
            portable(&publication_context(&overlay.publication)?, &base, false)?,
        );
        map_mut(&mut pending)?.insert(
            "contributions".into(),
            portable(
                &V::List(
                    overlay
                        .contributions
                        .iter()
                        .map(publication_context)
                        .collect::<Result<Vec<_>>>()?,
                ),
                &base,
                false,
            )?,
        );
        map_mut(&mut context)?.insert("pending".into(), pending);
        map_mut(&mut context)?.insert(
            "conflicts".into(),
            portable(&overlay.conflicts, &base, true)?,
        );
        if !map(&overlay.history_contributions)?.is_empty() {
            map_mut(&mut context)?.insert(
                "history_contributions".into(),
                overlay.history_contributions.clone(),
            );
        }
        let mut target = overlay.target.clone().unwrap_or_else(empty);
        let observed = map(&target)?
            .get("revision")
            .is_some_and(crate::history_view::truth);
        let target_map = map_mut(&mut target)?;
        target_map.insert(
            "status".into(),
            s(if overlay.unavailable.is_some() {
                "unavailable"
            } else if observed {
                "observed"
            } else {
                "unassessed"
            }),
        );
        target_map.insert(
            "reason".into(),
            overlay
                .unavailable
                .as_ref()
                .map(|v| s(v))
                .unwrap_or(V::Null),
        );
        target_map.entry("ref".into()).or_insert(V::Null);
        target_map.entry("revision".into()).or_insert(V::Null);
        if let Some(snapshot) = &overlay.target_snapshot {
            target_map.insert("snapshot".into(), snapshot.clone());
        }
        map_mut(&mut context)?.insert("target".into(), portable(&target, &base, false)?);
    } else if let Some(pending) = &initial.pending {
        // An unrelated artifact receives no contribution layers, but its capture
        // still binds the same project publisher and locally resolved target.
        let context_map = map_mut(&mut context)?;
        map_mut(context_map.get_mut("pending").unwrap())?.insert(
            "observations".into(),
            portable(&publication_context(&pending.publication)?, &base, false)?,
        );
        if let Some(target) = &pending.target {
            let mut target = target.clone();
            let m = map_mut(&mut target)?;
            m.insert("status".into(), s("unassessed"));
            m.insert("reason".into(), V::Null);
            context_map.insert("target".into(), portable(&target, &base, false)?);
        }
    }
    let mut files = BTreeMap::new();
    for ((_, path), event) in &inventory.events {
        if let Some(history) = &doc.history
            && [
                history.root.join(&history.layout.objects),
                history.root.join(&history.layout.commits),
            ]
            .iter()
            .any(|root| path.starts_with(root))
        {
            continue;
        }
        let (hash, status, error) = match event {
            Observation::Bytes(hash) => (s(hash), s("read"), None),
            Observation::Unreadable(error) => (V::Null, s("unreadable"), Some(s(error))),
            _ => continue,
        };
        let key = origin(&base, path)?;
        let mut file = object([("origin", s(&key)), ("sha256", hash), ("status", status)]);
        if let Some(error) = error {
            map_mut(&mut file)?.insert("error".into(), error);
        }
        require(
            files.insert(key, file).is_none(),
            "invalid_authored_revision",
        )?;
    }
    let mut revision = object([("files", V::List(files.into_values().collect()))]);
    let digest = S::digest(&revision)?;
    map_mut(&mut revision)?.insert("digest".into(), s(&digest));
    let mut hypotheses = doc.hypotheses.clone();
    for hypothesis in map_mut(&mut hypotheses)?.values_mut() {
        if map(hypothesis)?
            .get("kind")
            .is_some_and(|v| string_is(v, "contribution"))
            && let Some(head) = map_mut(hypothesis)?.get_mut("head")
            && let Some(publication) = map_mut(head)?.get_mut("publication")
        {
            *publication = publication_context(publication)?;
        }
    }
    Snapshot::from_data(
        &doc.source.typed(),
        CaptureOptions {
            context: Some(context),
            hypotheses: Some(portable(&hypotheses, &base, false)?),
            as_of,
            authored_revision: Some(revision),
        },
    )
}
pub struct CapturedSource {
    snapshot: Snapshot,
    inventory: Inventory,
    routing: Routing,
    document: Document,
    paths: Vec<PathBuf>,
    cwd: PathBuf,
    mode: ReadMode,
}
impl CapturedSource {
    pub(crate) fn pending_observation(&self) -> Option<&crate::pending_state::Observation> {
        self.routing.pending.as_ref()
    }
    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }
    pub fn document(&self) -> V {
        self.document.source.typed()
    }
    pub fn source(&self) -> &crate::history_yaml::SourceValue {
        &self.document.source
    }
    pub fn hypotheses(&self) -> &V {
        &self.document.hypotheses
    }
    pub fn origins(&self) -> &BTreeMap<String, BTreeMap<String, PathBuf>> {
        &self.document.origins
    }
    pub fn members(&self) -> &[PathBuf] {
        &self.document.members
    }
    pub fn files(&self) -> &BTreeMap<PathBuf, Vec<u8>> {
        &self.inventory.files
    }
    pub fn history_capture(&self) -> Option<&crate::history_capture::Capture> {
        self.document.history.as_ref()
    }
    pub fn verify(&self) -> Result<()> {
        self.inventory.verify()?;
        if let Some(history) = &self.document.history {
            history.verify_current()?;
        }
        let mut members = self.document.members.clone();
        members.extend(self.routing.selected.iter().cloned());
        crate::history_transaction_fs::check_member_journals(&members)?;
        require(
            self.routing == observation(&self.paths, &self.cwd, self.mode)?,
            "snapshot_changed",
        )
    }
}
pub fn capture_source(
    paths: &[PathBuf],
    cwd: &Path,
    mode: ReadMode,
    as_of: Option<V>,
) -> Result<CapturedSource> {
    capture_source_with_runtime(paths, cwd, mode, as_of, None)
}
/// Supply a verified runtime when a committed core target requires computation.
pub fn capture_source_with_runtime(
    paths: &[PathBuf],
    cwd: &Path,
    mode: ReadMode,
    as_of: Option<V>,
    runtime: Option<&crate::reasoning_runtime::Runtime>,
) -> Result<CapturedSource> {
    capture_with(paths, cwd, mode, as_of, &mut |_, _| Ok(()), runtime)
}
fn capture_with(
    paths: &[PathBuf],
    cwd: &Path,
    mode: ReadMode,
    as_of: Option<V>,
    after_load: &mut dyn FnMut(usize, &Document) -> Result<()>,
    runtime: Option<&crate::reasoning_runtime::Runtime>,
) -> Result<CapturedSource> {
    require(!paths.is_empty(), "invalid_snapshot")?;
    S::normalize_as_of(as_of.as_ref().unwrap_or(&V::Null))?;
    let paths = paths
        .iter()
        .map(|p| absolute(&cwd.join(p)))
        .collect::<Result<Vec<_>>>()?;
    let cwd = absolute(cwd)?;
    let initial = observation(&paths, &cwd, mode)?;
    let mut previous = None;
    let mut final_load = None;
    for pass in 0..2 {
        let mut inventory = Inventory::default();
        let canonical = initial.selected[0]
            .canonicalize()
            .unwrap_or_else(|_| initial.selected[0].clone())
            == initial.record;
        let allow_missing = canonical
            && initial
                .pending
                .as_ref()
                .is_some_and(|p| p.ledger.head.is_some());
        let mut doc = D::load(&initial.selected, &mut inventory, allow_missing)?;
        if canonical && let Some(pending) = &initial.pending {
            doc.overlay = Some(crate::source_overlay::apply(
                &mut doc,
                pending,
                &initial.root,
                text(&map(&initial.config)?["record"])?,
                runtime,
            )?);
        }
        after_load(pass, &doc)?;
        inventory.verify()?;
        if let Some(history) = &doc.history {
            history.verify_current()?;
        }
        require(
            initial == observation(&paths, &cwd, mode)?,
            "snapshot_changed",
        )?;
        let history_inventory = doc.history.as_ref().map(|h| h.inventory.clone());
        if let Some((old, old_history)) = &previous {
            require(
                old == &inventory.events && old_history == &history_inventory,
                "snapshot_changed",
            )?;
        }
        previous = Some((inventory.events.clone(), history_inventory));
        final_load = Some((doc, inventory));
    }
    let (document, inventory) = final_load.unwrap();
    let snapshot = snapshot(&document, &inventory, &initial, &paths, mode, as_of)?;
    let captured = CapturedSource {
        snapshot,
        inventory,
        routing: initial,
        document,
        paths,
        cwd,
        mode,
    };
    captured.verify()?;
    Ok(captured)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_bytes_membership_and_routing_are_rechecked() {
        for mutation in 0..4 {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            let entry = root.join("GROUNDING.yaml");
            std::fs::write(&entry, "p: {p.a: {v: 1}}\nalso: absent.yaml\n").unwrap();
            let result = capture_with(
                std::slice::from_ref(&entry),
                &root,
                ReadMode::Frozen,
                None,
                &mut |pass, _| {
                    if pass == 0 {
                        match mutation {
                            0 => std::fs::write(
                                &entry,
                                "# same meaning\np: {p.a: {v: 1}}\nalso: absent.yaml\n",
                            )?,
                            1 => std::fs::write(root.join("absent.yaml"), "{}\n")?,
                            2 => {
                                let hyp = root.join(".kpopper/hypotheses");
                                std::fs::create_dir_all(&hyp)?;
                                std::fs::write(hyp.join("new.yaml"), "{}\n")?;
                            }
                            _ => {
                                std::fs::create_dir_all(root.join(".kpopper"))?;
                                std::fs::write(root.join(".kpopper/project.json"), "{}")?;
                            }
                        }
                    }
                    Ok(())
                },
                None,
            );
            assert!(result.is_err(), "accepted mutation {mutation}");
        }
    }
    #[test]
    fn member_guards_block_direct_reads_and_are_revalidated() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let entry = root.join("data.yaml");
        std::fs::write(&entry, "p: {p.a: {v: 1}}\n").unwrap();
        let capture =
            capture_source(std::slice::from_ref(&entry), &root, ReadMode::Frozen, None).unwrap();
        let home = root.join(".history-local");
        std::fs::create_dir_all(&home).unwrap();
        let digest = "a".repeat(64);
        let journal = home.join(format!("{digest}.json"));
        let mut guard = serde_json::json!({"version":1,"kind":"member_guard","operation":"op","digest":digest,"members":["unrelated.yaml"]});
        std::fs::write(&journal, serde_json::to_vec(&guard).unwrap()).unwrap();
        capture.verify().unwrap();
        guard["members"] = serde_json::json!(["data.yaml"]);
        std::fs::write(&journal, serde_json::to_vec(&guard).unwrap()).unwrap();
        assert_eq!(capture.verify().unwrap_err().0, "recovery_required");
        assert!(
            capture_source(std::slice::from_ref(&entry), &root, ReadMode::Frozen, None).is_err()
        );
        guard["members"] = serde_json::json!(["../elsewhere.yaml"]);
        std::fs::write(&journal, serde_json::to_vec(&guard).unwrap()).unwrap();
        assert!(
            capture
                .verify()
                .unwrap_err()
                .0
                .starts_with("invalid_pending_journal")
        );
    }
}
