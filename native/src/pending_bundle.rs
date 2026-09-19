//! Portable contribution identity and complete closure, independent of publication.
use crate::{
    Result,
    history_authoring::{empty, n, obj, s, strings},
    history_authority::{self as Authority, Files},
    history_bundle as H,
    history_contract::*,
    history_view::{list, map_mut, truth},
    history_yaml as Y,
    identity::sha256,
    reasoning_authoring as A, reasoning_capabilities as C, reasoning_fields as F,
    reasoning_language as L,
    reasoning_snapshot::{CaptureOptions, Snapshot, entries},
    require,
    value::TypedValue as V,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::LazyLock,
};
static ID: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+").unwrap());
static REF: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\{\{\s*([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+)\s*\}\}").unwrap()
});
fn names(value: &V) -> Result<Vec<String>> {
    list(value)?
        .iter()
        .map(|v| text(v).map(str::to_owned))
        .collect()
}
fn texts(value: &V) -> Vec<String> {
    let mut out = vec![];
    let mut todo = vec![value];
    while let Some(v) = todo.pop() {
        match v {
            V::Text(v) => out.push(v.clone()),
            V::List(v) => todo.extend(v),
            V::Map(v) => {
                out.extend(v.keys().cloned());
                todo.extend(v.values());
            }
            _ => {}
        }
    }
    out
}
fn mentioned(body: &Map) -> Vec<String> {
    let mut out = vec![];
    for value in body.values() {
        match value {
            V::Text(v) => out.extend(ID.find_iter(v).map(|m| m.as_str().into())),
            V::List(v) => {
                for value in v {
                    if let V::Text(v) = value {
                        out.extend(ID.find_iter(v).map(|m| m.as_str().into()));
                    }
                }
            }
            V::Map(m) => {
                let shape = m.keys().map(String::as_str).collect::<Vec<_>>();
                if [
                    vec!["ref"],
                    vec!["num"],
                    vec!["text"],
                    vec!["bool"],
                    vec!["args", "op"],
                    vec!["expr"],
                ]
                .contains(&shape)
                {
                    out.extend(L::legacy_references(value));
                } else {
                    for key in m.keys() {
                        out.extend(ID.find_iter(key).map(|m| m.as_str().into()));
                    }
                }
            }
            _ => {}
        }
    }
    out
}
fn legacy_closure(document: &V, roots: &[String]) -> Result<V> {
    let all = entries(document)?;
    require(
        roots.iter().all(|r| all.contains_key(r)),
        "invalid_contribution_roots",
    )?;
    let schema = map(document)?
        .get("schema")
        .filter(|v| truth(v))
        .cloned()
        .unwrap_or_else(empty);
    let deps = map(&schema)?
        .get("deps")
        .map(text)
        .transpose()?
        .unwrap_or("rests_on");
    let mut selected = BTreeSet::new();
    let mut todo = roots.to_vec();
    while let Some(id) = todo.pop() {
        if selected.contains(&id) || F::BUILTINS.contains(&id.as_str()) {
            continue;
        }
        let (_, body) = all
            .get(&id)
            .ok_or_else(|| error("missing_contribution_dependency"))?;
        selected.insert(id);
        if let V::Map(body) = body {
            if let Some(explicit) = body.get(deps).filter(|v| truth(v)) {
                todo.extend(
                    names(explicit).map_err(|_| error("invalid_contribution_dependencies"))?,
                );
            }
            if let Some(source) = body.get("from") {
                let sources = if matches!(source, V::List(_)) {
                    names(source)?
                } else {
                    vec![text(source)?.into()]
                };
                todo.extend(sources.into_iter().filter(|s| all.contains_key(s)));
            }
            todo.extend(mentioned(body).into_iter().filter(|s| all.contains_key(s)));
        }
        for value in texts(body) {
            todo.extend(REF.captures_iter(&value).map(|c| c[1].to_owned()));
        }
    }
    let mut out = if truth(&schema) {
        obj([("schema", schema)])
    } else {
        empty()
    };
    for id in selected {
        let (collection, body) = &all[&id];
        map_mut(
            map_mut(&mut out)?
                .entry(collection.clone())
                .or_insert_with(empty),
        )?
        .insert(id, body.clone());
    }
    Ok(out)
}
pub fn closure(document: &V, roots: &[String]) -> Result<V> {
    Y::validate_value(document, 16 * 1024 * 1024)?;
    let cap = C::document_capabilities(document)?;
    let mut selected = legacy_closure(document, roots)?;
    if string_is(&map(&cap)?["profile"], "core/v1") {
        let fields = F::snapshot_fields(document)?;
        let all = entries(document)?;
        let mut scopes = BTreeSet::new();
        let mut query_scopes = BTreeSet::new();
        let mut processed = BTreeSet::new();
        let mut snapshot = None;
        loop {
            let pending = entries(&selected)?
                .keys()
                .filter(|id| !processed.contains(*id))
                .cloned()
                .collect::<Vec<_>>();
            if pending.is_empty() {
                break;
            }
            for id in pending {
                processed.insert(id.clone());
                let V::Map(body) = &all[&id].1 else {
                    continue;
                };
                let mut dependencies = vec![];
                for field in ["rule", text(&fields["predicate"])?] {
                    if let Some(expression @ V::Map(_)) = body.get(field) {
                        if let Some(query) = A::query_expression(document, expression)? {
                            let scope = query["query"]["scope"]
                                .as_str()
                                .ok_or_else(|| error("invalid_query"))?
                                .to_owned();
                            query_scopes.insert(scope.clone());
                            dependencies.push(scope);
                        } else {
                            dependencies.extend(L::references(&L::lower(expression)?));
                        }
                    }
                }
                if body.contains_key("collection_scope") {
                    if snapshot.is_none() {
                        snapshot = Some(Snapshot::from_data(document, CaptureOptions::default())?);
                    }
                    let snapshot = snapshot.as_ref().unwrap();
                    let capture = if query_scopes.contains(&id) {
                        snapshot.capture_query_scope(&id, None)?
                    } else {
                        snapshot.capture_scope(&id, None)?
                    };
                    let collection = text(&map(capture.definition())?["collection"])?;
                    scopes.insert(collection.to_owned());
                    dependencies.extend(map(&map(document)?[collection])?.keys().cloned());
                }
                if !dependencies.is_empty() {
                    for (collection, members) in map(&legacy_closure(document, &dependencies)?)? {
                        if collection != "schema" {
                            map_mut(
                                map_mut(&mut selected)?
                                    .entry(collection.clone())
                                    .or_insert_with(empty),
                            )?
                            .extend(map(members)?.clone());
                        }
                    }
                }
            }
        }
        for collection in scopes {
            map_mut(&mut selected)?
                .entry(collection)
                .or_insert_with(empty);
        }
    }
    if let Some(reasoning) = map(document)?
        .get("meta")
        .and_then(|v| map(v).ok())
        .and_then(|m| m.get("reasoning"))
    {
        map_mut(&mut selected)?.insert("meta".into(), obj([("reasoning", reasoning.clone())]));
    }
    Ok(selected)
}
fn declared(document: &V) -> Result<V> {
    let Some(reasoning) = map(document)?
        .get("meta")
        .and_then(|v| map(v).ok())
        .and_then(|m| m.get("reasoning"))
    else {
        return Ok(obj([
            ("version", n("1")),
            ("profile", s("ordinary-reader/v1")),
            ("requires", strings(vec![])),
        ]));
    };
    let m = schema(reasoning, &["version", "profile", "requires"], &[])?;
    require(matches!(m["version"], V::Integer(_)), "invalid_capability")?;
    text(&m["profile"])?;
    let req = names(&m["requires"])?;
    require(req.windows(2).all(|w| w[0] < w[1]), "invalid_capability")?;
    Ok(reasoning.clone())
}
pub(crate) fn required_files(value: &V) -> Result<BTreeSet<String>> {
    let mut todo = vec![value];
    let mut out = BTreeSet::new();
    while let Some(v) = todo.pop() {
        match v {
            V::Map(m) => {
                if let Some(p) = m.get("file") {
                    let p = text(p)?;
                    Authority::relative_path(p)?;
                    out.insert(p.into());
                }
                todo.extend(m.values());
            }
            V::List(v) => todo.extend(v),
            _ => {}
        }
    }
    Ok(out)
}
fn prepare_version(
    document: &V,
    roots: &[String],
    scope: &V,
    files: &Files,
    version: u8,
) -> Result<V> {
    let scoped = map(scope)?;
    let kind = text(field(scoped, "kind")?)?;
    require(
        ["project", "external", "code"].contains(&kind),
        "invalid_contribution_scope",
    )?;
    require(
        !text(field(scoped, "environment")?)?.trim().is_empty(),
        "invalid_contribution_scope",
    )?;
    if kind == "code" {
        let commit = text(field(scoped, "commit")?)?;
        require(
            [40, 64].contains(&commit.len())
                && commit
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "invalid_contribution_scope",
        )?;
    }
    require(!roots.is_empty(), "invalid_contribution_roots")?;
    let meta = V::Map(
        map(document)?
            .iter()
            .filter(|(k, _)| {
                ["meta", "privacy", "visibility", "private", "shareability"].contains(&k.as_str())
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    );
    H::privacy(&meta)?;
    let document = if version == 2 {
        closure(document, roots)?
    } else {
        legacy_closure(document, roots)?
    };
    H::privacy(&document)?;
    H::privacy(scope)?;
    H::files_valid(files)?;
    require(
        required_files(&document)? == files.keys().cloned().collect(),
        "contribution_evidence_allowlist",
    )?;
    let all = entries(&document)?;
    for root in roots {
        let body = map(&all
            .get(root)
            .ok_or_else(|| error("invalid_contribution_roots"))?
            .1)?;
        require(
            body.get("scope").map(V::digest).transpose()?.as_ref() == Some(&scope.digest()?),
            "contribution_root_scope_mismatch",
        )?;
    }
    let mut manifest = obj([
        ("version", n(&version.to_string())),
        (
            "roots",
            strings(roots.iter().cloned().collect::<BTreeSet<_>>()),
        ),
        ("document", document.clone()),
        ("scope", scope.clone()),
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
    if version == 2 {
        map_mut(&mut manifest)?.insert("reasoning".into(), C::document_capabilities(&document)?);
    }
    Ok(obj([
        ("revision", s(&manifest.digest()?)),
        ("manifest", manifest),
    ]))
}
/// Evidence bytes are explicit; no source path is opened by preparation.
pub fn prepare(
    document: &V,
    roots: &[String],
    scope: &V,
    shareability: &str,
    files: &Files,
) -> Result<V> {
    require(
        shareability == "project",
        "private_contribution_requires_draft",
    )?;
    prepare_version(document, roots, scope, files, 2)
}
/// Validate v1 archival identity, complete v2 closures, or retained v3 history.
pub fn validate(bundle: &V, files: &Files) -> Result<V> {
    Y::validate_value(bundle, 16 * 1024 * 1024)?;
    let b = map(bundle)?;
    let manifest = field(b, "manifest")?;
    let m = map(manifest)?;
    let version = field(m, "version")?;
    require(
        ["1", "2", "3"].iter().any(|v| is_int(version, v))
            && *field(b, "revision")? == s(&manifest.digest()?),
        "invalid_contribution_identity",
    )?;
    if is_int(version, "3") {
        H::validate_contribution(bundle, files)?;
        return Ok(field(m, "reasoning")?.clone());
    }
    let document = field(m, "document")?;
    let cap = declared(document)?;
    if is_int(version, "2") {
        require(
            m.get("reasoning") == Some(&cap),
            "contribution_capability_mismatch",
        )?;
    }
    H::privacy(document)?;
    H::privacy(field(m, "scope")?)?;
    H::files_valid(files)?;
    let evidence = map(field(m, "evidence")?)?;
    require(
        evidence.len() == files.len()
            && files
                .iter()
                .all(|(p, b)| evidence.get(p) == Some(&s(&sha256(b)))),
        "contribution_evidence_mismatch",
    )?;
    let cap = C::document_capabilities(document)?;
    if is_int(version, "2") {
        let expected = prepare_version(
            document,
            &names(field(m, "roots")?)?,
            field(m, "scope")?,
            files,
            2,
        )?;
        require(
            map(&expected)?["revision"] == *field(b, "revision")?,
            "incomplete_contribution_closure",
        )?;
    }
    Ok(cap)
}
/// Reconcile the effective advanced-mode overlay while its policy lock is held.
/// Accepted revisions remain attached; only terminal decisions remove them.
pub fn compatible(
    destination: &V,
    bundles: &Map,
    files: &BTreeMap<String, Files>,
    decisions: &V,
    resume: &[String],
) -> Result<()> {
    let desired = C::document_capabilities(destination)?;
    let desired = map(&desired)?;
    let decisions = map(decisions)?;
    for (revision, bundle) in bundles {
        let state = decisions
            .get(revision)
            .and_then(|v| map(v).ok())
            .and_then(|m| m.get("state"));
        if !resume.contains(revision)
            && state.is_some_and(|v| {
                ["withdrawn", "rejected", "superseded"]
                    .iter()
                    .any(|s| string_is(v, s))
            })
        {
            continue;
        }
        require(
            map(bundle)?.get("revision") == Some(&s(revision)),
            "invalid_contribution_identity",
        )?;
        let empty_files = Files::new();
        let cap = validate(bundle, files.get(revision).unwrap_or(&empty_files))?;
        let cap = map(&cap)?;
        require(
            cap["profile"] == desired["profile"] && cap["requires"] == desired["requires"],
            &format!("pending_profile_reconciliation_required: {revision}"),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde_json::Value as J;
    fn corpus() -> J {
        serde_json::from_str(include_str!("../tests/fixtures/pending-bundle.json")).unwrap()
    }
    fn files(case: &J) -> Files {
        case["files"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(p, b)| (p.clone(), STANDARD.decode(b.as_str().unwrap()).unwrap()))
            .collect()
    }
    #[test]
    fn portable_contributions_match_complete_python_closures() {
        let mut failures = vec![];
        for case in corpus()["cases"].as_array().unwrap() {
            let doc = V::from_tagged(&case["document"]).unwrap();
            let scope = V::from_tagged(&case["scope"]).unwrap();
            let roots = case["roots"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_owned())
                .collect::<Vec<_>>();
            let files = files(case);
            if let Some(expected) = case.get("legacy") {
                let expected = V::from_tagged(expected).unwrap();
                assert_eq!(
                    prepare_version(&doc, &roots, &scope, &files, 1).unwrap(),
                    expected
                );
                validate(&expected, &files).unwrap();
            }
            match (
                case.get("output"),
                prepare(&doc, &roots, &scope, "project", &files),
            ) {
                (Some(expected), Ok(actual)) => {
                    if actual != V::from_tagged(expected).unwrap() {
                        failures.push(format!(
                            "{}: mismatch: {:?}",
                            case["name"],
                            actual.to_tagged()
                        ));
                    } else if let Err(e) = validate(&actual, &files) {
                        failures.push(format!("{}: replay refused {e}", case["name"]));
                    }
                }
                (None, Err(_)) => {}
                (Some(_), Err(e)) => failures.push(format!("{}: refused {e}", case["name"])),
                (None, Ok(_)) => failures.push(format!("{}: accepted", case["name"])),
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
    #[test]
    fn accepted_pending_profiles_remain_effective_until_terminal_decision() {
        let case = corpus()["cases"].as_array().unwrap()[0].clone();
        let bundle = V::from_tagged(&case["output"]).unwrap();
        let revision = text(&map(&bundle).unwrap()["revision"]).unwrap().to_owned();
        let bundles = Map::from([(revision.clone(), bundle)]);
        let files = BTreeMap::from([(revision.clone(), files(&case))]);
        let destination = A::declare_document(&V::from_tagged(&case["document"]).unwrap()).unwrap();
        for state in ["pending", "accepted"] {
            let decisions = V::Map(Map::from([(revision.clone(), obj([("state", s(state))]))]));
            assert!(
                compatible(&destination, &bundles, &files, &decisions, &[])
                    .unwrap_err()
                    .0
                    .starts_with("pending_profile_reconciliation_required")
            );
        }
        for state in ["withdrawn", "rejected", "superseded"] {
            let decisions = V::Map(Map::from([(revision.clone(), obj([("state", s(state))]))]));
            compatible(&destination, &bundles, &files, &decisions, &[]).unwrap();
            assert!(
                compatible(
                    &destination,
                    &bundles,
                    &files,
                    &decisions,
                    std::slice::from_ref(&revision)
                )
                .is_err()
            );
        }
    }
    #[test]
    fn rehashed_extra_closure_and_changed_evidence_refuse() {
        let case = corpus()["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["name"] == "file")
            .unwrap()
            .clone();
        let bundle = V::from_tagged(&case["output"]).unwrap();
        let mut files = files(&case);
        files.insert("evidence.txt".into(), b"changed".to_vec());
        assert_eq!(
            validate(&bundle, &files).unwrap_err().0,
            "contribution_evidence_mismatch"
        );
        let case = corpus()["cases"].as_array().unwrap()[0].clone();
        let mut bundle = V::from_tagged(&case["output"]).unwrap();
        let m = map_mut(map_mut(&mut bundle).unwrap().get_mut("manifest").unwrap()).unwrap();
        let doc = map_mut(m.get_mut("document").unwrap()).unwrap();
        map_mut(doc.get_mut("known").unwrap())
            .unwrap()
            .insert("p.extra".into(), obj([("v", n("42"))]));
        let revision = m.clone();
        map_mut(&mut bundle)
            .unwrap()
            .insert("revision".into(), s(&V::Map(revision).digest().unwrap()));
        assert_eq!(
            validate(&bundle, &Files::new()).unwrap_err().0,
            "incomplete_contribution_closure"
        );
    }
}
