//! Bounded committed target closure, materialized only in a private scratch tree.
use crate::{
    Result,
    history_authority::Files,
    history_contract::*,
    history_view::{list, map_mut, truth},
    history_yaml as Y,
    identity::sha256,
    pending_state as P,
    reasoning_runtime::{OperationalBounds, Runtime},
    reasoning_snapshot::{CaptureOptions, Snapshot},
    require,
    source_capture::{
        CapturedSource, ReadMode, capture_ordinary_layer, capture_ordinary_source_with_runtime,
    },
    value::{Integer, TypedValue as V},
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Component, Path},
};
fn s(v: &str) -> V {
    V::Text(v.into())
}
fn obj(items: impl IntoIterator<Item = (&'static str, V)>) -> V {
    V::Map(items.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
fn name(path: &Path) -> Result<&str> {
    path.to_str().ok_or_else(|| error("invalid_target_path"))
}
fn safe(path: &str) -> Result<String> {
    require(
        !Path::new(path).is_absolute() && !path.contains('\\'),
        "invalid_target_path",
    )?;
    let mut parts = vec![];
    for part in Path::new(path).components() {
        match part {
            Component::Normal(p) => parts.push(name(Path::new(p))?),
            Component::CurDir => {}
            Component::ParentDir => return Err(error("invalid_target_path")),
            _ => return Err(error("invalid_target_path")),
        }
    }
    let value = parts.join("/");
    crate::history_branch::portable_path(&value)?;
    Ok(value)
}
fn pointer(path: &Path) -> Result<String> {
    require(!path.is_absolute(), "invalid_target_path")?;
    let mut normalized = std::path::PathBuf::new();
    for part in path.components() {
        if part == Component::ParentDir {
            require(normalized.pop(), "invalid_target_path")?;
        } else {
            normalized.push(part);
        }
    }
    safe(name(&normalized)?)
}
struct Reader<'a> {
    root: &'a Path,
    tree: BTreeMap<String, (String, String, String)>,
}
impl Reader<'_> {
    fn read(&self, path: &str, maximum: usize) -> Result<Vec<u8>> {
        let (mode, kind, oid) = self
            .tree
            .get(path)
            .ok_or_else(|| error("target_record_unavailable"))?;
        require(
            kind == "blob" && ["100644", "100755"].contains(&mode.as_str()),
            "nonregular_target_file",
        )?;
        let raw = P::git(self.root, &["cat-file", "blob", oid], maximum, false)?.unwrap();
        P::verify_blob(oid, &raw)?;
        Ok(raw)
    }
}
fn byte_value(raw: &[u8]) -> V {
    obj([
        ("encoding", s("hex")),
        (
            "data",
            s(&raw.iter().map(|b| format!("{b:02x}")).collect::<String>()),
        ),
        ("sha256", s(&sha256(raw))),
    ])
}
pub(crate) fn history_evidence(capture: &crate::history_capture::Capture) -> Result<V> {
    let mut files = Files::from([
        ("entry.yaml".into(), capture.entry_bytes.clone()),
        ("authority.yaml".into(), capture.authority_bytes.clone()),
    ]);
    let mut commits = capture.commits.clone();
    for generation in capture.inactive_generations.values() {
        commits.extend(generation.commits.clone());
    }
    for (op, raw) in commits {
        files.insert(format!("commits/{op}.yaml"), raw);
    }
    for (key, raw) in &capture.object_bytes {
        files.insert(
            format!("objects/{}", capture.object_paths[key]),
            raw.clone(),
        );
    }
    for (path, raw) in &capture.cancellation_bytes {
        files.insert(format!("cancellations/{path}"), raw.clone());
    }
    let retained = V::Map(
        capture
            .inactive_generations
            .iter()
            .map(|(k, g)| (k.clone(), s(&g.digest)))
            .collect(),
    );
    let adapted = crate::history_adapter::from_store_capture(capture)?;
    let mut value = obj([
        (
            "version",
            V::Integer(Integer::new(if capture.inactive_generations.is_empty() {
                "1"
            } else {
                "2"
            })?),
        ),
        (
            "files",
            V::Map(
                files
                    .iter()
                    .map(|(k, r)| (k.clone(), byte_value(r)))
                    .collect(),
            ),
        ),
        (
            "sha256",
            V::Map(
                files
                    .iter()
                    .map(|(k, r)| (k.clone(), s(&sha256(r))))
                    .collect(),
            ),
        ),
        ("rules", map(&capture.state)?["rules"].clone()),
        ("baseline", capture.baseline.clone()),
        ("projection", adapted.projection().clone()),
    ]);
    if !capture.inactive_generations.is_empty() {
        map_mut(&mut value)?.insert("inactive_generations".into(), retained);
    }
    crate::history_bundle::validate_observation(&value, &files)?;
    Ok(value)
}
pub(crate) fn records(
    root: &Path,
    entry: &str,
    revision: &str,
    runtime: Option<&Runtime>,
) -> Result<V> {
    // The caller retains all live project/record guards. A detached immutable
    // revision is replayed in a new temporary directory and must not inherit
    // the caller's directory lock-order namespace.
    std::thread::scope(|scope| {
        scope
            .spawn(|| records_isolated(root, entry, revision, runtime))
            .join()
            .map_err(|_| error("target_replay_failed"))?
    })
}
pub(crate) struct OrdinaryRecords {
    pub document: crate::ordinary_value::Value,
    pub hypotheses: crate::ordinary_value::Map,
}
/// Another branch's committed record, read to be laid over this one: no snapshot of its
/// own is formed, so a record whose field roles read only over this one is not refused
/// here, and one that cannot be read even there is told by the reader that lays it.
pub(crate) fn records_ordinary(
    root: &Path,
    entry: &str,
    revision: &str,
    runtime: Option<&Runtime>,
) -> Result<OrdinaryRecords> {
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let materialized = materialize(root, entry, revision, |paths, scratch, mode| {
                    capture_ordinary_layer(paths, scratch, mode, runtime)
                })?;
                // This adapter is consumed by the ordinary branch reader. Keep
                // a compact record's declared computation semantics intact.
                if materialized.captured.node_history_capture().is_some() {
                    materialized.captured.require_ordinary_reader()?;
                }
                Ok(OrdinaryRecords {
                    document: materialized.captured.ordinary_document().clone(),
                    hypotheses: crate::ordinary_value::map(materialized.captured.hypotheses())?
                        .clone(),
                })
            })
            .join()
            .map_err(|_| error("target_replay_failed"))?
    })
}
/// Another branch's committed record with its hypotheses and file text, in finite values as
/// `records` gives them, and the same documents in the order their files hold them, read to
/// be laid over this one: no snapshot of its own is formed, so its field roles are taken only
/// over this record, by the reader that lays it there. What else the snapshot refused still
/// stands - its size limits here, and an entry held twice where the record is laid, which
/// would lose one body - and a record the core computes is not an ordinary layer.
pub(crate) fn records_layer(
    root: &Path,
    entry: &str,
    revision: &str,
    runtime: Option<&Runtime>,
) -> Result<(V, OrdinaryRecords)> {
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let Materialized {
                    _temp,
                    captured,
                    files,
                    raw_names,
                    active,
                } = materialize(root, entry, revision, |paths, scratch, mode| {
                    capture_ordinary_layer(paths, scratch, mode, runtime)
                })?;
                let ordered = OrdinaryRecords {
                    document: captured.ordinary_document().clone(),
                    hypotheses: crate::ordinary_value::map(captured.hypotheses())?.clone(),
                };
                let (document, hypotheses) = contents(&captured.try_finite()?, active)?;
                require(
                    active || !crate::reasoning_operations::selected(&document)?,
                    "unsupported_capability: use core/v1 consumer",
                )?;
                crate::reasoning_snapshot::validate_layer(&document)?;
                for hypothesis in &hypotheses {
                    crate::reasoning_snapshot::validate_layer(&map(hypothesis)?["doc"])?;
                }
                Ok((
                    obj([
                        ("doc", document),
                        ("hypotheses", V::List(hypotheses)),
                        ("files", texts(&files, &raw_names)?),
                    ]),
                    ordered,
                ))
            })
            .join()
            .map_err(|_| error("target_replay_failed"))?
    })
}

/// A missing record is an empty merge base only when its owned evidence is also
/// absent. A broken pointer or a missing view of retained history must still fail.
pub(crate) fn comparison_layer(
    root: &Path,
    entry: &str,
    revision: &str,
    runtime: Option<&Runtime>,
) -> Result<V> {
    let captured = committed(root, entry, revision)?;
    if captured.reader.tree.contains_key(&captured.entry) {
        let (layer, _) = records_layer(root, entry, revision, runtime)?;
        return Ok(obj([("doc", map(&layer)?["doc"].clone())]));
    }
    let mut layouts = vec![captured.layout];
    let entry_path = Path::new(&captured.entry);
    if matches!(
        entry_path.file_name().and_then(|n| n.to_str()),
        Some("GROUNDING.yaml" | "PROVENANCE.yaml")
    ) {
        let alternate = if entry_path.file_name().unwrap() == "GROUNDING.yaml" {
            "PROVENANCE.yaml"
        } else {
            "GROUNDING.yaml"
        };
        layouts.push(crate::history_transaction::Layout::for_entry(name(
            &entry_path.with_file_name(alternate),
        )?)?);
    }
    for layout in layouts {
        let owned = [
            &layout.hypotheses,
            &layout.view,
            &layout.replaced,
            &layout.authority,
            &layout.objects,
            &layout.commits,
            &layout.cancellations,
            &layout.retained,
            &layout.journal,
        ];
        require(
            !captured.reader.tree.keys().any(|path| {
                owned
                    .iter()
                    .any(|base| path == *base || path.starts_with(&format!("{base}/")))
            }),
            "target_record_unavailable",
        )?;
    }
    Ok(obj([("doc", V::Map(Map::new()))]))
}
struct Materialized {
    _temp: tempfile::TempDir,
    captured: crate::source_capture::OrdinaryCapture,
    files: Files,
    raw_names: BTreeSet<String>,
    active: bool,
}
/// A committed tree read as far as its record's authority marker: the tree's files, the
/// record's entry and layout, and whether the record is kept by its history.
struct Committed<'a> {
    reader: Reader<'a>,
    entry: String,
    layout: crate::history_transaction::Layout,
    active: bool,
    /// Kept by compact node history; read only through its verified node capture.
    node: bool,
}
/// Whether the record committed at `revision` is kept by its history. An ordinary reader
/// stops at the authority marker, so this reads nothing beyond it.
pub(crate) fn kept_by_history(root: &Path, entry: &str, revision: &str) -> Result<bool> {
    Ok(committed(root, entry, revision)?.active)
}
fn committed<'a>(root: &'a Path, entry: &str, revision: &str) -> Result<Committed<'a>> {
    require(
        [40, 64].contains(&revision.len())
            && revision
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid_target_revision",
    )?;
    let raw = P::git(
        root,
        &["ls-tree", "-r", "-z", revision],
        16 * 1024 * 1024,
        false,
    )?
    .unwrap();
    let mut reader = Reader {
        root,
        tree: BTreeMap::new(),
    };
    for row in raw.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let split = row
            .iter()
            .position(|b| *b == b'\t')
            .ok_or_else(|| error("invalid_target_tree"))?;
        let info = std::str::from_utf8(&row[..split])
            .map_err(|_| error("invalid_target_tree"))?
            .split(' ')
            .collect::<Vec<_>>();
        require(info.len() == 3, "invalid_target_tree")?;
        let path =
            std::str::from_utf8(&row[split + 1..]).map_err(|_| error("invalid_target_path"))?;
        reader.tree.insert(
            path.into(),
            (info[0].into(), info[1].into(), info[2].into()),
        );
        require(reader.tree.len() <= 100_000, "target_limit")?;
    }
    let mut entry = safe(entry)?;
    if !reader.tree.contains_key(&entry)
        && [Some("GROUNDING.yaml"), Some("PROVENANCE.yaml")]
            .contains(&Path::new(&entry).file_name().and_then(|v| v.to_str()))
    {
        let parent = Path::new(&entry).parent().unwrap();
        for alternate in ["GROUNDING.yaml", "PROVENANCE.yaml"] {
            let p = name(&parent.join(alternate))?.to_owned();
            if reader.tree.contains_key(&p) {
                entry = p;
                break;
            }
        }
    }
    let layout = crate::history_transaction::Layout::for_entry(&entry)?;
    let mut node = false;
    let marker = if reader.tree.contains_key(&layout.authority) {
        let raw = reader.read(&layout.authority, 1024 * 1024)?;
        let value = Y::decode_document(&raw)?;
        if map(&value)?
            .get("profile")
            .is_some_and(|v| string_is(v, crate::history_node_codec::FORMAT))
        {
            crate::history_node_publication::validate_authority(&value)?;
            node = true;
        } else {
            crate::history_authority::validate_authority(&value)?;
        }
        Some(value)
    } else {
        None
    };
    let active = marker
        .as_ref()
        .is_some_and(|m| string_is(&map(m).unwrap()["authority"], "history"));
    Ok(Committed {
        reader,
        entry,
        layout,
        active,
        node,
    })
}
/// A compact target's verified semantic view and named layers, pinned at `revision`.
fn node_records(root: &Path, entry: &str, revision: &str) -> Result<V> {
    let capture = crate::history_branch_git::capture_node(root, revision, entry)?.capture()?;
    let projected = crate::history_node_projection::capture(&capture)?;
    let (named, _) = crate::history_hypotheses::layers(projected.projection(), capture.document())?;
    let mut hypotheses = vec![];
    for (name, layer) in map(&named)? {
        let layer = map(layer)?;
        hypotheses.push(obj([
            ("name", s(name)),
            (
                "doc",
                layer
                    .get("doc")
                    .or_else(|| layer.get("document"))
                    .ok_or_else(|| error("invalid_target_hypothesis"))?
                    .clone(),
            ),
            ("head", layer.get("head").cloned().unwrap_or(V::Null)),
        ]));
    }
    let node_history = obj([
        ("authority", capture.snapshot.authority.clone()),
        ("revision", s(capture.revision())),
    ]);
    Ok(obj([
        ("doc", capture.document().clone()),
        ("hypotheses", V::List(hypotheses)),
        (
            "hash",
            s(&sha256(&crate::history_emit::encode_document(
                &node_history,
            )?)),
        ),
        ("node_history", node_history),
    ]))
}
fn materialize(
    root: &Path,
    entry: &str,
    revision: &str,
    capture: impl FnOnce(
        &[std::path::PathBuf],
        &Path,
        ReadMode,
    ) -> Result<crate::source_capture::OrdinaryCapture>,
) -> Result<Materialized> {
    let Committed {
        reader,
        entry,
        layout,
        active,
        node,
    } = committed(root, entry, revision)?;
    if node {
        // Replay only the pinned, verified compact closure. Feeding the physical
        // YAML to an ordinary loader would discard its history and field semantics.
        let observation = crate::history_branch_git::capture_node(root, revision, &entry)?;
        let temp = observation.bundle.reconstruct()?;
        let scratch = temp.path().canonicalize()?;
        let captured = capture(
            &[scratch.join("GROUNDING.yaml")],
            &scratch,
            ReadMode::Frozen,
        )?;
        let parent = Path::new(&entry).parent().unwrap_or(Path::new(""));
        let mut files = Files::new();
        let mut raw_names = BTreeSet::new();
        for (relative, raw) in observation.bundle.files()? {
            let path = safe(name(&parent.join(&relative))?)?;
            if path != entry && !path.starts_with(&format!("{}/", layout.hypotheses)) {
                raw_names.insert(path.clone());
            }
            files.insert(path, raw);
        }
        return Ok(Materialized {
            _temp: temp,
            captured,
            files,
            raw_names,
            active: true,
        });
    }
    let maximum = if active {
        64 * 1024 * 1024
    } else {
        4 * 1024 * 1024
    };
    let limit = if active { 2 * MAX_OBJECTS + 128 } else { 128 };
    let mut queue = VecDeque::from([entry.clone()]);
    let mut raw_names = BTreeSet::new();
    if active {
        queue.push_back(layout.authority.clone());
        raw_names.insert(layout.authority.clone());
        for (name, (mode, kind, _)) in &reader.tree {
            if [&layout.objects, &layout.commits, &layout.cancellations]
                .iter()
                .any(|dir| name.starts_with(&format!("{dir}/")))
            {
                require(mode == "100644" && kind == "blob", "nonregular_target_file")?;
                queue.push_back(safe(name)?);
                raw_names.insert(name.clone());
            }
        }
    }
    for name in reader.tree.keys().filter(|p| {
        p.starts_with(&format!("{}/", layout.hypotheses))
            && (p.ends_with(".yaml") || p.ends_with(".yml"))
    }) {
        queue.push_back(name.clone());
    }
    let mut files = Files::new();
    let mut total = 0;
    while let Some(path) = queue.pop_front() {
        let path = safe(&path)?;
        if files.contains_key(&path) {
            continue;
        }
        require(files.len() < limit, "target_limit")?;
        let raw = reader.read(&path, if active { 16 * 1024 * 1024 } else { maximum })?;
        total += raw.len();
        require(total <= maximum, "target_limit")?;
        files.insert(path.clone(), raw.clone());
        if raw_names.contains(&path) {
            continue;
        }
        let body = Y::decode_full_ordinary_source_value(&raw)?;
        if active && path == entry {
            if let Some(imported) = body.get("meta").and_then(|v| v.get("history_import")) {
                let value = imported.strict_typed()?;
                for member in list(&map(&value)?["members"])? {
                    let p = safe(name(
                        &Path::new(&entry)
                            .parent()
                            .unwrap()
                            .join(text(&map(member)?["path"])?),
                    )?)?;
                    queue.push_back(p.clone());
                    raw_names.insert(p);
                }
            }
        } else {
            for key in ["record", "also"] {
                let values = match body.get(key) {
                    Some(
                        v @ crate::ordinary_source::Source::Scalar(
                            crate::ordinary_value::Scalar::Finite(V::Text(_)),
                        ),
                    ) => vec![v],
                    Some(crate::ordinary_source::Source::List(a)) => a.iter().collect(),
                    Some(crate::ordinary_source::Source::Map(m)) => {
                        m.iter().map(|(_, v)| v).collect()
                    }
                    _ => vec![],
                };
                for value in values {
                    if let crate::ordinary_source::Source::Scalar(
                        crate::ordinary_value::Scalar::Finite(V::Text(p)),
                    ) = value
                        && (p.ends_with(".yaml") || p.ends_with(".yml"))
                    {
                        queue.push_back(pointer(&Path::new(&path).parent().unwrap().join(p))?);
                    }
                }
            }
        }
    }
    let temp = tempfile::tempdir()?;
    let scratch = temp.path().canonicalize()?;
    for (path, raw) in &files {
        let path = scratch.join(path);
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(path, raw)?;
    }
    let captured = capture(
        &[scratch.join(&entry)],
        &scratch,
        if active {
            ReadMode::Frozen
        } else {
            ReadMode::Live
        },
    )?;
    Ok(Materialized {
        _temp: temp,
        captured,
        files,
        raw_names,
        active,
    })
}
fn records_isolated(
    root: &Path,
    entry: &str,
    revision: &str,
    runtime: Option<&Runtime>,
) -> Result<V> {
    if committed(root, entry, revision)?.node {
        return node_records(root, entry, revision);
    }
    let Materialized {
        _temp,
        captured,
        files,
        raw_names,
        active,
    } = materialize(root, entry, revision, |paths, scratch, mode| {
        capture_ordinary_source_with_runtime(paths, scratch, mode, None, runtime)
    })?;
    let captured = captured.try_finite()?;
    let (document, hypotheses) = contents(&captured, active)?;
    let hashes = V::Map(
        files
            .iter()
            .map(|(path, raw)| {
                Ok((
                    path.clone(),
                    if active {
                        s(&sha256(raw))
                    } else {
                        s(std::str::from_utf8(raw).map_err(|_| error("invalid_target_text"))?)
                    },
                ))
            })
            .collect::<Result<Map>>()?,
    );
    let hash = sha256(&crate::history_emit::encode_document(&hashes)?);
    let mut output = obj([
        ("doc", document.clone()),
        ("hypotheses", V::List(hypotheses.clone())),
        ("hash", s(&hash)),
    ]);
    let snapshot = if active {
        map_mut(&mut output)?.insert(
            "history".into(),
            history_evidence(captured.history_capture().unwrap())?,
        );
        captured.snapshot()?.to_data()
    } else if crate::reasoning_operations::selected(&document)? {
        require(runtime.is_some(), "target_runtime_required")?;
        let context = crate::reasoning_context::CapturedAssessment::from_snapshot(
            captured.snapshot()?.clone(),
            None,
            "focused-review/v1",
            runtime,
            OperationalBounds::default(),
            None,
        )?;
        map_mut(&mut output)?.insert(
            "core".into(),
            obj([
                ("snapshot", s(&captured.snapshot()?.to_json()?)),
                ("assessment", context.base_assessment().clone()),
                ("findings", crate::reasoning_operations::findings(&context)?),
            ]),
        );
        captured.snapshot()?.to_data()
    } else {
        let hyps = V::Map(
            hypotheses
                .iter()
                .map(|h| {
                    let h = map(h).unwrap();
                    (
                        text(&h["name"]).unwrap().into(),
                        obj([("doc", h["doc"].clone()), ("head", h["head"].clone())]),
                    )
                })
                .collect(),
        );
        Snapshot::from_data(
            &document,
            CaptureOptions {
                hypotheses: Some(hyps),
                context: Some(obj([
                    ("read_mode", s("supplied")),
                    ("watch_capture", obj([("hash", s(&hash))])),
                ])),
                ..Default::default()
            },
        )?
        .to_data()
    };
    map_mut(&mut output)?.insert("snapshot".into(), snapshot);
    map_mut(&mut output)?.insert("files".into(), texts(&files, &raw_names)?);
    Ok(output)
}
/// A captured record's document, and each hypothesis beside it by name with its document
/// and head.
fn contents(captured: &CapturedSource, active: bool) -> Result<(V, Vec<V>)> {
    let document = captured.strict_document()?;
    let mut hypotheses = vec![];
    for (name, hyp) in map(captured.hypotheses())? {
        let hyp = map(hyp)?;
        require(
            !hyp.get("error").is_some_and(truth),
            "unreadable_target_hypothesis",
        )?;
        let mut value = obj([
            ("name", s(name)),
            (
                "doc",
                hyp.get("doc")
                    .or_else(|| hyp.get("document"))
                    .ok_or_else(|| error("invalid_target_hypothesis"))?
                    .clone(),
            ),
            ("head", hyp["head"].clone()),
        ]);
        if active && let Some(kind) = hyp.get("kind").filter(|v| truth(v)) {
            map_mut(&mut value)?.insert("kind".into(), kind.clone());
        }
        hypotheses.push(value);
    }
    Ok((document, hypotheses))
}
/// The text of each file read, the history's own objects aside.
fn texts(files: &Files, raw_names: &BTreeSet<String>) -> Result<V> {
    Ok(V::Map(
        files
            .iter()
            .filter(|(p, _)| !raw_names.contains(*p))
            .map(|(p, r)| {
                Ok((
                    p.clone(),
                    s(std::str::from_utf8(r).map_err(|_| error("invalid_target_text"))?),
                ))
            })
            .collect::<Result<Map>>()?,
    ))
}

#[cfg(test)]
mod path_tests {
    use super::*;
    #[test]
    fn entries_and_imports_refuse_parent_components_but_pointers_normalize_within_root() {
        assert!(safe("sub/../GROUNDING.yaml").is_err());
        assert_eq!(
            pointer(Path::new("sub/../GROUNDING.yaml")).unwrap(),
            "GROUNDING.yaml"
        );
        assert!(pointer(Path::new("sub/../../escape.yaml")).is_err());
        assert!(safe("/outside.yaml").is_err());
        assert!(safe("C:\\outside.yaml").is_err());
    }
}

#[cfg(test)]
mod compact_layer_tests {
    use super::*;
    use crate::{history_authoring::Options, history_node_writer as W, history_paths::Scheme};
    use std::fs;
    use std::process::Command;

    fn git(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.test",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().into()
    }
    fn commit(root: &Path) -> String {
        git(root, &["add", "."]);
        git(root, &["commit", "-qm", "fixture"]);
        git(root, &["rev-parse", "HEAD"])
    }
    fn fixture() -> (tempfile::TempDir, String) {
        fixture_with_core(false)
    }
    fn fixture_with_core(core: bool) -> (tempfile::TempDir, String) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        git(root, &["init", "-q", "-b", "main"]);
        let record = root.join("nested");
        fs::create_dir_all(record.join(".kpopper")).unwrap();
        fs::write(record.join(".kpopper/history.yaml"), "version: 3\nprofile: node-history/v1\nauthority: history\nrecord_id: fixture\ngeneration: 1\nrequires: [node-history/v1]\n").unwrap();
        fs::write(record.join("GROUNDING.yaml"), "meta: {purpose: Fixture}\nschema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown: {}\n").unwrap();
        let options = Options {
            operation: "seed".into(),
            recorded_at: "2026-09-25T12:00:00+00:00".into(),
            recording_day: "2026-09-25".into(),
            by: s("Fixture"),
            strict: false,
            paths: Scheme::Hashed,
            receipt_version: None,
        };
        let action =
            V::from_json(&serde_json::json!({"kind":"add", "id":"p.value", "body":{"v":7}}))
                .unwrap();
        let cache = tempfile::tempdir().unwrap();
        let runtime = if core {
            fs::write(record.join("GROUNDING.yaml"), "meta: {reasoning: {version: 2, profile: core/v1, requires: [arithmetic/v1]}}\nschema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown: {}\n").unwrap();
            let archive = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../scripts/reasoning/native")
                .join(format!(
                    "{}.kpopper-runtime",
                    crate::reasoning_runtime::target_name().unwrap()
                ));
            Some(Runtime::open(&archive, cache.path(), OperationalBounds::default()).unwrap())
        } else {
            None
        };
        let prepared = W::prepare(&record, &action, &options, runtime.as_ref()).unwrap();
        W::publish(&record, &prepared, runtime.as_ref(), |_| Ok(())).unwrap();
        if core {
            let action = V::from_json(&serde_json::json!({"kind":"hypothesis", "name":"scenario", "action":{"kind":"set","id":"p.value","value":11}})).unwrap();
            let mut options = options;
            options.operation = "scenario".into();
            let prepared = W::prepare(&record, &action, &options, runtime.as_ref()).unwrap();
            W::publish(&record, &prepared, runtime.as_ref(), |_| Ok(())).unwrap();
        }
        let revision = commit(root);
        (temp, revision)
    }
    #[test]
    fn pinned_compact_layers_and_merge_base_keep_verified_core_semantics() {
        let (temp, revision) = fixture();
        let root = temp.path();
        let entry = "nested/GROUNDING.yaml";
        let before = git(root, &["status", "--porcelain=v1", "--untracked-files=all"]);
        let (layer, ordered) = records_layer(root, entry, &revision, None).unwrap();
        let doc = map(&layer).unwrap()["doc"].clone();
        let capture = crate::history_node_capture::Capture::read(&root.join("nested")).unwrap();
        let projected = crate::history_node_projection::capture(&capture).unwrap();
        assert_eq!(&doc, projected.document());
        assert!(
            map(&map(&doc).unwrap()["meta"])
                .unwrap()
                .contains_key("history")
        );
        assert!(
            map(&layer)
                .unwrap()
                .get("files")
                .and_then(|v| map(v).ok())
                .unwrap()
                .contains_key(entry)
        );
        assert_eq!(
            comparison_layer(root, entry, &revision, None).unwrap(),
            obj([("doc", doc.clone())])
        );
        assert_eq!(
            records_ordinary(root, entry, &revision, None)
                .unwrap()
                .document,
            ordered.document
        );
        assert_eq!(
            git(root, &["status", "--porcelain=v1", "--untracked-files=all"]),
            before
        );
        // A different working-tree view cannot alter a pinned committed reading.
        fs::write(root.join(entry), b"known: {p.value: {v: 99}}\n").unwrap();
        assert_eq!(
            comparison_layer(root, entry, &revision, None).unwrap(),
            obj([("doc", doc)])
        );
    }
    #[test]
    fn ordinary_public_branch_read_refuses_compact_core_interpretation() {
        let (temp, revision) = fixture_with_core(true);
        let root = temp.path();
        fs::remove_dir_all(root.join("nested/.kpopper")).unwrap();
        fs::write(
            root.join("nested/GROUNDING.yaml"),
            "known: {p.value: {v: 8}}\n",
        )
        .unwrap();
        commit(root);
        let before = git(root, &["status", "--porcelain=v1", "--untracked-files=all"]);
        let error = crate::public_readers::run(
            "pull",
            &crate::public_readers::Options {
                subjects: vec!["p.value".into(), "nested/GROUNDING.yaml".into()],
                from_ref: Some(revision.clone()),
                ..Default::default()
            },
            root,
            ReadMode::Frozen,
            crate::public_readers::Reply::Text,
            &|_| Ok(None),
            &mut None,
        )
        .unwrap_err();
        assert_eq!(error.0, crate::source_capture::CORE_CONSUMER);
        let output = crate::public_consolidation::dispatch(
            &crate::public_consolidation::Options {
                from_refs: vec![revision],
                record: Some(root.join("nested/GROUNDING.yaml")),
                dry_run: true,
                frozen: true,
                ..Default::default()
            },
            root,
        );
        assert_eq!(
            (output.code, output.stderr.as_str()),
            (1, "unsupported_capability: use core/v1 consumer\n")
        );
        assert_eq!(
            git(root, &["status", "--porcelain=v1", "--untracked-files=all"]),
            before
        );
    }
    #[test]
    fn compact_named_layers_keep_their_document_and_provenance() {
        let (temp, revision) = fixture_with_core(true);
        let (layer, ordered) =
            records_layer(temp.path(), "nested/GROUNDING.yaml", &revision, None).unwrap();
        let hypotheses = list(&map(&layer).unwrap()["hypotheses"]).unwrap();
        assert_eq!(hypotheses.len(), 1);
        let hypothesis = map(&hypotheses[0]).unwrap();
        assert_eq!(hypothesis["name"], s("scenario"));
        assert_eq!(
            map(&map(&hypothesis["doc"]).unwrap()["known"]).unwrap()["p.value"]
                .to_json()
                .unwrap()["v"],
            11
        );
        let ordered_hypothesis =
            crate::ordinary_value::map(&ordered.hypotheses["scenario"]).unwrap();
        assert_eq!(
            ordered_hypothesis["head"].try_typed().unwrap(),
            hypothesis["head"]
        );
    }
    #[test]
    fn ordinary_public_pull_can_compare_a_compact_unprofiled_record() {
        let (temp, revision) = fixture();
        let root = temp.path();
        fs::remove_dir_all(root.join("nested/.kpopper")).unwrap();
        fs::write(
            root.join("nested/GROUNDING.yaml"),
            "known: {p.value: {v: 8}}\n",
        )
        .unwrap();
        commit(root);
        let output = crate::public_readers::run(
            "pull",
            &crate::public_readers::Options {
                subjects: vec!["p.value".into(), "nested/GROUNDING.yaml".into()],
                from_ref: Some(revision),
                ..Default::default()
            },
            root,
            ReadMode::Frozen,
            crate::public_readers::Reply::Text,
            &|_| Ok(None),
            &mut None,
        )
        .unwrap();
        assert_eq!(output.code, 0);
        assert!(output.text.contains("proposes 8 -> 7"), "{}", output.text);
    }
    #[test]
    fn ordinary_branch_delta_reads_a_compact_merge_base() {
        let (temp, _) = fixture();
        let root = temp.path();
        git(root, &["switch", "-qc", "source"]);
        fs::remove_dir_all(root.join("nested/.kpopper")).unwrap();
        fs::write(
            root.join("nested/GROUNDING.yaml"),
            "known: {p.value: {v: 7}, p.added: {v: 9}}\n",
        )
        .unwrap();
        commit(root);
        git(root, &["switch", "-q", "main"]);
        fs::remove_dir_all(root.join("nested/.kpopper")).unwrap();
        fs::write(
            root.join("nested/GROUNDING.yaml"),
            "known: {p.value: {v: 10}, p.local: {v: 8}}\n",
        )
        .unwrap();
        commit(root);
        let before = fs::read(root.join("nested/GROUNDING.yaml")).unwrap();
        let output = crate::public_consolidation::dispatch(
            &crate::public_consolidation::Options {
                from_refs: vec!["source".into()],
                record: Some(root.join("nested/GROUNDING.yaml")),
                dry_run: true,
                frozen: true,
                ..Default::default()
            },
            root,
        );
        assert_eq!(output.code, 0, "{}{}", output.stdout, output.stderr);
        assert!(output.stdout.contains("p.added"), "{}", output.stdout);
        assert!(
            !output.stdout.contains("p.value: 10 -> 7"),
            "{}",
            output.stdout
        );
        assert_eq!(
            fs::read(root.join("nested/GROUNDING.yaml")).unwrap(),
            before
        );
    }
    #[test]
    fn compact_merge_base_without_view_is_not_an_empty_record() {
        let (temp, _) = fixture();
        fs::remove_file(temp.path().join("nested/GROUNDING.yaml")).unwrap();
        let revision = commit(temp.path());
        assert_eq!(
            comparison_layer(temp.path(), "nested/GROUNDING.yaml", &revision, None)
                .unwrap_err()
                .0,
            "target_record_unavailable"
        );
    }
    #[test]
    fn compact_comparison_rejects_a_view_that_disagrees_with_history() {
        let (temp, _) = fixture();
        fs::write(
            temp.path().join("nested/GROUNDING.yaml"),
            b"known: {p.value: {v: 99}}\n",
        )
        .unwrap();
        let revision = commit(temp.path());
        assert!(records_layer(temp.path(), "nested/GROUNDING.yaml", &revision, None).is_err());
        assert!(comparison_layer(temp.path(), "nested/GROUNDING.yaml", &revision, None).is_err());
    }
}
