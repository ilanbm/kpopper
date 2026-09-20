//! Explicit frozen core conversion. The source inventory owns discovery; this
//! operation copies a candidate and its evidence without activating history.
use super::{core_expression_conversion as C, core_expression_yaml as Y};
use crate::{
    Error, Result,
    history_authority::Files,
    history_contract::{map, text},
    history_migration_source as Paths,
    history_transaction::Layout,
    history_view::{map_mut, truth},
    identity::sha256,
    project_modes::{self as P, Project},
    reasoning_runtime::Runtime,
    reasoning_snapshot::{Snapshot, entries},
    require,
    source_capture::{CapturedSource, ReadMode, capture_source_with_runtime},
    source_inventory::{Inventory, Observation, absolute, name},
    value::TypedValue as V,
};
use C::{empty, obj, s};
use serde_json::{Value as J, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
};
pub(super) const ARTIFACTS: &str = ".kpopper-migration";
fn get<'a>(v: &'a V, k: &str) -> &'a V {
    if let V::Map(m) = v {
        m.get(k).unwrap_or(&V::Null)
    } else {
        &V::Null
    }
}
fn list(v: &V) -> &[V] {
    if let V::List(a) = v { a } else { &[] }
}
fn origin(record: &Path, path: &Path) -> Result<String> {
    let portable = Paths::portable(record, path)?;
    Ok(
        if let Some(external) = portable.strip_prefix("_external/") {
            format!("external:{external}")
        } else {
            format!("origin:{portable}")
        },
    )
}
fn tagged(value: &V) -> Result<Vec<u8>> {
    value.canonical_bytes()
}
fn plain(value: &V) -> Result<J> {
    Ok(match value {
        V::Map(m) => J::Object(
            m.iter()
                .map(|(k, v)| Ok((k.clone(), plain(v)?)))
                .collect::<Result<_>>()?,
        ),
        V::List(a) => J::Array(a.iter().map(plain).collect::<Result<_>>()?),
        _ => value
            .to_json()
            .unwrap_or(json!({"typed":value.to_tagged()?})),
    })
}
fn parse(raw: &[u8]) -> Result<V> {
    Y::parse(raw)
}
fn merge(base: &V, document: &V) -> Result<V> {
    let mut out = base.clone();
    let out = map_mut(&mut out)?;
    for (k, v) in map(document)? {
        if let (Some(V::Map(current)), V::Map(incoming)) = (out.get_mut(k), v) {
            let mut incoming = incoming.clone();
            if k == "meta"
                && let (Some(V::Map(old)), Some(V::Map(new))) =
                    (current.get("prefixes"), incoming.get("prefixes"))
            {
                let mut joined = old.clone();
                joined.extend(new.clone());
                incoming.insert("prefixes".into(), V::Map(joined));
            }
            current.extend(incoming);
        } else {
            out.insert(k.clone(), v.clone());
        }
    }
    Ok(V::Map(out.clone()))
}
fn layer(base: &V, document: &V) -> Result<V> {
    let mut out = base.clone();
    for (collection, members) in crate::reasoning_fields::collections(document)? {
        for (name, value) in map_mut(&mut out)?.iter_mut() {
            if *name != collection
                && let V::Map(m) = value
            {
                for id in members.keys() {
                    m.remove(id);
                }
            }
        }
        let current = map_mut(&mut out)?.entry(collection).or_insert_with(empty);
        if let V::Map(m) = current {
            m.extend(members);
        } else {
            *current = V::Map(members);
        }
    }
    Ok(out)
}
fn meaning(document: &V) -> Result<V> {
    let caps = crate::reasoning_capabilities::document_capabilities(document)?;
    Ok(obj([
        ("profile", get(&caps, "profile").clone()),
        ("requires", get(&caps, "requires").clone()),
    ]))
}
fn conflicts(document: &V, hypotheses: &V, context: &V) -> Result<V> {
    let mut holders = BTreeMap::<String, Vec<V>>::new();
    let mut meanings = BTreeMap::<String, BTreeSet<String>>::new();
    let mut active = BTreeSet::new();
    let mut add = |label: &str, body: &V, meaning_document: &V| -> Result<()> {
        let roles = crate::reasoning_fields::semantic_roles(meaning_document)?;
        for (id, (collection, value)) in entries(body)? {
            holders
                .entry(id.clone())
                .or_default()
                .push(V::List(vec![s(label), value.clone()]));
            let role = if let Some((judgments, fields)) = &roles {
                obj([
                    ("judgment", V::Bool(judgments.contains(&id))),
                    (
                        "fields",
                        if judgments.contains(&id) {
                            V::Map(fields.clone())
                        } else {
                            empty()
                        },
                    ),
                ])
            } else {
                V::Null
            };
            let identity = obj([
                ("collection", s(&collection)),
                ("body", value),
                ("reasoning", meaning(meaning_document)?),
                ("roles", role),
            ])
            .digest()?;
            meanings.entry(id).or_default().insert(identity);
        }
        Ok(())
    };
    add("checkout", document, document)?;
    for (name, hyp) in map(hypotheses)? {
        if truth(get(hyp, "error")) {
            continue;
        }
        let body = get(hyp, "document");
        if get(hyp, "kind") == &s("contribution") {
            active.extend(entries(body)?.into_keys());
            add(name, body, body)?;
        } else {
            add(&format!("hypothesis:{name}"), body, &layer(document, body)?)?;
        }
    }
    let target = get(context, "target");
    let captured = get(target, "snapshot");
    if truth(captured) {
        let label = format!(
            "target:{}",
            crate::source_text::ordinary_python_str(get(target, "ref"))
        );
        add(&label, get(captured, "doc"), get(captured, "doc"))?;
        for hyp in list(get(captured, "hypotheses")) {
            let body = layer(get(captured, "doc"), get(hyp, "doc"))?;
            add(
                &format!("{label}:hypothesis:{}", text(get(hyp, "name"))?),
                &body,
                &body,
            )?;
        }
    }
    if !truth(captured)
        && let V::Map(conflicts) = get(context, "conflicts")
    {
        for (id, variants) in conflicts {
            for variant in list(variants) {
                let v = list(variant);
                if v.len() == 2 && text(&v[0]).is_ok_and(|s| s.starts_with("target:")) {
                    holders.entry(id.clone()).or_default().push(variant.clone());
                    meanings
                        .entry(id.clone())
                        .or_default()
                        .insert(obj([("target", v[1].clone())]).digest()?);
                }
            }
        }
    }
    Ok(V::Map(
        active
            .into_iter()
            .filter(|id| meanings.get(id).is_some_and(|v| v.len() > 1))
            .map(|id| {
                let v = V::List(holders.remove(&id).unwrap_or_default());
                (id, v)
            })
            .collect(),
    ))
}
fn source_groups(source: &crate::history_yaml::OrdinaryValue) -> Vec<(String, Vec<String>)> {
    use crate::history_yaml::OrdinaryValue as S;
    let mut groups = vec![];
    if let S::Map(collections) = source {
        for (k, v) in collections {
            let Some(key) = k.text() else { continue };
            if ["meta", "schema", "record", "also", "hypothesis"].contains(&key) {
                continue;
            }
            if let S::Map(m) = v {
                groups.push((
                    key.to_owned(),
                    m.iter()
                        .filter_map(|(k, _)| k.text().map(str::to_owned))
                        .collect(),
                ));
            }
        }
    }
    groups
}
pub(super) fn source_order(source: &crate::history_yaml::OrdinaryValue) -> Vec<String> {
    source_groups(source)
        .into_iter()
        .flat_map(|(_, ids)| ids)
        .collect()
}
fn layer_order(
    base: &crate::history_yaml::OrdinaryValue,
    hypothesis: &crate::history_yaml::OrdinaryValue,
) -> Vec<String> {
    let mut groups = source_groups(base);
    for (collection, ids) in source_groups(hypothesis) {
        for (name, members) in &mut groups {
            if name != &collection {
                members.retain(|id| !ids.contains(id));
            }
        }
        if let Some((_, members)) = groups.iter_mut().find(|(name, _)| *name == collection) {
            for id in ids {
                if !members.contains(&id) {
                    members.push(id);
                }
            }
        } else {
            groups.push((collection, ids));
        }
    }
    groups.into_iter().flat_map(|(_, ids)| ids).collect()
}

fn references(value: &V, out: &mut BTreeSet<String>) {
    match value {
        V::Map(m) => {
            if let Some(V::Text(file)) = m.get("file") {
                out.insert(file.clone());
            }
            for v in m.values() {
                references(v, out)
            }
        }
        V::List(a) => {
            for v in a {
                references(v, out)
            }
        }
        _ => {}
    }
}
fn extend(
    source: &CapturedSource,
    record: &Path,
    project: &Project,
) -> Result<(Inventory, BTreeSet<PathBuf>)> {
    let mut inventory = source.inventory().clone();
    let root = record.parent().unwrap();
    let entry = name(Path::new(record.file_name().unwrap()))?;
    let other = if entry == "PROVENANCE.yaml" {
        "GROUNDING.yaml"
    } else {
        "PROVENANCE.yaml"
    };
    let optional = source
        .files()
        .keys()
        .map(PathBuf::as_path)
        .chain(std::iter::once(record))
        .map(|p| {
            Ok(p.parent()
                .unwrap()
                .join(Layout::for_entry(name(Path::new(p.file_name().unwrap()))?)?.authority))
        })
        .collect::<Result<BTreeSet<_>>>()?;
    for ((kind, path), observed) in &inventory.events {
        require(
            !(kind == "exists"
                && *observed == Observation::Exists(false)
                && !optional.contains(path)),
            &format!("missing referenced migration source: {}", path.display()),
        )?;
    }
    for role in ["view", "measure", "session", "replaced", "hypotheses"] {
        let alternate = root.join(Paths::adjunct(other, role));
        let exists = inventory.exists(&alternate)?;
        if role == "hypotheses" {
            let mut found = vec![];
            for suffix in ["*.yaml", "*.yml"] {
                found.extend(inventory.glob(&alternate.join(suffix))?);
                inventory.glob(&root.join(Paths::adjunct(entry, role)).join(suffix))?;
            }
            require(
                found.is_empty(),
                "half-moved record layout: leftover hypotheses",
            )?;
        } else {
            require(
                !exists,
                &format!("half-moved record layout: leftover {role}"),
            )?;
            let path = root.join(Paths::adjunct(entry, role));
            if inventory.exists(&path)? {
                inventory.read(&path).map_err(|_| {
                    Error(format!("unreadable migration sidecar: {}", path.display()))
                })?;
            }
        }
    }
    inventory.verify()?;
    let mut evidence = BTreeSet::new();
    references(
        get(&source.snapshot()?.to_data(), "document"),
        &mut evidence,
    );
    for hyp in map(get(&source.snapshot()?.to_data(), "hypotheses"))?.values() {
        if get(hyp, "kind") != &s("contribution") {
            references(get(hyp, "document"), &mut evidence);
        }
    }
    for file in evidence {
        let relative = Path::new(&file);
        if relative.is_absolute()
            || file.contains("://")
            || relative
                .components()
                .any(|c| c == std::path::Component::ParentDir)
        {
            continue;
        }
        let normalized = relative
            .components()
            .filter(|c| *c != std::path::Component::CurDir)
            .collect::<PathBuf>();
        crate::history_authority::relative_path(name(&normalized)?)?;
        require(
            !relative.components().any(|c| c.as_os_str() == ".git")
                && !file.starts_with(&format!("{ARTIFACTS}/")),
            "evidence cannot name administration or migration artifacts",
        )?;
        let path = root.join(relative);
        require(
            P::resolved(&path)?.starts_with(P::resolved(root)?),
            "evidence locator escapes the captured record directory",
        )?;
        inventory.exists(&path)?;
        require(
            !path.is_dir(),
            &format!("directory evidence locator requires an explicit file: {file}"),
        )?;
        require(
            path.is_file(),
            &format!("missing referenced evidence: {file}"),
        )?;
        inventory
            .read(&path)
            .map_err(|_| Error(format!("unreadable referenced evidence: {file}")))?;
    }
    source.verify()?;
    let mut admin = BTreeSet::from([
        project.config_path.clone(),
        project.state.join("publication.json"),
    ]);
    if let Some(common) = &project.common {
        admin.insert(common.join("kpopper-record"));
    }
    for path in &admin {
        if inventory.exists(path)? {
            inventory
                .observe_bytes(path)
                .map_err(|_| Error("unreadable project observation".into()))?;
        }
    }
    inventory.verify()?;
    source.verify()?;
    Ok((inventory, admin))
}
fn relative(from: &Path, to: &Path) -> Result<String> {
    let a = from.components().collect::<Vec<_>>();
    let b = to.components().collect::<Vec<_>>();
    let shared = a.iter().zip(&b).take_while(|(a, b)| a == b).count();
    let mut path = PathBuf::new();
    for _ in shared..a.len() {
        path.push("..");
    }
    for part in &b[shared..] {
        path.push(part.as_os_str());
    }
    if path.as_os_str().is_empty() {
        path.push(".");
    }
    Ok(name(&path)?.replace('\\', "/"))
}
fn pointers(document: &V, source: &Path, mapping: &BTreeMap<PathBuf, String>) -> Result<V> {
    fn rewrite(value: &V, source: &Path, mapping: &BTreeMap<PathBuf, String>) -> Result<V> {
        Ok(match value {
            V::Text(v) if v.ends_with(".yaml") || v.ends_with(".yml") => {
                let target = absolute(&source.parent().unwrap().join(v))?;
                let target = mapping
                    .get(&target)
                    .ok_or_else(|| Error(format!("missing copied record pointer: {v}")))?;
                s(&relative(
                    Path::new(&mapping[source])
                        .parent()
                        .unwrap_or(Path::new("")),
                    Path::new(target),
                )?)
            }
            V::List(a) => V::List(
                a.iter()
                    .map(|v| rewrite(v, source, mapping))
                    .collect::<Result<_>>()?,
            ),
            V::Map(m) => V::Map(
                m.iter()
                    .map(|(k, v)| Ok((k.clone(), rewrite(v, source, mapping)?)))
                    .collect::<Result<_>>()?,
            ),
            _ => value.clone(),
        })
    }
    let mut document = document.clone();
    for key in ["record", "also"] {
        if let Some(value) = map_mut(&mut document)?.get_mut(key) {
            *value = rewrite(value, source, mapping)?;
        }
    }
    Ok(document)
}
pub(super) struct Plan {
    pub record: PathBuf,
    source: CapturedSource,
    inventory: Inventory,
    project: Project,
    record_files: Vec<PathBuf>,
    mapping: BTreeMap<PathBuf, String>,
    pub files: Files,
    pub manifest: V,
    pub original: Snapshot,
    pub candidate: Snapshot,
    pub reports: Vec<V>,
    pub problems: Vec<V>,
}
impl Plan {
    pub(super) fn prepare(
        record: &Path,
        cwd: &Path,
        mode: ReadMode,
        runtime: Option<&Runtime>,
    ) -> Result<Self> {
        Self::prepare_routed(record, cwd, mode, runtime, true)
    }
    fn prepare_routed(
        record: &Path,
        cwd: &Path,
        mode: ReadMode,
        runtime: Option<&Runtime>,
        route: bool,
    ) -> Result<Self> {
        let record = P::resolved(&cwd.join(record))?;
        let record = if route {
            P::write_paths(&[record], cwd)?[0].clone()
        } else {
            record
        };
        let project = P::project_for(std::slice::from_ref(&record), cwd)?;
        let source =
            capture_source_with_runtime(std::slice::from_ref(&record), cwd, mode, None, runtime)?;
        let original = Snapshot::from_snapshot(&source.snapshot()?.to_data())?;
        let candidate = Snapshot::from_snapshot(&original.to_data())?;
        let record_files = source.members().to_vec();
        let (inventory, admin) = extend(&source, &record, &project)?;
        let mapping = inventory
            .files
            .keys()
            .map(|p| Ok((p.clone(), Paths::portable(&record, p)?)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        for path in mapping.values() {
            crate::history_authority::relative_path(path)?;
            require(
                path != ARTIFACTS && !path.starts_with(&format!("{ARTIFACTS}/")),
                "source conflicts with migration artifacts",
            )?;
        }
        require(
            mapping.values().collect::<BTreeSet<_>>().len() == mapping.len(),
            "migration path collision",
        )?;
        let files = inventory
            .files
            .iter()
            .map(|(path, data)| (mapping[path].clone(), data.clone()))
            .collect();
        let mut plan = Self {
            record,
            source,
            inventory,
            project,
            record_files,
            mapping,
            files,
            manifest: empty(),
            original,
            candidate,
            reports: vec![],
            problems: vec![],
        };
        plan.prepare_files(runtime)?;
        plan.artifacts(&admin)?;
        plan.verify()?;
        Ok(plan)
    }
    fn report(&mut self, path: &Path, mut report: V) -> Result<()> {
        let m = map_mut(&mut report)?;
        m.remove("document");
        m.insert("origin".into(), s(&origin(&self.record, path)?));
        for problem in list(get(&report, "problems")) {
            if !self.problems.contains(problem) {
                self.problems.push(problem.clone());
            }
        }
        self.reports.push(report);
        Ok(())
    }
    fn prepare_files(&mut self, runtime: Option<&Runtime>) -> Result<()> {
        let data = self.original.to_data();
        let order = source_order(self.source.source());
        for hyp in map(get(&data, "hypotheses"))?.values() {
            require(
                !truth(get(hyp, "error")),
                &format!(
                    "unreadable hypothesis: {}",
                    crate::source_text::ordinary_python_str(get(hyp, "error"))
                ),
            )?;
        }
        for (kind, path) in self.inventory.events.keys() {
            require(
                kind != "unreadable",
                &format!("unreadable migration source: {}", path.display()),
            )?;
        }
        require(
            self.record_files.contains(&self.record),
            "record routing changed or entry is not in captured closure",
        )?;
        let layout = Layout::for_entry(name(Path::new(self.record.file_name().unwrap()))?)?;
        let hypdir = self.record.parent().unwrap().join(layout.hypotheses);
        let hypotheses = self
            .inventory
            .files
            .keys()
            .filter(|path| {
                path.parent() == Some(hypdir.as_path())
                    && path.extension().is_some_and(|e| e == "yaml" || e == "yml")
            })
            .map(|path| {
                (
                    path.clone(),
                    path.file_stem().unwrap().to_string_lossy().to_string(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let docs = self
            .record_files
            .iter()
            .chain(hypotheses.keys())
            .map(|path| Ok((path.clone(), parse(&self.inventory.files[path])?)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        let mut transformed = BTreeMap::new();
        fn edit(physical: &V, report: &V) -> Result<V> {
            let mut edited = physical.clone();
            for change in list(get(report, "changes")) {
                let collection = text(get(change, "collection"))?;
                let id = text(get(change, "id"))?;
                let field = text(get(change, "field"))?;
                let target = text(get(change, "target"))?;
                if let Some(V::Map(body)) = map_mut(&mut edited)?
                    .get_mut(collection)
                    .and_then(|v| map_mut(v).ok())
                    .and_then(|m| m.get_mut(id))
                    && body.contains_key(field)
                {
                    if target != field {
                        body.remove(field);
                    }
                    body.insert(target.into(), get(change, "after").clone());
                }
            }
            map_mut(
                map_mut(&mut edited)?
                    .entry("meta".into())
                    .or_insert_with(empty),
            )?
            .insert(
                "reasoning".into(),
                get(get(get(report, "document"), "meta"), "reasoning").clone(),
            );
            Ok(edited)
        }
        for path in self.record_files.clone() {
            let physical = &docs[&path];
            let world = merge(get(&data, "document"), physical)?;
            let report = C::convert(
                &C::world(&world, &self.original, Some(empty()), None)?,
                runtime,
                &order,
            )?;
            transformed.insert(path.clone(), edit(physical, &report)?);
            self.report(&path, report)?;
        }
        let mut document = empty();
        for path in &self.record_files {
            document = merge(&document, &transformed[path])?;
        }
        let mut candidate_hyps = get(&data, "hypotheses").clone();
        for (path, hypname) in &hypotheses {
            let order = layer_order(
                self.source.source(),
                &crate::history_yaml::decode_ordinary_source_value(&self.inventory.files[path])?,
            );
            let mut physical = docs[path].clone();
            let head = map_mut(&mut physical)?.remove("hypothesis");
            let world = layer(get(&data, "document"), &physical)?;
            let report = C::convert(
                &C::world(&world, &self.original, Some(empty()), None)?,
                runtime,
                &order,
            )?;
            let mut edited = edit(&physical, &report)?;
            self.report(path, report)?;
            map_mut(
                map_mut(&mut candidate_hyps)?
                    .get_mut(hypname)
                    .ok_or_else(|| Error("missing captured hypothesis".into()))?,
            )?
            .insert("document".into(), edited.clone());
            let before = C::world(&world, &self.original, Some(empty()), None)?;
            let after = C::world(
                &layer(&document, &edited)?,
                &self.original,
                Some(empty()),
                None,
            )?;
            self.report(path, C::compare(&before, &after, runtime, &order)?)?;
            if let Some(head) = head {
                map_mut(&mut edited)?.insert("hypothesis".into(), head);
            }
            transformed.insert(path.clone(), edited);
        }
        let mut context = get(&data, "context").clone();
        let rewritten = conflicts(&document, &candidate_hyps, &context)?;
        map_mut(&mut context)?.insert("conflicts".into(), rewritten);
        let mut incompatible = vec![];
        let capabilities = meaning(&document)?;
        if let V::Map(bundles) = get(get(&context, "pending"), "bundles") {
            for (revision, bundle) in bundles {
                match meaning(get(get(bundle, "manifest"), "document")) {
                    Ok(m) if m == capabilities => {}
                    Ok(_) => incompatible.push(s(revision)),
                    Err(e) if e.0.starts_with("unsupported_capability") => {
                        incompatible.push(s(revision))
                    }
                    Err(e) => return Err(e),
                }
            }
        }
        map_mut(&mut context)?.insert(
            "migration".into(),
            obj([
                ("transformation", s(C::TRANSFORMATION)),
                ("source_snapshot_id", s(self.original.snapshot_id())),
                ("frozen_candidate", V::Bool(true)),
                ("publication_authority", V::Bool(false)),
                ("incompatible_original_pending", V::List(incompatible)),
            ]),
        );
        self.candidate = C::world(
            &document,
            &self.original,
            Some(candidate_hyps),
            Some(context),
        )?;
        let report = C::compare(&self.original, &self.candidate, runtime, &order)?;
        self.report(&self.record.clone(), report)?;
        for (path, edited) in transformed {
            let raw = &self.inventory.files[&path];
            let (bom, body) = raw
                .strip_prefix(b"\xef\xbb\xbf")
                .map(|body| (&b"\xef\xbb\xbf"[..], body))
                .unwrap_or((&b""[..], raw.as_slice()));
            let mut patched = bom.to_vec();
            patched.extend(Y::patch(
                body,
                &docs[&path],
                &pointers(&edited, &path, &self.mapping)?,
            )?);
            self.files.insert(self.mapping[&path].clone(), patched);
        }
        Ok(())
    }
    fn artifacts(&mut self, admin: &BTreeSet<PathBuf>) -> Result<()> {
        self.files.insert(
            format!("{ARTIFACTS}/original.json"),
            self.original.to_json()?.into_bytes(),
        );
        self.files.insert(
            format!("{ARTIFACTS}/candidate.json"),
            self.candidate.to_json()?.into_bytes(),
        );
        for (path, data) in &self.inventory.files {
            self.files.insert(
                format!("{ARTIFACTS}/originals/{}", self.mapping[path]),
                data.clone(),
            );
        }
        let data = self.original.to_data();
        let pending = get(get(&data, "context"), "pending");
        self.files
            .insert(format!("{ARTIFACTS}/pending.json"), tagged(pending)?);
        if let V::Map(bundles) = get(pending, "bundles") {
            for (revision, bundle) in bundles {
                require(
                    !revision.is_empty()
                        && revision
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                    "invalid pending revision",
                )?;
                if let V::Map(files) = get(bundle, "files") {
                    for (path, value) in files {
                        crate::history_authority::relative_path(path)?;
                        let bytes = if get(value, "encoding") == &s("hex") {
                            let text = text(get(value, "data"))?;
                            require(text.len() % 2 == 0, "invalid captured pending evidence")?;
                            (0..text.len())
                                .step_by(2)
                                .map(|i| {
                                    u8::from_str_radix(&text[i..i + 2], 16).map_err(|_| {
                                        Error("invalid captured pending evidence".into())
                                    })
                                })
                                .collect::<Result<Vec<_>>>()?
                        } else {
                            return Err(Error("invalid captured pending evidence".into()));
                        };
                        self.files
                            .insert(format!("{ARTIFACTS}/pending/{revision}/{path}"), bytes);
                    }
                }
            }
        }
        let mut inventory = vec![];
        for ((kind, path), observed) in &self.inventory.events {
            if admin.contains(path) {
                continue;
            }
            let value = match observed {
                Observation::Bytes(v) | Observation::Unreadable(v) => s(v),
                Observation::Exists(v) | Observation::File(v) | Observation::Directory(v) => {
                    V::Bool(*v)
                }
                Observation::Glob(paths) => V::List(
                    paths
                        .iter()
                        .map(|path| Ok(s(&origin(&self.record, path)?)))
                        .collect::<Result<_>>()?,
                ),
            };
            inventory.push(V::List(vec![
                s(kind),
                s(&origin(&self.record, path)?),
                value,
            ]));
        }
        // Python sorts after replacing absolute names with portable origin labels.
        inventory.sort_by_key(|v| {
            (
                text(&list(v)[0]).unwrap().to_owned(),
                text(&list(v)[1]).unwrap().to_owned(),
            )
        });
        let config = self.project.config()?;
        self.manifest = obj([
            ("version", V::from_json(&json!(1))?),
            ("transformation", s(C::TRANSFORMATION)),
            ("record", s(&self.mapping[&self.record])),
            ("state", s("frozen_candidate")),
            ("publication_authority", V::Bool(false)),
            ("source_snapshot_id", s(self.original.snapshot_id())),
            ("candidate_snapshot_id", s(self.candidate.snapshot_id())),
            (
                "observation_sha256",
                s(&sha256(&tagged(&obj([
                    ("context", get(&data, "context").clone()),
                    ("config_exists", V::Bool(self.project.config_path.exists())),
                ]))?)),
            ),
            ("config_sha256", s(&sha256(&tagged(&config)?))),
            (
                "inventory_sha256",
                s(&sha256(&tagged(&V::List(inventory))?)),
            ),
            (
                "source",
                V::List(
                    self.inventory
                        .files
                        .iter()
                        .map(|(path, bytes)| {
                            Ok(obj([
                                ("origin", s(&origin(&self.record, path)?)),
                                ("path", s(&self.mapping[path])),
                                ("sha256", s(&sha256(bytes))),
                            ]))
                        })
                        .collect::<Result<_>>()?,
                ),
            ),
            (
                "destination",
                V::Map(
                    self.files
                        .iter()
                        .map(|(path, bytes)| (path.clone(), s(&sha256(bytes))))
                        .collect(),
                ),
            ),
            (
                "reports",
                V::from_json(&V::List(self.reports.clone()).to_tagged()?)?,
            ),
            ("problems", V::List(self.problems.clone())),
            ("complete", V::Bool(self.problems.is_empty())),
        ]);
        self.files.insert(
            format!("{ARTIFACTS}/receipt.json"),
            serde_json::to_vec(&self.manifest.to_json()?)?,
        );
        Ok(())
    }
    pub(super) fn summary(&self) -> Result<J> {
        Ok(
            json!({"profile":"core/v1","state":if self.problems.is_empty(){"preview"}else{"blocked"},"record":name(&self.record)?,"complete":self.problems.is_empty(),"problems":plain(&V::List(self.problems.clone()))?,"changes":plain(&V::List(self.reports.iter().flat_map(|r|list(get(r,"changes")).to_vec()).collect()))?,"fired":self.reports.iter().flat_map(|r|list(get(r,"fired")).iter().filter_map(|v|text(v).ok()).map(str::to_owned)).collect::<BTreeSet<_>>(),"reports":plain(&V::List(self.reports.clone()))?,"manifest":self.manifest.to_json()?}),
        )
    }
    pub(super) fn verify(&self) -> Result<()> {
        self.inventory.verify().map_err(|e| {
            if e.0 == "snapshot_changed" {
                Error("snapshot_changed: source inventory changed".into())
            } else {
                e
            }
        })?;
        self.source.verify().map_err(|e| {
            if e.0 == "snapshot_changed" {
                Error("snapshot_changed: routing, pending, or target observation changed".into())
            } else {
                e
            }
        })
    }
    pub(super) fn validate(&self, destination: &Path) -> Result<()> {
        self.verify()?;
        require(self.problems.is_empty(), "incomplete core conversion")?;
        require(
            fs::read(destination.join(ARTIFACTS).join("receipt.json"))?
                == self.files[&format!("{ARTIFACTS}/receipt.json")],
            "migration receipt does not match recomputed source conversion",
        )?;
        fn walk(root: &Path, path: &Path, out: &mut BTreeMap<String, PathBuf>) -> Result<()> {
            for item in fs::read_dir(path)? {
                let path = item?.path();
                if path.is_symlink() || path.is_file() {
                    out.insert(name(path.strip_prefix(root).unwrap())?.to_owned(), path);
                } else if path.is_dir() {
                    walk(root, &path, out)?;
                }
            }
            Ok(())
        }
        let mut actual = BTreeMap::new();
        walk(destination, destination, &mut actual)?;
        require(
            actual.keys().eq(self.files.keys()),
            "migration destination inventory changed",
        )?;
        for (path, bytes) in &self.files {
            require(
                !actual[path].is_symlink() && fs::read(&actual[path])? == *bytes,
                &format!("migration destination bytes changed: {path}"),
            )?;
        }
        for (kind, expected) in [("original", &self.original), ("candidate", &self.candidate)] {
            let snapshot = Snapshot::from_json(&fs::read(
                destination.join(ARTIFACTS).join(format!("{kind}.json")),
            )?)?;
            require(
                snapshot.snapshot_id() == expected.snapshot_id(),
                "migration snapshot changed",
            )?;
        }
        let mut inventory = Inventory::default();
        let copied = crate::source_document::load(
            &[destination.join(&self.mapping[&self.record])],
            &mut inventory,
            false,
        )?;
        let hypdir = self.record.parent().unwrap().join(
            Layout::for_entry(name(Path::new(self.record.file_name().unwrap()))?)?.hypotheses,
        );
        let expected = self
            .record_files
            .iter()
            .chain(
                self.inventory
                    .files
                    .keys()
                    .filter(|p| p.parent() == Some(hypdir.as_path())),
            )
            .map(|p| destination.join(&self.mapping[p]))
            .collect::<BTreeSet<_>>();
        require(
            inventory.files.keys().cloned().collect::<BTreeSet<_>>() == expected,
            "copied reader closure differs from mapped inventory",
        )?;
        let data = self.candidate.to_data();
        fn strip(mut value: V) -> Result<V> {
            map_mut(&mut value)?.remove("record");
            map_mut(&mut value)?.remove("also");
            Ok(value)
        }
        require(
            strip(copied.source.strict_typed()?)?.digest()?
                == strip(get(&data, "document").clone())?.digest()?,
            "copied document differs from transformed candidate",
        )?;
        for (hypname, hyp) in map(&copied.hypotheses)? {
            let expected = get(get(&data, "hypotheses"), hypname);
            require(
                get(hyp, "head").digest()? == get(expected, "head").digest()?,
                "copied hypothesis head differs from captured candidate",
            )?;
            require(
                strip(get(hyp, "doc").clone())?.digest()?
                    == strip(get(expected, "document").clone())?.digest()?,
                "copied hypothesis differs from transformed candidate",
            )?;
        }
        self.verify()
    }
    pub(super) fn publish(&self, destination: &Path) -> Result<J> {
        require(self.problems.is_empty(), "incomplete core conversion")?;
        self.verify()?;
        require(
            !destination.exists() && !destination.is_symlink(),
            "snapshot destination already exists; select a new directory",
        )?;
        let destination =
            crate::history_migration_copy::publish_tree(destination, &self.files, &mut |root| {
                self.validate(root)
            })?;
        let mut result = self.summary()?;
        result["state"] = json!("materialized");
        result["record"] = json!(destination.join(&self.mapping[&self.record]));
        result["receipt"] = json!(destination.join(ARTIFACTS).join("receipt.json"));
        result["read_mode"] = json!("frozen");
        Ok(result)
    }
    pub(super) fn apply(&self) -> Result<J> {
        let changed = self
            .inventory
            .files
            .iter()
            .filter(|(path, bytes)| self.files[&self.mapping[*path]] != **bytes)
            .map(|(path, _)| path.clone())
            .collect::<Vec<_>>();
        require(
            changed == [self.record.clone()] && self.record_files.len() == 1,
            "requires_copy_migration: in-place conversion needs one changed canonical file",
        )?;
        require(self.problems.is_empty(), "incomplete core conversion")?;
        let guard = self.project.lock()?;
        let _directory = crate::history_transaction_fs::DirectoryGuard::acquire(
            self.record.parent().unwrap(),
            true,
        )?;
        self.verify()?;
        if self.project.is_git()
            && get(guard.config(), "mode") == &s("advanced")
            && self.record == self.project.record(Some(guard.config()))?
            && let Some(observation) = self.source.pending_observation()
        {
            let capabilities = meaning(get(&self.candidate.to_data(), "document"))?;
            let data = self.original.to_data();
            if let V::Map(bundles) = get(get(get(&data, "context"), "pending"), "bundles") {
                for (revision, bundle) in bundles {
                    let state = get(
                        get(get(&observation.publication, "decisions"), revision),
                        "state",
                    );
                    if [s("withdrawn"), s("rejected"), s("superseded")].contains(state) {
                        continue;
                    }
                    require(
                        meaning(get(get(bundle, "manifest"), "document"))? == capabilities,
                        &format!("pending_profile_reconciliation_required: {revision}"),
                    )?;
                }
            }
        }
        let mut backup = tempfile::Builder::new()
            .prefix(&format!(
                ".{}.pre-core-",
                self.record.file_name().unwrap().to_string_lossy()
            ))
            .suffix(".bak")
            .tempfile_in(self.record.parent().unwrap())?;
        backup.write_all(&self.inventory.files[&self.record])?;
        let (_, backup) = backup.keep().map_err(|e| Error(e.to_string()))?;
        self.verify()?;
        guard.verify()?;
        let mut temp = tempfile::NamedTempFile::new_in(self.record.parent().unwrap())?;
        temp.write_all(&self.files[&self.mapping[&self.record]])?;
        temp.persist(&self.record)
            .map_err(|e| Error(e.to_string()))?;
        let mut result = self.summary()?;
        result["state"] = json!("applied");
        result["applied"] = json!(true);
        result["backup"] = json!(backup);
        Ok(result)
    }
}

/// Exact read-only proof used by project-mode configuration. It exposes only
/// captured file images; callers cannot materialize or mutate a migration.
pub(super) struct ConfigurationProof {
    pub(super) snapshots: BTreeMap<PathBuf, Vec<u8>>,
    pub(super) receipt_sha256: String,
    pub(super) source_signature: String,
}

pub(super) fn validate_configuration_transition(
    original: &Path,
    candidate: &Path,
    receipt: &Path,
    rollback: bool,
    cwd: &Path,
    runtime: Option<&Runtime>,
) -> Result<ConfigurationProof> {
    let plan = Plan::prepare_routed(
        original,
        cwd,
        if rollback {
            ReadMode::Frozen
        } else {
            ReadMode::Live
        },
        runtime,
        false,
    )?;
    if !plan.problems.is_empty() {
        return Err(Error(format!(
            "migration cannot be validated: {}",
            plan.problems
                .iter()
                .map(crate::source_text::ordinary_python_str)
                .collect::<Vec<_>>()
                .join("; ")
        )));
    }
    require(
        candidate
            == candidate
                .parent()
                .ok_or_else(|| Error("invalid migration destination".into()))?
                .join(&plan.mapping[&plan.record]),
        "migration destination must be the complete mapped entry record",
    )?;
    let receipt_bytes = fs::read(receipt)?;
    let expected_receipt = &plan.files[&format!("{ARTIFACTS}/receipt.json")];
    if rollback {
        let manifest_json: J = serde_json::from_slice(&receipt_bytes)
            .map_err(|_| Error("invalid rollback migration evidence".into()))?;
        let manifest = V::from_json(&manifest_json)
            .map_err(|_| Error("invalid rollback migration evidence".into()))?;
        let retained =
            map(&manifest).map_err(|_| Error("invalid rollback migration evidence".into()))?;
        let recomputed = map(&plan.manifest)?;
        for key in [
            "version",
            "transformation",
            "state",
            "publication_authority",
            "record",
            "complete",
        ] {
            require(
                retained.get(key) == recomputed.get(key),
                "rollback migration contract differs",
            )?;
        }
        let retained_source = crate::history_view::list(
            retained
                .get("source")
                .ok_or_else(|| Error("invalid rollback migration evidence".into()))?,
        )?;
        let mut originals = BTreeMap::new();
        for row in retained_source {
            let row = map(row)?;
            require(
                originals
                    .insert(
                        text(&row["path"])?.to_owned(),
                        text(&row["sha256"])?.to_owned(),
                    )
                    .is_none(),
                "rollback original closure changed",
            )?;
        }
        let expected = plan
            .inventory
            .files
            .iter()
            .map(|(path, data)| (plan.mapping[path].clone(), sha256(data)))
            .collect::<BTreeMap<_, _>>();
        require(originals == expected, "rollback original closure changed")?;
        let inventory = map(retained
            .get("destination")
            .ok_or_else(|| Error("invalid rollback migration evidence".into()))?)?;
        let destination = candidate.parent().unwrap();
        fn walk(root: &Path, path: &Path, out: &mut BTreeMap<String, PathBuf>) -> Result<()> {
            for item in fs::read_dir(path)? {
                let path = item?.path();
                if path.is_symlink() || path.is_file() {
                    out.insert(name(path.strip_prefix(root).unwrap())?.to_owned(), path);
                } else if path.is_dir() {
                    walk(root, &path, out)?;
                }
            }
            Ok(())
        }
        let mut actual = BTreeMap::new();
        walk(destination, destination, &mut actual)?;
        let mut expected_names = inventory.keys().cloned().collect::<BTreeSet<_>>();
        expected_names.insert(format!("{ARTIFACTS}/receipt.json"));
        require(
            actual.keys().cloned().collect::<BTreeSet<_>>() == expected_names,
            "rollback candidate inventory changed",
        )?;
        require(
            fs::read(&actual[&format!("{ARTIFACTS}/receipt.json")])? == receipt_bytes,
            "rollback receipt differs from the candidate receipt",
        )?;
        for (path, expected) in inventory {
            require(
                !actual[path].is_symlink() && sha256(&fs::read(&actual[path])?) == text(expected)?,
                "rollback candidate bytes changed",
            )?;
        }
        for (path, data) in &plan.inventory.files {
            require(
                fs::read(
                    destination
                        .join(ARTIFACTS)
                        .join("originals")
                        .join(&plan.mapping[path]),
                )? == *data,
                "rollback original evidence changed",
            )?;
        }
        for (path, data) in &plan.files {
            if !path.starts_with(&format!("{ARTIFACTS}/")) {
                require(
                    fs::read(destination.join(path))? == *data,
                    "rollback is unavailable after further core authoring",
                )?;
            }
        }
        let retained_original = Snapshot::from_json(&fs::read(
            destination.join(ARTIFACTS).join("original.json"),
        )?)?;
        let retained_candidate = Snapshot::from_json(&fs::read(
            destination.join(ARTIFACTS).join("candidate.json"),
        )?)?;
        require(
            retained.get("source_snapshot_id") == Some(&s(retained_original.snapshot_id()))
                && retained.get("candidate_snapshot_id")
                    == Some(&s(retained_candidate.snapshot_id())),
            "rollback snapshot identity differs",
        )?;
        for (old, now) in [
            (&retained_original, &plan.original),
            (&retained_candidate, &plan.candidate),
        ] {
            let old = old.to_data();
            let now = now.to_data();
            require(
                get(&old, "document").digest()? == get(&now, "document").digest()?
                    && get(&old, "hypotheses").digest()? == get(&now, "hypotheses").digest()?,
                "rollback snapshots differ from recomputed conversion",
            )?;
        }
        plan.verify()?;
    } else {
        require(
            receipt_bytes == *expected_receipt,
            "migration receipt does not match recomputed source conversion",
        )?;
        plan.validate(candidate.parent().unwrap())?;
    }
    let mut snapshots = plan.inventory.files.clone();
    for path in plan.files.keys() {
        let path = candidate.parent().unwrap().join(path);
        snapshots.insert(path.clone(), fs::read(path)?);
    }
    Ok(ConfigurationProof {
        snapshots,
        receipt_sha256: sha256(&receipt_bytes),
        source_signature: V::Map(
            plan.inventory
                .files
                .iter()
                .map(|(path, bytes)| (plan.mapping[path].clone(), s(&sha256(bytes))))
                .collect(),
        )
        .digest()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn unhex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }
    fn fixture(root: &Path, files: &J) {
        for (name, hex) in files.as_object().unwrap() {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let bytes = unhex(hex.as_str().unwrap());
            let bytes = String::from_utf8(bytes)
                .unwrap()
                .replace("$ROOT", root.to_str().unwrap())
                .into_bytes();
            fs::write(path, bytes).unwrap();
        }
    }
    fn images(root: &Path) -> J {
        fn walk(root: &Path, path: &Path, out: &mut serde_json::Map<String, J>) {
            for entry in fs::read_dir(path).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(root, &path, out);
                } else {
                    let bytes = fs::read(&path).unwrap();
                    let bytes = String::from_utf8(bytes)
                        .unwrap()
                        .replace(root.to_str().unwrap(), "$ROOT")
                        .into_bytes();
                    out.insert(
                        path.strip_prefix(root).unwrap().to_str().unwrap().into(),
                        json!(bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()),
                    );
                }
            }
        }
        let mut out = serde_json::Map::new();
        walk(root, root, &mut out);
        J::Object(out)
    }
    #[test]
    fn stale_record_evidence_and_policy_refuse_before_candidate_write() {
        let Some(runtime) = crate::public_workspace::runtime().unwrap() else {
            eprintln!("set KPOPPER_NATIVE_RESOURCES to run migration source-revalidation oracles");
            return;
        };
        let cases: J = serde_json::from_str(include_str!(
            "../tests/fixtures/core-expression-tamper.json"
        ))
        .unwrap();
        for case in cases.as_array().unwrap() {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            fixture(&root, &case["input"]);
            let plan = Plan::prepare(
                &root.join("GROUNDING.yaml"),
                &root,
                ReadMode::Live,
                Some(&runtime),
            )
            .unwrap();
            assert!(plan.problems.is_empty(), "{:?}", plan.problems);
            fixture(&root, &case["mutated"]);
            assert_eq!(
                plan.publish(&root.join("copy")).unwrap_err().0,
                case["result"]["publish"]["error"]
            );
            assert_eq!(
                plan.apply().unwrap_err().0,
                case["result"]["apply"]["error"]
            );
            assert!(!root.join("copy").exists());
            assert_eq!(images(&root), case["output"], "{}", case["name"]);
        }
    }
    #[test]
    fn copied_candidate_validation_rejects_receipt_inventory_and_byte_tampering() {
        let Some(runtime) = crate::public_workspace::runtime().unwrap() else {
            eprintln!("set KPOPPER_NATIVE_RESOURCES to run destination validation");
            return;
        };
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let record = root.join("GROUNDING.yaml");
        fs::write(&record, "known:\n  p.a: {v: 2}\n  p.b: {rule: p.a + 1}\n").unwrap();
        let plan = Plan::prepare(&record, &root, ReadMode::Live, Some(&runtime)).unwrap();
        let destination = root.join("copy");
        plan.publish(&destination).unwrap();
        plan.validate(&destination).unwrap();
        for (path, reason) in [
            (
                format!("{ARTIFACTS}/receipt.json"),
                "migration receipt does not match recomputed source conversion",
            ),
            (
                "GROUNDING.yaml".into(),
                "migration destination bytes changed: GROUNDING.yaml",
            ),
        ] {
            let original = fs::read(destination.join(&path)).unwrap();
            fs::write(destination.join(&path), b"tampered").unwrap();
            assert_eq!(plan.validate(&destination).unwrap_err().0, reason);
            fs::write(destination.join(&path), original).unwrap();
        }
        fs::write(destination.join("extra.txt"), b"extra").unwrap();
        assert_eq!(
            plan.validate(&destination).unwrap_err().0,
            "migration destination inventory changed"
        );
    }
}
