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
use serde_json::{Value as J, json};
use std::path::Path;

pub(crate) struct Context<'a> {
    pub route: WriteRoute,
    pub record: &'a Path,
    pub state_root: &'a Path,
    pub source_path: &'a Path,
    pub envelope_sha256: &'a str,
    pub expected_target: Option<&'a J>,
    pub supplied_runtime: Option<&'a crate::reasoning_runtime::Runtime>,
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

/// Return `None` for routes that keep the existing local/private report path.
/// A captured result owns its complete ingestion receipt and writes no record image.
pub(crate) fn capture(
    report: &Report,
    event: &str,
    context: Context<'_>,
) -> Result<Option<Output>> {
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
    let project = context.route.project().clone();
    let expected_policy = context.route.config().clone();
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
    let (mut answer, signals) = super::receipt(
        report,
        context.record,
        context.state_root,
        event,
        context.source_path,
        context.envelope_sha256,
        "project_captured",
        Some("Complete report captured in pending_grounding"),
        false,
        None,
        None,
        context.supplied_runtime,
    )?;
    answer["source"] = J::String(source_id);
    answer["pending"] = pending.to_json()?;
    answer["diagnostics"] = J::Array(prepared.diagnostics.into_iter().map(J::String).collect());
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
    Ok(Some(Output {
        text: format!("{}\n", serde_json::to_string(&answer)?),
        code: 0,
    }))
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
        let invoke = || {
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
                },
            )
            .unwrap()
            .unwrap()
        };
        let first: J = serde_json::from_str(&invoke().text).unwrap();
        assert_eq!(first["state"], "project_captured");
        assert_eq!(first["pending"]["replay"], false);
        assert_eq!(fs::read(&record).unwrap(), before);
        let replay: J = serde_json::from_str(&invoke().text).unwrap();
        assert_eq!(replay["pending"]["replay"], true);
        assert_eq!(replay["pending"]["revision"], first["pending"]["revision"]);
        assert_eq!(fs::read(&record).unwrap(), before);
    }
}
