//! Selected committed-history transport for an already prepared mutation.
use crate::{
    Result, history_adapter, history_authority as A,
    history_capture::{self as H, Capture},
    history_contract::*,
    history_emit as E, history_paths as P, history_preparation as Prep, history_reduce,
    history_transaction::{self as T, PreparedMutation},
    history_view,
    history_view::map_mut,
    identity::sha256,
    reasoning_capabilities as C, require,
    value::{Integer, TypedValue as V},
};
use std::collections::{BTreeMap, BTreeSet};

fn s(v: &str) -> V {
    V::Text(v.into())
}
fn n(v: &str) -> V {
    V::Integer(Integer::new(v).unwrap())
}
fn obj(v: impl IntoIterator<Item = (&'static str, V)>) -> V {
    V::Map(v.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
fn strings(v: impl IntoIterator<Item = String>) -> V {
    V::List(v.into_iter().map(V::Text).collect())
}

fn candidate(source: &Capture, prepared: &PreparedMutation) -> Result<Capture> {
    let data = prepared.to_data();
    require(
        field(map(&data)?, "authority")?.digest()? == source.marker.digest()?
            && field(map(&data)?, "baseline")?.digest()? == source.baseline.digest()?,
        "stale_baseline",
    )?;
    crate::history_authoring::candidate(source, prepared)
}

fn subject_inventory(capture: &Capture, subject: &str) -> V {
    V::Map(
        capture
            .objects
            .iter()
            .filter(|(_, o)| map(o).ok().and_then(|m| text(&m["subject"]).ok()) == Some(subject))
            .map(|(id, _)| {
                (
                    id.clone(),
                    s(&sha256(
                        &capture.object_bytes[&(subject.into(), id.clone())],
                    )),
                )
            })
            .collect(),
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_subset(
    source: &Capture,
    roots: &[String],
    scope: &V,
    operation: &str,
    recorded_at: &str,
    source_entry: &str,
    disclosed_locators: &V,
    prepared: &PreparedMutation,
) -> Result<(V, A::Files)> {
    token(&s(operation))?;
    require(!recorded_at.is_empty(), "invalid_recorded_time")?;
    A::relative_path(source_entry)?;
    source.verify_current()?;
    let verified = A::committed_objects(&source.marker, &source.commits, &source.object_bytes)?;
    require(
        V::Map(verified).digest()? == V::Map(source.objects.clone()).digest()?,
        "invalid_source_capture",
    )?;
    require(
        history_reduce::reduce_bytes(
            &source.object_bytes,
            Some(map(&map(&source.state)?["rules"])?),
            None,
        )?
        .digest()?
            == source.state.digest()?,
        "source_reduction_mismatch",
    )?;
    let candidate = candidate(source, prepared)?;
    let root_value = strings(roots.iter().cloned().collect::<BTreeSet<_>>());
    let subjects = crate::history_bundle::subset_subjects(&candidate, &root_value)?;
    let objects: Map = candidate
        .objects
        .iter()
        .filter(|(_, o)| subjects.contains(text(&map(o).unwrap()["subject"]).unwrap()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let raw = candidate
        .object_bytes
        .iter()
        .filter(|((subject, _), _)| subjects.contains(subject))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let state =
        history_reduce::reduce_bytes(&raw, Some(map(&map(&candidate.state)?["rules"])?), None)?;
    for subject in &subjects {
        require(
            map(&map(&state)?["subjects"])?
                .get(subject)
                .unwrap()
                .digest()?
                == map(&map(&candidate.state)?["subjects"])?
                    .get(subject)
                    .unwrap()
                    .digest()?,
            "subset_reduction_mismatch",
        )?;
    }
    let projection = history_adapter::from_store_capture(source)?
        .projection()
        .clone();
    let subject_data: Map = subjects
        .iter()
        .map(|subject| {
            let prepared_subject = objects.iter().any(|(id, o)| {
                text(&map(o).unwrap()["subject"]).unwrap() == subject
                    && !source.objects.contains_key(id)
            });
            Ok((
                subject.clone(),
                obj([
                    (
                        "source_state",
                        s(if prepared_subject {
                            "prepared_candidate"
                        } else {
                            "committed"
                        }),
                    ),
                    (
                        "objects_digest",
                        s(&subject_inventory(
                            &Capture {
                                objects: objects.clone(),
                                object_bytes: raw.clone(),
                                ..candidate.clone()
                            },
                            subject,
                        )
                        .digest()?),
                    ),
                    (
                        "reduction_digest",
                        s(&map(&map(&state)?["subjects"])?
                            .get(subject)
                            .unwrap()
                            .digest()?),
                    ),
                ]),
            ))
        })
        .collect::<Result<_>>()?;
    let origin = obj([
        ("version", n("1")),
        ("kind", s("selected-subject-observation")),
        ("operation", s(operation)),
        ("recorded_at", s(recorded_at)),
        ("source_authority", source.marker.clone()),
        (
            "source_capture_digest",
            map(&projection)?["closure_digest"].clone(),
        ),
        ("source_entry", s(source_entry)),
        ("roots", root_value.clone()),
        ("disclosed_locators", disclosed_locators.clone()),
        ("prepared_digest", s(&sha256(&prepared.to_bytes()?))),
        ("subjects", V::Map(subject_data)),
    ]);
    crate::history_projection::validate_subset_origin(&origin)?;
    let marker = A::authority(
        &format!(
            "subset-{}",
            obj([
                ("source", source.marker.clone()),
                ("operation", s(operation))
            ])
            .digest()?
        ),
        "history",
        &n("1"),
        Map::new(),
    )?;
    let adapted = history_adapter::from_store_capture(&candidate)?;
    let mut template = obj([
        (
            "schema",
            map(adapted.document())?
                .get("schema")
                .cloned()
                .unwrap_or_else(|| V::Map(Map::new())),
        ),
        ("meta", obj([("history_subset", origin.clone())])),
    ]);
    if let Some(reasoning) = map(adapted.document())?
        .get("meta")
        .and_then(|m| map(m).ok())
        .and_then(|m| m.get("reasoning"))
    {
        map_mut(map_mut(&mut template)?.get_mut("meta").unwrap())?
            .insert("reasoning".into(), reasoning.clone());
    }
    for object in objects.values() {
        let m = map(object)?;
        if !string_is(&m["kind"], "act") {
            map_mut(&mut template)?
                .entry(text(&map(&m["authored"])?["collection"])?.into())
                .or_insert_with(|| V::Map(Map::new()));
        }
    }
    let mut plain = template.clone();
    map_mut(map_mut(&mut plain)?.get_mut("meta").unwrap())?.remove("history_subset");
    let cap = C::document_capabilities(&plain)?;
    let receipt = T::semantic_receipt(
        text(&map(&cap)?["profile"])?,
        &cap,
        &V::Map(Map::new()),
        &obj([("history_subset", origin)]),
    )?;
    let pairs = objects
        .iter()
        .map(|(id, o)| {
            let subject = text(&map(o).unwrap()["subject"]).unwrap();
            (o.clone(), raw[&(subject.into(), id.clone())].clone())
        })
        .collect::<Vec<_>>();
    let before = H::baseline(
        &marker,
        &A::Files::new(),
        &history_reduce::reduce_bytes(&BTreeMap::new(), None, None)?,
    )?;
    let draft = Prep::make_commit(
        &marker,
        operation,
        &Map::new(),
        &before,
        &pairs,
        &receipt,
        b"",
        Some(&template),
        Some(&V::List(vec![
            s("explicit-root-disposition/v1"),
            s(P::CAPABILITY),
        ])),
    )?;
    let mut commits = A::Files::from([(operation.into(), E::encode_document(&draft)?)]);
    let mut detached = Capture {
        root: std::path::PathBuf::new(),
        layout: H::Layout::for_entry("entry.yaml")?,
        entry_bytes: vec![],
        document: template.clone(),
        view_alternatives: vec![],
        authority_bytes: E::encode_document(&marker)?,
        marker: marker.clone(),
        commits: commits.clone(),
        object_bytes: raw.clone(),
        objects: objects.clone(),
        state: state.clone(),
        baseline: V::Null,
        inactive_generations: BTreeMap::new(),
        cancellation_bytes: A::Files::new(),
        storage_bytes: raw
            .iter()
            .map(|((s, id), b)| (P::object_path(s, id, P::Scheme::Hashed).unwrap(), b.clone()))
            .collect(),
        object_paths: raw
            .keys()
            .map(|(s, id)| {
                (
                    (s.clone(), id.clone()),
                    P::object_path(s, id, P::Scheme::Hashed).unwrap(),
                )
            })
            .collect(),
        inventory: BTreeMap::new(),
    };
    detached.baseline = H::baseline(&marker, &commits, &state)?;
    let rendered = history_view::render(&detached, &objects, &raw, &commits)?;
    let manifest_commit = Prep::make_commit(
        &marker,
        operation,
        &Map::new(),
        &before,
        &pairs,
        &receipt,
        &rendered,
        Some(&template),
        Some(&V::List(vec![
            s("explicit-root-disposition/v1"),
            s(P::CAPABILITY),
        ])),
    )?;
    commits.insert(operation.into(), E::encode_document(&manifest_commit)?);
    let mut files = A::Files::from([
        ("authority.yaml".into(), E::encode_document(&marker)?),
        ("entry.yaml".into(), rendered),
        (
            format!("commits/{operation}.yaml"),
            commits[operation].clone(),
        ),
    ]);
    for ((subject, id), bytes) in raw {
        files.insert(
            format!(
                "objects/{}",
                P::object_path(&subject, &id, P::Scheme::Hashed)?
            ),
            bytes,
        );
    }
    let binding = obj([
        ("version", n("2")),
        (
            "requires",
            V::List(vec![
                s("history-closure/v1"),
                s("history-subset/v1"),
                s(P::CAPABILITY),
            ]),
        ),
        ("roots", root_value),
        ("scope", scope.clone()),
        ("shareability", s("project")),
        ("rules", map(&state)?["rules"].clone()),
        ("baseline", detached.baseline),
        (
            "files",
            V::Map(
                files
                    .iter()
                    .map(|(p, b)| (p.clone(), s(&sha256(b))))
                    .collect(),
            ),
        ),
    ]);
    let artifact = obj([("revision", s(&binding.digest()?)), ("manifest", binding)]);
    crate::history_bundle::validate_artifact(
        field(map(&artifact)?, "manifest")?,
        field(map(&artifact)?, "revision")?,
        &files,
    )?;
    Ok((artifact, files))
}

pub(crate) fn wrap(
    artifact: &V,
    history_files: &A::Files,
    evidence: &A::Files,
) -> Result<(V, A::Files)> {
    let captured = crate::history_bundle::validate_artifact(
        field(map(artifact)?, "manifest")?,
        field(map(artifact)?, "revision")?,
        history_files,
    )?;
    let adapted = history_adapter::from_store_capture(&captured)?;
    let document = adapted.document().clone();
    let reasoning = C::history_document_capabilities(&document)?;
    let artifact_fields = map(artifact)?;
    let artifact_manifest = map(field(artifact_fields, "manifest")?)?;
    let mut files = evidence.clone();
    require(
        !files.keys().any(|p| p.starts_with("history-closure/")),
        "evidence uses reserved history closure namespace",
    )?;
    for (path, raw) in history_files {
        files.insert(format!("history-closure/{path}"), raw.clone());
    }
    let manifest = obj([
        ("version", n("3")),
        ("requires", artifact_manifest["requires"].clone()),
        ("roots", artifact_manifest["roots"].clone()),
        ("document", document),
        ("scope", artifact_manifest["scope"].clone()),
        ("reasoning", reasoning),
        (
            "history",
            obj([
                ("revision", artifact_fields["revision"].clone()),
                ("manifest", artifact_fields["manifest"].clone()),
            ]),
        ),
        (
            "evidence",
            V::Map(
                files
                    .iter()
                    .map(|(p, b)| (p.clone(), s(&sha256(b))))
                    .collect(),
            ),
        ),
    ]);
    let bundle = obj([("revision", s(&manifest.digest()?)), ("manifest", manifest)]);
    crate::pending_bundle::validate(&bundle, &files)?;
    Ok((bundle, files))
}
