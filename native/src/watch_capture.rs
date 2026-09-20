//! Bounded local record capture for watch; snapshots never fetch remote objects.
use crate::{
    Result,
    history_authoring::{obj, s},
    history_contract::{Map, map, text},
    history_view::{list, map_mut, truth},
    require,
    value::TypedValue as V,
    watch_store::{Watch, digest, digest_value, error, git},
};
use serde_json::{Value as J, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    path::Path,
};
#[derive(Clone)]
pub struct Snapshot {
    pub data: V,
    pub value: J,
}
impl std::ops::Deref for Snapshot {
    type Target = J;
    fn deref(&self) -> &J {
        &self.value
    }
}
fn safe(path: &Path) -> Result<String> {
    require(
        !path.is_absolute(),
        "record pointer must use a portable path inside its checkout",
    )?;
    let mut parts = vec![];
    for part in path.components() {
        match part {
            std::path::Component::Normal(p) => parts.push(p.to_string_lossy().into_owned()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                require(
                    !parts.is_empty(),
                    "record pointer must use a portable path inside its checkout",
                )?;
                parts.pop();
            }
            _ => {
                return Err(error(
                    "record pointer must use a portable path inside its checkout",
                ));
            }
        }
    }
    require(
        !parts.is_empty() && !parts.iter().any(|p| p.contains('\\') || p.contains(':')),
        "record pointer must use a portable path inside its checkout",
    )?;
    Ok(parts.join("/"))
}
fn tree_files(root: &Path, folder: &str) -> Result<Vec<String>> {
    fn visit(root: &Path, path: &Path, out: &mut Vec<String>) -> Result<()> {
        if !path.exists() {
            return Ok(());
        }
        for entry in fs::read_dir(path)? {
            let path = entry?.path();
            require(!path.is_symlink(), "history target contains a symlink")?;
            if path.is_dir() {
                visit(root, &path, out)?
            } else if path.is_file() {
                out.push(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
        Ok(())
    }
    let mut output = vec![];
    visit(root, &root.join(folder), &mut output)?;
    output.sort();
    Ok(output)
}
pub fn working(root: &Path, entry: &str, include_files: bool) -> Result<V> {
    let layout = crate::history_transaction::Layout::for_entry(entry)?;
    let marker = root.join(&layout.authority);
    let active = if marker.is_file() {
        let value = crate::history_yaml::decode_document(&fs::read(&marker)?)?;
        crate::history_authority::validate_authority(&value)?;
        map(&value)?["authority"] == s("history")
    } else {
        false
    };
    let maximum = if active {
        64 * 1024 * 1024
    } else {
        4 * 1024 * 1024
    };
    let limit = if active {
        2 * crate::history_contract::MAX_OBJECTS + 128
    } else {
        128
    };
    let mut queue = VecDeque::from([entry.to_owned()]);
    let mut raw_names = BTreeSet::new();
    if active {
        queue.push_back(layout.authority.clone());
        raw_names.insert(layout.authority.clone());
        for folder in [&layout.objects, &layout.commits, &layout.cancellations] {
            for file in tree_files(root, folder)? {
                queue.push_back(file.clone());
                raw_names.insert(file);
            }
        }
    }
    let hypdir = root.join(&layout.hypotheses);
    if hypdir.is_dir() {
        let mut paths = fs::read_dir(&hypdir)?
            .map(|p| p.map(|p| p.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        paths.sort();
        for path in paths {
            if path.extension().is_some_and(|e| e == "yaml" || e == "yml") {
                queue.push_back(safe(path.strip_prefix(root).unwrap())?);
            }
        }
    }
    let mut files = BTreeMap::new();
    let mut total = 0usize;
    while let Some(name) = queue.pop_front() {
        let name = safe(Path::new(&name))?;
        if files.contains_key(&name) {
            continue;
        }
        require(
            files.len() < limit,
            "record closure exceeds captured file limit",
        )?;
        let path = root.join(&name);
        let resolved = path.canonicalize().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                error(format!(
                    "[Errno 2] No such file or directory: '{}'",
                    path.display()
                ))
            } else {
                error(e.to_string())
            }
        })?;
        require(
            resolved.starts_with(root.canonicalize()?),
            "record pointer must use a portable path inside its checkout",
        )?;
        let cap = if active {
            16 * 1024 * 1024
        } else {
            4 * 1024 * 1024
        };
        require(
            fs::metadata(&path)?.len() <= cap as u64,
            "record file exceeds captured byte limit",
        )?;
        let raw = fs::read(path)?;
        total += raw.len();
        require(
            raw.len() <= maximum && total <= maximum,
            "record closure exceeds captured byte limit",
        )?;
        files.insert(name.clone(), raw.clone());
        if raw_names.contains(&name) {
            continue;
        }
        let body = crate::history_yaml::decode_document(&raw)?;
        if active && name == entry {
            if let Some(meta) = map(&body)?.get("meta").and_then(|v| map(v).ok())
                && let Some(import) = meta.get("history_import").and_then(|v| map(v).ok())
            {
                for member in import.get("members").map(list).transpose()?.unwrap_or(&[]) {
                    let name = safe(
                        &Path::new(entry)
                            .parent()
                            .unwrap()
                            .join(text(&map(member)?["path"])?),
                    )?;
                    raw_names.insert(name.clone());
                    queue.push_back(name);
                }
            }
        } else {
            for key in ["record", "also"] {
                let values = match map(&body)?.get(key) {
                    Some(v @ V::Text(_)) => vec![v],
                    Some(V::List(v)) => v.iter().collect(),
                    Some(V::Map(v)) => v.values().collect(),
                    _ => vec![],
                };
                for value in values {
                    if let V::Text(value) = value
                        && (value.ends_with(".yaml") || value.ends_with(".yml"))
                    {
                        queue.push_back(safe(&Path::new(&name).parent().unwrap().join(value))?);
                    }
                }
            }
        }
    }
    materialize(&files, entry, active, &raw_names, include_files)
}
fn materialize(
    files: &BTreeMap<String, Vec<u8>>,
    entry: &str,
    active: bool,
    raw_names: &BTreeSet<String>,
    include_files: bool,
) -> Result<V> {
    let temp = tempfile::tempdir()?;
    let scratch = temp.path().canonicalize()?;
    for (name, raw) in files {
        let path = scratch.join(name);
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(path, raw)?;
    }
    let raw_doc = crate::history_yaml::decode_document(
        files
            .get(entry)
            .ok_or_else(|| error("record unavailable"))?,
    )?;
    let runtime = crate::public_workspace::runtime_for_document(&raw_doc)?;
    let captured = crate::source_capture::capture_source_with_runtime(
        &[scratch.join(entry)],
        &scratch,
        if active {
            crate::source_capture::ReadMode::Frozen
        } else {
            crate::source_capture::ReadMode::Live
        },
        None,
        runtime.as_ref(),
    )?;
    let document = captured.strict_document()?;
    let mut hypotheses = vec![];
    for (name, hyp) in map(captured.hypotheses())? {
        let hyp = map(hyp)?;
        require(
            !hyp.get("error").is_some_and(truth),
            &format!("unreadable hypothesis {name}"),
        )?;
        let mut row = obj([
            ("name", s(name)),
            (
                "doc",
                hyp.get("doc")
                    .or_else(|| hyp.get("document"))
                    .ok_or_else(|| error("invalid hypothesis"))?
                    .clone(),
            ),
            ("head", hyp["head"].clone()),
        ]);
        if active && let Some(kind) = hyp.get("kind").filter(|v| truth(v)) {
            map_mut(&mut row)?.insert("kind".into(), kind.clone());
        }
        hypotheses.push(row);
    }
    let hashes = V::Map(
        files
            .iter()
            .map(|(path, raw)| {
                Ok((
                    path.clone(),
                    if active {
                        s(&crate::identity::sha256(raw))
                    } else {
                        s(std::str::from_utf8(raw).map_err(|e| error(e.to_string()))?)
                    },
                ))
            })
            .collect::<Result<Map>>()?,
    );
    let hash = digest_value(&hashes)?;
    let mut output = obj([
        ("doc", document.clone()),
        ("hypotheses", V::List(hypotheses.clone())),
        ("hash", s(&hash)),
    ]);
    let snapshot = if active {
        map_mut(&mut output)?.insert(
            "history".into(),
            crate::source_target::history_evidence(captured.history_capture().unwrap())?,
        );
        captured.snapshot()?.to_data()
    } else if crate::reasoning_operations::selected(&document)? {
        let context = crate::reasoning_context::CapturedAssessment::from_snapshot(
            captured.snapshot()?.clone(),
            None,
            "focused-review/v1",
            runtime.as_ref(),
            crate::reasoning_runtime::OperationalBounds::default(),
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
        crate::reasoning_snapshot::Snapshot::from_data(
            &document,
            crate::reasoning_snapshot::CaptureOptions {
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
    if include_files {
        map_mut(&mut output)?.insert(
            "files".into(),
            V::Map(
                files
                    .iter()
                    .filter(|(path, _)| !raw_names.contains(*path))
                    .map(|(path, raw)| {
                        Ok((
                            path.clone(),
                            s(std::str::from_utf8(raw).map_err(|e| error(e.to_string()))?),
                        ))
                    })
                    .collect::<Result<Map>>()?,
            ),
        );
    }
    Ok(output)
}
pub fn snapshot(watch: &Watch) -> Result<Snapshot> {
    let config = watch
        .config()?
        .filter(|c| crate::watch_store::truth(&c["enabled"]))
        .ok_or_else(|| error("watch is not enabled; use kpop watch setup"))?;
    let base = config["base_ref"]
        .as_str()
        .ok_or_else(|| error("missing watch base_ref"))?;
    let main = git(
        &watch.tree,
        &["rev-parse", "--verify", &format!("{base}^{{commit}}")],
    )?;
    let head = git(&watch.tree, &["rev-parse", "--verify", "HEAD^{commit}"])?;
    let ancestor = git(&watch.tree, &["merge-base", &head, &main])?;
    let shared = if let Some(path) = config["shared_record"].as_str() {
        let path = Path::new(path);
        working(
            path.parent().unwrap(),
            path.file_name().unwrap().to_str().unwrap(),
            false,
        )?
    } else {
        V::Null
    };
    let inbox = if shared != V::Null {
        json!(digest(&json!(crate::watch_shared::receipts(watch)?))?)
    } else {
        J::Null
    };
    let working = working(&watch.tree, &watch.entry, false)?;
    let runtime = crate::public_workspace::runtime_for_document(&map(&working)?["doc"])?;
    let mut ancestor_record =
        crate::source_target::records(&watch.tree, &watch.entry, &ancestor, runtime.as_ref())?;
    map_mut(&mut ancestor_record)?.remove("files");
    let mut main_record =
        crate::source_target::records(&watch.tree, &watch.entry, &main, runtime.as_ref())?;
    map_mut(&mut main_record)?.remove("files");
    let versions = json!({"base_ref":base,"main":main,"head":head,"merge_base":ancestor,"comparison":"history-scenarios/v1","working":text(&map(&working)?["hash"])?,"shared":if shared!=V::Null{Some(text(&map(&shared)?["hash"])?.to_owned())}else{None},"inbox":inbox,"config":digest(&config)?,"freshness":"local Git objects; remote freshness not verified"});
    let data = obj([
        ("working", working),
        ("ancestor", ancestor_record),
        ("main", main_record),
        ("shared", shared),
        ("versions", V::from_json(&versions)?),
        ("identity", s(&digest(&versions)?)),
    ]);
    let value = crate::ordinary_reader::json_value(&data, 0)?;
    Ok(Snapshot { data, value })
}
