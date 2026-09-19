//! Detached authority, manifest and retained-generation validation.
//! Captured membership is supplied by the caller; hashes never prove omitted files.
use crate::{
    Result,
    history_contract::*,
    history_paths as paths, history_yaml as yaml,
    identity::sha256,
    require,
    value::{Integer, TypedValue as V},
};
use num_bigint::BigInt;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub const CANCELLATION: &str = "generation-cancellation/v1";
pub const ROOT_DISPOSITION: &str = "explicit-root-disposition/v1";
pub const TEMPORAL_APPLICABILITY: &str = "temporal-applicability/v1";
pub type Files = BTreeMap<String, Vec<u8>>;
pub type ObjectBytes = BTreeMap<(String, String), Vec<u8>>;
pub type ObjectPaths = BTreeMap<(String, String), String>;

fn integer(v: &V) -> Result<BigInt> {
    let V::Integer(n) = v else {
        return Err(error("invalid_generation"));
    };
    let n = n
        .as_str()
        .parse::<BigInt>()
        .map_err(|_| error("invalid_generation"))?;
    require(n >= BigInt::from(0), "invalid_generation")?;
    Ok(n)
}
fn number(n: BigInt) -> V {
    V::Integer(Integer::new(&n.to_string()).expect("canonical integer"))
}
fn s(v: &str) -> V {
    V::Text(v.into())
}
fn hash(v: &V) -> Result<()> {
    let t = text(v)?;
    require(t.len() == 64 && paths::object_id(t), "invalid_identifier")
}
fn list(v: &V) -> Result<&Vec<V>> {
    if let V::List(a) = v {
        Ok(a)
    } else {
        Err(error("invalid_schema"))
    }
}
fn bounded(v: &V, maximum: usize) -> Result<()> {
    yaml::validate_value(v, maximum)
}
fn bindings(marker: &Map) -> Result<Map> {
    marker
        .get("cancellations")
        .map(map)
        .transpose()
        .map(|m| m.cloned().unwrap_or_default())
}
fn capability(manifest: &Map, cap: &str) -> bool {
    matches!(manifest.get("requires"), Some(V::List(a)) if a.iter().any(|v| string_is(v,cap)))
}
pub fn relative_path(value: &str) -> Result<()> {
    require(
        !value.is_empty()
            && !value.contains(['\\', '\0'])
            && !value.starts_with('/')
            && value
                .split('/')
                .all(|p| !p.is_empty() && p != "." && p != ".."),
        "invalid_path",
    )
}
pub fn authority(record_id: &str, kind: &str, generation: &V, cancellations: Map) -> Result<V> {
    let mut m = Map::from([
        ("version".into(), number(1.into())),
        ("record_id".into(), s(record_id)),
        ("authority".into(), s(kind)),
        ("generation".into(), generation.clone()),
        ("profile".into(), s("history/v1")),
    ]);
    if !cancellations.is_empty() {
        m.insert("version".into(), number(2.into()));
        m.insert("cancellations".into(), V::Map(cancellations));
        m.insert("requires".into(), V::List(vec![s(CANCELLATION)]));
    }
    let value = V::Map(m);
    validate_authority(&value)?;
    Ok(value)
}
pub fn validate_authority(value: &V) -> Result<()> {
    bounded(value, 1_048_576)?;
    let m = schema(
        value,
        &["version", "record_id", "authority", "generation", "profile"],
        &["cancellations", "requires"],
    )?;
    require(
        is_int(&m["version"], "1") || is_int(&m["version"], "2"),
        "unsupported_authority",
    )?;
    token(&m["record_id"])?;
    let generation = integer(&m["generation"])?;
    require(
        (string_is(&m["authority"], "legacy") || string_is(&m["authority"], "history"))
            && string_is(&m["profile"], "history/v1"),
        "unsupported_authority",
    )?;
    if is_int(&m["version"], "1") {
        return require(
            !m.contains_key("cancellations") && !m.contains_key("requires"),
            "unsupported_authority",
        );
    }
    require(
        m.get("requires") == Some(&V::List(vec![s(CANCELLATION)]))
            && matches!(m.get("cancellations"), Some(V::Map(b)) if !b.is_empty()),
        "unsupported_authority",
    )?;
    let mut used = BTreeSet::new();
    for (key, item) in map(&m["cancellations"])? {
        require(
            !key.is_empty()
                && key.bytes().all(|c| c.is_ascii_digit())
                && (key == "0" || !key.starts_with('0')),
            "invalid_cancellation_generation",
        )?;
        let n = key
            .parse::<BigInt>()
            .map_err(|_| error("invalid_cancellation_generation"))?;
        let b = schema(
            item,
            &[
                "operation",
                "path",
                "sha256",
                "reserved_generation",
                "legacy_generation",
            ],
            &[],
        )?;
        token(&b["operation"])?;
        hash(&b["sha256"])?;
        let reserved = integer(&b["reserved_generation"])
            .map_err(|_| error("invalid_cancellation_generation"))?;
        let legacy = integer(&b["legacy_generation"])
            .map_err(|_| error("invalid_cancellation_generation"))?;
        require(
            n == reserved
                && reserved >= 1.into()
                && legacy == &reserved + 1
                && legacy <= generation,
            "invalid_cancellation_generation",
        )?;
        let path = text(&b["path"]).map_err(|_| error("invalid_cancellation_path"))?;
        require(
            path == format!("{}.yaml", text(&b["operation"])?) && used.insert(path),
            "invalid_cancellation_path",
        )?;
    }
    Ok(())
}
pub fn validate_cancellation(value: &V) -> Result<()> {
    bounded(value, 16_777_216)?;
    let m = schema(
        value,
        &[
            "version",
            "kind",
            "record_id",
            "operation",
            "reserved_generation",
            "legacy_generation",
            "before_authority_digest",
            "reserved_authority_digest",
            "mutation_digest",
            "entry",
            "original_entry_sha256",
            "manifest_sha256",
            "objects",
            "artifacts",
            "visibility",
        ],
        &[],
    )?;
    require(
        is_int(&m["version"], "1")
            && string_is(&m["kind"], CANCELLATION)
            && string_is(&m["visibility"], "never_released_to_readers"),
        "invalid_cancellation_receipt",
    )?;
    token(&m["record_id"])?;
    token(&m["operation"])?;
    let reserved =
        integer(&m["reserved_generation"]).map_err(|_| error("invalid_cancellation_generation"))?;
    let legacy =
        integer(&m["legacy_generation"]).map_err(|_| error("invalid_cancellation_generation"))?;
    require(
        reserved >= 1.into() && legacy == reserved + 1,
        "invalid_cancellation_generation",
    )?;
    for key in [
        "before_authority_digest",
        "reserved_authority_digest",
        "mutation_digest",
        "original_entry_sha256",
        "manifest_sha256",
    ] {
        hash(&m[key])?;
    }
    let entry = text(&m["entry"])?;
    relative_path(entry)?;
    require(!entry.contains('/'), "invalid_cancellation_path")?;
    for key in ["objects", "artifacts"] {
        require(
            matches!(&m[key], V::Map(a) if a.len() <= MAX_OBJECTS),
            "history_limit",
        )?;
    }
    for (version, item) in map(&m["objects"])? {
        id(&s(version))?;
        let item = schema(item, &["subject", "sha256"], &[])?;
        subject(&item["subject"])?;
        hash(&item["sha256"])?;
    }
    for (path, digest) in map(&m["artifacts"])? {
        relative_path(path)?;
        require(
            path.starts_with(".kpopper-history-migration/"),
            "invalid_cancellation_path",
        )?;
        hash(digest)?;
    }
    Ok(())
}
pub fn validate_baseline(value: &V) -> Result<()> {
    bounded(value, 8_388_608)?;
    let m = schema(
        value,
        &[
            "version",
            "record_id",
            "authority_generation",
            "committed_set_digest",
            "heads",
            "open_acts",
        ],
        &[],
    )?;
    require(is_int(&m["version"], "1"), "invalid_baseline")?;
    token(&m["record_id"])?;
    integer(&m["authority_generation"])?;
    hash(&m["committed_set_digest"])?;
    for role in ["heads", "open_acts"] {
        let a = map(&m[role]).map_err(|_| error("invalid_baseline"))?;
        for (name, versions) in a {
            paths::subject(name)?;
            ids(versions, true)?;
        }
    }
    Ok(())
}
pub fn bind_authority(marker: &V, baseline: &V) -> Result<()> {
    validate_authority(marker)?;
    validate_baseline(baseline)?;
    let a = map(marker)?;
    let b = map(baseline)?;
    require(
        string_is(&a["authority"], "history")
            && a["record_id"] == b["record_id"]
            && a["generation"] == b["authority_generation"],
        "authority_mismatch",
    )
}
pub fn document_template(value: &V) -> Result<V> {
    bounded(value, 16_777_216)?;
    let mut m = map(value).map_err(|_| error("invalid_document"))?.clone();
    let core = m
        .get("meta")
        .and_then(|v| map(v).ok())
        .and_then(|m| m.get("reasoning"))
        .and_then(|v| map(v).ok())
        .and_then(|m| m.get("profile"))
        .is_some_and(|v| string_is(v, "core/v1"));
    for (key, value) in &mut m {
        if ["meta", "schema", "record", "also"].contains(&key.as_str()) {
            continue;
        }
        if let V::Map(a) = value
            && !a.is_empty()
            && a.values().all(|v| core || !matches!(v, V::List(_)))
        {
            a.clear();
        }
    }
    if let Some(meta) = m.get_mut("meta") {
        let V::Map(a) = meta else {
            return Err(error("invalid_document"));
        };
        a.remove("history");
    }
    Ok(V::Map(m))
}
pub fn validate_history_requires(value: &V) -> Result<()> {
    let V::List(a) = value else {
        return Err(error("unsupported_history_capability"));
    };
    let strings = a
        .iter()
        .map(text)
        .collect::<Result<Vec<_>>>()
        .map_err(|_| error("unsupported_history_capability"))?;
    require(
        strings.windows(2).all(|w| w[0] < w[1])
            && strings
                .iter()
                .all(|v| [ROOT_DISPOSITION, paths::CAPABILITY, TEMPORAL_APPLICABILITY].contains(v)),
        "unsupported_history_capability",
    )
}
pub fn validate_temporal_capability(manifest: &V, objects: &Map) -> Result<()> {
    validate_commit(manifest)?;
    let m = map(manifest)?;
    for member in list(&m["objects"])? {
        if let Some(object) = objects.get(text(&map(member)?["id"])?)
            && has_temporal_metadata(object)?
        {
            require(
                capability(m, TEMPORAL_APPLICABILITY),
                "temporal_capability_required",
            )?;
        }
    }
    Ok(())
}
pub(crate) fn has_temporal_metadata(object: &V) -> Result<bool> {
    let m = map(object)?;
    Ok(m.get("id_scheme")
        .is_some_and(|v| string_is(v, "typed-history/v2"))
        && m.get("body")
            .and_then(|v| map(v).ok())
            .and_then(|b| b.get("temporal"))
            .is_some_and(|v| *v != V::Null))
}
pub fn validate_commit(value: &V) -> Result<()> {
    bounded(value, 16_777_216)?;
    let m = schema(
        value,
        &[
            "version",
            "record_id",
            "authority_generation",
            "operation",
            "parents",
            "baseline_digest",
            "objects",
            "receipt",
            "view_sha256",
        ],
        &["view_template", "requires", "authority_digest"],
    )?;
    require(is_int(&m["version"], "1"), "unsupported_commit")?;
    if let Some(v) = m.get("requires") {
        validate_history_requires(v)?;
    }
    token(&m["record_id"])?;
    integer(&m["authority_generation"])?;
    token(&m["operation"])?;
    for (op, digest) in map(&m["parents"]).map_err(|_| error("invalid_parents"))? {
        token(&s(op))?;
        hash(digest)?;
        require(!string_is(&m["operation"], op), "invalid_parents")?;
    }
    hash(&m["baseline_digest"])?;
    hash(&m["view_sha256"])?;
    if let Some(v) = m.get("authority_digest") {
        hash(v)?;
    }
    require(
        matches!(&m["receipt"], V::Map(a) if !a.is_empty()),
        "missing_receipt",
    )?;
    let items = list(&m["objects"]).map_err(|_| error("history_limit"))?;
    require(items.len() <= MAX_OBJECTS, "history_limit")?;
    let mut versions = Vec::new();
    for item in items {
        let item = schema(item, &["subject", "id", "sha256"], &[])?;
        subject(&item["subject"])?;
        id(&item["id"])?;
        hash(&item["sha256"])?;
        versions.push(text(&item["id"])?);
    }
    require(
        versions.windows(2).all(|w| w[0] < w[1]),
        "invalid_object_inventory",
    )?;
    if let Some(template) = m.get("view_template") {
        require(
            document_template(template)?.digest()? == template.digest()?,
            "invalid_view_template",
        )?;
    }
    Ok(())
}
fn decoded_commit(raw: &[u8]) -> Result<V> {
    let v = yaml::decode_document(raw)?;
    validate_commit(&v)?;
    Ok(v)
}
pub fn committed_set_digest(commits: &Files) -> Result<String> {
    require(commits.len() <= MAX_OBJECTS, "history_limit")?;
    let mut effects = Map::new();
    for (op, raw) in commits {
        let v = decoded_commit(raw)?;
        let mut m = map(&v)?.clone();
        require(string_is(&m["operation"], op), "operation_mismatch")?;
        m.remove("receipt");
        m.remove("view_sha256");
        effects.insert(op.clone(), V::Map(m));
    }
    V::Map(effects).digest()
}
pub fn commit_frontier(commits: &Files) -> Result<Map> {
    require(commits.len() <= MAX_OBJECTS, "history_limit")?;
    let mut parents = BTreeSet::new();
    for (op, raw) in commits {
        let v = decoded_commit(raw)?;
        let m = map(&v)?;
        require(string_is(&m["operation"], op), "operation_mismatch")?;
        parents.extend(map(&m["parents"])?.keys().cloned());
    }
    Ok(commits
        .iter()
        .filter(|(op, _)| !parents.contains(*op))
        .map(|(op, raw)| (op.clone(), s(&sha256(raw))))
        .collect())
}
pub fn objects_from_storage(
    commits: &Files,
    storage: &Files,
) -> Result<(ObjectBytes, ObjectPaths)> {
    require(storage.len() <= MAX_OBJECTS, "history_limit")?;
    let mut objects = ObjectBytes::new();
    let mut found = BTreeMap::new();
    let paths_set = storage.keys().cloned().collect();
    for (path, raw) in storage {
        let (scheme, name, version) = paths::parse_object_path(path)?;
        if scheme == paths::Scheme::Legacy {
            objects.insert((name.into(), version.into()), raw.clone());
        }
    }
    for raw in commits.values() {
        let v = decoded_commit(raw)?;
        let m = map(&v)?;
        for item in list(&m["objects"])? {
            let item = map(item)?;
            let key = (
                text(&item["subject"])?.to_owned(),
                text(&item["id"])?.to_owned(),
            );
            let path = paths::resolve_object_path(&paths_set, &key.0, &key.1).map_err(|e| {
                if e.0 == "missing_history_object_path" {
                    error("incomplete_commit")
                } else {
                    e
                }
            })?;
            require(
                paths::parse_object_path(&path)?.0 != paths::Scheme::Hashed
                    || capability(m, paths::CAPABILITY),
                "subject_path_capability_required",
            )?;
            objects.insert(key.clone(), storage[&path].clone());
            found.insert(key, path);
        }
    }
    Ok((objects, found))
}
pub fn committed_objects(marker: &V, commits: &Files, objects: &ObjectBytes) -> Result<Map> {
    validate_authority(marker)?;
    let marker_map = map(marker)?;
    require(
        string_is(&marker_map["authority"], "history"),
        "authority_mismatch",
    )?;
    require(commits.len() <= MAX_OBJECTS, "history_limit")?;
    let mut manifests = BTreeMap::new();
    let mut selected = Map::new();
    for (op, raw) in commits {
        let v = decoded_commit(raw)?;
        let m = map(&v)?;
        require(string_is(&m["operation"], op), "operation_mismatch")?;
        require(
            m["record_id"] == marker_map["record_id"]
                && m["authority_generation"] == marker_map["generation"],
            "authority_mismatch",
        )?;
        require(
            (is_int(&marker_map["version"], "1") && !m.contains_key("authority_digest"))
                || m.get("authority_digest") == Some(&s(&marker.digest()?)),
            "authority_digest_mismatch",
        )?;
        for item in list(&m["objects"])? {
            let item = map(item)?;
            let key = (
                text(&item["subject"])?.to_owned(),
                text(&item["id"])?.to_owned(),
            );
            let raw = objects
                .get(&key)
                .ok_or_else(|| error("incomplete_commit"))?;
            require(
                string_is(&item["sha256"], &sha256(raw)),
                "object_bytes_mismatch",
            )?;
            let obj = yaml::decode_document(raw)?;
            validate_object(&obj)?;
            let obj_map = map(&obj)?;
            require(
                string_is(&obj_map["subject"], &key.0) && string_is(&obj_map["id"], &key.1),
                "reference_mismatch",
            )?;
            selected.insert(key.1, obj);
        }
        validate_temporal_capability(&v, &selected)?;
        manifests.insert(op.clone(), map(&v)?.clone());
    }
    let mut children: BTreeMap<String, Vec<String>> =
        manifests.keys().map(|op| (op.clone(), vec![])).collect();
    let mut remaining = BTreeMap::new();
    for (op, m) in &manifests {
        let parents = map(&m["parents"])?;
        remaining.insert(op.clone(), parents.len());
        for (parent, expected) in parents {
            let raw = commits
                .get(parent)
                .ok_or_else(|| error("incomplete_commit"))?;
            require(string_is(expected, &sha256(raw)), "parent_bytes_mismatch")?;
            children
                .get_mut(parent)
                .ok_or_else(|| error("incomplete_commit"))?
                .push(op.clone());
        }
    }
    let mut ready: VecDeque<String> = remaining
        .iter()
        .filter(|(_, n)| **n == 0)
        .map(|(op, _)| op.clone())
        .collect();
    let mut visited = 0;
    while let Some(op) = ready.pop_front() {
        visited += 1;
        for child in &children[&op] {
            let n = remaining.get_mut(child).expect("known child");
            *n -= 1;
            if *n == 0 {
                ready.push_back(child.clone());
            }
        }
    }
    require(visited == manifests.len(), "cyclic_commits")?;
    let mut membership = BTreeMap::new();
    for (op, m) in &manifests {
        let mut members = BTreeSet::new();
        for item in list(&m["objects"])? {
            members.insert(text(&map(item)?["id"])?.to_owned());
        }
        membership.insert(op.clone(), members);
    }
    let mut work = 0;
    for (op, m) in &manifests {
        if !capability(m, ROOT_DISPOSITION) {
            continue;
        }
        let mut disposed = BTreeSet::new();
        let mut unresolved = BTreeSet::new();
        for version in &membership[op] {
            let obj = map(&selected[version])?;
            if string_is(&obj["kind"], "act") {
                let body = map(&obj["body"])?;
                if ["accept", "propose", "retire"]
                    .iter()
                    .any(|s| string_is(&body["act"], s))
                {
                    disposed.insert(text(&body["of"])?.to_owned());
                }
            } else if list(&obj["saw"])?.is_empty() {
                unresolved.insert(version.clone());
            }
        }
        unresolved.retain(|v| !disposed.contains(v));
        let mut pending: Vec<String> = map(&m["parents"])?.keys().cloned().collect();
        let mut examined = BTreeSet::new();
        while !unresolved.is_empty() && !pending.is_empty() {
            let parent = pending.pop().expect("pending parent");
            if !examined.insert(parent.clone()) {
                continue;
            }
            work += 1;
            require(work <= 4_000_000, "history_limit")?;
            unresolved.retain(|v| !membership[&parent].contains(v));
            pending.extend(map(&manifests[&parent]["parents"])?.keys().cloned());
        }
        require(unresolved.is_empty(), "missing_root_disposition")?;
    }
    validate_closure(&selected)?;
    Ok(selected)
}

pub fn validate_cancellations(
    marker: &V,
    files: &Files,
    commits: &Files,
    objects: &ObjectBytes,
) -> Result<()> {
    validate_authority(marker)?;
    let m = map(marker)?;
    let b = bindings(m)?;
    let expected = b
        .values()
        .map(|v| text(&map(v)?["path"]).map(str::to_owned))
        .collect::<Result<BTreeSet<_>>>()?;
    require(
        files.keys().cloned().collect::<BTreeSet<_>>() == expected,
        "cancellation_membership_mismatch",
    )?;
    for (generation, binding) in &b {
        let binding = map(binding)?;
        let raw = &files[text(&binding["path"])?];
        require(
            string_is(&binding["sha256"], &sha256(raw)),
            "cancellation_bytes_mismatch",
        )?;
        let receipt = yaml::decode_document(raw)?;
        validate_cancellation(&receipt)?;
        let r = map(&receipt)?;
        require(
            r["record_id"] == m["record_id"]
                && ["operation", "reserved_generation", "legacy_generation"]
                    .iter()
                    .all(|k| r[*k] == binding[*k]),
            "cancellation_binding_mismatch",
        )?;
        let n = generation
            .parse::<BigInt>()
            .map_err(|_| error("invalid_cancellation_generation"))?;
        let older = b
            .iter()
            .filter(|(k, _)| k.parse::<BigInt>().is_ok_and(|v| v < n))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<Map>();
        let before = authority(
            text(&m["record_id"])?,
            "legacy",
            &number(&n - 1),
            older.clone(),
        )?;
        let reserved = authority(text(&m["record_id"])?, "history", &number(n.clone()), older)?;
        require(
            string_is(&r["before_authority_digest"], &before.digest()?)
                && string_is(&r["reserved_authority_digest"], &reserved.digest()?),
            "cancellation_authority_mismatch",
        )?;
        let raw_commit = commits
            .get(text(&binding["operation"])?)
            .ok_or_else(|| error("cancellation_manifest_mismatch"))?;
        require(
            string_is(&r["manifest_sha256"], &sha256(raw_commit)),
            "cancellation_manifest_mismatch",
        )?;
        let manifest = decoded_commit(raw_commit)?;
        let manifest = map(&manifest)?;
        require(
            manifest["record_id"] == m["record_id"]
                && integer(&manifest["authority_generation"])? == n,
            "cancellation_manifest_mismatch",
        )?;
        let mut inventory = Map::new();
        for item in list(&manifest["objects"])? {
            let item = map(item)?;
            inventory.insert(
                text(&item["id"])?.to_owned(),
                V::Map(Map::from([
                    ("subject".into(), item["subject"].clone()),
                    ("sha256".into(), item["sha256"].clone()),
                ])),
            );
        }
        require(
            r["objects"] == V::Map(inventory.clone()),
            "cancellation_object_mismatch",
        )?;
        for (version, item) in inventory {
            let item = map(&item)?;
            let raw = objects
                .get(&(text(&item["subject"])?.to_owned(), version))
                .ok_or_else(|| error("cancellation_object_mismatch"))?;
            require(
                string_is(&item["sha256"], &sha256(raw)),
                "cancellation_object_mismatch",
            )?;
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct Generation {
    pub authority: V,
    pub commits: Files,
    pub objects: Map,
    pub object_bytes: ObjectBytes,
    pub digest: String,
    pub cancellation: Option<V>,
}
impl Generation {
    pub fn evidence(&self) -> V {
        let mut m = Map::from([
            ("authority".into(), self.authority.clone()),
            ("objects".into(), V::Map(self.objects.clone())),
            ("digest".into(), s(&self.digest)),
        ]);
        if let Some(v) = &self.cancellation {
            m.insert("disposition".into(), s("cancelled"));
            m.insert("cancellation".into(), v.clone());
        }
        V::Map(m)
    }
}
pub fn committed_generations(
    marker: &V,
    commits: &Files,
    objects: &ObjectBytes,
    cancellations: &Files,
) -> Result<BTreeMap<String, Generation>> {
    validate_authority(marker)?;
    require(
        commits.len() <= MAX_OBJECTS && objects.len() <= MAX_OBJECTS,
        "history_limit",
    )?;
    validate_cancellations(marker, cancellations, commits, objects)?;
    let m = map(marker)?;
    let b = bindings(m)?;
    let current = integer(&m["generation"])?;
    let mut grouped: BTreeMap<BigInt, Files> = BTreeMap::new();
    for (op, raw) in commits {
        let manifest = decoded_commit(raw)?;
        let manifest = map(&manifest)?;
        require(string_is(&manifest["operation"], op), "operation_mismatch")?;
        require(
            manifest["record_id"] == m["record_id"],
            "foreign_history_generation",
        )?;
        let generation = integer(&manifest["authority_generation"])?;
        require(
            generation <= current
                && (string_is(&m["authority"], "history") || generation < current),
            "future_history_generation",
        )?;
        grouped
            .entry(generation)
            .or_default()
            .insert(op.clone(), raw.clone());
    }
    let mut result = BTreeMap::new();
    for (n, commits) in grouped {
        let older = b
            .iter()
            .filter(|(k, _)| k.parse::<BigInt>().is_ok_and(|v| v < n))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let authority = authority(text(&m["record_id"])?, "history", &number(n.clone()), older)?;
        let selected = committed_objects(&authority, &commits, objects)?;
        let mut object_bytes = ObjectBytes::new();
        let mut inventory = Map::new();
        for (version, obj) in &selected {
            let obj = map(obj)?;
            let key = (text(&obj["subject"])?.to_owned(), version.clone());
            let raw = &objects[&key];
            inventory.insert(
                version.clone(),
                V::Map(Map::from([
                    ("subject".into(), obj["subject"].clone()),
                    ("sha256".into(), s(&sha256(raw))),
                ])),
            );
            object_bytes.insert(key, raw.clone());
        }
        let digest = V::Map(Map::from([
            ("authority".into(), authority.clone()),
            (
                "commits".into(),
                V::Map(
                    commits
                        .iter()
                        .map(|(k, v)| (k.clone(), s(&sha256(v))))
                        .collect(),
                ),
            ),
            ("objects".into(), V::Map(inventory)),
        ]))
        .digest()?;
        let cancellation = b.get(&n.to_string()).cloned();
        if let Some(v) = &cancellation {
            require(
                commits.len() == 1 && commits.contains_key(text(&map(v)?["operation"])?),
                "cancelled_generation_changed",
            )?;
        }
        result.insert(
            n.to_string(),
            Generation {
                authority,
                commits,
                objects: selected,
                object_bytes,
                digest,
                cancellation,
            },
        );
    }
    Ok(result)
}
