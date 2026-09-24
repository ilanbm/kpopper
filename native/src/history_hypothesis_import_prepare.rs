//! Immutable proposal objects from exact captured physical hypothesis bytes.
use crate::{
    Result,
    history_authoring::{self as A, empty, n, obj, s, strings},
    history_contract::*,
    history_hypothesis_import as I, history_reduce as R,
    history_view::{list, map_mut, truth},
    history_yaml as Y,
    identity::{sha256, typed_object_identity},
    reasoning_fields as F,
    reasoning_snapshot::{MAX_REQUEST_BYTES, entries},
    require,
    value::TypedValue as V,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
pub struct Source {
    pub name: String,
    pub document: V,
    pub head: V,
    pub path: String,
    pub bytes: Vec<u8>,
    pub profile: Option<String>,
    pub fields: Option<V>,
    pub error: Option<V>,
}
struct Normalized {
    source: Source,
    profile: V,
    sha256: String,
    evidence: &'static str,
}
fn normalize(sources: &[Source], entry: &str) -> Result<BTreeMap<String, Normalized>> {
    require(sources.len() <= MAX_OBJECTS, "history_limit")?;
    let mut out = BTreeMap::new();
    let mut total = 0usize;
    for item in sources {
        hypothesis_name(&s(&item.name))?;
        require(!out.contains_key(&item.name), "duplicate_hypothesis_import")?;
        require(
            !item.error.as_ref().is_some_and(truth),
            "unreadable_hypothesis",
        )?;
        total = total.saturating_add(item.bytes.len());
        require(total <= MAX_REQUEST_BYTES, "history_limit")?;
        let mut parsed = Y::decode_document(&item.bytes)?;
        let head = map_mut(&mut parsed)?
            .remove("hypothesis")
            .filter(|v| *v != V::Null)
            .unwrap_or_else(empty);
        map(&head).map_err(|_| error("invalid_hypothesis_head"))?;
        map(&item.head)?;
        map(&item.document)?;
        require(
            parsed == item.document && head == item.head,
            "hypothesis_source_mismatch",
        )?;
        let digest = sha256(&item.bytes);
        I::validate_mapping(
            &obj([
                ("version", n("1")),
                (
                    "physical",
                    V::List(vec![obj([
                        ("name", s(&item.name)),
                        ("path", s(&item.path)),
                        ("sha256", s(&digest)),
                    ])]),
                ),
            ]),
            entry,
        )?;
        let cap = F::capabilities(&parsed, item.profile.as_deref())
            .map_err(|_| error("unsupported_hypothesis_profile"))?;
        let cap = map(&cap)?;
        require(
            !cap.get("experimental_override").is_some_and(truth),
            "hypothesis_profile_promotion_refused",
        )?;
        out.insert(
            item.name.clone(),
            Normalized {
                source: item.clone(),
                profile: cap["profile"].clone(),
                sha256: digest,
                evidence: if item.profile.is_some() {
                    "supplied_capture_profile"
                } else {
                    "original_document"
                },
            },
        );
    }
    Ok(out)
}
fn roles(source: &Source, base: &V) -> Result<V> {
    let doc = map(&source.document)?;
    let empty_map = Map::new();
    let declared = doc
        .get("schema")
        .map(map)
        .transpose()?
        .unwrap_or(&empty_map);
    if let Some(fields) = &source.fields {
        let f = schema(fields, &["deps", "snapshot", "predicate"], &[])
            .map_err(|_| error("invalid_hypothesis_field_roles"))?;
        let names = f
            .values()
            .map(|v| text(v))
            .collect::<Result<BTreeSet<_>>>()?;
        require(
            names.len() == 3 && names.iter().all(|s| !s.is_empty()),
            "invalid_hypothesis_field_roles",
        )?;
        require(
            f.iter()
                .all(|(k, v)| declared.get(k).is_none_or(|x| x == v)),
            "hypothesis_field_role_mismatch",
        )?;
        return Ok(fields.clone());
    }
    let groups = obj([("layer", obj([("doc", source.document.clone())]))]);
    let mut layered = crate::history_hypothesis_authoring::layer(base, &groups, &["layer".into()])?;
    if !declared.is_empty() {
        map_mut(&mut layered)?.insert("schema".into(), V::Map(declared.clone()));
    }
    Ok(V::Map(
        F::snapshot_fields(&layered).map_err(|_| error("ambiguous_hypothesis_field_roles"))?,
    ))
}

pub fn prepare(
    base_objects: &Map,
    sources: &[Source],
    base_document: &V,
    entry: &str,
    operation: &str,
    recorded_at: &str,
) -> Result<V> {
    prepare_with_reduce(
        base_objects,
        sources,
        base_document,
        entry,
        operation,
        recorded_at,
        &|objects| R::reduce(objects, None, None),
    )
}
pub(crate) fn prepare_with_reduce(
    base_objects: &Map,
    sources: &[Source],
    base_document: &V,
    entry: &str,
    operation: &str,
    recorded_at: &str,
    reduce: &dyn Fn(&Map) -> Result<V>,
) -> Result<V> {
    token(&s(operation))?;
    require(!recorded_at.is_empty(), "invalid_recorded_time")?;
    Y::validate_value(base_document, MAX_REQUEST_BYTES)?;
    map(base_document)?;
    validate_closure(base_objects)?;
    let sources = normalize(sources, entry)?;
    let state = reduce(base_objects)?;
    let states = map(&map(&state)?["subjects"])?;
    let known_base = entries(base_document)?;
    for (subject, (collection, body)) in &known_base {
        let current = states
            .get(subject)
            .map(map)
            .transpose()?
            .ok_or_else(|| error("hypothesis_import_base_mismatch"))?;
        require(
            string_is(&current["acceptance"], "accepted") && current.contains_key("head"),
            "hypothesis_import_base_mismatch",
        )?;
        let o = map(&base_objects[text(&current["head"])?])?;
        require(
            string_is(&map(&o["authored"])?["collection"], collection) && o["body"] == *body,
            "hypothesis_import_base_mismatch",
        )?;
    }
    for (subject, current) in states {
        require(
            !string_is(&map(current)?["acceptance"], "accepted")
                || known_base.contains_key(subject),
            "hypothesis_import_base_mismatch",
        )?;
    }
    let mut objects = Map::new();
    let mut groups = Map::new();
    let mut physical = vec![];
    let mut diagnostics = vec![];
    let mut collections = BTreeSet::new();
    let options = A::Options {
        operation: operation.into(),
        recorded_at: recorded_at.into(),
        recording_day: recorded_at.into(),
        by: V::Null,
        strict: true,
        paths: crate::history_paths::Scheme::Hashed,
        receipt_version: None,
    };
    for (name, normalized) in sources {
        let source = &normalized.source;
        let document = &source.document;
        let known = entries(document).map_err(|_| error("ambiguous_hypothesis_entries"))?;
        require(!known.is_empty(), "empty_hypothesis_requires_group_anchor")?;
        let fields = roles(source, base_document)?;
        let member_collections = F::collections(document)?;
        let headers = V::Map(
            map(document)?
                .iter()
                .filter(|(k, _)| k.as_str() == "meta" || !member_collections.contains_key(*k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        );
        let group = obj([
            ("version", n("1")),
            ("name", s(&name)),
            ("head", source.head.clone()),
        ]);
        let mut versions = Map::new();
        for (subject, (collection, body)) in &known {
            crate::history_contract::subject(&s(subject))?;
            require(
                !["meta", "schema", "record", "also"].contains(&collection.as_str()),
                "invalid_hypothesis_collection",
            )?;
            collections.insert(collection.clone());
            let deps_name = text(&map(&fields)?["deps"])?;
            let deps = map(body).ok().and_then(|m| m.get(deps_name));
            let deps = deps
                .map(list)
                .transpose()
                .map_err(|_| error("unsupported_legacy_hypothesis_dependencies"))?
                .unwrap_or(&[]);
            let kind = if map(body).is_ok_and(|m| m.contains_key(deps_name)) {
                "judgment"
            } else {
                "reading"
            };
            for previous in base_objects.values().chain(objects.values()) {
                let p = map(previous)?;
                require(
                    !string_is(&p["subject"], subject)
                        || string_is(&p["kind"], "act")
                        || string_is(&p["kind"], kind),
                    "hypothesis_kind_conflict",
                )?;
            }
            let mut gaps = Map::new();
            let mut observations = Map::new();
            for dependency in deps {
                crate::history_contract::subject(dependency)?;
                let dependency = text(dependency)?;
                let (observation, gap) = if let Some((c, b)) = known.get(dependency) {
                    (
                        obj([
                            ("kind", s("hypothesis_entry")),
                            ("name", s(&name)),
                            ("path", s(&source.path)),
                            ("sha256", s(&normalized.sha256)),
                            ("collection", s(c)),
                            ("subject", s(dependency)),
                            ("body_digest", s(&b.digest()?)),
                        ]),
                        "not_recorded",
                    )
                } else if known_base.contains_key(dependency) {
                    let selected = map(&states[dependency])?["head"].clone();
                    (
                        obj([
                            ("kind", s("base_version")),
                            ("subject", s(dependency)),
                            ("version", selected.clone()),
                            (
                                "object_digest",
                                s(&base_objects[text(&selected)?].digest()?),
                            ),
                        ]),
                        "not_recorded",
                    )
                } else {
                    (
                        obj([("kind", s("unavailable")), ("subject", s(dependency))]),
                        "unavailable",
                    )
                };
                observations.insert(dependency.into(), observation);
                gaps.insert(dependency.into(), s(gap));
                diagnostics.push(obj([
                    ("code", s(&format!("historical_pin_{gap}"))),
                    ("hypothesis", s(&name)),
                    ("subject", s(subject)),
                    ("dependency", s(dependency)),
                ]));
            }
            let locator = obj([
                ("version", n("1")),
                ("kind", s("legacy-hypothesis-import/v1")),
                ("path", s(&source.path)),
                ("sha256", s(&normalized.sha256)),
                ("collection", s(collection)),
                ("subject", s(subject)),
                ("document_headers", headers.clone()),
                (
                    "original",
                    obj([
                        ("writer", V::Null),
                        ("operation", V::Null),
                        ("recorded_at", V::Null),
                        ("pin_versions", V::Null),
                    ]),
                ),
                (
                    "import",
                    obj([
                        ("operation", s(operation)),
                        ("recorded_at", s(recorded_at)),
                        ("observations", V::Map(observations)),
                    ]),
                ),
                (
                    "profile",
                    obj([
                        ("name", normalized.profile.clone()),
                        ("evidence", s(normalized.evidence)),
                    ]),
                ),
            ]);
            let saw = base_objects
                .iter()
                .filter(|(_, v)| map(v).is_ok_and(|m| string_is(&m["subject"], subject)))
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            let mut claim = A::make_object(
                subject,
                kind,
                body.clone(),
                strings(saw.clone()),
                Some(obj([
                    ("collection", s(collection)),
                    ("fields", fields.clone()),
                    ("profile", normalized.profile.clone()),
                    ("hypothesis", group.clone()),
                    ("locator", locator),
                ])),
                empty(),
                V::Map(gaps),
                &options,
            )?;
            map_mut(&mut claim)?
                .entry("pin_gaps".into())
                .or_insert_with(empty);
            map_mut(&mut claim)?.remove("id");
            let claim_id = typed_object_identity(&claim)?;
            map_mut(&mut claim)?.insert("id".into(), s(&claim_id));
            validate_object(&claim)?;
            let mut observed = saw;
            observed.push(claim_id.clone());
            observed.sort();
            let proposal = A::make_object(
                subject,
                "act",
                obj([
                    ("act", s("propose")),
                    ("of", s(&claim_id)),
                    ("over", V::List(vec![])),
                    (
                        "because",
                        s("imported physical hypothesis; no base acceptance is recorded"),
                    ),
                ]),
                strings(observed),
                None,
                empty(),
                empty(),
                &options,
            )?;
            for value in [claim, proposal] {
                let id = text(&map(&value)?["id"])?.to_owned();
                require(
                    !base_objects.contains_key(&id) && !objects.contains_key(&id),
                    "duplicate_hypothesis_import",
                )?;
                objects.insert(id, value);
            }
            versions.insert(subject.clone(), s(&claim_id));
        }
        groups.insert(
            name.clone(),
            obj([
                ("document", document.clone()),
                ("head", source.head.clone()),
                ("profile", normalized.profile),
                ("fields", fields),
                ("versions", V::Map(versions)),
            ]),
        );
        physical.push(obj([
            ("name", s(&name)),
            ("path", s(&source.path)),
            ("sha256", s(&normalized.sha256)),
        ]));
    }
    let mut combined = base_objects.clone();
    combined.extend(objects.clone());
    validate_closure(&combined)?;
    let result = reduce(&combined)?;
    let result = map(&map(&result)?["subjects"])?;
    for group in groups.values() {
        for (subject, version) in map(&map(group)?["versions"])? {
            require(
                list(&map(&result[subject])?["proposals"])?.contains(version),
                "hypothesis_import_disposition_mismatch",
            )?;
        }
    }
    for (subject, current) in states {
        require(
            map(&result[subject])?["heads"] == map(current)?["heads"],
            "hypothesis_import_changed_base",
        )?;
    }
    let physical = obj([("version", n("1")), ("physical", V::List(physical))]);
    I::validate_mapping(&physical, entry)?;
    Ok(obj([
        ("version", n("1")),
        ("objects", V::Map(objects)),
        ("physical", physical),
        ("groups", V::Map(groups)),
        ("collections", strings(collections)),
        (
            "historical_support_complete",
            V::Bool(diagnostics.is_empty()),
        ),
        ("diagnostics", V::List(diagnostics)),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD};
    #[test]
    fn captured_physical_proposals_match_python_without_rewriting_originals() {
        let cases: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/history-hypothesis-import.json"
        ))
        .unwrap();
        for case in cases.as_array().unwrap() {
            let objects = V::from_tagged(&case["objects"]).unwrap();
            let sources = V::from_tagged(&case["sources"]).unwrap();
            let options = V::from_tagged(&case["options"]).unwrap();
            let options = map(&options).unwrap();
            let input = match sources {
                V::Map(items) => items
                    .into_iter()
                    .map(|(name, mut item)| {
                        map_mut(&mut item).unwrap().insert("name".into(), s(&name));
                        item
                    })
                    .collect(),
                V::List(items) => items,
                _ => panic!("invalid fixture"),
            };
            let sources = input
                .into_iter()
                .map(|item| {
                    let m = map(&item).unwrap();
                    Source {
                        name: text(&m["name"]).unwrap().into(),
                        document: m["document"].clone(),
                        head: m["head"].clone(),
                        path: text(&m["path"]).unwrap().into(),
                        bytes: STANDARD.decode(text(&m["bytes"]).unwrap()).unwrap(),
                        profile: m.get("profile").map(|v| text(v).unwrap().into()),
                        fields: m.get("fields").cloned(),
                        error: m.get("error").cloned(),
                    }
                })
                .collect::<Vec<_>>();
            let result = prepare(
                map(&objects).unwrap(),
                &sources,
                &options["base_document"],
                options
                    .get("entry")
                    .map(|v| text(v).unwrap())
                    .unwrap_or("GROUNDING.yaml"),
                text(&options["operation"]).unwrap(),
                text(&options["recorded_at"]).unwrap(),
            );
            if let Some(expected) = case.get("output") {
                assert_eq!(
                    result.unwrap_or_else(|e| panic!("{}: {e}", case["name"])),
                    V::from_tagged(expected).unwrap(),
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
