//! Advanced-mode source reports captured as one immutable pending contribution.
use super::{Output, Report};
use crate::{
    Error, Result,
    history_authority::Files,
    history_contract::{Map, field, map, text},
    history_view::map_mut,
    legacy_batch, pending_bundle,
    project_modes::WriteRoute,
    require,
    source_inventory::Inventory,
    value::TypedValue as V,
};
use base64::Engine as _;
use serde_json::{Value as J, json};
use std::path::Path;

use crate::history_contribution_prepare as history_prepare;

fn s(value: &str) -> V {
    V::Text(value.into())
}

pub(crate) struct Context<'a> {
    pub route: WriteRoute,
    pub record: &'a Path,
    pub state_root: &'a Path,
    pub source_path: &'a Path,
    pub envelope_sha256: &'a str,
    pub expected_target: Option<&'a J>,
    pub supplied_runtime: Option<&'a crate::reasoning_runtime::Runtime>,
    pub after_capture: &'a mut dyn FnMut() -> Result<()>,
}
#[derive(Clone, Copy)]
struct ReceiptContext<'a> {
    record: &'a Path,
    state_root: &'a Path,
    source_path: &'a Path,
    envelope_sha256: &'a str,
    supplied_runtime: Option<&'a crate::reasoning_runtime::Runtime>,
}
impl<'a> From<&Context<'a>> for ReceiptContext<'a> {
    fn from(c: &Context<'a>) -> Self {
        Self {
            record: c.record,
            state_root: c.state_root,
            source_path: c.source_path,
            envelope_sha256: c.envelope_sha256,
            supplied_runtime: c.supplied_runtime,
        }
    }
}

pub(crate) struct PreparedHistory {
    pub bundle: V,
    pub files: Files,
    pub diagnostics: Vec<String>,
    pub source_capture: crate::history_capture::Capture,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_history(
    report: &Report,
    event: &str,
    captured: &crate::history_capture::Capture,
    mutation: &crate::history_transaction::PreparedMutation,
    recorded_at: &str,
    source_entry: &str,
    record: &Path,
    diagnostics: Vec<String>,
) -> Result<PreparedHistory> {
    let scope = scope(report)
        .transpose()?
        .ok_or_else(|| Error("missing report scope".into()))?;
    let source_id = format!("s.ingest_{event}");
    let roots = roots(report, &source_id);
    let disclosures = report
        .raw
        .get("disclosed_locators")
        .map(V::from_json)
        .transpose()?
        .unwrap_or_else(|| V::List(vec![]));
    let (artifact, history_files) = history_prepare::prepare_subset(
        captured,
        &roots,
        &scope,
        &format!("report-subset-{event}"),
        recorded_at,
        source_entry,
        &disclosures,
        mutation,
    )
    .map_err(|e| Error(format!("history subset preparation: {e}")))?;
    let historical = crate::history_bundle::validate_artifact(
        field(map(&artifact)?, "manifest")?,
        field(map(&artifact)?, "revision")?,
        &history_files,
    )
    .map_err(|e| Error(format!("history subset replay: {e}")))?;
    let adapted = crate::history_adapter::from_store_capture(&historical)?;
    let document = adapted.document().clone();
    let portable = portable_source(record, event)?;
    let mut evidence = Files::new();
    for name in pending_bundle::required_files(&document)? {
        if name == portable {
            evidence.insert(name, report.quote.as_bytes().to_vec());
        } else {
            evidence.insert(name.clone(), captured_evidence(captured, &name)?);
        }
    }
    require(
        evidence
            .get(&portable)
            .is_some_and(|raw| raw == report.quote.as_bytes()),
        "scoped report source bytes differ from retained quote",
    )?;
    let mut files = evidence;
    for (path, raw) in &history_files {
        files.insert(format!("history-closure/{path}"), raw.clone());
    }
    let mut plain = document.clone();
    if let Some(meta) = map_mut(&mut plain)?.get_mut("meta") {
        map_mut(meta)?.remove("history");
    }
    let reasoning = crate::reasoning_capabilities::document_capabilities(&plain)?;
    let artifact_fields = map(&artifact)?;
    let manifest = obj([
        ("version", V::Integer(crate::value::Integer::new("3")?)),
        (
            "requires",
            map(field(artifact_fields, "manifest")?)?["requires"].clone(),
        ),
        (
            "roots",
            map(field(artifact_fields, "manifest")?)?["roots"].clone(),
        ),
        ("document", document),
        ("scope", scope),
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
                    .map(|(p, b)| (p.clone(), s(&crate::identity::sha256(b))))
                    .collect(),
            ),
        ),
    ]);
    let bundle = obj([("revision", s(&manifest.digest()?)), ("manifest", manifest)]);
    pending_bundle::validate(&bundle, &files)
        .map_err(|e| Error(format!("history contribution wrapper: {e}")))?;
    Ok(PreparedHistory {
        bundle,
        files,
        diagnostics,
        source_capture: captured.clone(),
    })
}

fn captured_evidence(captured: &crate::history_capture::Capture, name: &str) -> Result<Vec<u8>> {
    crate::history_authority::relative_path(name)?;
    let expected = captured
        .inventory
        .get(&("bytes".into(), name.into()))
        .ok_or_else(|| Error(format!("uncaptured report contribution evidence: {name}")))?;
    let crate::history_capture::Observation::Bytes { sha256, maximum } = expected else {
        return Err(Error(format!(
            "uncaptured report contribution evidence: {name}"
        )));
    };
    let mut path = captured.root.clone();
    for part in name.split('/') {
        path.push(part);
        require(
            !std::fs::symlink_metadata(&path)?.file_type().is_symlink(),
            "symlink_path",
        )?;
    }
    let metadata = path.metadata()?;
    require(
        metadata.is_file() && metadata.len() <= *maximum as u64,
        "history_limit",
    )?;
    let raw = std::fs::read(path)?;
    require(
        raw.len() <= *maximum && crate::identity::sha256(&raw) == *sha256,
        "snapshot_changed",
    )?;
    Ok(raw)
}

fn obj(items: impl IntoIterator<Item = (&'static str, V)>) -> V {
    V::Map(items.into_iter().map(|(k, v)| (k.into(), v)).collect())
}

/// Finalize the existing history report pipeline after its batch, graph and
/// core-gate preparation have succeeded. The caller supplies the versioned
/// history contribution; this function owns durable pending capture and replay.
pub(crate) fn capture_prepared_history(
    report: &Report,
    event: &str,
    context: Context<'_>,
    prepared: PreparedHistory,
) -> Result<Output> {
    if let Some(reason) = super::private_reason(report)? {
        return Err(Error(reason.into()));
    }
    require(
        applies(report, &context.route)?,
        "advanced history report is not project scoped",
    )?;
    pending_bundle::validate(&prepared.bundle, &prepared.files)?;
    prepared.source_capture.verify_current()?;
    let manifest = map(field(map(&prepared.bundle)?, "manifest")?)?;
    require(
        crate::history_contract::is_int(field(manifest, "version")?, "3"),
        "history report requires a versioned history contribution",
    )?;
    let source_id = format!("s.ingest_{event}");
    let pending_event = format!("report-{event}");
    {
        let _state_lock =
            crate::history_transaction_fs::DirectoryGuard::acquire(context.state_root, true)?;
        super::save(
            &context
                .state_root
                .join("journals")
                .join(format!("{event}.json")),
            &retained(
                event,
                &pending_event,
                &source_id,
                &prepared.bundle,
                &prepared.files,
                &prepared.diagnostics,
                &context,
            )?,
        )?;
    }
    let project = context.route.project().clone();
    let expected_policy = context.route.config().clone();
    let receipt_context = ReceiptContext::from(&context);
    context.route.verify()?;
    drop(context.route);
    let pending = crate::public_knowledge::import_helper::capture_pending(
        &project,
        &prepared.bundle,
        &prepared.files,
        &pending_event,
        &pending_event,
        &mut || {
            prepared.source_capture.verify_current()?;
            require(
                project.config()? == expected_policy,
                "project policy or destination changed before capture; retry",
            )?;
            require(
                crate::history_transaction_fs::read(context.source_path)?.as_deref()
                    == Some(report.quote.as_bytes()),
                "scoped report source bytes differ from retained quote",
            )
        },
    )?;
    (context.after_capture)()?;
    finish(
        report,
        event,
        receipt_context,
        &source_id,
        prepared.diagnostics,
        pending,
        "Complete scoped history report captured in pending_grounding",
    )
}

fn scope(report: &Report) -> Option<Result<V>> {
    report.raw.get("scope").map(V::from_json)
}

pub(crate) fn applies(report: &Report, route: &WriteRoute) -> Result<bool> {
    let Some(scope) = scope(report).transpose()? else {
        return Ok(false);
    };
    Ok(route.pending_required()?
        && report.raw.get("shareability") == Some(&J::String("project".into()))
        && map(&scope)
            .ok()
            .and_then(|scope| scope.get("kind"))
            .and_then(|kind| text(kind).ok())
            .is_some_and(|kind| ["project", "external"].contains(&kind)))
}

fn roots(report: &Report, source: &str) -> Vec<String> {
    let mut roots = report
        .updates
        .iter()
        .filter_map(|update| update.get("id").and_then(J::as_str).map(str::to_owned))
        .collect::<Vec<_>>();
    roots.push(source.into());
    roots.sort();
    roots.dedup();
    roots
}

fn scoped_actions(actions: &mut [V], scope: &V) -> Result<()> {
    for action in actions {
        let action = map_mut(action)?;
        action.insert("_record_scope".into(), scope.clone());
        if text(field(action, "kind")?)? == "add" {
            let id = text(field(action, "id")?)?.to_owned();
            let body = map_mut(field_mut(action, "body")?)?;
            if let Some(existing) = body.get("scope") {
                require(
                    existing.digest()? == scope.digest()?,
                    &format!("entry scope differs from report scope: {}", id),
                )?;
            }
            body.insert("scope".into(), scope.clone());
        }
    }
    Ok(())
}

pub(crate) fn apply_declared_scope(report: &Report, actions: &mut [V]) -> Result<()> {
    if let Some(scope) = scope(report).transpose()? {
        scoped_actions(actions, &scope)?;
    }
    Ok(())
}

fn field_mut<'a>(fields: &'a mut Map, key: &str) -> Result<&'a mut V> {
    fields
        .get_mut(key)
        .ok_or_else(|| Error(format!("missing_field:{key}")))
}

fn portable_source(record: &Path, event: &str) -> Result<String> {
    let entry = record
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| Error("invalid_path".into()))?;
    let layout = crate::history_transaction::Layout::for_entry(entry)?;
    Ok(if layout.home.is_empty() {
        format!("evidence/reports/{event}.txt")
    } else {
        format!("{}/evidence/reports/{event}.txt", layout.home)
    })
}

fn evidence(
    document: &V,
    roots: &[String],
    source_id: &str,
    portable: &str,
    quote: &[u8],
    record: &Path,
    inventory: &mut Inventory,
) -> Result<Files> {
    let closure = pending_bundle::closure(document, roots)?;
    let entries = crate::reasoning_snapshot::entries(&closure)?;
    for (id, (_, body)) in &entries {
        if id != source_id && pending_bundle::required_files(body)?.contains(portable) {
            return Err(Error(
                "captured report evidence path conflicts with an existing source".into(),
            ));
        }
    }
    let required = pending_bundle::required_files(&closure)?;
    let mut files = Files::new();
    let root = record
        .parent()
        .ok_or_else(|| Error("invalid_path".into()))?;
    for name in required {
        crate::history_branch::portable_path(&name)?;
        if name == portable {
            files.insert(name, quote.to_vec());
            continue;
        }
        let path = root.join(&name);
        let resolved = path.canonicalize()?;
        require(
            resolved.starts_with(root.canonicalize()?) && resolved.is_file(),
            "missing report contribution evidence",
        )?;
        files.insert(name, inventory.read(&resolved)?);
    }
    Ok(files)
}

fn retained(
    event: &str,
    pending_event: &str,
    source_id: &str,
    bundle: &V,
    files: &Files,
    diagnostics: &[String],
    context: &Context<'_>,
) -> Result<J> {
    Ok(json!({
        "version":1,
        "kind":"native-advanced-report/v1",
        "event_id":event,
        "pending_event_id":pending_event,
        "pending_revision":text(field(map(bundle)?, "revision")?)?,
        "source_id":source_id,
        "bundle":bundle.to_tagged()?,
        "files":files.iter().map(|(path, raw)| (
            path.clone(), J::String(base64::engine::general_purpose::STANDARD.encode(raw))
        )).collect::<serde_json::Map<_,_>>(),
        "diagnostics":diagnostics,
        "record":context.record, "policy":context.route.config().to_json()?,
        "routing":crate::source_capture::routing_observation(context.route.paths(), &context.route.project().root)?.to_json()?,
        "envelope_sha256":context.envelope_sha256,
    }))
}

fn retained_bundle(value: &J) -> Result<(V, Files)> {
    require(
        value["version"] == 1 && value["kind"] == "native-advanced-report/v1",
        "invalid advanced report journal",
    )?;
    let bundle = V::from_tagged(&value["bundle"])?;
    let mut files = Files::new();
    let encoded = value["files"]
        .as_object()
        .ok_or_else(|| Error("invalid advanced report journal".into()))?;
    for (path, raw) in encoded {
        let raw = raw
            .as_str()
            .ok_or_else(|| Error("invalid advanced report journal".into()))?;
        files.insert(
            path.clone(),
            base64::engine::general_purpose::STANDARD
                .decode(raw)
                .map_err(|_| Error("invalid advanced report journal".into()))?,
        );
    }
    pending_bundle::validate(&bundle, &files)?;
    require(
        value["pending_revision"] == text(field(map(&bundle)?, "revision")?)?,
        "retained advanced report revision mismatch",
    )?;
    Ok((bundle, files))
}

fn finish(
    report: &Report,
    event: &str,
    context: ReceiptContext<'_>,
    source_id: &str,
    diagnostics: Vec<String>,
    pending: V,
    reason: &str,
) -> Result<Output> {
    let (mut answer, signals) = super::receipt(
        report,
        context.record,
        context.state_root,
        event,
        context.source_path,
        context.envelope_sha256,
        "project_captured",
        Some(reason),
        false,
        None,
        None,
        context.supplied_runtime,
    )?;
    answer["source"] = J::String(source_id.into());
    answer["pending"] = pending.to_json()?;
    answer["diagnostics"] = J::Array(diagnostics.into_iter().map(J::String).collect());
    let _state_lock =
        crate::history_transaction_fs::DirectoryGuard::acquire(context.state_root, true)?;
    super::save(
        &context
            .state_root
            .join("receipts")
            .join(format!("{event}.json")),
        &answer,
    )?;
    super::save(
        &context
            .state_root
            .join("results")
            .join(format!("{event}.json")),
        &json!({"receipt":answer,"signals":signals}),
    )?;
    crate::history_transaction_fs::remove(
        &context
            .state_root
            .join("journals")
            .join(format!("{event}.json")),
    )?;
    Ok(Output {
        text: format!("{}\n", serde_json::to_string(&answer)?),
        code: 0,
    })
}

/// Return `None` for routes that keep the existing local/private report path.
/// A captured result owns its complete ingestion receipt and writes no record image.
pub(crate) fn capture(
    report: &Report,
    event: &str,
    context: Context<'_>,
) -> Result<Option<Output>> {
    if let Some(reason) = super::private_reason(report)? {
        return Err(Error(reason.into()));
    }
    if !applies(report, &context.route)? {
        return Ok(None);
    }
    let scope = scope(report)
        .transpose()?
        .ok_or_else(|| Error("missing report scope".into()))?;
    let mut inventory = Inventory::default();
    let captured = crate::source_document::load(
        std::slice::from_ref(&context.record.to_path_buf()),
        &mut inventory,
        false,
    )?;
    require(
        captured.members == [context.record.to_path_buf()],
        "multi-file and pointer records require primary review",
    )?;
    require(
        map(&captured.hypotheses)?.is_empty(),
        "a record with hypothesis context requires primary review",
    )?;
    let document = captured.source.projected();
    super::verify_captured_target(
        &document,
        inventory
            .files
            .get(context.record)
            .ok_or_else(|| Error("snapshot_changed".into()))?,
        report,
        context.expected_target,
    )?;
    if report.raw.get("source").is_some()
        || report.updates.iter().any(|update| update["kind"] == "add")
    {
        let expected = report
            .raw
            .get("record_sha256")
            .and_then(J::as_str)
            .ok_or_else(|| Error("new entries and existing source citations require record_sha256 from the primary's prior read".into()))?;
        require(
            crate::identity::sha256(
                inventory
                    .files
                    .get(context.record)
                    .ok_or_else(|| Error("snapshot_changed".into()))?,
            ) == expected,
            "record changed since the primary read it; reread the premises before resubmitting",
        )?;
    }
    require(
        !crate::recording_privacy::private_marker(&document),
        "private or unclear original source permission; report retained privately",
    )?;
    let collection = super::source_collection(&document, report)?;
    let source_id = format!("s.ingest_{event}");
    let portable = portable_source(context.record, event)?;
    let (mut actions, source_bodies) =
        super::actions(report, event, Path::new(&portable), &collection)?;
    scoped_actions(&mut actions, &scope)?;
    let roots = roots(report, &source_id);

    let selected = pending_bundle::closure(&preliminary_document(&document, &actions)?, &roots)?;
    require(
        !crate::recording_privacy::private_marker(&selected),
        "private or unclear original source permission; report retained privately",
    )?;
    let mut context_value = json!({
        "kind":"source-report/v1", "event_id":event,
        "source_sha256":crate::identity::sha256(report.quote.as_bytes()),
        "envelope_sha256":context.envelope_sha256, "record":context.record,
        "state_dir":context.state_root, "policy":context.route.config().to_json()?,
        "routing":crate::source_capture::routing_observation(
            context.route.paths(), &context.route.project().root,
        )?.to_json()?
    });
    if let Some(target) = context.expected_target {
        context_value["target_snapshot"] = target.clone();
    }
    let prepared = legacy_batch::prepare_pending(
        &actions,
        &context.route,
        &legacy_batch::Options {
            operation: format!("report-{event}"),
            context: V::from_json(&context_value)?,
        },
        inventory,
        None,
        &source_bodies,
    )?;
    let mut inventory = prepared.inventory;
    let files = evidence(
        &prepared.document,
        &roots,
        &source_id,
        &portable,
        report.quote.as_bytes(),
        context.record,
        &mut inventory,
    )?;
    let bundle = pending_bundle::prepare(&prepared.document, &roots, &scope, "project", &files)?;
    let pending_event = format!("report-{event}");
    {
        let _state_lock =
            crate::history_transaction_fs::DirectoryGuard::acquire(context.state_root, true)?;
        super::save(
            &context
                .state_root
                .join("journals")
                .join(format!("{event}.json")),
            &retained(
                event,
                &pending_event,
                &source_id,
                &bundle,
                &files,
                &prepared.diagnostics,
                &context,
            )?,
        )?;
    }
    let project = context.route.project().clone();
    let expected_policy = context.route.config().clone();
    let receipt_context = ReceiptContext::from(&context);
    inventory.verify()?;
    context.route.verify()?;
    drop(context.route);
    let pending = crate::public_knowledge::import_helper::capture_pending(
        &project,
        &bundle,
        &files,
        &pending_event,
        &pending_event,
        &mut || {
            inventory.verify()?;
            require(
                project.config()? == expected_policy,
                "project policy or destination changed before capture; retry",
            )?;
            require(
                crate::history_transaction_fs::read(context.source_path)?.as_deref()
                    == Some(report.quote.as_bytes()),
                "scoped report source bytes differ from retained quote",
            )
        },
    )?;
    (context.after_capture)()?;
    let reason = if map(field(map(&bundle)?, "manifest")?)?
        .get("version")
        .is_some_and(|value| crate::history_contract::is_int(value, "3"))
    {
        "Complete scoped history report captured in pending_grounding"
    } else {
        "Complete report captured in pending_grounding"
    };
    Ok(Some(finish(
        report,
        event,
        receipt_context,
        &source_id,
        prepared.diagnostics,
        pending,
        reason,
    )?))
}

fn preliminary_document(document: &V, actions: &[V]) -> Result<V> {
    let mut candidate = document.clone();
    for action in actions {
        let action = map(action)?;
        let id = text(field(action, "id")?)?;
        if text(field(action, "kind")?)? == "add" {
            let collection = action
                .get("into")
                .and_then(|value| text(value).ok())
                .unwrap_or("known");
            let members = map_mut(&mut candidate)?
                .entry(collection.into())
                .or_insert_with(|| V::Map(Map::new()));
            if *members == V::Null {
                *members = V::Map(Map::new());
            }
            map_mut(members)?.insert(id.into(), field(action, "body")?.clone());
        }
    }
    Ok(candidate)
}

/// Resume an exact retained Advanced report bundle before the generic direct-write
/// journal decoder runs. Recovery uses only the durable bundle and current policy;
/// it never reconstructs authority from changed source files.
pub(crate) fn recover(
    report: &Report,
    event: &str,
    context: Context<'_>,
) -> Result<Option<Output>> {
    let journal_path = context
        .state_root
        .join("journals")
        .join(format!("{event}.json"));
    let Some(raw) = crate::history_transaction_fs::read(&journal_path)? else {
        return Ok(None);
    };
    let journal =
        crate::json_ingress::parse_slice(&raw, crate::json_ingress::DuplicateKeys::Reject)?;
    if journal.get("kind") != Some(&J::String("native-advanced-report/v1".into())) {
        return Ok(None);
    }
    require(
        journal["event_id"] == event,
        "advanced report journal event mismatch",
    )?;
    let (bundle, files) = retained_bundle(&journal)?;
    let pending_event = journal["pending_event_id"]
        .as_str()
        .ok_or_else(|| Error("invalid advanced report journal".into()))?;
    let source_id = journal["source_id"]
        .as_str()
        .ok_or_else(|| Error("invalid advanced report journal".into()))?;
    require(
        pending_event == format!("report-{event}") && source_id == format!("s.ingest_{event}"),
        "advanced report journal context mismatch",
    )?;
    let diagnostics = journal["diagnostics"]
        .as_array()
        .ok_or_else(|| Error("invalid advanced report journal".into()))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| Error("invalid advanced report journal".into()))
        })
        .collect::<Result<Vec<_>>>()?;
    let project = context.route.project().clone();
    let expected_policy = context.route.config().clone();
    let receipt_context = ReceiptContext::from(&context);
    let revision = text(field(map(&bundle)?, "revision")?)?;
    let already_captured = crate::pending_state::Ledger::capture(&project)?
        .events
        .iter()
        .any(|item| {
            map(item).is_ok_and(|fields| {
                fields.get("event_id") == Some(&V::Text(pending_event.into()))
                    && fields.get("contribution_id") == Some(&V::Text(pending_event.into()))
                    && fields.get("revision") == Some(&V::Text(revision.into()))
            })
        });
    if !already_captured {
        require(
            journal["record"] == json!(context.record)
                && journal["envelope_sha256"] == context.envelope_sha256
                && journal["policy"] == context.route.config().to_json()?
                && journal["routing"]
                    == crate::source_capture::routing_observation(
                        context.route.paths(),
                        &context.route.project().root,
                    )?
                    .to_json()?,
            "advanced report journal context mismatch",
        )?;
    }
    context.route.verify()?;
    drop(context.route);
    let pending = crate::public_knowledge::import_helper::capture_pending(
        &project,
        &bundle,
        &files,
        pending_event,
        pending_event,
        &mut || {
            if already_captured {
                Ok(())
            } else {
                require(
                    project.config()? == expected_policy,
                    "project policy or destination changed before capture; retry",
                )
            }
        },
    )?;
    let reason = if map(field(map(&bundle)?, "manifest")?)?
        .get("version")
        .is_some_and(|value| crate::history_contract::is_int(value, "3"))
    {
        "Complete scoped history report captured in pending_grounding"
    } else {
        "Complete report captured in pending_grounding"
    };
    Ok(Some(finish(
        report,
        event,
        receipt_context,
        source_id,
        diagnostics,
        pending,
        reason,
    )?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, process::Command};

    fn git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn atomic_report_capture_preserves_record_and_replays_one_bundle() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        fs::create_dir(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["config", "user.name", "Fixture"]);
        git(&root, &["config", "user.email", "fixture@example.test"]);
        let record = root.join("GROUNDING.yaml");
        fs::write(
            &record,
            "sources:\n  s.one: {url: 'https://example.test', read: 2026-09-01}\nknown:\n  fact.x: {v: 1, from: s.one, at: old, of: 2026-09-01}\n  fact.y: {v: 2, from: s.one, at: old, of: 2026-09-01}\n",
        )
        .unwrap();
        git(&root, &["add", "GROUNDING.yaml"]);
        git(
            &root,
            &["-c", "commit.gpgsign=false", "commit", "-qm", "base"],
        );
        let before = fs::read(&record).unwrap();
        let state = root.join("state");
        fs::create_dir_all(state.join("sources")).unwrap();
        for name in ["receipts", "results"] {
            fs::create_dir_all(state.join(name)).unwrap();
        }
        let event = "0123456789abcdef0123456789abcdef";
        let source_path = state.join("sources").join(format!("{event}.txt"));
        fs::write(&source_path, "The values are 3 and 4.").unwrap();
        let raw = json!({
            "event_id":"caller", "date":"2026-09-20", "source_quote":"The values are 3 and 4.",
            "record_sha256":crate::identity::sha256(&before), "shareability":"project",
            "scope":{"kind":"external","environment":"account A"},
            "updates":[
                {"kind":"set","id":"fact.x","value":3,"at":"clause 3"},
                {"kind":"set","id":"fact.y","value":4,"at":"clause 4"}
            ]
        });
        let encoded = serde_json::to_vec(&raw).unwrap();
        let report = super::super::parse(&encoded).unwrap();
        let mut stop = || Err(Error("stop after capture".into()));
        assert_eq!(
            capture(
                &report,
                event,
                Context {
                    route: WriteRoute::capture(std::slice::from_ref(&record), &root).unwrap(),
                    record: &record,
                    state_root: &state,
                    source_path: &source_path,
                    envelope_sha256: "envelope",
                    expected_target: None,
                    supplied_runtime: None,
                    after_capture: &mut stop,
                },
            )
            .unwrap_err()
            .0,
            "stop after capture"
        );
        assert_eq!(fs::read(&record).unwrap(), before);
        assert!(
            state
                .join("journals")
                .join(format!("{event}.json"))
                .is_file()
        );
        fs::remove_file(&source_path).unwrap();
        let mut after_capture = || Ok(());
        let first: J = serde_json::from_str(
            &recover(
                &report,
                event,
                Context {
                    route: WriteRoute::capture(std::slice::from_ref(&record), &root).unwrap(),
                    record: &record,
                    state_root: &state,
                    source_path: &source_path,
                    envelope_sha256: "envelope",
                    expected_target: None,
                    supplied_runtime: None,
                    after_capture: &mut after_capture,
                },
            )
            .unwrap()
            .unwrap()
            .text,
        )
        .unwrap();
        assert_eq!(first["state"], "project_captured");
        assert_eq!(first["pending"]["replay"], true);
        assert_eq!(fs::read(&record).unwrap(), before);
        assert!(
            !state
                .join("journals")
                .join(format!("{event}.json"))
                .exists()
        );
    }
}
