//! Lossless, copy-only import of existing authored records into immutable history.
//! Import captures now; it never supplies an unknown original actor or observation.
pub use crate::history_migration_copy::{replay_from_copy, restore_from_copy};
use crate::{
    Result, history_adapter as Adapter,
    history_authoring::{empty, n, obj, s, strings},
    history_authority as A, history_capture as H,
    history_contract::*,
    history_emit as E, history_hypothesis_import_prepare as HI, history_migration_copy as Copy,
    history_migration_source as S, history_paths as HP, history_preparation as Prep,
    history_reduce as R, history_transaction as T,
    history_view::{self as View, list, map_mut, truth},
    history_yaml as Y,
    identity::{sha256, typed_object_identity},
    project_modes as P, reasoning_fields as Fields,
    reasoning_runtime::Runtime,
    reasoning_snapshot::{CaptureOptions, Snapshot, entries},
    require,
    source_capture::{CapturedSource, ReadMode, capture_source_with_runtime},
    source_inventory::{Inventory, absolute, name},
    value::TypedValue as V,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
pub const ARTIFACTS: &str = S::ARTIFACTS;
#[derive(Clone)]
pub struct Options {
    pub operation: String,
    pub recorded_at: String,
    pub record_id: Option<String>,
    pub read_mode: ReadMode,
    pub route: bool,
    pub as_of: Option<V>,
}
pub struct Plan {
    record: PathBuf,
    options: Options,
    source: CapturedSource,
    observed: Option<CapturedSource>,
    inventory: Inventory,
    pub(crate) files: A::Files,
    pub(crate) manifest: V,
    pub(crate) original: Snapshot,
    pub(crate) candidate: Snapshot,
    pub(crate) artifacts: String,
    problems: Vec<String>,
}
pub(crate) fn original_provenance() -> V {
    obj([
        ("writer", V::Null),
        ("operation", V::Null),
        ("recorded_at", V::Null),
        ("source_clock", V::Null),
        ("condition_profile", V::Null),
    ])
}
pub(crate) fn entry_identity(doc: &V) -> Result<String> {
    V::Map(
        entries(doc)?
            .into_iter()
            .map(|(subject, (collection, body))| (subject, V::List(vec![s(&collection), body])))
            .collect(),
    )
    .digest()
}
pub(crate) fn hypothesis_identity(hypotheses: &V) -> Result<String> {
    V::Map(
        map(hypotheses)?
            .iter()
            .map(|(k, v)| {
                let m = map(v)?;
                Ok((
                    k.clone(),
                    obj([
                        (
                            "document",
                            m.get("document")
                                .or_else(|| m.get("doc"))
                                .cloned()
                                .unwrap_or_else(empty),
                        ),
                        ("head", m.get("head").cloned().unwrap_or_else(empty)),
                        ("error", m.get("error").cloned().unwrap_or(V::Null)),
                    ]),
                ))
            })
            .collect::<Result<_>>()?,
    )
    .digest()
}
fn locator(
    path: &Path,
    collection: &str,
    subject: &str,
    mapping: &BTreeMap<PathBuf, String>,
    files: &BTreeMap<PathBuf, Vec<u8>>,
    options: &Options,
) -> Result<V> {
    Ok(obj([
        ("version", n("1")),
        ("kind", s("legacy_import")),
        (
            "path",
            s(mapping
                .get(path)
                .ok_or_else(|| error("unresolved_import_origin"))?),
        ),
        (
            "sha256",
            s(&sha256(
                files
                    .get(path)
                    .ok_or_else(|| error("unresolved_import_origin"))?,
            )),
        ),
        ("collection", s(collection)),
        ("subject", s(subject)),
        ("original", original_provenance()),
        (
            "import",
            obj([
                ("operation", s(&options.operation)),
                ("recorded_at", s(&options.recorded_at)),
            ]),
        ),
    ]))
}
fn record_object(mut value: V, objects: &mut Map, locators: &mut Vec<V>, locator: &V) -> Result<V> {
    let id = typed_object_identity(&value)?;
    map_mut(&mut value)?.insert("id".into(), s(&id));
    validate_object(&value)?;
    require(objects.len() < MAX_OBJECTS, "history_limit")?;
    objects.insert(id.clone(), value.clone());
    locators.push(obj([
        ("subject", map(&value)?["subject"].clone()),
        ("version", s(&id)),
        ("locator", locator.clone()),
    ]));
    Ok(value)
}
#[allow(clippy::too_many_arguments)]
fn claim(
    subject: &str,
    collection: &str,
    body: &V,
    fields: &V,
    profile: &V,
    locator: &V,
    saw: &[String],
    objects: &mut Map,
    locators: &mut Vec<V>,
    options: &Options,
) -> Result<V> {
    let deps_name = text(&map(fields)?["deps"])?;
    let b = map(body).ok();
    let deps = b
        .and_then(|b| b.get(deps_name))
        .cloned()
        .unwrap_or(V::List(vec![]));
    let deps = list(&deps).map_err(|_| error("unsupported_legacy_dependency_mapping"))?;
    let gaps = deps
        .iter()
        .map(|v| {
            Ok((
                text(v)
                    .map_err(|_| error("unsupported_legacy_dependency_mapping"))?
                    .into(),
                s("not_recorded"),
            ))
        })
        .collect::<Result<Map>>()?;
    let mut saw = saw.to_vec();
    saw.sort();
    record_object(
        obj([
            ("schema_version", n("2")),
            ("id_scheme", s("typed-history/v2")),
            ("subject", s(subject)),
            (
                "kind",
                s(if b.is_some_and(|b| b.contains_key(deps_name)) {
                    "judgment"
                } else {
                    "reading"
                }),
            ),
            ("by", V::Null),
            ("on", s(&options.recorded_at)),
            ("op", s(&options.operation)),
            ("body", body.clone()),
            ("saw", strings(saw)),
            (
                "authored",
                obj([
                    ("collection", s(collection)),
                    ("fields", fields.clone()),
                    ("profile", profile.clone()),
                    ("locator", locator.clone()),
                ]),
            ),
            ("pins", empty()),
            ("pin_gaps", V::Map(gaps)),
        ]),
        objects,
        locators,
        locator,
    )
}
#[allow(clippy::too_many_arguments)]
fn act(
    subject: &str,
    previous: &V,
    current: Option<&V>,
    saw: &[String],
    location: &V,
    objects: &mut Map,
    locators: &mut Vec<V>,
    options: &Options,
) -> Result<V> {
    let mut saw = saw.to_vec();
    saw.sort();
    let (kind, of, over, because, key) = if let Some(current) = current {
        (
            "accept",
            map(current)?["id"].clone(),
            V::List(vec![map(previous)?["id"].clone()]),
            "imported recorded replacement; original actor and operation are unknown".into(),
            "imported_replacement",
        )
    } else {
        let body = map(&map(previous)?["body"])?;
        let reason = body
            .get("ended")
            .filter(|v| truth(v))
            .or_else(|| body.get("dropped").filter(|v| truth(v)))
            .cloned()
            .unwrap_or(s("recorded archive without a current successor"));
        (
            "retire",
            map(previous)?["id"].clone(),
            V::List(vec![]),
            crate::source_text::python_str(&Y::SourceValue::from_typed(&reason)),
            "imported_retirement",
        )
    };
    record_object(
        obj([
            ("schema_version", n("2")),
            ("id_scheme", s("typed-history/v2")),
            ("subject", s(subject)),
            ("kind", s("act")),
            ("by", V::Null),
            ("on", s(&options.recorded_at)),
            ("op", s(&options.operation)),
            ("saw", strings(saw)),
            ("at", obj([(key, location.clone())])),
            (
                "body",
                obj([
                    ("act", s(kind)),
                    ("of", of),
                    ("over", over),
                    ("because", s(&because)),
                ]),
            ),
        ]),
        objects,
        locators,
        location,
    )
}
fn archive_version(versions: &[V], index: usize) -> Result<V> {
    let mut value = &versions[index];
    static ALIAS_INTEGER: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"^[+-]?[0-9](?:_?[0-9])*$").unwrap());
    for _ in 0..versions.len() {
        let Some(alias) = map(value)?.get("same_as") else {
            break;
        };
        let numeric = match alias {
            V::Integer(i) => i.as_str().parse::<i128>().ok(),
            V::Bool(b) => Some(i128::from(*b)),
            V::Float(f) => {
                let x = f.get();
                (x.is_finite() && x >= i128::MIN as f64 && x < i128::MAX as f64)
                    .then_some(x.trunc() as i128)
            }
            V::Text(t) => {
                let t = crate::reasoning_authoring::decimal_digits(t.trim());
                if ALIAS_INTEGER.is_match(&t) {
                    t.replace('_', "").parse::<i128>().ok()
                } else {
                    None
                }
            }
            _ => None,
        };
        let Some(index) = numeric
            .and_then(|n| n.checked_sub(1))
            .and_then(|n| {
                if n < 0 {
                    n.checked_add(versions.len() as i128)
                } else {
                    Some(n)
                }
            })
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| *n < versions.len())
        else {
            break;
        };
        value = &versions[index];
    }
    Ok(value.clone())
}
pub(crate) fn empty_capture(root: &Path, entry: &str, marker: &V) -> Result<H::Capture> {
    let state = R::reduce(&Map::new(), None, None)?;
    Ok(H::Capture {
        root: root.into(),
        layout: H::Layout::for_entry(entry)?,
        entry_bytes: vec![],
        document: empty(),
        view_alternatives: vec![],
        authority_bytes: E::encode_document(marker)?,
        marker: marker.clone(),
        commits: A::Files::new(),
        object_bytes: A::ObjectBytes::new(),
        objects: Map::new(),
        baseline: H::baseline(marker, &A::Files::new(), &state)?,
        state,
        inactive_generations: BTreeMap::new(),
        cancellation_bytes: A::Files::new(),
        storage_bytes: A::Files::new(),
        object_paths: A::ObjectPaths::new(),
        inventory: BTreeMap::new(),
    })
}
impl Plan {
    pub fn prepare(
        record: &Path,
        cwd: &Path,
        options: Options,
        runtime: Option<&Runtime>,
    ) -> Result<Self> {
        token(&s(&options.operation))?;
        require(!options.recorded_at.is_empty(), "missing_recording_time")?;
        let paths = if options.route {
            P::write_paths(&[record.into()], cwd)?
        } else {
            vec![cwd.join(record)]
        };
        let record = paths[0].canonicalize()?;
        let root = record.parent().unwrap();
        let entry = name(Path::new(record.file_name().unwrap()))?;
        let source = capture_source_with_runtime(
            std::slice::from_ref(&record),
            cwd,
            ReadMode::Frozen,
            options.as_of.clone(),
            runtime,
        )?;
        let original = source.snapshot().clone();
        let project = P::project_for(std::slice::from_ref(&record), cwd)?;
        let layout = T::Layout::for_entry(entry)?;
        let record_files = source
            .files()
            .keys()
            .filter(|p| {
                p.parent() != Some(root.join(&layout.hypotheses).as_path())
                    && **p != root.join(&layout.authority)
            })
            .cloned()
            .collect::<Vec<_>>();
        let inventory = S::extend(&source, &record, &project)?;
        let source_files = &inventory.files;
        let previous = source_files
            .get(&root.join(&layout.authority))
            .map(|raw| Y::decode_document(raw))
            .transpose()?;
        let generation = previous
            .as_ref()
            .map(|v| text_integer(&map(v)?["generation"]))
            .transpose()?
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| error("history_limit"))?;
        let artifacts = if generation == 1 {
            ARTIFACTS.into()
        } else {
            format!("{ARTIFACTS}/generations/{generation}")
        };
        let mut mapping = BTreeMap::new();
        for path in source_files.keys() {
            let relative = S::portable(&record, path)?;
            A::relative_path(&relative)?;
            require(
                relative != artifacts && !relative.starts_with(&format!("{artifacts}/")),
                "existing_generation_artifacts",
            )?;
            mapping.insert(path.clone(), relative);
        }
        require(
            mapping.values().collect::<BTreeSet<_>>().len() == mapping.len(),
            "migration_path_collision",
        )?;
        let mut common = root.to_path_buf();
        for path in source_files.keys() {
            while !path.starts_with(&common) {
                require(common.pop(), "invalid_original_topology")?;
            }
        }
        let topology = if common != root {
            Some(obj([
                ("version", n("1")),
                (
                    "entry",
                    s(&S::posix(record.strip_prefix(&common).unwrap())?),
                ),
                (
                    "originals",
                    V::Map(
                        mapping
                            .iter()
                            .map(|(p, n)| {
                                Ok((n.clone(), s(&S::posix(p.strip_prefix(&common).unwrap())?)))
                            })
                            .collect::<Result<_>>()?,
                    ),
                ),
            ]))
        } else {
            None
        };
        let observed = if options.read_mode == ReadMode::Live {
            Some(capture_source_with_runtime(
                std::slice::from_ref(&record),
                cwd,
                ReadMode::Live,
                options.as_of.clone(),
                runtime,
            )?)
        } else {
            None
        };
        if let Some(observed) = &observed {
            Copy::validate_pending(observed.snapshot())?;
            require(
                entry_identity(&observed.document())? == entry_identity(&source.document())?,
                "migration_source_changed",
            )?;
        }
        let mut absolute_members = BTreeSet::new();
        for path in &record_files {
            for pointer in S::pointer_values(&S::parse(&source_files[path])?) {
                if Path::new(&pointer).is_absolute() {
                    absolute_members.insert(mapping[path].clone());
                }
                require(
                    mapping.contains_key(&absolute(&path.parent().unwrap().join(pointer))?),
                    "unresolved_original_pointer",
                )?;
            }
        }
        let inverse = if absolute_members.is_empty() {
            None
        } else {
            Some(obj([
                ("representable", V::Bool(false)),
                ("reason", s("absolute_original_pointer")),
                ("members", strings(absolute_members)),
            ]))
        };
        let document = source.document();
        let d = map(&document)?;
        let meta = d.get("meta").cloned().unwrap_or_else(empty);
        let meta = map(&meta).map_err(|_| error("unsupported_history_metadata"))?;
        require(
            !meta.get("history").is_some_and(truth),
            "already_active_history",
        )?;
        require(
            !meta.get("history_import").is_some_and(truth),
            "existing_import_mapping",
        )?;
        for h in map(source.hypotheses())?.values() {
            require(
                !map(h)?.get("error").is_some_and(truth),
                "unreadable_hypothesis",
            )?;
        }
        let fields = V::Map(Fields::snapshot_fields(&document)?);
        let capabilities = Fields::capabilities(&document, None)?;
        let profile = map(&capabilities)?["profile"].clone();
        let known = entries(&document)?;
        let archive_path = root.join(&layout.replaced);
        let archive = source_files
            .get(&archive_path)
            .map(|raw| S::parse(raw))
            .transpose()?
            .unwrap_or_else(empty);
        let archive = map(&archive)?;
        for versions in archive.values() {
            for raw in list(versions).map_err(|_| error("invalid_replaced_archive"))? {
                map(raw).map_err(|_| error("invalid_replaced_archive"))?;
            }
        }
        let mut objects = Map::new();
        let mut locators = vec![];
        let mut problems = vec![];
        let subjects = known
            .keys()
            .cloned()
            .chain(archive.keys().filter(|k| !known.contains_key(*k)).cloned())
            .collect::<Vec<_>>();
        for subject in subjects {
            let current = known.get(&subject);
            let collection = current.map(|(c, _)| c.as_str()).unwrap_or("replaced");
            let mut saw = vec![];
            let mut previous_claim = None;
            let mut last_location = None;
            if let Some(versions) = archive.get(&subject) {
                let versions = list(versions)?;
                for (index, raw) in versions.iter().enumerate() {
                    let mut body = archive_version(versions, index)?;
                    if map(&body)?.contains_key("same_as") {
                        problems.push(format!("unresolved_archive_alias: {subject}:{}", index + 1));
                        continue;
                    }
                    for key in ["day", "ended", "dropped"] {
                        if let Some(v) = map(raw)?.get(key) {
                            map_mut(&mut body)?.insert(key.into(), v.clone());
                        }
                    }
                    let mut location = locator(
                        &archive_path,
                        collection,
                        &subject,
                        &mapping,
                        source_files,
                        &options,
                    )?;
                    map_mut(&mut location)?.extend(Map::from([
                        ("archive_index".into(), n(&(index + 1).to_string())),
                        ("archive_metadata".into(), raw.clone()),
                        ("original_dependency_versions".into(), s("not_recorded")),
                        (
                            "interpretation".into(),
                            obj([
                                ("profile", s("ordinary-reader/v1")),
                                ("scope", s("retained_archive_only")),
                                ("original_condition_profile", s("unknown")),
                            ]),
                        ),
                    ]));
                    if current.is_none() {
                        map_mut(&mut location)?.extend(Map::from([
                            ("original_collection".into(), V::Null),
                            ("mapping".into(), s("retained_archive_container")),
                        ]));
                    }
                    let claim = claim(
                        &subject,
                        collection,
                        &body,
                        &fields,
                        &s("ordinary-reader/v1"),
                        &location,
                        &saw,
                        &mut objects,
                        &mut locators,
                        &options,
                    )?;
                    if let Some(old) = &previous_claim {
                        let mut seen = saw.clone();
                        seen.push(text(&map(&claim)?["id"])?.into());
                        let act = act(
                            &subject,
                            old,
                            Some(&claim),
                            &seen,
                            &location,
                            &mut objects,
                            &mut locators,
                            &options,
                        )?;
                        saw.push(text(&map(&act)?["id"])?.into());
                    }
                    saw.push(text(&map(&claim)?["id"])?.into());
                    previous_claim = Some(claim);
                    last_location = Some(location);
                }
            }
            if let Some((collection, body)) = current {
                let origin = source
                    .origins()
                    .get(collection)
                    .and_then(|m| m.get(&subject))
                    .ok_or_else(|| error("unresolved_import_origin"))?;
                let location = locator(
                    origin,
                    collection,
                    &subject,
                    &mapping,
                    source_files,
                    &options,
                )?;
                let claim = claim(
                    &subject,
                    collection,
                    body,
                    &fields,
                    &profile,
                    &location,
                    &saw,
                    &mut objects,
                    &mut locators,
                    &options,
                )?;
                if let Some(old) = &previous_claim {
                    saw.push(text(&map(&claim)?["id"])?.into());
                    act(
                        &subject,
                        old,
                        Some(&claim),
                        &saw,
                        &location,
                        &mut objects,
                        &mut locators,
                        &options,
                    )?;
                }
            } else if let Some(old) = &previous_claim {
                act(
                    &subject,
                    old,
                    None,
                    &saw,
                    last_location.as_ref().unwrap(),
                    &mut objects,
                    &mut locators,
                    &options,
                )?;
            }
        }
        let mut physical = vec![];
        for (hyp, h) in map(source.hypotheses())? {
            let h = map(h)?;
            let raw_h = map(&map(source.hypotheses())?[hyp])?;
            let path = raw_h
                .get("path")
                .and_then(|v| text(v).ok())
                .map(PathBuf::from)
                .or_else(|| {
                    source_files
                        .keys()
                        .find(|p| {
                            p.parent() == Some(root.join(&layout.hypotheses).as_path())
                                && p.file_stem().and_then(|p| p.to_str()) == Some(hyp)
                        })
                        .cloned()
                })
                .ok_or_else(|| error("missing_imported_hypothesis"))?;
            physical.push(HI::Source {
                name: hyp.clone(),
                document: h
                    .get("document")
                    .or_else(|| h.get("doc"))
                    .cloned()
                    .unwrap_or_else(empty),
                head: h.get("head").cloned().unwrap_or_else(empty),
                path: mapping
                    .get(&path)
                    .ok_or_else(|| error("missing_imported_hypothesis"))?
                    .clone(),
                bytes: source_files
                    .get(&path)
                    .ok_or_else(|| error("missing_imported_hypothesis"))?
                    .clone(),
                profile: None,
                fields: None,
                error: h.get("error").cloned(),
            });
        }
        let imported = HI::prepare(
            &objects,
            &physical,
            &document,
            entry,
            &options.operation,
            &options.recorded_at,
        )?;
        objects.extend(map(&map(&imported)?["objects"])?.clone());
        validate_closure(&objects)?;
        let record_id = if let Some(previous) = &previous {
            let p = map(previous)?;
            require(
                options
                    .record_id
                    .as_ref()
                    .is_none_or(|id| p["record_id"] == s(id)),
                "authority_mismatch",
            )?;
            text(&p["record_id"])?.to_string()
        } else {
            options
                .record_id
                .clone()
                .ok_or_else(|| error("missing_record_id"))?
        };
        let cancellations = previous
            .as_ref()
            .and_then(|v| map(v).ok()?.get("cancellations"))
            .map(map)
            .transpose()?
            .cloned()
            .unwrap_or_default();
        let marker = A::authority(
            &record_id,
            "history",
            &n(&generation.to_string()),
            cancellations,
        )?;
        let mut files = mapping
            .iter()
            .map(|(p, n)| (n.clone(), source_files[p].clone()))
            .collect::<A::Files>();
        files.insert(
            format!("{artifacts}/original.json"),
            original.to_json()?.into_bytes(),
        );
        let observation = if let Some(observed) = &observed {
            let path = format!("{artifacts}/live.json");
            let bytes = observed.snapshot().to_json()?.into_bytes();
            let v = obj([
                ("read_mode", s("live")),
                ("path", s(&path)),
                ("snapshot_id", s(observed.snapshot().snapshot_id())),
                ("sha256", s(&sha256(&bytes))),
            ]);
            files.insert(path, bytes);
            Some(v)
        } else {
            None
        };
        let mut originals = Map::new();
        let mut retained_any = false;
        for (path, raw) in source_files {
            let name = &mapping[path];
            let retained = S::immutable(name, entry, &artifacts)?;
            retained_any |= retained;
            let storage = if retained {
                name.clone()
            } else {
                format!("{artifacts}/originals/{name}")
            };
            if !retained {
                files.insert(storage.clone(), raw.clone());
            }
            originals.insert(
                name.clone(),
                obj([
                    ("path", s(&storage)),
                    ("sha256", s(&sha256(raw))),
                    ("storage", s(if retained { "retained" } else { "copy" })),
                ]),
            );
        }
        let receipt_version = if retained_any { 2 } else { 1 };
        if !retained_any {
            for v in originals.values_mut() {
                map_mut(v)?.remove("storage");
            }
        }
        let mut members =
            BTreeMap::from([(format!("{artifacts}/original.json"), "retained_original")]);
        if let Some(o) = &observation {
            members.insert(text(&map(o)?["path"])?.into(), "retained_original");
        }
        for v in originals.values() {
            members.insert(text(&map(v)?["path"])?.into(), "retained_original");
        }
        for path in source_files.keys() {
            if mapping[path].starts_with(&format!("{ARTIFACTS}/")) {
                members.insert(mapping[path].clone(), "retained_original");
            }
        }
        for item in list(&map(&map(&imported)?["physical"])?["physical"])? {
            members.insert(text(&map(item)?["path"])?.into(), "retained_original");
        }
        for path in &record_files {
            if *path != record {
                members.insert(mapping[path].clone(), "retained_original");
            }
        }
        if source_files.contains_key(&archive_path) {
            members.insert(mapping[&archive_path].clone(), "replaced");
        }
        let mut template = A::document_template(&document)?;
        for o in objects.values() {
            let m = map(o)?;
            if !string_is(&m["kind"], "act") {
                map_mut(&mut template)?
                    .entry(text(&map(&m["authored"])?["collection"])?.into())
                    .or_insert_with(empty);
            }
        }
        let entry_doc = S::pointers(&S::parse(&source_files[&record])?, &record, &mapping)?;
        for key in ["record", "also"] {
            if let Some(v) = map(&entry_doc)?.get(key) {
                map_mut(&mut template)?.insert(key.into(), v.clone());
            } else {
                map_mut(&mut template)?.remove(key);
            }
        }
        let template_meta = map_mut(
            map_mut(&mut template)?
                .entry("meta".into())
                .or_insert_with(empty),
        )?;
        if !physical.is_empty() {
            template_meta.insert(
                "history_hypothesis_import".into(),
                map(&imported)?["physical"].clone(),
            );
        }
        template_meta.insert(
            "history_import".into(),
            obj([
                ("version", n("1")),
                ("operation", s(&options.operation)),
                ("recorded_at", s(&options.recorded_at)),
                (
                    "members",
                    V::List(
                        members
                            .iter()
                            .map(|(p, r)| {
                                obj([
                                    ("path", s(p)),
                                    ("sha256", s(&sha256(&files[p]))),
                                    ("role", s(r)),
                                ])
                            })
                            .collect(),
                    ),
                ),
            ]),
        );
        let mut captured = empty_capture(root, entry, &marker)?;
        let baseline = captured.baseline.clone();
        let pairs = objects
            .values()
            .map(|o| Ok((o.clone(), E::encode_document(o)?)))
            .collect::<Result<Vec<_>>>()?;
        let mut before = obj([
            ("kind", s("legacy-import/v1")),
            ("snapshot_id", s(original.snapshot_id())),
            (
                "source",
                V::Map(
                    source_files
                        .iter()
                        .map(|(p, b)| (mapping[p].clone(), s(&sha256(b))))
                        .collect(),
                ),
            ),
        ]);
        if artifacts != ARTIFACTS {
            map_mut(&mut before)?.insert("artifact_root".into(), s(&artifacts));
        }
        if retained_any {
            map_mut(&mut before)?.insert("originals_storage".into(), V::Map(originals.clone()));
        }
        for (key, v) in [
            ("topology", &topology),
            ("observation", &observation),
            ("inverse", &inverse),
        ] {
            if let Some(v) = v {
                map_mut(&mut before)?.insert(key.into(), v.clone());
            }
        }
        let after = obj([
            ("kind", s("preserved-authored-import/v1")),
            ("operation", s(&options.operation)),
            ("recorded_at", s(&options.recorded_at)),
            ("original_provenance", original_provenance()),
            ("locators", V::List(locators.clone())),
            ("unresolved", strings(problems.clone())),
        ]);
        let receipt = T::semantic_receipt(text(&profile)?, &capabilities, &before, &after)?;
        let requires = V::List(vec![s(HP::CAPABILITY)]);
        let draft = Prep::make_commit(
            &marker,
            &options.operation,
            &Map::new(),
            &baseline,
            &pairs,
            &receipt,
            b"",
            Some(&template),
            Some(&requires),
        )?;
        let mut commits =
            A::Files::from([(options.operation.clone(), E::encode_document(&draft)?)]);
        let object_bytes = pairs
            .iter()
            .map(|(o, b)| {
                Ok((
                    (
                        text(&map(o)?["subject"])?.into(),
                        text(&map(o)?["id"])?.into(),
                    ),
                    b.clone(),
                ))
            })
            .collect::<Result<A::ObjectBytes>>()?;
        let rendered = View::render(&captured, &objects, &object_bytes, &commits)?;
        let commit = Prep::make_commit(
            &marker,
            &options.operation,
            &Map::new(),
            &baseline,
            &pairs,
            &receipt,
            &rendered,
            Some(&template),
            Some(&requires),
        )?;
        commits.insert(options.operation.clone(), E::encode_document(&commit)?);
        captured.entry_bytes = rendered.clone();
        captured.document = Y::decode_document(&rendered)?;
        captured.state = R::reduce(&objects, None, None)?;
        captured.baseline = H::baseline(&marker, &commits, &captured.state)?;
        captured.commits = commits;
        captured.objects = objects;
        let (prior_commits, storage, cancelled) = S::retained(&files, entry)?;
        let (prior_objects, prior_paths) = A::objects_from_storage(&prior_commits, &storage)?;
        captured.inactive_generations =
            A::committed_generations(&marker, &prior_commits, &prior_objects, &cancelled)?;
        captured.cancellation_bytes = cancelled;
        captured.object_bytes = prior_objects;
        captured.object_bytes.extend(object_bytes);
        captured.object_paths = prior_paths;
        for (o, _) in &pairs {
            let m = map(o)?;
            let subject = text(&m["subject"])?;
            let id = text(&m["id"])?;
            captured.object_paths.insert(
                (subject.into(), id.into()),
                HP::object_path(subject, id, HP::Scheme::Hashed)?,
            );
        }
        let adapted = Adapter::from_store_capture(&captured)?;
        let data = original.to_data();
        let observation_data = observed
            .as_ref()
            .map(|o| o.snapshot().to_data())
            .unwrap_or_else(|| data.clone());
        let mut context = map(&observation_data)?["context"].clone();
        map_mut(&mut context)?.insert("read_mode".into(), s("frozen"));
        map_mut(&mut context)?.insert(
            "migration".into(),
            obj([
                ("kind", s("history-import/v1")),
                ("source_snapshot_id", s(original.snapshot_id())),
                (
                    "original_provenance",
                    s("unknown unless retained in original body"),
                ),
                (
                    "source_read_mode",
                    s(if options.read_mode == ReadMode::Live {
                        "live"
                    } else {
                        "frozen"
                    }),
                ),
                ("copied_state", s("frozen")),
                (
                    "observation_snapshot_id",
                    map(&observation_data)?["snapshot_id"].clone(),
                ),
            ]),
        );
        let (named, index) =
            crate::history_hypotheses::layers(adapted.projection(), adapted.document())?;
        if !map(&named)?.is_empty() {
            map_mut(&mut context)?.insert("history_hypotheses".into(), index);
        }
        let mut hypotheses = map(&observation_data)?["hypotheses"].clone();
        map_mut(&mut hypotheses)?.extend(map(&named)?.clone());
        let candidate = adapted.snapshot(CaptureOptions {
            context: Some(context),
            hypotheses: Some(hypotheses),
            as_of: Some(map(&data)?["as_of"].clone()),
            ..Default::default()
        })?;
        files.insert(entry.into(), rendered);
        files.insert(layout.authority.clone(), E::encode_document(&marker)?);
        let commit_path = format!("{}/{}.yaml", layout.commits, options.operation);
        require(
            !source_files.contains_key(&root.join(&commit_path)),
            "operation_collision",
        )?;
        files.insert(commit_path, E::encode_document(&commit)?);
        for (o, raw) in pairs {
            let m = map(&o)?;
            let relative = format!(
                "{}/{}",
                layout.objects,
                HP::object_path(text(&m["subject"])?, text(&m["id"])?, HP::Scheme::Hashed)?
            );
            require(
                files.get(&relative).is_none_or(|b| *b == raw),
                "immutable_collision",
            )?;
            files.insert(relative, raw);
        }
        files.insert(
            format!("{artifacts}/candidate.json"),
            candidate.to_json()?.into_bytes(),
        );
        let mut manifest = obj([
            ("version", n(&receipt_version.to_string())),
            ("kind", s(&format!("history-import/v{receipt_version}"))),
            ("operation", s(&options.operation)),
            ("record", s(entry)),
            ("record_id", s(&record_id)),
            ("complete", V::Bool(problems.is_empty())),
            ("problems", strings(problems.clone())),
            ("source_snapshot_id", s(original.snapshot_id())),
            ("candidate_snapshot_id", s(candidate.snapshot_id())),
            ("locators", V::List(locators)),
            ("originals", V::Map(originals)),
            (
                "destination",
                V::Map(
                    files
                        .iter()
                        .map(|(p, b)| (p.clone(), s(&sha256(b))))
                        .collect(),
                ),
            ),
        ]);
        if artifacts != ARTIFACTS {
            map_mut(&mut manifest)?.insert("artifact_root".into(), s(&artifacts));
        }
        for (key, v) in [
            ("topology", topology),
            ("observation", observation),
            ("inverse", inverse),
        ] {
            if let Some(v) = v {
                map_mut(&mut manifest)?.insert(key.into(), v);
            }
        }
        files.insert(
            format!("{artifacts}/receipt.json"),
            manifest.canonical_bytes()?,
        );
        let plan = Self {
            record,
            options,
            source,
            observed,
            inventory,
            files,
            manifest,
            original,
            candidate,
            artifacts,
            problems,
        };
        plan.verify_source()?;
        Ok(plan)
    }
    pub fn verify_source(&self) -> Result<()> {
        self.source.verify()?;
        self.inventory.verify()?;
        for (kind, path) in self.inventory.events.keys() {
            if kind == "directory" {
                S::no_link(path)?;
            }
        }
        if let Some(o) = &self.observed {
            o.verify()?;
        }
        Ok(())
    }
    pub fn record(&self) -> &Path {
        &self.record
    }
    pub fn files(&self) -> &A::Files {
        &self.files
    }
    pub fn manifest(&self) -> &V {
        &self.manifest
    }
    pub fn original(&self) -> &Snapshot {
        &self.original
    }
    pub fn candidate(&self) -> &Snapshot {
        &self.candidate
    }
    pub fn summary(&self) -> Result<V> {
        let data = self.candidate.to_data();
        let context = map(&map(&data)?["context"])?;
        let complete = map(&map(&context["history"])?["coverage"])?["complete"].clone();
        Ok(obj([
            (
                "state",
                s(if self.problems.is_empty() {
                    "preview"
                } else {
                    "blocked"
                }),
            ),
            ("complete", V::Bool(self.problems.is_empty())),
            ("historical_support_complete", complete),
            ("record", s(name(&self.record)?)),
            ("operation", s(&self.options.operation)),
            ("problems", strings(self.problems.clone())),
            (
                "source_read_mode",
                s(if self.options.read_mode == ReadMode::Live {
                    "live"
                } else {
                    "frozen"
                }),
            ),
            ("copied_read_mode", s("frozen")),
            ("captured_pending_context", V::Bool(self.observed.is_some())),
            (
                "inverse",
                map(&self.manifest)?
                    .get("inverse")
                    .cloned()
                    .unwrap_or(obj([("representable", V::Bool(true))])),
            ),
            ("original_provenance", original_provenance()),
            ("manifest", self.manifest.clone()),
        ]))
    }
    pub fn validate_destination(&self, destination: &Path) -> Result<V> {
        self.verify_source()?;
        require(self.problems.is_empty(), "incomplete_history_import")?;
        require(
            Copy::inventory(destination)? == self.files,
            "migration_bytes_changed",
        )?;
        let capture = crate::source_capture::capture_source(
            &[destination.join(self.record.file_name().unwrap())],
            destination,
            ReadMode::Frozen,
            self.options.as_of.clone(),
        )?;
        let snapshot = capture.snapshot();
        let replay = Snapshot::from_json(snapshot.to_json()?.as_bytes())?;
        require(
            replay.snapshot_id() == snapshot.snapshot_id(),
            "migration_replay_mismatch",
        )?;
        require(
            entry_identity(&capture.document())? == entry_identity(&self.source.document())?,
            "migration_body_mismatch",
        )?;
        require(
            hypothesis_identity(&map(&snapshot.to_data())?["hypotheses"])?
                == hypothesis_identity(&map(&self.original.to_data())?["hypotheses"])?,
            "migration_hypothesis_mismatch",
        )?;
        for (kind, expected) in [("original", &self.original), ("candidate", &self.candidate)] {
            let artifact = Snapshot::from_json(&std::fs::read(
                destination
                    .join(&self.artifacts)
                    .join(format!("{kind}.json")),
            )?)?;
            require(
                artifact.snapshot_id() == expected.snapshot_id(),
                "migration_replay_mismatch",
            )?;
        }
        self.verify_source()?;
        Ok(obj([
            ("valid", V::Bool(true)),
            (
                "record",
                s(name(&destination.join(self.record.file_name().unwrap()))?),
            ),
            ("snapshot_id", s(snapshot.snapshot_id())),
            ("manifest", self.manifest.clone()),
        ]))
    }
    pub fn publish(&self, destination: &Path) -> Result<V> {
        require(self.problems.is_empty(), "incomplete_history_import")?;
        self.verify_source()?;
        let root = Copy::publish_tree(destination, &self.files, &mut |root| {
            self.validate_destination(root).map(|_| ())
        })?;
        let mut result = self.summary()?;
        map_mut(&mut result)?.extend(Map::from([
            ("state".into(), s("materialized")),
            (
                "record".into(),
                s(name(&root.join(self.record.file_name().unwrap()))?),
            ),
            (
                "receipt".into(),
                s(name(&root.join(&self.artifacts).join("receipt.json"))?),
            ),
            ("read_mode".into(), s("frozen")),
        ]));
        Ok(result)
    }
}
fn text_integer(v: &V) -> Result<u64> {
    match v {
        V::Integer(i) => i.as_str().parse().map_err(|_| error("history_limit")),
        _ => Err(error("invalid_authority")),
    }
}
