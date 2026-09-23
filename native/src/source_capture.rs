//! Bracketed source capture. Portable meaning and private byte observations stay
//! separate; neither grants a write or publication permission.
use crate::{
    Result,
    history_contract::*,
    history_view::map_mut,
    identity::sha256,
    ordinary_document::{self as D, Document},
    project_modes as M,
    reasoning_snapshot::{self as S, CaptureOptions, Snapshot},
    require,
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
/// Exact routing observation for guarded lifecycle verification, never a Snapshot
/// projection and never evidence of writer exclusion.
pub(crate) fn routing_observation(paths: &[PathBuf], cwd: &Path) -> Result<V> {
    let routing = observation(paths, cwd, ReadMode::Live)?;
    let mut value = object([
        ("config", routing.config),
        ("config_exists", V::Bool(routing.config_bytes.is_some())),
        ("root", s(name(&routing.root)?)),
        ("record", s(name(&routing.record)?)),
    ]);
    if let Some(pending) = routing.pending {
        let m = map_mut(&mut value)?;
        m.insert(
            "pending_ref".into(),
            pending.ledger.head.map(|h| s(&h)).unwrap_or(V::Null),
        );
        m.insert("publication".into(), pending.publication);
        if let Some(target) = pending.target {
            m.insert("target".into(), target);
        }
    }
    Ok(value)
}
fn origin(base: &Path, path: &Path) -> Result<String> {
    let path = absolute(path)?;
    Ok(match path.strip_prefix(base) {
        Ok(relative) => {
            let components = relative.components()
                .map(|part| part.as_os_str().to_str().ok_or_else(|| error("invalid_path")))
                .collect::<Result<Vec<_>>>()?;
            format!("origin:{}", components.join("/"))
        }
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
            target_map.insert("snapshot".into(), snapshot.finite_projection()?);
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
    let mut hypotheses = doc.hypotheses.finite_projection()?;
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
    let hypotheses = crate::history_yaml::strict_ordinary_projection(&hypotheses)?;
    Snapshot::from_data(
        &doc.source.strict_typed()?,
        CaptureOptions {
            context: Some(context),
            hypotheses: Some(portable(&hypotheses, &base, false)?),
            as_of,
            authored_revision: Some(revision),
        },
    )
}
pub struct CapturedSource<T = V> {
    snapshot: Option<Snapshot>,
    snapshot_error: Option<String>,
    ordinary_document: T,
    hypotheses: T,
    source_finite: Option<crate::history_yaml::OrdinaryValue>,
    inventory: Inventory,
    routing: Routing,
    document: Document,
    paths: Vec<PathBuf>,
    cwd: PathBuf,
    mode: ReadMode,
}
impl<T> CapturedSource<T> {
    pub(crate) fn inventory(&self) -> &Inventory {
        &self.inventory
    }
    pub(crate) fn pending_observation(&self) -> Option<&crate::pending_state::Observation> {
        self.routing.pending.as_ref()
    }
    /// Local contribution statuses observed by this read, retired ones included.
    /// A read without a pending overlay observes none.
    pub(crate) fn contributions(&self) -> &[V] {
        self.document
            .overlay
            .as_ref()
            .map_or(&[], |overlay| overlay.contributions.as_slice())
    }
    /// Exact public knowledge-status fields from this already captured view.
    /// This performs no reads and preserves ordinary records that cannot form a
    /// strict portable Snapshot.
    pub(crate) fn knowledge_status_context(&self) -> V {
        let overlay = self.document.overlay.as_ref();
        object([
            (
                "contributions",
                overlay
                    .map(|value| V::List(value.contributions.clone()))
                    .unwrap_or_else(|| V::List(vec![])),
            ),
            (
                "conflicts",
                overlay
                    .map(|value| value.conflicts.clone())
                    .unwrap_or_else(empty),
            ),
            (
                "publication",
                overlay
                    .map(|value| value.publication.clone())
                    .unwrap_or(V::Null),
            ),
            (
                "target_unavailable",
                overlay
                    .and_then(|value| value.unavailable.as_ref())
                    .map(|value| s(value))
                    .unwrap_or(V::Null),
            ),
        ])
    }
    pub fn snapshot(&self) -> Result<&Snapshot> {
        self.snapshot.as_ref().ok_or_else(|| {
            crate::Error(
                self.snapshot_error
                    .clone()
                    .unwrap_or_else(|| "invalid_yaml_key".into()),
            )
        })
    }
    pub fn strict_document(&self) -> Result<V> {
        self.document.source.strict_typed()
    }
    pub fn ordinary_document(&self) -> &T {
        &self.ordinary_document
    }
    pub fn hypotheses(&self) -> &T {
        &self.hypotheses
    }
    /// The ordinary revision binds the original knowledge observation, before
    /// Snapshot portability rewrites paths and adds its own target status fields.
    pub(crate) fn ordinary_context(&self) -> V {
        let overlay = self.document.overlay.as_ref();
        object([
            ("read_mode", s(self.mode.name())),
            (
                "conflicts",
                overlay.map(|v| v.conflicts.clone()).unwrap_or_else(empty),
            ),
            (
                "target",
                overlay.and_then(|v| v.target.clone()).unwrap_or(V::Null),
            ),
            (
                "target_unavailable",
                overlay
                    .and_then(|v| v.unavailable.as_ref())
                    .map(|v| s(v))
                    .unwrap_or(V::Null),
            ),
        ])
    }
    /// Original contribution labels accompany ordinary reads without portable
    /// path rewriting or a new observation of pending state.
    pub(crate) fn reader_lines(&self) -> Result<Vec<String>> {
        let Some(overlay) = &self.document.overlay else {
            return Ok(vec![]);
        };
        let mut lines = vec![];
        for contribution in &overlay.contributions {
            let c = map(contribution)?;
            let scope = map(&c["scope"])?;
            let roots = crate::history_view::list(&c["roots"])?
                .iter()
                .map(text)
                .collect::<Result<Vec<_>>>()?;
            lines.push(format!(
                "PENDING {} · {} · {}: {} · {}",
                text(&c["revision"])?.chars().take(12).collect::<String>(),
                text(&c["state"])?,
                text(&scope["kind"])?,
                text(&scope["environment"])?,
                roots.join(", ")
            ));
        }
        for (id, variants) in map(&overlay.conflicts)? {
            let names = crate::history_view::list(variants)?
                .iter()
                .map(|v| text(&crate::history_view::list(v)?[0]))
                .collect::<Result<Vec<_>>>()?;
            lines.push(format!("CONFLICT {id}: {}", names.join(", ")));
        }
        if let Some(reason) = &overlay.unavailable {
            lines.push(format!("TARGET UNVERIFIED: {reason}"));
        }
        Ok(lines)
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
pub type OrdinaryCapture = CapturedSource<crate::ordinary_value::Value>;
impl CapturedSource<V> {
    pub fn source(&self) -> &crate::history_yaml::OrdinaryValue {
        self.source_finite
            .as_ref()
            .expect("finite capture constructor")
    }
}
impl OrdinaryCapture {
    pub fn source(&self) -> &crate::ordinary_source::Source {
        &self.document.source
    }
    pub fn try_finite(self) -> Result<CapturedSource> {
        let source_finite = Some(self.document.source.try_finite()?);
        let ordinary_document = self.ordinary_document.finite_projection()?;
        let hypotheses = self.hypotheses.finite_projection()?;
        if let Some(target) = self
            .document
            .overlay
            .as_ref()
            .and_then(|o| o.target_snapshot.as_ref())
        {
            target.finite_projection()?;
        }
        Ok(CapturedSource {
            snapshot: self.snapshot,
            snapshot_error: self.snapshot_error,
            ordinary_document,
            hypotheses,
            source_finite,
            inventory: self.inventory,
            routing: self.routing,
            document: self.document,
            paths: self.paths,
            cwd: self.cwd,
            mode: self.mode,
        })
    }
}
pub fn capture_ordinary_source(
    paths: &[PathBuf],
    cwd: &Path,
    mode: ReadMode,
    as_of: Option<V>,
) -> Result<OrdinaryCapture> {
    capture_ordinary_source_with_runtime(paths, cwd, mode, as_of, None)
}
pub fn capture_ordinary_source_with_runtime(
    paths: &[PathBuf],
    cwd: &Path,
    mode: ReadMode,
    as_of: Option<V>,
    runtime: Option<&crate::reasoning_runtime::Runtime>,
) -> Result<OrdinaryCapture> {
    capture_ordinary_with(paths, cwd, mode, as_of, &mut |_, _| Ok(()), runtime)
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
    capture_ordinary_source_with_runtime(paths, cwd, mode, as_of, runtime)?.try_finite()
}
#[cfg(test)]
fn capture_with(
    paths: &[PathBuf],
    cwd: &Path,
    mode: ReadMode,
    as_of: Option<V>,
    after_load: &mut dyn FnMut(usize, &Document) -> Result<()>,
    runtime: Option<&crate::reasoning_runtime::Runtime>,
) -> Result<CapturedSource> {
    capture_ordinary_with(paths, cwd, mode, as_of, after_load, runtime)?.try_finite()
}
fn capture_ordinary_with(
    paths: &[PathBuf],
    cwd: &Path,
    mode: ReadMode,
    as_of: Option<V>,
    after_load: &mut dyn FnMut(usize, &Document) -> Result<()>,
    runtime: Option<&crate::reasoning_runtime::Runtime>,
) -> Result<OrdinaryCapture> {
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
            doc.overlay = Some(crate::ordinary_overlay::apply(
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
    let ordinary_document = document.source.projected();
    let (snapshot, snapshot_error) =
        match snapshot(&document, &inventory, &initial, &paths, mode, as_of) {
            Ok(snapshot) => (Some(snapshot), None),
            Err(error)
                if error.0 == "invalid_yaml_key"
                    || error.0.starts_with("invalid_history_value: snapshot ")
                    || error.0 == "nonfinite_value_not_canonical" =>
            {
                (None, Some(error.0))
            }
            Err(error) if error.0 == "invalid_snapshot" => {
                return Err(crate::ordinary_fields::explain_tie(
                    &ordinary_document,
                    error,
                ));
            }
            Err(error) => return Err(error),
        };
    let captured = CapturedSource {
        snapshot,
        snapshot_error,
        hypotheses: document.hypotheses.clone(),
        source_finite: None,
        ordinary_document,
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
    fn ordinary_capture_preserves_nontext_body_keys_without_a_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let entry = root.join("GROUNDING.yaml");
        std::fs::write(
            &entry,
            "p:\n  p.a:\n    v: {on: x}\n    seen: {p.z: {1: one, '1': text}}\n",
        )
        .unwrap();
        let capture =
            capture_source(std::slice::from_ref(&entry), &root, ReadMode::Frozen, None).unwrap();
        assert_eq!(capture.snapshot().unwrap_err().0, "invalid_yaml_key");
        assert_eq!(capture.strict_document().unwrap_err().0, "invalid_yaml_key");
        let body = &map(capture.ordinary_document()).unwrap()["p"];
        let body = &map(body).unwrap()["p.a"];
        assert!(map(body).unwrap()["v"] != V::Null);
        assert_eq!(capture.files()[&entry], std::fs::read(&entry).unwrap());
        std::fs::write(&entry, "p: {p.a: {v: changed}}\n").unwrap();
        assert_eq!(capture.verify().unwrap_err().0, "snapshot_changed");
    }

    #[test]
    fn ordinary_capture_refuses_nontext_collection_ids() {
        for text in ["true: {v: 1}\n", "p:\n  true: {v: 1}\n"] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            let entry = root.join("GROUNDING.yaml");
            std::fs::write(&entry, text).unwrap();
            let error = match capture_source(&[entry], &root, ReadMode::Frozen, None) {
                Ok(_) => panic!("accepted a nontext structural key"),
                Err(error) => error,
            };
            assert_eq!(error.0, "invalid_ordinary_structural_key");
        }
    }

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

#[cfg(test)]
mod ordinary_domain_tests {
    use super::*;
    use crate::ordinary_value::{NonFiniteFloat, Value as O};
    #[test]
    fn full_ordinary_capture_retains_source_and_hypotheses_without_a_canonical_snapshot() {
        for live in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            if live {
                assert!(
                    std::process::Command::new("git")
                        .args(["init", "-q"])
                        .arg(&root)
                        .status()
                        .unwrap()
                        .success()
                );
            }
            let entry = root.join("GROUNDING.yaml");
            let raw =
                b"known:\n  p.value: {name: Sample, v: .nan, quoted: 99}\nmeta: {note: .inf}\n";
            std::fs::write(&entry, raw).unwrap();
            let hyp = root.join(".kpopper/hypotheses");
            std::fs::create_dir_all(&hyp).unwrap();
            std::fs::write(hyp.join("other.yaml"), b"known: {p.value: {v: -.inf}}\n").unwrap();
            let mode = if live {
                ReadMode::Live
            } else {
                ReadMode::Frozen
            };
            let capture =
                capture_ordinary_source(std::slice::from_ref(&entry), &root, mode, None).unwrap();
            let doc = crate::ordinary_value::map(capture.ordinary_document()).unwrap();
            let known = crate::ordinary_value::map(&doc["known"]).unwrap();
            let body = crate::ordinary_value::map(&known["p.value"]).unwrap();
            assert_eq!(body["v"], O::NonFinite(NonFiniteFloat::NaN));
            assert!(capture.snapshot().is_err());
            assert!(capture.strict_document().is_err());
            assert!(
                capture
                    .hypotheses()
                    .python_json(false)
                    .unwrap()
                    .contains("-Infinity")
            );
            assert_eq!(capture.files()[&entry], raw);
            capture.verify().unwrap();
            assert!(capture.try_finite().is_err());
            assert!(capture_source(std::slice::from_ref(&entry), &root, mode, None).is_err());
            assert_eq!(std::fs::read(&entry).unwrap(), raw);
        }
    }
}

#[cfg(test)]
mod nonfinite_target_tests {
    use super::*;
    fn git(root: &Path, args: &[&str]) {
        let result = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    #[test]
    fn ordinary_target_refusals_preserve_python_snapshot_boundary() {
        for (source, reason) in [
            (
                "known: {p.value: {v: .nan}}\n",
                "snapshot data contains a nonfinite value",
            ),
            (
                "known: {p.value: {v: 1, .nan: source}}\n",
                "snapshot mappings require string keys",
            ),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            git(&root, &["init", "-q"]);
            let entry = root.join("GROUNDING.yaml");
            std::fs::write(&entry, source).unwrap();
            git(&root, &["add", "GROUNDING.yaml"]);
            git(
                &root,
                &[
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.invalid",
                    "commit",
                    "-qm",
                    "target",
                ],
            );
            git(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
            std::fs::write(&entry, "known: {p.value: {v: 1}}\n").unwrap();
            let config = root.join(".git/kpopper/project");
            std::fs::create_dir_all(&config).unwrap();
            std::fs::write(config.join("project.json"), r#"{"version":1,"mode":"advanced","record":"GROUNDING.yaml","generation":0,"publication":{"remote":"origin","repository":"test/test","target":"main","branch":"pending_grounding","standing_permission":false}}"#).unwrap();
            let capture = capture_ordinary_source(&[entry], &root, ReadMode::Live, None).unwrap();
            assert_eq!(
                map(&capture.ordinary_context()).unwrap()["target_unavailable"],
                s(reason)
            );
            assert_eq!(
                capture.reader_lines().unwrap(),
                vec![format!("TARGET UNVERIFIED: {reason}")]
            );
            capture.verify().unwrap();
        }
    }
    #[test]
    fn full_domain_capture_rechecks_nonfinite_source_and_hypothesis_bytes() {
        for change_hypothesis in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            let entry = root.join("GROUNDING.yaml");
            std::fs::write(&entry, "known: {p.value: {v: .nan}}\n").unwrap();
            let dir = root.join(".kpopper/hypotheses");
            std::fs::create_dir_all(&dir).unwrap();
            let hypothesis = dir.join("other.yaml");
            std::fs::write(&hypothesis, "known: {p.value: {v: .inf}}\n").unwrap();
            let error = match capture_ordinary_with(
                std::slice::from_ref(&entry),
                &root,
                ReadMode::Frozen,
                None,
                &mut |pass, _| {
                    if pass == 0 {
                        std::fs::write(
                            if change_hypothesis {
                                &hypothesis
                            } else {
                                &entry
                            },
                            "known: {p.value: {v: -.inf}}\n",
                        )?;
                    }
                    Ok(())
                },
                None,
            ) {
                Ok(_) => panic!("accepted changed nonfinite capture"),
                Err(error) => error,
            };
            assert_eq!(error.0, "snapshot_changed");
        }
    }
}

#[cfg(test)]
mod malformed_ordinary_hypothesis_tests {
    use super::*;
    #[test]
    fn captured_hypothesis_diagnostics_match_python_without_new_reads() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/ordinary-yaml-diagnostics.json"
        ))
        .unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            let entry = root.join("GROUNDING.yaml");
            std::fs::write(&entry, "known: {p.value: {v: .nan}}\n").unwrap();
            let directory = root.join(".kpopper/hypotheses");
            std::fs::create_dir_all(&directory).unwrap();
            let hypothesis = directory.join("broken.yaml");
            let raw = case["source"].as_str().unwrap();
            std::fs::write(&hypothesis, raw).unwrap();
            let capture = capture_ordinary_source(&[entry], &root, ReadMode::Frozen, None).unwrap();
            let hypotheses = crate::ordinary_value::map(capture.hypotheses()).unwrap();
            let error = &crate::ordinary_value::map(&hypotheses["broken"]).unwrap()["error"];
            assert_eq!(
                error,
                &crate::ordinary_value::Value::Text(case["expected"].as_str().unwrap().into()),
                "{}",
                case["name"]
            );
            assert_eq!(capture.files()[&hypothesis], raw.as_bytes());
            capture.verify().unwrap();
        }
    }
}
