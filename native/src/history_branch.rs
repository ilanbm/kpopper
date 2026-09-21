//! Source-free audit of committed Git branch evidence. The commit association is
//! an observation made by a local Git reader, not a Merkle inclusion proof.
use crate::{
    Result, history_adapter as A, history_authority as H,
    history_authority::Files,
    history_bundle as B,
    history_capture::{Capture, Layout},
    history_contract::*,
    history_hypotheses as HH, history_hypothesis_import as HI,
    history_view::{list, map_mut},
    history_yaml as Y,
    identity::sha256,
    pending_bundle, reasoning_capabilities,
    reasoning_snapshot::{CaptureOptions, MAX_REQUEST_BYTES, Snapshot},
    require,
    value::TypedValue as V,
};
use sha2::Digest;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
pub struct Observation {
    pub envelope: V,
    pub files: Files,
}
pub const KIND: &str = "committed-branch-history/v1";
pub const SCOPED_KIND: &str = "committed-branch-history/v2";
pub const MAX_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_FILES: usize = 3 * MAX_OBJECTS + 1024;
fn s(v: &str) -> V {
    V::Text(v.into())
}
fn object(items: impl IntoIterator<Item = (&'static str, V)>) -> V {
    V::Map(items.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
fn hash(value: &V) -> Result<()> {
    require(
        text(value)?.len() == 64 && crate::history_paths::object_id(text(value)?),
        "invalid_identifier",
    )
}
pub(crate) fn portable_path(path: &str) -> Result<()> {
    H::relative_path(path)?;
    require(
        !path.chars().any(|c| (c as u32) < 32) && !path.split('/').any(|p| p == ".git"),
        "invalid_path",
    )
}
fn child<'a>(value: &'a V, keys: &[&str]) -> Option<&'a V> {
    keys.iter().try_fold(value, |v, k| map(v).ok()?.get(*k))
}
pub(crate) fn parts(entry: &str) -> Result<(&str, &str)> {
    portable_path(entry)?;
    Ok(entry.rsplit_once('/').unwrap_or(("", entry)))
}
pub(crate) fn joined(entry: &str, relative: &str) -> Result<String> {
    portable_path(relative)?;
    let (parent, _) = parts(entry)?;
    Ok(if parent.is_empty() {
        relative.into()
    } else {
        format!("{parent}/{relative}")
    })
}
pub(crate) fn roles(entry: &str) -> Result<Layout> {
    let (_, name) = parts(entry)?;
    let mut l = Layout::for_entry(name)?;
    l.entry = entry.into();
    l.authority = joined(entry, &l.authority)?;
    l.objects = joined(entry, &l.objects)?;
    l.commits = joined(entry, &l.commits)?;
    l.cancellations = joined(entry, &l.cancellations)?;
    Ok(l)
}
pub(crate) fn hypothesis_directory(entry: &str) -> Result<String> {
    joined(entry, &HI::directory(parts(entry)?.1)?)
}
pub(crate) fn hypothesis_file<'a>(path: &'a str, directory: &str) -> Option<&'a str> {
    let (parent, name) = path.rsplit_once('/')?;
    if parent != directory {
        return None;
    }
    name.strip_suffix(".yaml")
        .or_else(|| name.strip_suffix(".yml"))
}
pub(crate) fn blob_identity(raw: &[u8], algorithm: &str) -> Result<String> {
    let header = format!("blob {}\0", raw.len());
    Ok(match algorithm {
        "sha1" => {
            let mut h = sha1::Sha1::new();
            h.update(header.as_bytes());
            h.update(raw);
            format!("{:x}", h.finalize())
        }
        "sha256" => {
            let mut h = sha2::Sha256::new();
            h.update(header.as_bytes());
            h.update(raw);
            format!("{:x}", h.finalize())
        }
        _ => return Err(error("invalid_branch_source")),
    })
}
pub(crate) fn core(files: &Files, entry: &str) -> Result<(Capture, BTreeSet<String>)> {
    let roles = roles(entry)?;
    let raw = files
        .get(&roles.authority)
        .ok_or_else(|| error("branch_history_not_active"))?;
    require(files.contains_key(entry), "branch_history_not_active")?;
    let marker = Y::decode_document(raw)?;
    H::validate_authority(&marker)?;
    require(
        string_is(&map(&marker)?["authority"], "history"),
        "branch_history_not_active",
    )?;
    let mut core = Files::from([
        ("entry.yaml".into(), files[entry].clone()),
        ("authority.yaml".into(), raw.clone()),
    ]);
    let mut paths = BTreeSet::from([entry.into(), roles.authority]);
    for (root, prefix) in [
        (roles.commits, "commits"),
        (roles.objects, "objects"),
        (roles.cancellations, "cancellations"),
    ] {
        let root = format!("{root}/");
        for (path, raw) in files {
            if let Some(tail) = path.strip_prefix(&root) {
                core.insert(format!("{prefix}/{tail}"), raw.clone());
                paths.insert(path.clone());
            }
        }
    }
    Ok((B::capture(&core, None)?, paths))
}
fn commits(captured: &Capture) -> impl Iterator<Item = &Vec<u8>> {
    captured.commits.values().chain(
        captured
            .inactive_generations
            .values()
            .flat_map(|g| g.commits.values()),
    )
}
/// Prior adoption capsules are hash observations; scoped transport does not
/// recursively transfer their raw bytes or weaken authored evidence requirements.
pub(crate) fn audit_coverage(captured: &Capture, entry: &str) -> Result<V> {
    let mut prior = BTreeMap::new();
    for raw in commits(captured) {
        let manifest = Y::decode_document(raw)?;
        H::validate_commit(&manifest)?;
        let Some(adoption) = child(&manifest, &["receipt", "after", "history_branch_adoption"])
        else {
            continue;
        };
        let a = map(adoption)?;
        let bindings = if a.get("version").is_some_and(|v| is_int(v, "1")) {
            a.get("source").into_iter().collect::<Vec<_>>()
        } else {
            a.get("sources")
                .map(list)
                .transpose()?
                .unwrap_or(&[])
                .iter()
                .collect()
        };
        for binding in bindings {
            let Some(audit) = map(binding)?.get("audit").filter(|v| **v != V::Null) else {
                continue;
            };
            let audit = schema(audit, &["path", "sha256", "revision"], &[])?;
            hash(&audit["revision"])?;
            hash(&audit["sha256"])?;
            let expected = format!(
                "{}evidence/branches/{}.json",
                if parts(entry)?.1 == "GROUNDING.yaml" {
                    ".kpopper/"
                } else {
                    ""
                },
                text(&audit["revision"])?
            );
            require(
                string_is(&audit["path"], &expected)
                    && child(
                        &manifest,
                        &["receipt", "before", "authoring", "evidence", &expected],
                    ) == Some(&audit["sha256"]),
                "branch_adoption_audit_mismatch",
            )?;
            let path = joined(entry, &expected)?;
            let item = object([
                ("path", s(&path)),
                ("sha256", audit["sha256"].clone()),
                ("revision", audit["revision"].clone()),
                ("availability", s("not_transferred")),
            ]);
            require(
                prior.get(&path).is_none_or(|old| *old == item),
                "branch_evidence_mismatch",
            )?;
            prior.insert(path, item);
        }
    }
    Ok(object([
        ("history", s("current_and_inactive_generations")),
        (
            "prior_branch_capsules",
            V::List(prior.into_values().collect()),
        ),
    ]))
}
/// Enumerate only explicit record-relative evidence in current and inactive
/// generations, manifests, imports and physical hypotheses. No path is read here.
pub(crate) fn required(
    captured: &Capture,
    entry: &str,
    files: &Files,
    include_prior_audit: bool,
) -> Result<BTreeMap<String, Option<String>>> {
    let adapted = A::from_store_capture(captured)?;
    let document = adapted.document();
    let directory = hypothesis_directory(entry)?;
    let mut documents = vec![document.clone()];
    for (path, raw) in files {
        if hypothesis_file(path, &directory).is_some() {
            documents.push(Y::decode_document(raw)?);
        }
    }
    documents.extend(captured.objects.values().cloned());
    documents.extend(
        captured
            .inactive_generations
            .values()
            .flat_map(|g| g.objects.values())
            .cloned(),
    );
    for raw in commits(captured) {
        documents.push(Y::decode_document(raw)?);
    }
    let mut required = BTreeMap::new();
    let mut authored_files = BTreeSet::new();
    for doc in &documents {
        B::privacy(doc)?;
        for relative in pending_bundle::required_files(doc)
            .map_err(|_| error("external_branch_evidence_unavailable"))?
        {
            let path = joined(entry, &relative)?;
            authored_files.insert(path.clone());
            required.entry(path).or_insert(None);
        }
    }
    for raw in commits(captured) {
        let manifest = Y::decode_document(raw)?;
        H::validate_commit(&manifest)?;
        if let Some(evidence) = child(&manifest, &["receipt", "before", "authoring", "evidence"]) {
            for (relative, digest) in
                map(evidence).map_err(|_| error("invalid_branch_evidence_inventory"))?
            {
                hash(digest)?;
                bind(&mut required, joined(entry, relative)?, text(digest)?)?;
            }
        }
    }
    if !include_prior_audit {
        for item in list(&map(&audit_coverage(captured, entry)?)?["prior_branch_capsules"])? {
            let path = text(&map(item)?["path"])?;
            require(
                !authored_files.contains(path),
                "branch_recursive_audit_authored_evidence",
            )?;
            required.remove(path);
        }
    }
    let mut members = Map::new();
    if let Some(imported) = child(document, &["meta", "history_import"]).filter(|v| **v != V::Null)
    {
        let imported = schema(
            imported,
            &["version", "operation", "recorded_at", "members"],
            &[],
        )?;
        require(is_int(&imported["version"], "1"), "invalid_history_import")?;
        for item in list(&imported["members"])? {
            let m = schema(item, &["path", "sha256", "role"], &[])?;
            let path = text(&m["path"])?;
            require(
                (string_is(&m["role"], "retained_original") || string_is(&m["role"], "replaced"))
                    && !members.contains_key(path),
                "invalid_history_import",
            )?;
            hash(&m["sha256"])?;
            members.insert(path.into(), item.clone());
            bind(&mut required, joined(entry, path)?, text(&m["sha256"])?)?;
        }
    }
    for key in ["record", "also"] {
        let values = match map(document)?.get(key) {
            Some(v @ V::Text(_)) => vec![v],
            Some(V::List(a)) => a.iter().collect(),
            Some(V::Map(m)) => m.values().collect(),
            _ => vec![],
        };
        for pointer in values
            .into_iter()
            .filter_map(|v| text(v).ok())
            .filter(|v| v.ends_with(".yaml") || v.ends_with(".yml"))
        {
            portable_path(pointer)?;
            let mut found = false;
            for (name, item) in &members {
                if string_is(&map(item)?["role"], "retained_original")
                    && crate::history_sources::matches(name, pointer)?
                {
                    found = true;
                    break;
                }
            }
            require(found, "unbound_branch_record_pointer")?;
        }
    }
    if let Some(mapping) = child(document, &["meta", HI::MAPPING_KEY]).filter(|v| **v != V::Null) {
        HI::validate_mapping(mapping, parts(entry)?.1)?;
        for item in list(&map(mapping)?["physical"])? {
            let m = map(item)?;
            bind(
                &mut required,
                joined(entry, text(&m["path"])?)?,
                text(&m["sha256"])?,
            )?;
        }
    }
    Ok(required)
}
fn bind(required: &mut BTreeMap<String, Option<String>>, path: String, digest: &str) -> Result<()> {
    require(
        required
            .get(&path)
            .and_then(|v| v.as_deref())
            .is_none_or(|v| v == digest),
        "branch_evidence_mismatch",
    )?;
    required.insert(path, Some(digest.into()));
    Ok(())
}
pub(crate) fn captured_snapshot(
    captured: &Capture,
    files: &Files,
    source: &V,
    as_of: &V,
) -> Result<(Snapshot, BTreeSet<String>)> {
    let adapted = A::from_store_capture(captured)?;
    let (named, _) = HH::layers(adapted.projection(), adapted.document())?;
    let entry = text(&map(source)?["entry"])?;
    let directory = hypothesis_directory(entry)?;
    let mut physical = Map::new();
    let mut paths = BTreeSet::new();
    for (path, raw) in files {
        let Some(name) = hypothesis_file(path, &directory) else {
            continue;
        };
        hypothesis_name(&s(name))?;
        require(!physical.contains_key(name), "ambiguous_branch_hypothesis")?;
        let mut document = Y::decode_document(raw)?;
        B::privacy(&document)?;
        let head = map_mut(&mut document)?
            .remove("hypothesis")
            .unwrap_or_else(|| V::Map(Map::new()));
        map(&head).map_err(|_| error("invalid_branch_hypothesis"))?;
        reasoning_capabilities::document_capabilities(&document)?;
        physical.insert(
            name.into(),
            object([("document", document), ("head", head), ("error", V::Null)]),
        );
        paths.insert(path.clone());
    }
    if let Some(mapping) =
        child(adapted.document(), &["meta", HI::MAPPING_KEY]).filter(|v| **v != V::Null)
    {
        HI::validate_mapping(mapping, parts(entry)?.1)?;
        for item in list(&map(mapping)?["physical"])? {
            let m = map(item)?;
            let name = text(&m["name"])?;
            let path = joined(entry, text(&m["path"])?)?;
            require(
                physical.contains_key(name)
                    && paths.contains(&path)
                    && files
                        .get(&path)
                        .is_some_and(|raw| string_is(&m["sha256"], &sha256(raw))),
                "missing_imported_hypothesis",
            )?;
            physical.remove(name);
        }
    }
    for (name, group) in map(&named)? {
        require(
            !physical.contains_key(name),
            "hypothesis_authority_collision",
        )?;
        physical.insert(name.clone(), group.clone());
    }
    let snapshot = adapted.snapshot(CaptureOptions {
        context: Some(object([
            ("read_mode", s("frozen")),
            ("branch_source", source.clone()),
        ])),
        hypotheses: Some(V::Map(physical)),
        as_of: Some(as_of.clone()),
        ..Default::default()
    })?;
    Ok((snapshot, paths))
}
/// Audit all bytes and the exact Snapshot without Git, filesystem or evaluator I/O.
/// `envelope` holds revision and manifest; raw files are passed separately.
pub fn validate(envelope: &V, files: &Files) -> Result<Capture> {
    let envelope = schema(envelope, &["revision", "manifest"], &[])?;
    let manifest = &envelope["manifest"];
    Y::validate_value(manifest, MAX_REQUEST_BYTES)?;
    let m = schema(
        manifest,
        &["version", "kind", "source", "files", "snapshot"],
        &["audit_coverage"],
    )?;
    let scoped = is_int(&m["version"], "2");
    require(
        (is_int(&m["version"], "1")
            && string_is(&m["kind"], KIND)
            && !m.contains_key("audit_coverage"))
            || (scoped && string_is(&m["kind"], SCOPED_KIND) && m.contains_key("audit_coverage")),
        "unsupported_branch_capture",
    )?;
    require(
        string_is(&envelope["revision"], &manifest.digest()?),
        "branch_capture_identity",
    )?;
    let as_of =
        field(map(&m["snapshot"])?, "as_of").map_err(|_| error("invalid_branch_snapshot"))?;
    let source = schema(
        &m["source"],
        &["commit", "entry", "object_format", "association"],
        &[],
    )?;
    let algorithm = text(&source["object_format"])?;
    let commit = text(&source["commit"])?;
    require(
        ["sha1", "sha256"].contains(&algorithm)
            && string_is(&source["association"], "local_git_capture")
            && commit.len() == if algorithm == "sha1" { 40 } else { 64 }
            && commit
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid_branch_source",
    )?;
    let entry = text(&source["entry"])?;
    portable_path(entry)?;
    let inventory = map(&m["files"])?;
    require(
        files.len() <= MAX_FILES && files.keys().eq(inventory.keys()),
        "branch_capture_membership",
    )?;
    let mut total = 0usize;
    for (path, raw) in files {
        portable_path(path)?;
        for (i, _) in path.match_indices('/') {
            require(
                !files.contains_key(&path[..i]),
                "history_bundle_path_collision",
            )?;
        }
        total = total.saturating_add(raw.len());
        require(
            raw.len() <= MAX_REQUEST_BYTES && total <= MAX_BYTES,
            "branch_capture_limit",
        )?;
        let item = schema(&inventory[path], &["sha256", "git_oid", "mode"], &[])?;
        require(
            (string_is(&item["mode"], "100644") || string_is(&item["mode"], "100755"))
                && string_is(&item["sha256"], &sha256(raw))
                && string_is(&item["git_oid"], &blob_identity(raw, algorithm)?),
            "branch_blob_mismatch",
        )?;
    }
    let (captured, mut expected_paths) = core(files, entry)?;
    if scoped {
        let expected = audit_coverage(&captured, entry)?;
        require(
            m["audit_coverage"] == expected
                && !list(&map(&expected)?["prior_branch_capsules"])?.is_empty(),
            "branch_audit_coverage_mismatch",
        )?;
    }
    for (path, digest) in required(&captured, entry, files, !scoped)? {
        require(
            files
                .get(&path)
                .is_some_and(|raw| digest.as_ref().is_none_or(|d| *d == sha256(raw))),
            "branch_evidence_unavailable",
        )?;
        expected_paths.insert(path);
    }
    let (expected, hypothesis_paths) = captured_snapshot(&captured, files, &m["source"], as_of)?;
    expected_paths.extend(hypothesis_paths);
    require(
        files.keys().eq(expected_paths.iter()),
        "branch_capture_membership",
    )?;
    let recorded = Snapshot::from_snapshot(&m["snapshot"])?;
    require(
        recorded.to_data() == expected.to_data(),
        "branch_snapshot_mismatch",
    )?;
    Ok(captured)
}
pub fn snapshot(envelope: &V, files: &Files) -> Result<Snapshot> {
    validate(envelope, files)?;
    Snapshot::from_snapshot(&map(&map(envelope)?["manifest"])?["snapshot"])
}

impl Observation {
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        validate(&self.envelope, &self.files)?;
        let mut value = self.envelope.clone();
        map_mut(&mut value)?.insert(
            "files".into(),
            V::Map(
                self.files
                    .iter()
                    .map(|(path, raw)| (path.clone(), crate::history_transaction::blob(Some(raw))))
                    .collect(),
            ),
        );
        let bytes = value.canonical_bytes()?;
        require(
            bytes.len() <= 2 * MAX_BYTES + MAX_REQUEST_BYTES,
            "branch_capture_limit",
        )?;
        Ok(bytes)
    }
    pub fn from_bytes(raw: &[u8]) -> Result<Self> {
        require(
            raw.len() <= 2 * MAX_BYTES + MAX_REQUEST_BYTES,
            "branch_capture_limit",
        )?;
        let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
        for &b in raw {
            if quoted {
                if escaped {
                    escaped = false;
                } else if b == b'\\' {
                    escaped = true;
                } else if b == b'"' {
                    quoted = false;
                }
            } else {
                match b {
                    b'"' => quoted = true,
                    b'[' | b'{' => {
                        depth += 1;
                        require(depth <= 520, "branch_capture_limit")?;
                    }
                    b']' | b'}' => depth = depth.saturating_sub(1),
                    _ => {}
                }
            }
        }
        let encoded = crate::json_ingress::parse_slice_bounded(
            raw,
            crate::json_ingress::DuplicateKeys::LastWins,
            520,
        )?;
        let mut envelope = V::from_tagged(&encoded)?;
        require(envelope.to_tagged()? == encoded, "invalid_branch_capture")?;
        schema(&envelope, &["revision", "manifest", "files"], &[])?;
        let blobs = map_mut(&mut envelope)?.remove("files").unwrap();
        let blobs = map(&blobs)?;
        require(blobs.len() <= MAX_FILES, "branch_capture_limit")?;
        let mut total = 0usize;
        for blob in blobs.values() {
            let data = text(field(map(blob)?, "data")?)?;
            require(
                data.len() <= 4 * MAX_REQUEST_BYTES.div_ceil(3),
                "branch_capture_limit",
            )?;
            total = total.saturating_add(data.len() / 4 * 3);
            require(total <= MAX_BYTES + 2 * blobs.len(), "branch_capture_limit")?;
        }
        let files = blobs
            .iter()
            .map(|(path, blob)| {
                Ok((
                    path.clone(),
                    crate::history_transaction::unblob(blob)?
                        .ok_or_else(|| error("invalid_branch_capture"))?,
                ))
            })
            .collect::<Result<Files>>()?;
        validate(&envelope, &files)?;
        Ok(Self { envelope, files })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD};
    #[test]
    fn exact_branch_closure_and_snapshot_replay_without_sources() {
        let cases: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/history-branch.json")).unwrap();
        for case in cases.as_array().unwrap() {
            let envelope = V::from_tagged(&case["envelope"]).unwrap();
            let files = case["files"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(p, raw)| (p.clone(), STANDARD.decode(raw.as_str().unwrap()).unwrap()))
                .collect();
            let result = validate(&envelope, &files);
            if let Some(output) = case.get("output") {
                let capture = result.unwrap_or_else(|e| panic!("{}: {}", case["name"], e));
                let expected = V::from_tagged(output).unwrap();
                let expected = map(&expected).unwrap();
                assert_eq!(
                    V::Map(capture.objects.clone()),
                    expected["objects"],
                    "{}",
                    case["name"]
                );
                let m = map(&map(&envelope).unwrap()["manifest"]).unwrap();
                let entry = text(&map(&m["source"]).unwrap()["entry"]).unwrap();
                let got = required(&capture, entry, &files, is_int(&m["version"], "1")).unwrap();
                let got = V::Map(
                    got.into_iter()
                        .map(|(k, v)| (k, v.map(|v| s(&v)).unwrap_or(V::Null)))
                        .collect(),
                );
                assert_eq!(got, expected["required"], "{}", case["name"]);
                assert_eq!(
                    audit_coverage(&capture, entry).unwrap(),
                    expected["coverage"],
                    "{}",
                    case["name"]
                );
                assert_eq!(
                    snapshot(&envelope, &files).unwrap().to_data(),
                    expected["snapshot"],
                    "{}",
                    case["name"]
                );
            } else {
                assert!(
                    result.is_err(),
                    "accepted {}: {}",
                    case["name"],
                    case["error"]
                );
            }
        }
    }
}
