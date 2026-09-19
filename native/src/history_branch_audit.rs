//! Exact retained adoption capsules. This layer validates evidence, never mutates
//! the target or treats a caller's verification callback as a semantic proof.
use crate::{
    Result, history_adapter as A,
    history_authoring::{obj, s, strings},
    history_branch::{self as B, Observation},
    history_capture::Capture,
    history_contract::*,
    history_transaction as T,
    history_view::list,
    identity::sha256,
    pending_bundle, require,
    value::TypedValue as V,
};
fn child<'a>(value: &'a V, keys: &[&str]) -> Result<&'a V> {
    keys.iter().try_fold(value, |v, k| field(map(v)?, k))
}
fn hash(v: &V) -> Result<&str> {
    let v = text(v)?;
    require(
        v.len() == 64 && crate::history_paths::object_id(v),
        "invalid_branch_adoption_audit",
    )?;
    Ok(v)
}
pub(crate) fn audit_path(entry: &str, revision: &str) -> Result<String> {
    crate::history_capture::Layout::for_entry(entry)?;
    Ok(format!(
        "{}evidence/branches/{revision}.json",
        if entry == "GROUNDING.yaml" {
            ".kpopper/"
        } else {
            ""
        }
    ))
}
pub(crate) fn binding(observation: &Observation, source: &Capture) -> Result<V> {
    let e = map(&observation.envelope)?;
    let m = map(&e["manifest"])?;
    let mut result = Map::from([
        ("kind".into(), m["kind"].clone()),
        ("revision".into(), e["revision"].clone()),
        ("git".into(), m["source"].clone()),
        ("authority".into(), source.marker.clone()),
        ("baseline".into(), source.baseline.clone()),
        (
            "closure_digest".into(),
            map(A::from_store_capture(source)?.projection())?["closure_digest"].clone(),
        ),
        (
            "snapshot_id".into(),
            map(&m["snapshot"])?["snapshot_id"].clone(),
        ),
        ("files".into(), m["files"].clone()),
    ]);
    if let Some(coverage) = m.get("audit_coverage") {
        result.insert("audit_coverage".into(), coverage.clone());
    }
    Ok(V::Map(result))
}
pub(crate) fn evidence_requirements(observation: &Observation, source: &Capture) -> Result<V> {
    let e = map(&observation.envelope)?;
    let m = map(&e["manifest"])?;
    let source_info = map(&m["source"])?;
    let entry = text(&source_info["entry"])?;
    let mut documents = source.objects.values().collect::<Vec<_>>();
    for item in map(&map(&m["snapshot"])?["hypotheses"])?.values() {
        documents.push(field(map(item)?, "document")?);
    }
    let mut result = Map::new();
    for doc in documents {
        for relative in pending_bundle::required_files(doc)? {
            let path = B::joined(entry, &relative)?;
            let raw = observation
                .files
                .get(&path)
                .ok_or_else(|| error("branch_evidence_unavailable"))?;
            result.insert(
                relative.clone(),
                obj([
                    ("source_commit", source_info["commit"].clone()),
                    ("source_path", s(&path)),
                    ("target_relative", s(&relative)),
                    ("sha256", s(&sha256(raw))),
                ]),
            );
        }
    }
    Ok(V::Map(result))
}
pub(crate) fn source_set(
    observations: &[Observation],
) -> Result<(Vec<(&Observation, Capture)>, String)> {
    require(
        (2..=16).contains(&observations.len()),
        "invalid_branch_source_set",
    )?;
    let mut total = 0usize;
    for observation in observations {
        require(
            observation.files.len() <= B::MAX_FILES,
            "invalid_branch_source_set",
        )?;
        for raw in observation.files.values() {
            total = total.saturating_add(raw.len());
            require(total <= B::MAX_BYTES, "branch_capture_limit")?;
        }
    }
    let mut ordered = observations
        .iter()
        .map(|o| Ok((o, B::validate(&o.envelope, &o.files)?)))
        .collect::<Result<Vec<_>>>()?;
    ordered.sort_by(|(a, _), (b, _)| {
        text(&map(&a.envelope).unwrap()["revision"])
            .unwrap()
            .cmp(text(&map(&b.envelope).unwrap()["revision"]).unwrap())
    });
    let revisions = ordered
        .iter()
        .map(|(o, _)| text(&map(&o.envelope)?["revision"]).map(str::to_owned))
        .collect::<Result<Vec<_>>>()?;
    require(
        revisions.windows(2).all(|w| w[0] < w[1]),
        "duplicate_branch_source",
    )?;
    let revision = obj([
        ("kind", s("committed-branch-history-set/v1")),
        ("revisions", strings(revisions)),
    ])
    .digest()?;
    Ok((ordered, revision))
}
fn single(
    entry: &str,
    revision: &V,
    binding: &V,
    files: &[V],
    evidence: &V,
) -> Result<Observation> {
    let revision = hash(revision)?;
    let b = map(binding)?;
    let audit = schema(field(b, "audit")?, &["path", "sha256", "revision"], &[])?;
    let expected = audit_path(entry, revision)?;
    require(
        string_is(&audit["path"], &expected) && string_is(&audit["revision"], revision),
        "branch_adoption_audit_mismatch",
    )?;
    require(files.len() == 1, "branch_adoption_audit_mismatch")?;
    let item = map(&files[0])?;
    require(
        field(item, "path")? == &s(&expected) && field(item, "before")? == &V::Null,
        "branch_adoption_audit_mismatch",
    )?;
    let raw =
        T::unblob(field(item, "after")?)?.ok_or_else(|| error("branch_adoption_audit_mismatch"))?;
    require(
        string_is(&audit["sha256"], &sha256(&raw))
            && *evidence == V::Map(Map::from([(expected, audit["sha256"].clone())])),
        "branch_adoption_audit_mismatch",
    )?;
    let observation = Observation::from_bytes(&raw)?;
    let source = B::validate(&observation.envelope, &observation.files)?;
    require(
        string_is(&map(&observation.envelope)?["revision"], revision),
        "branch_adoption_audit_mismatch",
    )?;
    for (key, value) in map(&self::binding(&observation, &source)?)? {
        require(
            b.get(key).is_some_and(|v| v == value),
            "branch_adoption_audit_mismatch",
        )?;
    }
    require(
        b.get("required_evidence") == Some(&evidence_requirements(&observation, &source)?),
        "branch_adoption_audit_mismatch",
    )?;
    Ok(observation)
}
/// Accepts the mutation's typed data so construction does not recurse through
/// PreparedMutation. All file images use the canonical checksummed blob format.
pub fn audit_evidences(data: &V) -> Result<Vec<Observation>> {
    crate::history_yaml::validate_value(data, T::MAX_TRANSACTION_BYTES)?;
    let entry = text(child(data, &["entry"])?)?;
    let adoption = map(child(
        data,
        &["receipt", "after", "history_branch_adoption"],
    )?)?;
    let evidence = child(data, &["receipt", "before", "authoring", "evidence"])?;
    let files = list(child(data, &["files"])?)?
        .iter()
        .filter_map(|f| match map(f) {
            Ok(m)
                if m.get("role")
                    .is_some_and(|v| string_is(v, "history_evidence")) =>
            {
                Some(Ok(f.clone()))
            }
            Ok(_) => None,
            Err(e) => Some(Err(e)),
        })
        .collect::<Result<Vec<_>>>()?;
    if adoption.get("version").is_some_and(|v| is_int(v, "1")) {
        return Ok(vec![single(
            entry,
            field(adoption, "source_revision")?,
            field(adoption, "source")?,
            &files,
            evidence,
        )?]);
    }
    require(
        adoption.get("version").is_some_and(|v| is_int(v, "2")),
        "invalid_branch_adoption_audit",
    )?;
    let bindings = list(field(adoption, "sources")?)?;
    let revisions = list(field(adoption, "source_revisions")?)?;
    require(
        (2..=16).contains(&bindings.len()) && bindings.len() == revisions.len(),
        "invalid_branch_source_set",
    )?;
    let revisions = revisions.iter().map(hash).collect::<Result<Vec<_>>>()?;
    require(
        revisions.windows(2).all(|w| w[0] < w[1]),
        "invalid_branch_source_set",
    )?;
    let mut capsule_total = 0usize;
    for item in &files {
        let encoded = text(child(item, &["after", "data"])?)?;
        capsule_total = capsule_total.saturating_add(encoded.len().saturating_mul(3) / 4);
        require(capsule_total <= B::MAX_BYTES, "branch_capture_limit")?;
    }
    let mut expected = Map::new();
    for binding in bindings {
        let audit = map(child(binding, &["audit"])?)?;
        let path = text(field(audit, "path")?)?;
        require(
            !expected.contains_key(path),
            "branch_adoption_audit_mismatch",
        )?;
        expected.insert(path.into(), field(audit, "sha256")?.clone());
    }
    require(
        files.len() == bindings.len() && *evidence == V::Map(expected),
        "branch_adoption_audit_mismatch",
    )?;
    let mut result = vec![];
    for (revision, binding) in revisions.iter().zip(bindings) {
        let audit = map(child(binding, &["audit"])?)?;
        let path = field(audit, "path")?;
        let selected = files
            .iter()
            .filter(|v| child(v, &["path"]).ok() == Some(path))
            .cloned()
            .collect::<Vec<_>>();
        result.push(single(
            entry,
            &s(revision),
            binding,
            &selected,
            &V::Map(Map::from([(text(path)?.into(), audit["sha256"].clone())])),
        )?);
    }
    let (ordered, revision) = source_set(&result)?;
    require(
        field(adoption, "source_set_revision")? == &s(&revision)
            && ordered
                .iter()
                .zip(&result)
                .all(|((a, _), b)| a.envelope == b.envelope && a.files == b.files),
        "branch_adoption_audit_mismatch",
    )?;
    Ok(result)
}
