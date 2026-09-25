//! Complete v3 compatibility through checked conversion and compact branch union.
//! Existing source bytes are retained exactly; imported objects keep their original
//! storage operations, so independent imports can later merge without duplicate owners.
use crate::{
    Result,
    history_authoring::{obj, s},
    history_authority::Files,
    history_contract::*,
    history_node_capture::Capture,
    history_node_publication as P, history_node_writer as W, history_yaml as Y,
    identity::sha256,
    require,
    value::TypedValue as V,
};
use std::{collections::BTreeMap, path::Path};
const CLOSURE: &str = "history-closure/";
/// Archived artifact space for contribution bytes that must not be active files.
const ARCHIVE: &str = ".kpopper-contributions/";

/// Record storage, configuration, physical layers and Git control files. Locator
/// evidence at such a path is retained in archive space and never populates it.
/// Compared case-insensitively: a case-folding checkout resolves both spellings.
pub(crate) fn reserved_evidence(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let first = lower.split('/').next().unwrap_or_default();
    first.starts_with(".kpopper")
        || ["grounding.yaml", "provenance.yaml", "provenance.d"].contains(&first)
        || lower
            .split('/')
            .any(|part| [".git", ".gitattributes", ".gitignore", ".gitmodules"].contains(&part))
}
/// Original members the verified legacy entry binds by exact path and digest.
fn bound_members(source: &crate::history_capture::Capture) -> Result<BTreeMap<String, String>> {
    let adapted = crate::history_adapter::from_store_capture(source)?;
    let Some(imported) = map(adapted.document())?
        .get("meta")
        .and_then(|meta| map(meta).ok())
        .and_then(|meta| meta.get("history_import"))
        .filter(|imported| **imported != V::Null)
    else {
        return Ok(BTreeMap::new());
    };
    crate::history_view::list(field(map(imported)?, "members")?)?
        .iter()
        .map(|item| {
            let item = map(item)?;
            Ok((
                text(field(item, "path")?)?.to_owned(),
                text(field(item, "sha256")?)?.to_owned(),
            ))
        })
        .collect()
}
/// Bound legacy migration auxiliaries may be needed to reconstruct original
/// receipts. Core storage is supplied exclusively by the artifact inventory;
/// a locator or metadata member can never add another commit/object or Git config.
fn bound_auxiliary(path: &str) -> Result<bool> {
    for name in ["GROUNDING.yaml", "PROVENANCE.yaml"] {
        let layout = crate::history_transaction::Layout::for_entry(name)?;
        if path == layout.view || path == layout.replaced
            || path.starts_with(&format!("{}/", layout.retained))
            || path.starts_with(&format!("{}/", layout.hypotheses))
        {
            return Ok(true);
        }
    }
    Ok(false)
}
#[derive(Debug)]
struct Retained {
    revision: String,
    marker: V,
    rules: V,
    document: V,
    commits: Files,
    objects: Map,
    orders: BTreeMap<String, V>,
    evidence: Map,
    generations: BTreeMap<String, (String, Option<V>)>,
}
pub(crate) fn is_complete(bundle: &V) -> Result<bool> {
    let m = map(field(map(bundle)?, "manifest")?)?;
    if !is_int(field(m, "version")?, "3") {
        return Ok(false);
    }
    let artifact = map(field(map(field(m, "history")?)?, "manifest")?)?;
    let version = field(artifact, "version")?;
    Ok(is_int(version, "1") || is_int(version, "3"))
}
fn shape(bundle: &V) -> Result<()> {
    let m = map(field(map(bundle)?, "manifest")?)?;
    require(
        is_int(field(m, "version")?, "3"),
        "complete_union_requires_v3_history",
    )?;
    let version = field(
        map(field(map(field(m, "history")?)?, "manifest")?)?,
        "version",
    )?;
    require(
        !is_int(version, "2"),
        "complete_union_requires_full_history: scoped history requires explicit adoption choices",
    )?;
    require(
        is_int(version, "1") || is_int(version, "3"),
        "unsupported_history_bundle",
    )
}

impl Retained {
    pub(crate) fn from_bundle(bundle: &V, files: &Files) -> Result<Self> {
        shape(bundle)?;
        // Complete replay of the v3 manifest, artifact, generations, privacy, every file
        // hash and the incoming temporal capture.
        let captured = crate::pending_bundle::contribution_history(bundle, files)?;
        require(
            !map(field(map(&captured.document)?, "meta")?)?.contains_key("history_subset"),
            "complete_union_requires_full_history",
        )?;
        let mut orders = BTreeMap::new();
        for (id, object) in &captured.objects {
            let subject = text(field(map(object)?, "subject")?)?;
            let raw = captured
                .object_bytes
                .get(&(subject.to_owned(), id.clone()))
                .ok_or_else(|| error("complete_union_requires_full_history"))?;
            orders.insert(
                id.clone(),
                crate::history_node_source_order::encode(
                    &crate::history_node_source_order::without_saw(Y::decode_source_document(
                        raw,
                    )?)?,
                ),
            );
        }
        let manifest = map(field(map(bundle)?, "manifest")?)?;
        let document = field(manifest, "document")?.clone();
        require(!crate::history_node_contribution::has_domain_profile(&document)?,
            "unsupported_domain_profile")?;
        Ok(Self {
            revision: text(field(map(bundle)?, "revision")?)?.to_owned(),
            marker: captured.marker.clone(),
            rules: map(&captured.state)?["rules"].clone(),
            document,
            commits: captured.commits.clone(),
            objects: captured.objects.clone(),
            orders,
            evidence: map(field(manifest, "evidence")?)?.clone(),
            generations: captured
                .inactive_generations
                .iter()
                .map(|(g, held)| (g.clone(), (held.digest.clone(), held.cancellation.clone())))
                .collect(),
        })
    }
}
fn same_authority(authority: &V, marker: &V) -> Result<bool> {
    let (a, m) = (map(authority)?, map(marker)?);
    Ok(m.get("authority").is_some_and(|v| string_is(v, "history"))
        && a.get("authority").is_some_and(|v| string_is(v, "history"))
        && a.get("record_id").is_some()
        && a.get("record_id") == m.get("record_id")
        && a.get("generation").is_some()
        && a.get("generation") == m.get("generation"))
}
fn reasoning(document: &V) -> V {
    map(document)
        .ok()
        .and_then(|m| m.get("meta"))
        .and_then(|v| map(v).ok())
        .and_then(|m| m.get("reasoning"))
        .cloned()
        .unwrap_or(V::Null)
}
/// A migrated legacy commit held with exactly the incoming manifest bytes.
fn held_legacy(tx: &P::Transaction, raw: &[u8]) -> bool {
    tx.evidence.is_empty()
        && tx.context.as_ref().is_some_and(|c| {
            crate::history_node_legacy::kind(c, crate::history_node_legacy::FORMAT)
                && crate::history_node_writer::context(c)
                    .ok()
                    .and_then(|c| map(&c["options"]).ok())
                    .and_then(|o| o.get("original_manifest_sha256"))
                    .is_some_and(|h| string_is(h, &sha256(raw)))
        })
}
/// Every incoming inactive generation, with its cancellation, is the one the target archived.
fn generations_held(snapshot: &P::Snapshot, retained: &Retained) -> bool {
    retained
        .generations
        .iter()
        .all(|(generation, (digest, cancellation))| {
            snapshot.legacy.values().any(|legacy| {
                legacy
                    .captured
                    .inactive_generations
                    .get(generation)
                    .is_some_and(|held| {
                        held.digest == *digest && held.cancellation == *cancellation
                    })
            })
        })
}
/// Authority, rules, profile and retained generations as of the union.
fn contract(snapshot: &P::Snapshot, template: &V, rules: &V, retained: &Retained) -> Result<()> {
    require(
        same_authority(&snapshot.authority, &retained.marker)?,
        "complete_union_authority_mismatch",
    )?;
    require(
        rules.digest()? == retained.rules.digest()?,
        "complete_union_rules_mismatch",
    )?;
    require(!crate::history_node_contribution::has_domain_profile(template)?,
        "unsupported_domain_profile")?;
    let (target, incoming) = (reasoning(template), reasoning(&retained.document));
    require(
        target == V::Null || incoming == V::Null || target == incoming,
        "complete_union_profile_mismatch",
    )?;
    require(
        generations_held(snapshot, retained),
        "complete_union_generation_mismatch: retained generation evidence requires explicit target reconciliation",
    )
}

fn retained_by(target: &Capture, retained: &Retained) -> Result<bool> {
    if !same_authority(&target.snapshot.authority, &retained.marker)?
        || map(target.state())?["rules"].digest()? != retained.rules.digest()?
        || !generations_held(&target.snapshot, retained)
    {
        return Ok(false);
    }
    if crate::history_node_contribution::has_domain_profile(target.document())? {
        return Ok(false);
    }
    if !retained.commits.iter().all(|(op, raw)| {
        target
            .snapshot
            .transactions
            .get(op)
            .is_some_and(|tx| held_legacy(tx, raw))
    }) {
        return Ok(false);
    }
    for (id, object) in &retained.objects {
        if !target.history.objects().contains_key(id)
            || target.history.object(id)? != *object
            || target.history.source_orders().get(id).unwrap_or(&V::Null) != &retained.orders[id]
        {
            return Ok(false);
        }
    }
    Ok(true)
}
/// Full histories are accepted only through exact original legacy commit witnesses.
pub(crate) fn accepted(
    target: &Capture,
    bundle: &V,
    files: &Files,
    evidence: &Files,
) -> Result<bool> {
    let retained = Retained::from_bundle(bundle, files)?;
    Ok(retained_by(target, &retained)?
        && retained.evidence.iter().all(|(path, digest)| {
            path.starts_with(CLOSURE)
                || evidence
                    .get(path)
                    .is_some_and(|raw| *digest == s(&sha256(raw)))
        }))
}
/// Prepare a target-bound union using the established compact journal and replay verifier.
pub(crate) fn prepare(root: &Path, bundle: &V, files: &Files) -> Result<Option<P::Prepared>> {
    let retained = Retained::from_bundle(bundle, files)?;
    let target = Capture::read(root)?;
    require(!target.is_unborn(), "complete_union_target_unborn")?;
    contract(
        &target.snapshot,
        target.document(),
        &map(target.state())?["rules"],
        &retained,
    )?;
    if retained_by(&target, &retained)? {
        return Ok(None);
    }
    let operation = format!(
        "complete-union-{}",
        obj([
            ("source", s(&retained.revision)),
            ("target", s(target.revision())),
        ])
        .digest()?
    );
    let source = tempfile::tempdir()?;
    materialize_complete(source.path(), bundle, files)?;
    crate::history_node_branch::prepare(root, &[P::export(source.path())?], &operation).map(Some)
}
/// Publish one complete same-authority history. Original temporal observations and
/// source operations remain imported originals; the union creates no accept act.
pub(crate) fn union_complete(
    root: &Path,
    bundle: &V,
    files: &Files,
    guard: &mut dyn FnMut() -> Result<()>,
) -> Result<()> {
    let Some(prepared) = prepare(root, bundle, files)? else {
        return Ok(());
    };
    guard()?;
    let guard = std::cell::RefCell::new(guard);
    P::publish(
        root,
        &prepared,
        |p| {
            (guard.borrow_mut())()?;
            W::verify(root, p, None)?;
            (guard.borrow_mut())()
        },
        |_| {
            W::verify_sources(root, &prepared)?;
            (guard.borrow_mut())()
        },
    )?;
    require(
        retained_by(
            &Capture::read(root)?,
            &Retained::from_bundle(bundle, files)?,
        )?,
        "complete_union_not_retained",
    )
}

/// Populate a new private snapshot with a complete v3 history converted to compact storage.
/// The caller publishes this directory atomically. The temporary legacy tree contains only
/// exact, already-validated source bytes; no legacy event or author is synthesized.
pub(crate) fn materialize_complete(root: &Path, bundle: &V, files: &Files) -> Result<V> {
    let manifest = map(field(map(bundle)?, "manifest")?)?;
    require(
        is_int(field(manifest, "version")?, "3"),
        "complete_union_requires_v3_history",
    )?;
    let artifact = map(field(map(field(manifest, "history")?)?, "manifest")?)?;
    require(
        ["1", "3"]
            .iter()
            .any(|v| is_int(field(artifact, "version").unwrap_or(&V::Null), v)),
        "complete_union_requires_full_history",
    )?;
    let source = crate::pending_bundle::contribution_history(bundle, files)?;
    for path in [
        "GROUNDING.yaml",
        "PROVENANCE.yaml",
        ".kpopper/history.yaml",
        ".kpopper/history",
        ".kpopper/history-commits",
        ".kpopper/.history-node-publication.json",
    ] {
        require(
            root.join(path).symlink_metadata().is_err(),
            "complete_materialize_existing_record",
        )?;
    }
    let temporary = tempfile::tempdir()?;
    let legacy = temporary.path().join("captured");
    std::fs::create_dir(&legacy)?;
    let mut originals = Files::new();
    for name in map(field(artifact, "files")?)?.keys() {
        let path = if name == "entry.yaml" {
            "GROUNDING.yaml".to_owned()
        } else if name == "authority.yaml" {
            ".kpopper/history.yaml".into()
        } else if let Some(name) = name.strip_prefix("commits/") {
            format!(".kpopper/history-commits/{name}")
        } else if let Some(name) = name.strip_prefix("objects/") {
            format!(".kpopper/history/{name}")
        } else if let Some(name) = name.strip_prefix("cancellations/") {
            format!(".kpopper/history-cancellations/{name}")
        } else {
            return Err(error("invalid_history_bundle_path"));
        };
        let raw = files
            .get(&format!("{CLOSURE}{name}"))
            .ok_or_else(|| error("history_bundle_membership"))?;
        originals.insert(path, raw.clone());
    }
    let revision = text(field(map(bundle)?, "revision")?)?.to_owned();
    let bound = bound_members(&source)?;
    let mut external = Files::new();
    for (path, raw) in files.iter().filter(|(path, _)| !path.starts_with(CLOSURE)) {
        require(
            originals.get(path).is_none_or(|old| old == raw),
            "complete_materialize_evidence_collision",
        )?;
        // A legacy cancellation can list its commit receipt again as evidence.
        // That exact owned source file is retained inside the migration archive;
        // copying it back beside compact manifests would activate legacy storage.
        if originals.contains_key(path) {
            continue;
        }
        if reserved_evidence(path) {
            // Locator data never becomes history, configuration or a physical layer.
            // Only an original the verified entry binds by digest enters the source.
            if bound_auxiliary(path)? && bound.get(path).is_some_and(|digest| *digest == sha256(raw)) {
                originals.insert(path.clone(), raw.clone());
            }
            external.insert(format!("{ARCHIVE}{revision}/evidence/{path}"), raw.clone());
            continue;
        }
        external.insert(path.clone(), raw.clone());
        originals.insert(path.clone(), raw.clone());
    }
    for (path, raw) in &originals {
        crate::history_transaction_fs::publish_immutable(&legacy, path, raw)?;
    }
    let reconstructed = crate::history_store::Store::new(&legacy.join("GROUNDING.yaml"))?.capture()?;
    require(reconstructed.commits == source.commits
        && reconstructed.storage_bytes == source.storage_bytes
        && reconstructed.cancellation_bytes == source.cancellation_bytes,
        "complete_materialize_source_membership")?;
    let converted = temporary.path().join("compact");
    crate::history_node_migration::Plan::prepare(&legacy.join("GROUNDING.yaml"))?
        .publish(&converted)?;
    let mut output = crate::history_node_publication::export(&converted)?.files()?;
    for (path, raw) in external {
        require(
            output.get(&path).is_none_or(|old| old == &raw),
            "complete_materialize_evidence_collision",
        )?;
        output.insert(path, raw);
    }
    // Check every output collision before writing any record authority or stream.
    for (path, raw) in &output {
        let target = crate::history_transaction_fs::target(root, path)?;
        if target.symlink_metadata().is_ok() {
            require(
                target.is_file() && std::fs::read(&target)? == *raw,
                "complete_materialize_evidence_collision",
            )?;
        }
    }
    for (path, raw) in output {
        crate::history_transaction_fs::publish_immutable(root, &path, &raw)?;
    }
    let capture = Capture::read(root)?;
    for (id, object) in &source.objects {
        require(
            capture.object(text(field(map(object)?, "subject")?)?, id)? == *object,
            "complete_materialize_object_mismatch",
        )?;
    }
    require(capture.history.objects().keys().eq(source.objects.keys()),
        "complete_materialize_object_membership: converted history holds an object outside the verified artifact")?;
    require(
        same_authority(&capture.snapshot.authority, &source.marker)?,
        "complete_union_authority_mismatch",
    )?;
    Ok(capture.document().clone())
}

#[cfg(test)]
#[path = "history_node_complete_union_tests.rs"]
mod tests;
