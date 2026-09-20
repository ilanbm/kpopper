use super::{Failure, Options, Result};
use crate::{
    history_contract::{Map, map, text},
    history_view::truth,
    history_yaml::OrdinaryValue as O,
    identity::sha256,
    reasoning_context::CapturedAssessment,
    reasoning_runtime::OperationalBounds,
    source_capture::{ReadMode, capture_source_with_runtime},
    source_inventory::{Inventory, name},
    value::TypedValue as V,
};
use serde_json::{Value as J, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
};
pub(super) const SOURCE_BYTES: usize = 1024 * 1024;
pub(super) fn get<'a>(value: &'a V, key: &str) -> &'a V {
    match value {
        V::Map(m) => m.get(key).unwrap_or(&V::Null),
        _ => &V::Null,
    }
}
pub(super) fn json_value(value: &V) -> crate::Result<J> {
    Ok(crate::json_ingress::parse_str(
        &crate::ordinary_assessment_report::legacy_json(value)?,
        crate::json_ingress::DuplicateKeys::LastWins,
    )?)
}
pub(super) fn expanded(cwd: &Path, path: &Path) -> crate::Result<PathBuf> {
    let path = if let Ok(relative) = path.strip_prefix("~") {
        PathBuf::from(
            std::env::var_os("HOME")
                .ok_or_else(|| crate::Error("home directory unavailable".into()))?,
        )
        .join(relative)
    } else {
        cwd.join(path)
    };
    crate::project_modes::resolved(&path)
}
pub(super) fn dump(body: &V, source: Option<&O>) -> crate::Result<String> {
    let source = source
        .filter(|s| s.projected() == *body)
        .cloned()
        .unwrap_or_else(|| O::from_typed(body));
    String::from_utf8(crate::public_ordinary_readers::python_safe_dump_unicode(
        &source,
    )?)
    .map_err(|_| crate::Error("invalid YAML text".into()))
}
pub(super) fn source_body<'a>(source: &'a O, id: &str) -> Option<&'a O> {
    let O::Map(collections) = source else {
        return None;
    };
    collections
        .iter()
        .filter(|(k, _)| {
            !k.text()
                .is_some_and(|k| ["meta", "schema", "record", "also", "hypothesis"].contains(&k))
        })
        .filter_map(|(_, m)| m.get(id))
        .next_back()
}
pub(super) fn full_source(
    value: &crate::ordinary_value::Value,
) -> crate::Result<crate::ordinary_source::Source> {
    use crate::{
        ordinary_source::{Key, Source},
        ordinary_value::{Scalar, Value},
    };
    Ok(match value {
        Value::Map(m) => Source::Map(
            m.iter()
                .map(|(key, value)| Ok((Key::new(m.source_key(key), None)?, full_source(value)?)))
                .collect::<crate::Result<_>>()?,
        ),
        Value::List(a) => Source::List(a.iter().map(full_source).collect::<crate::Result<_>>()?),
        Value::NonFinite(v) => Source::Scalar(Scalar::NonFinite(*v)),
        _ => Source::Scalar(Scalar::Finite(value.try_typed()?)),
    })
}
/// Compare separately parsed source bodies by corresponding structure. Constructor
/// numbers may differ, but repeated and distinct NaNs must retain the same relation.
pub(super) fn full_matches(
    source: &crate::ordinary_source::Source,
    expected: &crate::ordinary_value::Value,
) -> bool {
    use crate::{
        ordinary_source::Source,
        ordinary_value::{NonFiniteFloat as N, Scalar, Value},
    };
    fn scalar(a: &Scalar, b: &Scalar, ids: &mut BTreeMap<u64, u64>) -> bool {
        match (a, b) {
            (Scalar::NonFinite(N::ConstructedNaN(a)), Scalar::NonFinite(N::ConstructedNaN(b))) => {
                if let Some(prior) = ids.get(a) {
                    *prior == *b
                } else if ids.values().any(|old| old == b) {
                    false
                } else {
                    ids.insert(*a, *b);
                    true
                }
            }
            _ => a == b,
        }
    }
    fn visit(a: &Source, b: &Value, ids: &mut BTreeMap<u64, u64>) -> bool {
        match (a, b) {
            (Source::Scalar(a), Value::NonFinite(b)) => scalar(a, &Scalar::NonFinite(*b), ids),
            (Source::Scalar(Scalar::Finite(a)), b) => b.try_typed().is_ok_and(|b| *a == b),
            (Source::List(a), Value::List(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(a, b)| visit(a, b, ids))
            }
            (Source::Map(a), Value::Map(b)) if a.len() == b.len() => {
                if a.iter().all(|(k, _)| k.text().is_some()) {
                    a.iter().all(|(key, value)| {
                        b.get(key.text().unwrap())
                            .is_some_and(|b| visit(value, b, ids))
                    })
                } else {
                    a.iter().zip(b.iter()).all(|((ak, av), (bk, bv))| {
                        scalar(ak.scalar(), &b.source_key(bk), ids) && visit(av, bv, ids)
                    })
                }
            }
            _ => false,
        }
    }
    visit(source, expected, &mut BTreeMap::new())
}
pub(super) fn dump_full(
    body: &crate::ordinary_value::Value,
    source: Option<&crate::ordinary_source::Source>,
) -> crate::Result<String> {
    let fallback;
    let source = if let Some(source) = source.filter(|s| full_matches(s, body)) {
        source
    } else {
        fallback = full_source(body)?;
        &fallback
    };
    String::from_utf8(crate::history_emit::encode_ordinary_display(
        source, 80, None,
    )?)
    .map_err(|_| crate::Error("invalid YAML text".into()))
}
pub(super) fn full_source_bodies(
    source: &crate::ordinary_source::Source,
) -> BTreeMap<String, &crate::ordinary_source::Source> {
    use crate::ordinary_source::Source;
    let mut out = BTreeMap::new();
    if let Source::Map(collections) = source {
        for (key, members) in collections {
            if key
                .text()
                .is_some_and(|k| ["meta", "schema", "record", "also", "hypothesis"].contains(&k))
            {
                continue;
            }
            if let Source::Map(members) = members {
                for (key, value) in members {
                    if let Some(key) = key.text() {
                        out.insert(key.to_owned(), value);
                    }
                }
            }
        }
    }
    out
}
pub(super) fn full_source_body<'a>(
    source: &'a crate::ordinary_source::Source,
    id: &str,
) -> Option<&'a crate::ordinary_source::Source> {
    full_source_bodies(source).get(id).copied()
}
pub(super) fn roots(
    record: &Path,
    requested: &[PathBuf],
    cwd: &Path,
) -> crate::Result<Vec<PathBuf>> {
    let roots = std::iter::once(record.parent().unwrap().to_owned())
        .chain(
            requested
                .iter()
                .map(|p| expanded(cwd, p))
                .collect::<crate::Result<Vec<_>>>()?,
        )
        .collect::<BTreeSet<_>>();
    crate::require(
        roots.iter().all(|p| p.is_dir()),
        "source roots must be existing directories",
    )?;
    Ok(roots.into_iter().collect())
}
pub(super) fn text_suffix(path: &Path) -> bool {
    path.extension().and_then(|v| v.to_str()).is_some_and(|v| {
        [
            "txt", "md", "markdown", "rst", "csv", "tsv", "json", "yaml", "yml",
        ]
        .contains(&v.to_lowercase().as_str())
    })
}
pub(super) fn py_error(e: &std::io::Error, path: &Path) -> String {
    let code = e.raw_os_error().unwrap_or(5);
    let label = match e.kind() {
        std::io::ErrorKind::NotFound => "No such file or directory",
        std::io::ErrorKind::PermissionDenied => "Permission denied",
        std::io::ErrorKind::IsADirectory => "Is a directory",
        std::io::ErrorKind::NotADirectory => "Not a directory",
        _ => "Input/output error",
    };
    format!(
        "[Errno {code}] {label}: {}",
        crate::source_text::ordinary_python_repr(&V::Text(path.to_string_lossy().into_owned()))
    )
}
pub(super) fn read_text(path: &Path) -> std::result::Result<(String, Vec<u8>), String> {
    #[cfg(unix)]
    let file = {
        use rustix::fs::{Mode, OFlags, open};
        open(
            path,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map(fs::File::from)
        .map_err(std::io::Error::from)
        .map_err(|e| py_error(&e, path))?
    };
    #[cfg(not(unix))]
    let file = fs::File::open(path).map_err(|e| py_error(&e, path))?;
    if !file.metadata().map_err(|e| py_error(&e, path))?.is_file() {
        return Err(format!("[Errno 21] Is a directory: '{}'", path.display()));
    }
    let mut raw = vec![];
    file.take((SOURCE_BYTES + 1) as u64)
        .read_to_end(&mut raw)
        .map_err(|e| py_error(&e, path))?;
    decode_text(raw)
}
pub(super) fn decode_text(raw: Vec<u8>) -> std::result::Result<(String, Vec<u8>), String> {
    if raw.len() > SOURCE_BYTES {
        return Err("source exceeds the 1 MiB search limit; read it directly".into());
    }
    let content = std::str::from_utf8(&raw)
        .map_err(|e| {
            let at = e.valid_up_to();
            let byte = raw.get(at).copied().unwrap_or(0);
            let length = e.error_len().unwrap_or(raw.len() - at);
            let subject = if length == 1 {
                format!("byte 0x{byte:02x} in position {at}")
            } else {
                format!("bytes in position {at}-{}", at + length - 1)
            };
            format!(
                "'utf-8' codec can't decode {subject}: {}",
                if e.error_len().is_none() {
                    "unexpected end of data"
                } else if byte & 0xc0 == 0x80 || byte >= 0xf5 {
                    "invalid start byte"
                } else {
                    "invalid continuation byte"
                }
            )
        })?
        .to_owned();
    if content.contains('\0') {
        return Err("binary content is not indexed".into());
    }
    Ok((content, raw))
}
fn core_source_ids(id: &str, nodes: &Map, projected: &J) -> Vec<String> {
    let mut found = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut pending = vec![id.to_owned()];
    while let Some(id) = pending.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let Some(finding) = nodes.get(&id) else {
            continue;
        };
        let body = get(finding, "body");
        let Ok(body) = map(body) else { continue };
        let fields = get(finding, "fields");
        if !map(fields)
            .unwrap()
            .values()
            .filter_map(|v| text(v).ok())
            .any(|field| body.contains_key(field))
            && ["file", "url", "asked", "read"]
                .iter()
                .any(|f| body.get(*f).is_some_and(truth))
        {
            found.insert(id.clone());
        }
        if let Some(V::Text(citation)) = body.get("from")
            && nodes.contains_key(citation)
        {
            pending.push(citation.clone());
        }
        if let Some(V::List(deps)) = body.get(text(get(fields, "deps")).unwrap()) {
            pending.extend(
                deps.iter()
                    .filter_map(|v| text(v).ok())
                    .filter(|id| nodes.contains_key(*id))
                    .map(str::to_owned),
            );
        }
        if let Some(deps) = projected[&id]["dependencies"].as_array() {
            pending.extend(
                deps.iter()
                    .filter_map(|v| v["id"].as_str())
                    .filter(|id| nodes.contains_key(*id))
                    .map(str::to_owned),
            );
        }
    }
    found.into_iter().collect()
}
fn core_source_path(
    filename: &str,
    record: &Path,
    snapshot: &V,
    roots: &[PathBuf],
) -> crate::Result<PathBuf> {
    let locator = if filename.starts_with("~/") {
        expanded(Path::new("."), Path::new(filename))?
    } else {
        PathBuf::from(filename)
    };
    if locator.is_absolute() {
        return crate::project_modes::resolved(&locator);
    }
    let mut directories = roots
        .iter()
        .cloned()
        .chain(std::iter::once(record.parent().unwrap().into()))
        .collect::<BTreeSet<_>>();
    if let V::List(files) = get(get(snapshot, "authored_revision"), "files") {
        for file in files {
            if let V::Text(origin) = get(file, "origin")
                && let Some(path) = origin.strip_prefix("origin:")
            {
                directories.insert(
                    crate::project_modes::resolved(&record.parent().unwrap().join(path))?
                        .parent()
                        .unwrap()
                        .into(),
                );
            }
        }
    }
    let existing = directories
        .iter()
        .map(|dir| dir.join(&locator))
        .filter(|p| p.is_file())
        .map(|p| crate::project_modes::resolved(&p))
        .collect::<crate::Result<BTreeSet<_>>>()?;
    crate::require(
        existing.len() <= 1,
        &format!("core_search_source_origin_ambiguous: {filename}"),
    )?;
    existing
        .into_iter()
        .next()
        .map(Ok)
        .unwrap_or_else(|| crate::project_modes::resolved(&record.parent().unwrap().join(locator)))
}
fn core(record: &Path, options: &Options, cwd: &Path, mode: ReadMode) -> Result<J> {
    crate::require(
        options.state_dir.is_none(),
        "core_profile_option_unsupported: --state-dir",
    )?;
    let roots = roots(record, &options.source_roots, cwd)?;
    let runtime = crate::public_workspace::core_runtime()?;
    let capture =
        capture_source_with_runtime(&[record.to_owned()], cwd, mode, None, runtime.as_ref())
            .map_err(|e| Failure::captured(e, "capture"))?;
    let snapshot = crate::reasoning_snapshot::Snapshot::from_snapshot(
        &capture
            .snapshot()
            .map_err(|e| Failure::captured(e, "capture"))?
            .to_data(),
    )
    .map_err(|e| Failure::captured(e, "capture"))?;
    let context = CapturedAssessment::from_snapshot(
        snapshot,
        None,
        "focused-review/v1",
        runtime.as_ref(),
        OperationalBounds::default(),
        None,
    )
    .map_err(|e| Failure::captured(e, "assessment"))?;
    let assessment = context.assessment();
    let view = json_value(context.view())?;
    let snapshot = context.snapshot().to_data();
    let nodes = map(get(assessment, "nodes"))?;
    let mut identity_documents = BTreeMap::new();
    let mut body_files = BTreeMap::new();
    for path in capture.members() {
        if let Some(raw) = capture.files().get(path) {
            let parsed = crate::history_yaml::decode_ordinary_source_value(raw)?.projected();
            for (_, members) in crate::reasoning_fields::collections(&parsed)? {
                for id in members.keys() {
                    body_files.insert(id.clone(), path.clone());
                }
            }
            identity_documents.insert(
                path.clone(),
                super::search_yaml_identity::Document::new(raw)?,
            );
        }
    }

    let mut rows = vec![];
    let mut diagnostics = vec![];
    let mut stamps = BTreeMap::new();
    for (id, finding) in nodes {
        let projection = &view["nodes"][id];
        let body = get(finding, "body");
        let fields = get(finding, "fields");
        if mode == ReadMode::Frozen
            && matches!(body, V::Map(_))
            && crate::recording_privacy::private_marker(body)
        {
            diagnostics.push(json!({"ref":format!("node:{id}"),"reason":"private source excluded from frozen search"}));
            continue;
        }
        let deps = get(body, text(get(fields, "deps"))?);
        let deps = if matches!(deps,V::List(a)if a.iter().all(|v|matches!(v,V::Text(_)))) {
            json_value(deps)?
        } else {
            json!([])
        };
        let sources = core_source_ids(id, nodes, &view["nodes"]);
        let rule_deps = projection["dependencies"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v["id"].as_str())
            .filter(|v| *v != id && nodes.contains_key(*v))
            .collect::<BTreeSet<_>>();
        let mut row = json!({"ref":format!("node:{id}"),"id":id,"kind":if get(get(get(finding,"state"),"basis"),"status")!=&V::Text("not_applicable".into()){"judgment"}else{"entry"},"scope":"record","status":projection["status"],"findings":projection,"name":crate::reasoning_authoring::named(body),"sources":sources,"dependencies":deps,"rule_dependencies":rule_deps,"content":if let Some(document)=body_files.get(id).and_then(|path|identity_documents.get(path)){match document.dump_finite(id,body)?{Some(text)=>text,None=>dump(body,source_body(capture.source(),id))?}}else{dump(body,source_body(capture.source(),id))?}});
        if !projection["status"]["computation"]["value_text"].is_null() {
            row["value_text"] = projection["status"]["computation"]["value_text"].clone();
        }
        rows.push(row);
        if !matches!(body, V::Map(_)) || !sources.contains(id) {
            continue;
        }
        let reference = format!("source:{id}");
        let V::Text(filename) = get(body, "file") else {
            if truth(get(body, "url")) {
                diagnostics.push(json!({"ref":reference,"reason":"remote source not fetched"}));
            }
            continue;
        };
        let path = core_source_path(filename, record, &snapshot, &roots)?;
        let content = (|| {
            if mode == ReadMode::Frozen && !path.starts_with(record.parent().unwrap()) {
                return Err("external evidence".into());
            }
            if !roots.iter().any(|root| path.starts_with(root)) {
                return Err(
                    "outside allowed source roots; pass --source-root for an authorized directory"
                        .into(),
                );
            }
            if !text_suffix(&path) {
                return Err("only local UTF-8 text sources are indexed".into());
            }
            read_text(&path)
        })();
        match content {
            Ok((content, bytes)) => {
                stamps.insert(name(&path)?.to_owned(), sha256(&bytes));
                rows.push(json!({"ref":reference,"id":id,"kind":"source","scope":"record","status":"SOURCE","name":crate::reasoning_authoring::named(body),"content":content,"file":path,"sources":[id],"dependencies":[]}));
            }
            Err(reason) => diagnostics.push(json!({"ref":reference,"reason":reason})),
        }
    }
    for (hypname, hyp) in map(get(&snapshot, "hypotheses"))? {
        if truth(get(hyp, "error")) {
            diagnostics.push(json!({"ref":format!("hypothesis:{hypname}"),"reason":json_value(get(hyp,"error"))?}));
            continue;
        }
        for (_, members) in crate::reasoning_fields::collections(get(hyp, "document"))? {
            for (id, body) in members {
                let reference = format!("hypothesis:{hypname}:node:{id}");
                if mode == ReadMode::Frozen
                    && matches!(body, V::Map(_))
                    && crate::recording_privacy::private_marker(&body)
                {
                    diagnostics.push(json!({"ref":reference,"reason":"private source excluded from frozen search"}));
                    continue;
                }
                rows.push(json!({"ref":reference,"id":id,"kind":"entry","scope":format!("hypothesis:{hypname}"),"status":"HYPOTHESIS","name":crate::reasoning_authoring::named(&body),"sources":[],"dependencies":[],"rule_dependencies":[],"content":dump(&body,None)?}));
            }
        }
    }
    for (path, stamp) in &stamps {
        crate::require(
            fs::read(path).is_ok_and(|raw| sha256(&raw) == *stamp),
            "source changed during search; retry",
        )?;
    }
    let secondary = json!({"record":record,"source_files":stamps,"source_roots":roots,"rows":rows,"unindexed":diagnostics});
    let revision=sha256(crate::public_search_rank::encode(&json!({"snapshot_id":context.snapshot_id(),"findings_revision":context.findings_revision(),"corpus":secondary}))?.as_bytes());
    let files = if let V::List(a) = get(get(&snapshot, "authored_revision"), "files") {
        a.as_slice()
    } else {
        &[]
    };
    let named = files
        .iter()
        .filter(|f| {
            get(f, "status") == &V::Text("read".into())
                && get(f, "origin")
                    == &V::Text(format!(
                        "origin:{}",
                        record.file_name().unwrap().to_string_lossy()
                    ))
        })
        .map(|f| get(f, "sha256"))
        .collect::<Vec<_>>();
    let readable = files
        .iter()
        .filter(|f| get(f, "status") == &V::Text("read".into()) && truth(get(f, "sha256")))
        .map(|f| get(f, "sha256"))
        .collect::<Vec<_>>();
    let record_sha = if named.len() == 1 {
        json_value(named[0])?
    } else if readable.len() == 1 {
        json_value(readable[0])?
    } else {
        J::Null
    };
    Ok(
        json!({"read_mode":if mode==ReadMode::Frozen{"frozen"}else{"live"},"record":record,"revision":revision,"revision_kind":"search-corpus","corpus_revision":revision,"snapshot_id":context.snapshot_id(),"findings_revision":context.findings_revision(),"record_sha256":record_sha,"rows":rows,"unindexed":diagnostics}),
    )
}
pub(super) fn corpus(options: &Options, cwd: &Path, mode: ReadMode) -> Result<J> {
    let record = match &options.record {
        Some(path) => expanded(cwd, path)?,
        None => crate::public_workspace::records(cwd)?
            .into_iter()
            .next()
            .ok_or_else(|| crate::Error("ingestion requires one record path".into()))?,
    };
    crate::require(
        options.profile.as_deref().is_none_or(|p| p == "core/v1"),
        "unsupported search profile",
    )?;
    let selected = if options.profile.is_some() {
        true
    } else if record.is_file() {
        let mut inv = Inventory::default();
        let doc = crate::ordinary_document::load(std::slice::from_ref(&record), &mut inv, false)?;
        use crate::ordinary_value::{Value, map};
        let value = doc.source.projected();
        map(&value)
            .ok()
            .and_then(|m| m.get("meta"))
            .and_then(|v| map(v).ok())
            .and_then(|m| m.get("reasoning"))
            .and_then(|v| map(v).ok())
            .and_then(|m| m.get("profile"))
            == Some(&Value::Text("core/v1".into()))
    } else {
        false
    };
    if selected {
        core(&record, options, cwd, mode)
    } else {
        super::search_ordinary::corpus(&record, options, cwd, mode)
    }
}

#[cfg(test)]
mod constructor_guard_tests {
    use super::*;
    #[test]
    fn independently_parsed_nan_numbers_are_remapped_without_inferring_aliases() {
        let shared = b"known: {p.x: {v: [&n !!float nan, *n]}}\n";
        let independent = b"known: {p.x: {v: [!!float nan, !!float nan]}}\n";
        let decode =
            |raw: &[u8]| crate::history_yaml::decode_full_ordinary_source_value(raw).unwrap();
        let a = decode(shared);
        let b = decode(shared);
        let c = decode(independent);
        assert!(full_matches(&a, &b.projected()));
        assert!(!full_matches(&a, &c.projected()));
        assert!(!full_matches(&c, &a.projected()));
        let fresh_keys = b"known: {p.x: {v: {? !!float nan: one, ? !!float nan: two}}}\n";
        let a = decode(fresh_keys);
        let b = decode(fresh_keys);
        assert!(full_matches(&a, &b.projected()));
        let singleton = decode(b"known: {p.x: {v: {.nan: one, .nan: two}}}\n");
        assert!(!full_matches(&a, &singleton.projected()));
    }
}

#[cfg(all(test, unix))]
mod source_reader_tests {
    #[test]
    fn source_fifo_is_rejected_without_waiting_for_a_writer() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("source.txt");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        let start = std::time::Instant::now();
        assert!(super::read_text(&path).is_err());
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
    }
}
