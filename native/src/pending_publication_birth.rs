//! First canonical publication of a record the configured target does not hold yet.
//! The combined document the ordinary publisher validated is imported as compact
//! history through the established node import: its claims keep their profile, roles,
//! types, sources and scopes, with explicit unknown historical authorship. No plain
//! canonical record and no legacy history authority or event is written. Identity and
//! timing derive from the target revision and the included captures, so a managed
//! retry against the same target proposes identical bytes.
use super::Target;
use crate::{
    Error, Result,
    history_authority::Files,
    history_contract::{map, string_is},
    pending_state::Ledger,
    require,
    value::TypedValue as V,
};
use std::path::Path;

const ENTRY: &str = "GROUNDING.yaml";

/// The record directory relative to the repository, with a trailing separator.
fn prefix(record: &str) -> Result<String> {
    require(
        Path::new(record).file_name().is_some_and(|name| name == ENTRY),
        &format!(
            "compact_publication_entry_unsupported: {record}; a new compact record is born only as {ENTRY}. Configure {ENTRY} or create the record locally first"
        ),
    )?;
    Ok(record
        .rsplit_once('/')
        .map_or_else(String::new, |(dir, _)| format!("{dir}/")))
}

/// Existing storage for some record at the location the new record would own.
fn record_storage(relative: &str) -> Result<bool> {
    let lower = relative.to_ascii_lowercase();
    let within = |path: &str| {
        let path = path.to_ascii_lowercase();
        lower == path || lower.starts_with(&format!("{path}/"))
    };
    for name in ["GROUNDING.yaml", "PROVENANCE.yaml"] {
        let layout = crate::history_transaction::Layout::for_entry(name)?;
        for path in [layout.entry, layout.authority, layout.objects, layout.commits,
            layout.cancellations, layout.retained, layout.hypotheses, layout.replaced,
            layout.view, layout.journal]
        {
            if within(&path) { return Ok(true); }
        }
    }
    Ok(["PROVENANCE.d", ".kpopper/.history-node-publication.json",
        "evidence/legacy", "evidence/migration", "evidence/bootstrap", "evidence/reports",
        "evidence/view-edits", ".kpopper/evidence/domain"].iter().any(|path| within(path)))
}

/// Discovery stops at this private tree: an unusable gitfile ends Git discovery and
/// one simple project names `record` (relative to `root`), as import replay does.
/// Every tree the publisher reads or writes lives below such a root.
pub(super) fn boundary(root: &Path, record: &str) -> Result<()> {
    std::fs::write(
        root.join(".git"),
        b"gitdir: .kpopper-publication-no-repository\n",
    )?;
    let config = root.join(".kpopper/project.json");
    if config.symlink_metadata().is_err() {
        std::fs::create_dir_all(config.parent().unwrap())?;
        std::fs::write(
            &config,
            serde_json::to_vec(&serde_json::json!({
                "version": 1, "mode": "simple", "generation": 0,
                "record": record, "publication": null,
            }))?,
        )?;
    }
    Ok(())
}

/// The latest first capture time among the proposed revisions: when the newest
/// included knowledge entered the ledger. Never the publisher's clock.
pub(super) fn recorded_at(ledger: &Ledger, revisions: &[String]) -> Result<String> {
    let mut latest = None::<u128>;
    for revision in revisions {
        let first = ledger
            .events
            .iter()
            .filter_map(|event| map(event).ok())
            .filter(|event| {
                event
                    .get("revision")
                    .is_some_and(|value| string_is(value, revision))
            })
            .filter_map(|event| match event.get("captured_at_ns") {
                Some(V::Integer(value)) => value.as_str().parse::<u128>().ok(),
                _ => None,
            })
            .min()
            .ok_or_else(|| {
                Error(format!(
                    "compact_publication_capture_time_missing: {revision}"
                ))
            })?;
        latest = Some(latest.map_or(first, |held| held.max(first)));
    }
    let nanos = latest.ok_or_else(|| Error("compact_publication_without_revisions".into()))?;
    let seconds = i64::try_from(nanos / 1_000_000_000)
        .map_err(|_| Error("compact_publication_capture_time_invalid".into()))?;
    let time = chrono::DateTime::from_timestamp(seconds, (nanos % 1_000_000_000) as u32)
        .ok_or_else(|| Error("compact_publication_capture_time_invalid".into()))?;
    Ok(time.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true))
}

/// Repository files that create compact history at `record` from the validated combined
/// document `graph` and its evidence (repository paths, already collision-checked).
/// Evidence stays at its locator paths; it never populates record storage or control.
pub(super) fn files(
    target: &Target,
    record: &str,
    graph: &V,
    evidence: &Files,
    identity: &V,
    recorded_at: &str,
) -> Result<Files> {
    let prefix = prefix(record)?;
    for path in target.files.keys() {
        if let Some(relative) = path.strip_prefix(&prefix)
            && record_storage(relative)?
        {
            return Err(Error(format!(
                "compact_publication_existing_record_evidence: {path}; the target holds record storage where the new record would be born. Reconcile it before the first publication"
            )));
        }
    }
    for path in evidence.keys() {
        let relative = path
            .strip_prefix(&prefix)
            .ok_or_else(|| Error(format!("invalid evidence path: {path}")))?;
        require(
            !crate::history_node_complete_union::reserved_evidence(relative),
            &format!(
                "compact_publication_reserved_evidence: {path}; contribution evidence cannot populate record storage or control. Withdraw the revision or recapture it with ordinary evidence paths"
            ),
        )?;
    }
    let digest = identity.digest()?;
    // One bounded private tree: the plain source below `source/`, the copy below `copy/`.
    let private = tempfile::tempdir()?;
    boundary(private.path(), &format!("source/{record}"))?;
    let root = private.path().join("source");
    std::fs::create_dir(&root)?;
    let entry = crate::history_transaction_fs::target(&root, record)?;
    std::fs::create_dir_all(entry.parent().unwrap())?;
    std::fs::write(&entry, crate::history_emit::encode_document(graph)?)?;
    for (path, raw) in evidence {
        let destination = crate::history_transaction_fs::target(&root, path)?;
        std::fs::create_dir_all(destination.parent().unwrap())?;
        std::fs::write(destination, raw)?;
    }
    let plan = crate::history_node_import::Plan::prepare(
        Path::new(record),
        &root,
        crate::history_migration::Options {
            operation: format!("publication-{digest}"),
            recorded_at: recorded_at.into(),
            record_id: Some(format!("record-{digest}")),
            read_mode: crate::source_capture::ReadMode::Frozen,
            route: false,
            as_of: None,
        },
        None,
    )?;
    // Publication replays and verifies the complete copy, including its import archive.
    let parent = private.path().join("copy");
    std::fs::create_dir(&parent)?;
    let copy = parent.join("record");
    plan.publish(&copy)?;
    let mut created = Files::new();
    for (relative, raw) in crate::history_node_publication::export(&copy)?.files()? {
        if relative == ".gitattributes" {
            continue;
        }
        let path = format!("{prefix}{relative}");
        require(
            target.files.get(&path).is_none_or(|old| *old == raw)
                && evidence.get(&path).is_none_or(|old| *old == raw),
            &format!("compact_publication_collision: {path}"),
        )?;
        if target.files.get(&path) != Some(&raw) {
            created.insert(path, raw);
        }
    }
    // Exact bytes through Git: extend existing attributes as local compact birth does.
    let attributes = format!("{prefix}.gitattributes");
    let policy = crate::history_node_publication::GIT_ATTRIBUTES.as_bytes();
    let mut merged = target.files.get(&attributes).cloned().unwrap_or_default();
    if !merged.ends_with(policy) {
        if !merged.is_empty() && !merged.ends_with(b"\n") {
            merged.push(b'\n');
        }
        merged.extend_from_slice(policy);
        created.insert(attributes, merged);
    }
    Ok(created)
}
