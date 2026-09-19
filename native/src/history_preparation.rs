//! Exact transaction images after an application has prepared claims and receipts.
//! This layer allocates no intent, time, acceptance act or semantic evidence.
use crate::{
    Result, history_authority as A,
    history_capture::Capture,
    history_contract::*,
    history_emit as E, history_paths as P,
    history_transaction::{FileImage, PreparedMutation},
    history_view,
    identity::sha256,
    require,
    value::{Integer, TypedValue as V},
};
fn s(v: &str) -> V {
    V::Text(v.into())
}
#[allow(clippy::too_many_arguments)]
pub fn make_commit(
    marker: &V,
    operation: &str,
    parents: &Map,
    baseline: &V,
    objects: &[(V, Vec<u8>)],
    receipt: &V,
    view: &[u8],
    template: Option<&V>,
    requires: Option<&V>,
) -> Result<V> {
    A::bind_authority(marker, baseline)?;
    let mut inventory = Vec::new();
    for (obj, bytes) in objects {
        validate_object(obj)?;
        let decoded = crate::history_yaml::decode_document(bytes)?;
        validate_object(&decoded)?;
        require(decoded.digest()? == obj.digest()?, "object_bytes_mismatch")?;
        let m = map(obj)?;
        inventory.push(V::Map(Map::from([
            ("subject".into(), m["subject"].clone()),
            ("id".into(), m["id"].clone()),
            ("sha256".into(), s(&sha256(bytes))),
        ])));
    }
    inventory.sort_by(|a, b| {
        text(&map(a).unwrap()["id"])
            .unwrap()
            .cmp(text(&map(b).unwrap()["id"]).unwrap())
    });
    let marker_map = map(marker)?;
    let mut m = Map::from([
        ("version".into(), V::Integer(Integer::new("1")?)),
        ("record_id".into(), marker_map["record_id"].clone()),
        (
            "authority_generation".into(),
            marker_map["generation"].clone(),
        ),
        ("operation".into(), s(operation)),
        ("parents".into(), V::Map(parents.clone())),
        ("baseline_digest".into(), s(&baseline.digest()?)),
        ("objects".into(), V::List(inventory)),
        ("receipt".into(), receipt.clone()),
        ("view_sha256".into(), s(&sha256(view))),
    ]);
    if is_int(&marker_map["version"], "2") {
        m.insert("authority_digest".into(), s(&marker.digest()?));
    }
    if let Some(t) = template {
        m.insert("view_template".into(), t.clone());
    }
    if let Some(r) = requires {
        m.insert("requires".into(), r.clone());
    }
    let value = V::Map(m);
    A::validate_commit(&value)?;
    Ok(value)
}
/// Build the common render/manifest envelope from already normalized authored objects.
/// The application must independently replay its intent when committing this envelope.
/// `requires` preserves absence versus an empty legacy capability list on replay.
pub fn prepare_commit(
    capture: &Capture,
    operation: &str,
    new_objects: &[V],
    template: &V,
    receipt: &V,
    requires: Option<&V>,
) -> Result<PreparedMutation> {
    prepare_commit_with_files(
        capture,
        operation,
        new_objects,
        template,
        receipt,
        requires,
        &[],
    )
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_commit_with_files(
    capture: &Capture,
    operation: &str,
    new_objects: &[V],
    template: &V,
    receipt: &V,
    requires: Option<&V>,
    extra_files: &[FileImage],
) -> Result<PreparedMutation> {
    require(
        !capture.commits.contains_key(operation),
        "operation_already_prepared",
    )?;
    let pairs = new_objects
        .iter()
        .map(|o| E::encode_document(o).map(|raw| (o.clone(), raw)))
        .collect::<Result<Vec<_>>>()?;
    prepare_commit_pairs(
        capture,
        operation,
        pairs,
        template,
        receipt,
        requires,
        extra_files,
    )
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_commit_pairs(
    capture: &Capture,
    operation: &str,
    pairs: Vec<(V, Vec<u8>)>,
    template: &V,
    receipt: &V,
    requires: Option<&V>,
    extra_files: &[FileImage],
) -> Result<PreparedMutation> {
    require(
        !capture.commits.contains_key(operation),
        "operation_already_prepared",
    )?;
    let parents = A::commit_frontier(&capture.commits)?;
    let draft = make_commit(
        &capture.marker,
        operation,
        &parents,
        &capture.baseline,
        &pairs,
        receipt,
        b"",
        Some(template),
        requires,
    )?;
    let mut combined = capture.commits.clone();
    combined.insert(operation.into(), E::encode_document(&draft)?);
    let mut objects = capture.objects.clone();
    let mut raw = capture.object_bytes.clone();
    for (o, bytes) in &pairs {
        let m = map(o)?;
        let id = text(&m["id"])?;
        let subject = text(&m["subject"])?;
        require(
            objects.get(id).is_none_or(|prior| prior == o),
            "immutable_collision",
        )?;
        objects.insert(id.into(), o.clone());
        require(
            raw.get(&(subject.into(), id.into()))
                .is_none_or(|prior| prior == bytes),
            "immutable_collision",
        )?;
        raw.insert((subject.into(), id.into()), bytes.clone());
    }
    let rendered = history_view::render(capture, &objects, &raw, &combined)?;
    let manifest = make_commit(
        &capture.marker,
        operation,
        &parents,
        &capture.baseline,
        &pairs,
        receipt,
        &rendered,
        Some(template),
        requires,
    )?;
    let layout = crate::history_transaction::Layout::for_entry(&capture.layout.entry)?;
    let mut files = vec![FileImage {
        path: layout.entry.clone(),
        role: "record".into(),
        before: Some(capture.entry_bytes.clone()),
        after: Some(rendered),
    }];
    let hashed = requires
        .is_some_and(|r| matches!(r,V::List(a) if a.iter().any(|v|string_is(v,P::CAPABILITY))));
    for (o, bytes) in pairs {
        let m = map(&o)?;
        let subject = text(&m["subject"])?;
        let id = text(&m["id"])?;
        let path = if let Some(path) = capture.object_paths.get(&(subject.into(), id.into())) {
            P::validate_object_path(path, subject, id)?;
            path.clone()
        } else {
            P::object_path(
                subject,
                id,
                if hashed {
                    P::Scheme::Hashed
                } else {
                    P::Scheme::Legacy
                },
            )?
        };
        files.push(FileImage {
            path: format!("{}/{path}", layout.objects),
            role: "history_object".into(),
            before: None,
            after: Some(bytes),
        });
    }
    files.push(FileImage {
        path: format!("{}/{operation}.yaml", layout.commits),
        role: "history_commit".into(),
        before: None,
        after: Some(E::encode_document(&manifest)?),
    });
    files.extend_from_slice(extra_files);
    PreparedMutation::prepare(
        operation,
        &capture.marker,
        &capture.baseline,
        files,
        receipt,
        &layout.entry,
        None,
    )
}
