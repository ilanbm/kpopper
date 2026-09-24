//! Durable asynchronous ingestion built around the one public update writer.
use crate::{
    Result, history_contract::error, ingestion_state as S, project_modes::WriteRoute, require,
};
use serde_json::{Value as J, json};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

const MAX_EVENTS: usize = 32;
const MAX_WORKER_PASSES: usize = 4;
const LEASE_SECONDS: f64 = 300.0;

/// Capture validation is read-only. The writer repeats the complete validation
/// before preparing any mutation.
fn validate_envelope(raw: &[u8]) -> Result<J> {
    require(
        raw.len() <= 16 * 1024 * 1024,
        "capture envelope is too large",
    )?;
    crate::public_update::validate_report(raw)
}

pub(crate) fn selected_record(record: Option<&Path>, cwd: &Path) -> Result<(PathBuf, J, PathBuf)> {
    let original = if let Some(record) = record {
        vec![cwd.join(record)]
    } else {
        crate::public_workspace::records(cwd)?
    };
    let route = WriteRoute::capture(&original, cwd)?;
    require(
        route.paths().len() == 1,
        "ingestion requires one record path",
    )?;
    Ok((
        route.paths()[0].clone(),
        route.config().to_json()?,
        route.project().root.clone(),
    ))
}

fn event_id(record: &Path, envelope: &J) -> String {
    if let Some(id) = envelope.get("event_id").and_then(J::as_str) {
        crate::identity::sha256(format!("{}\0{id}", record.display()).as_bytes())[..32].into()
    } else {
        uuid::Uuid::new_v4().simple().to_string()
    }
}

fn terminal(value: &J) -> bool {
    value
        .get("state")
        .and_then(J::as_str)
        .is_some_and(|state| S::TERMINAL.contains(&state))
}

fn load_capture(layout: &S::Layout, event: &J) -> Result<Vec<u8>> {
    let id = event["event_id"]
        .as_str()
        .ok_or_else(|| error("invalid ingestion event"))?;
    let envelope = S::read(&layout.path("envelopes", id))?
        .ok_or_else(|| error("captured input is unavailable"))?;
    let source =
        S::read(&layout.source_path(id))?.ok_or_else(|| error("captured input is unavailable"))?;
    require(
        crate::identity::sha256(&envelope) == event["envelope_sha256"],
        "captured envelope changed after capture",
    )?;
    require(
        crate::identity::sha256(&source) == event["source_sha256"],
        "captured source text changed after capture",
    )?;
    require(
        event["source_file"] == json!(layout.source_path(id)),
        "captured source path changed after capture",
    )?;
    let parsed =
        crate::json_ingress::parse_slice(&envelope, crate::json_ingress::DuplicateKeys::Reject)?;
    require(
        parsed["source_quote"].as_str().map(str::as_bytes) == Some(source.as_slice()),
        "captured source text changed after capture",
    )?;
    Ok(envelope)
}

fn bound_target(
    layout: &S::Layout,
    event: &J,
    envelope: &J,
    record: &Path,
    cwd: &Path,
) -> Result<J> {
    let expected = event
        .get("target_snapshot")
        .ok_or_else(|| error("invalid ingestion event"))?;
    // A retained node report owns exact before/after worlds, including a pending
    // publication whose current view is intentionally unreadable. Its writer verifies
    // the original target binding while replaying the retained envelope.
    if let Some(id) = event["event_id"].as_str()
        && S::read_json(&layout.path("journals", id))?
            .is_some_and(|journal| journal["kind"] == "native-node-report/v1")
    {
        return Ok(expected.clone());
    }
    let current = crate::public_update::capture_target(envelope, record, cwd)?;
    if current["body_sha256"] == expected["body_sha256"] {
        return Ok(expected.clone());
    }
    if envelope.get("updates").is_some() {
        return Ok(expected.clone());
    }
    let Some(source) = current
        .get("source")
        .and_then(J::as_str)
        .and_then(|source| source.strip_prefix("s.ingest_"))
    else {
        return Ok(expected.clone());
    };
    let Some(prior_event) = S::read_json(&layout.path("events", source))? else {
        return Ok(expected.clone());
    };
    let Some(prior_receipt) = S::read_json(&layout.path("receipts", source))? else {
        return Ok(expected.clone());
    };
    let owned = prior_receipt["state"] == "applied"
        && prior_receipt["target"] == event["target"]
        && prior_event["order"].as_u64().unwrap_or(u64::MAX) < event["order"].as_u64().unwrap_or(0)
        && prior_receipt["target_after_sha256"] == current["body_sha256"];
    Ok(if owned { current } else { expected.clone() })
}

fn receipt_id(event: &str) -> String {
    crate::identity::sha256(format!("receipt\0{event}").as_bytes())[..32].into()
}

fn validated_handled(value: J) -> Result<J> {
    require(value.is_object(), "invalid handled state")?;
    require(value.get("signals").is_some_and(J::is_object), "invalid handled state")?;
    Ok(value)
}
fn signal_id(event: &str, question: &str) -> String {
    crate::identity::sha256(format!("{event}\0question\0{question}").as_bytes())[..32].into()
}
fn finish_question(layout: &S::Layout, event: &mut J, envelope: &J, reason: &str) -> Result<J> {
    let id = event["event_id"].as_str().unwrap().to_owned();
    let question = envelope
        .get("question")
        .and_then(J::as_str)
        .unwrap_or(reason);
    let signal = json!({
        "id":signal_id(&id, question), "event_id":id, "category":"question",
        "target":envelope.get("target").cloned().unwrap_or(J::Null),
        "source_quote":envelope.get("source_quote").cloned().unwrap_or(J::String(String::new())),
        "affected_judgments":[], "newly_fired_judgments":[], "actionable_judgments":[],
        "reason":reason, "question":question,
    });
    let receipt = json!({
        "record":layout.record, "state_dir":layout.root, "id":receipt_id(&id), "event_id":id,
        "state":"needs_primary", "target":envelope.get("target").cloned().unwrap_or(J::Null),
        "value":envelope.get("value").cloned().unwrap_or(J::Null), "source":J::Null,
        "source_file":event["source_file"], "reason":reason,
        "source_sha256":event["source_sha256"], "envelope_sha256":event["envelope_sha256"],
        "signal_ids":[signal["id"].clone()], "reach":{},
    });
    S::save_json(
        &layout.path("signals", signal["id"].as_str().unwrap()),
        &signal,
    )?;
    S::save_json(&layout.path("receipts", &id), &receipt)?;
    S::save_json(
        &layout.path("results", &id),
        &json!({"receipt":receipt,"signals":[signal]}),
    )?;
    event["state"] = json!("needs_primary");
    event["reason"] = json!(reason);
    event["finished_at"] = json!(S::now());
    S::save_json(&layout.path("events", &id), event)?;
    Ok(receipt)
}

fn has_work(layout: &S::Layout) -> Result<bool> {
    Ok(events(layout)?.iter().any(|event| {
        event
            .get("state")
            .and_then(J::as_str)
            .is_some_and(|state| S::AUTO_STATES.contains(&state))
    }))
}

fn lease_active(value: Option<&J>) -> bool {
    let Some(lease) = value else { return false };
    let now = S::now();
    let Some(pid) = lease.get("pid").and_then(J::as_u64) else {
        return lease.get("started_at").and_then(J::as_f64)
            .is_some_and(|started| started.is_finite() && now - started < 5.0);
    };
    if pid == 0 || pid > i32::MAX as u64 { return false; }
    #[cfg(unix)]
    let alive = match nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid as i32),
        None,
    ) {
        Ok(()) => Some(true),
        Err(_) => Some(false),
    };
    #[cfg(windows)]
    let alive = windows_process_alive(pid as u32);
    #[cfg(not(any(unix, windows)))]
    let alive = None;
    alive.unwrap_or_else(|| lease.get("expires_at").and_then(J::as_f64)
        .is_some_and(|deadline| deadline.is_finite() && deadline > now))
}

#[cfg(windows)]
fn windows_process_alive(pid: u32) -> Option<bool> {
    // Keep the crate's safe-Rust boundary and avoid relying on PATH. Query only
    // this PID through the standard Windows utility, under a bounded deadline.
    let executable = PathBuf::from(std::env::var_os("SystemRoot")?)
        .join("System32").join("tasklist.exe");
    let mut command = Command::new(executable);
    command.args(["/FO", "CSV", "/NH", "/FI", &format!("PID eq {pid}")]);
    let output = crate::reasoning_runtime::run_command_capture(
        &mut command, vec![], Duration::from_secs(2), 64 * 1024,
    ).ok()?;
    if !output.status.success() {
        return None;
    }
    tasklist_liveness(&String::from_utf8_lossy(&output.stdout), pid)
}

#[cfg(any(windows, test))]
fn tasklist_liveness(output: &str, pid: u32) -> Option<bool> {
    let row = regex::Regex::new(r#"^"(?:[^"]|"")*","([0-9]+)","#).ok()?;
    let rows = output.lines().filter_map(|line| row.captures(line)
        .and_then(|row| row[1].parse::<u32>().ok())).collect::<Vec<_>>();
    if !rows.is_empty() {
        return Some(rows.contains(&pid));
    }
    if output.trim() == "INFO: No tasks are running which match the specified criteria." {
        Some(false)
    } else {
        // Unknown/partial/localized replies are not evidence that a worker died.
        None
    }
}

fn record_writer_journal(record: &Path) -> bool {
    if record.parent().is_some_and(|root| {
        root.join(".kpopper/.history-node-publication.json")
            .is_file()
    }) {
        return true;
    }
    let Ok(store) = crate::history_store::Store::new(record) else { return false };
    let primary = store.root.join(&store.layout.journal);
    let shared = store.root.join(crate::direct_history::journal(&store));
    primary.is_file() || shared.is_file()
}

fn retryable_capture_io(layout: &S::Layout, event: &str, error: &crate::Error) -> bool {
    error.0.starts_with("io: ")
        && S::read_json(&layout.path("journals", event)).ok().flatten()
            .is_some_and(|journal| journal["kind"] == "native-advanced-report/v1")
}

fn reserve_worker(layout: &S::Layout) -> Result<Option<String>> {
    let path = layout.root.join("worker.lease");
    if lease_active(S::read_json(&path)?.as_ref()) {
        return Ok(None);
    }
    let token = uuid::Uuid::new_v4().simple().to_string();
    let lease = json!({"token":token,"started_at":S::now(),"expires_at":S::now()+LEASE_SECONDS,"pid":J::Null});
    S::save_json(&path, &lease)?;
    Ok(Some(token))
}

fn start_worker_locked(layout: &S::Layout) -> Result<bool> {
    start_worker_with(layout, &std::env::current_exe()?)
}

fn start_worker_with(layout: &S::Layout, executable: &Path) -> Result<bool> {
    let Some(token) = reserve_worker(layout)? else {
        return Ok(false);
    };
    let path = layout.root.join("worker.lease");
    let child = Command::new(executable)
        .args(["ingest", "_worker", "--record"])
        .arg(&layout.record)
        .arg("--state-dir")
        .arg(&layout.root)
        .arg("--lease-token")
        .arg(&token)
        .current_dir(layout.record.parent().unwrap())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match child {
        Ok(child) => {
            let mut lease =
                S::read_json(&path)?.ok_or_else(|| error("worker lease is unavailable"))?;
            require(
                lease["token"] == token,
                "worker lease changed while spawning",
            )?;
            lease["pid"] = json!(child.id());
            S::save_json(&path, &lease)?;
            Ok(true)
        }
        Err(error) => {
            S::remove(&path)?;
            Err(error.into())
        }
    }
}

pub fn capture(
    raw: &[u8],
    record: Option<&Path>,
    state_dir: Option<&Path>,
    cwd: &Path,
    start: bool,
) -> Result<J> {
    let envelope = validate_envelope(raw)?;
    let (record, policy, project_root) = selected_record(record, cwd)?;
    let layout = S::Layout::create_from(&record, state_dir, cwd)?;
    let record = layout.record.clone();
    let (target_snapshot, capture_issue) =
        match crate::public_update::capture_target(&envelope, &record, cwd) {
            Ok(snapshot) => (snapshot, J::Null),
            Err(error) => (J::Null, J::String(error.to_string())),
        };
    let _lock = S::FileLock::acquire(&layout.root.join("capture.lock"))?;
    let id = event_id(&record, &envelope);
    let event_path = layout.path("events", &id);
    let canonical = S::canonical_json(&envelope)?;
    if let Some(event) = S::read_json(&event_path)? {
        let prior = S::read_json(&layout.path("envelopes", &id))?;
        require(
            prior.as_ref() == Some(&envelope),
            "event_id was reused for different input",
        )?;
        if start
            && event
                .get("state")
                .and_then(J::as_str)
                .is_some_and(|state| S::AUTO_STATES.contains(&state))
        {
            start_worker_locked(&layout)?;
        }
        return status(Some(&id), Some(&record), Some(&layout.root), cwd)?
            .ok_or_else(|| error("invalid ingestion event"));
    }
    let counter_path = layout.root.join("counter.json");
    let order = S::read_json(&counter_path)?
        .and_then(|value| value.get("value").and_then(J::as_u64))
        .unwrap_or(0)
        + 1;
    S::save_json(&counter_path, &json!({"value":order}))?;
    let quote = envelope["source_quote"].as_str().unwrap().as_bytes();
    S::save_bytes(&layout.source_path(&id), quote)?;
    S::save_bytes(&layout.path("envelopes", &id), &canonical)?;
    let event = json!({
        "record":record, "state_dir":layout.root, "project_root":project_root, "project_policy":policy,
        "event_id":id, "state":"captured", "target":envelope.get("target").cloned().unwrap_or(J::Null),
        "captured_at":S::now(), "order":order, "record_hash":crate::identity::sha256(&std::fs::read(&record)?),
        "target_snapshot":target_snapshot, "capture_issue":capture_issue, "source_file":layout.source_path(&id),
        "source_sha256":crate::identity::sha256(quote), "envelope_sha256":crate::identity::sha256(&canonical), "attempts":0,
    });
    S::save_json(&event_path, &event)?;
    if start {
        start_worker_locked(&layout)?;
    }
    Ok(S::summary(&event))
}

fn events(layout: &S::Layout) -> Result<Vec<J>> {
    let mut values = Vec::new();
    for entry in std::fs::read_dir(layout.root.join("events"))? {
        let path = entry?.path();
        if path.extension().and_then(|value| value.to_str()) == Some("json")
            && let Some(value) = S::read_json(&path)?
        {
            values.push(value);
        }
    }
    values.sort_by(|a, b| a["order"].as_u64().cmp(&b["order"].as_u64()));
    Ok(values)
}

pub fn process(
    record: Option<&Path>,
    state_dir: Option<&Path>,
    cwd: &Path,
    event_id: Option<&str>,
    maximum: usize,
) -> Result<Vec<J>> {
    let (record, captured_policy, _) = selected_record(record, cwd)?;
    let layout = S::Layout::create_from(&record, state_dir, cwd)?;
    let record = layout.record.clone();
    let _processor = S::FileLock::acquire(&layout.root.join("processor.lock"))?;
    let mut output = Vec::new();
    for mut event in events(&layout)?
        .into_iter()
        .filter(|event| event_id.is_none_or(|id| event["event_id"] == id))
        .filter(|event| !terminal(event))
        .take(maximum.min(MAX_EVENTS))
    {
        let id = event["event_id"]
            .as_str()
            .ok_or_else(|| error("invalid ingestion event"))?
            .to_owned();
        event["state"] = json!("processing");
        event["attempts"] = json!(event["attempts"].as_u64().unwrap_or(0) + 1);
        event["started_at"] = json!(S::now());
        S::save_json(&layout.path("events", &id), &event)?;
        let envelope = match load_capture(&layout, &event) {
            Ok(raw) => raw,
            Err(error) => {
                let fallback = json!({"source_quote":"","target":event["target"]});
                output.push(finish_question(&layout, &mut event, &fallback, &error.0)?);
                continue;
            }
        };
        let parsed = crate::json_ingress::parse_slice(
            &envelope,
            crate::json_ingress::DuplicateKeys::Reject,
        )?;
        if event["project_policy"] != captured_policy {
            output.push(finish_question(
                &layout,
                &mut event,
                &parsed,
                "project policy changed after report capture; review the retained report",
            )?);
            continue;
        }
        if let Some(issue) = event
            .get("capture_issue")
            .and_then(J::as_str)
            .map(str::to_owned)
        {
            output.push(finish_question(&layout, &mut event, &parsed, &issue)?);
            continue;
        }
        let options = crate::public_update::Options {
            file: "-".into(),
            record: Some(record.clone()),
            state_dir: Some(layout.root.clone()),
        };
        let expected = match bound_target(&layout, &event, &parsed, &record, cwd) {
            Ok(expected) => expected,
            Err(error) => {
                output.push(finish_question(
                    &layout,
                    &mut event,
                    &parsed,
                    &format!("target conflict: {error}"),
                )?);
                continue;
            }
        };
        let result =
            crate::public_update::run_captured(&options, cwd, Some(&envelope), &id, &expected);
        match result {
            Ok(result) => {
                let receipt = crate::json_ingress::parse_slice(
                    result.text.as_bytes(),
                    crate::json_ingress::DuplicateKeys::Reject,
                )?;
                event["state"] = receipt["state"].clone();
                event["reason"] = receipt.get("reason").cloned().unwrap_or(J::Null);
                event["finished_at"] = json!(S::now());
                S::save_json(&layout.path("events", &id), &event)?;
                output.push(receipt);
            }
            Err(error) => {
                if record_writer_journal(&record) || retryable_capture_io(&layout, &id, &error) {
                    event["state"] = json!("recovery_required");
                    event["reason"] = json!(error.to_string());
                    S::save_json(&layout.path("events", &id), &event)?;
                    output.push(S::summary(&event));
                } else {
                    output.push(finish_question(&layout, &mut event, &parsed, &error.0)?);
                }
            }
        }
    }
    Ok(output)
}

pub fn status(
    event_id: Option<&str>,
    record: Option<&Path>,
    state_dir: Option<&Path>,
    cwd: &Path,
) -> Result<Option<J>> {
    let (record, _, _) = selected_record(record, cwd)?;
    let Some(layout) = S::Layout::existing_from(&record, state_dir, cwd)? else {
        return Ok(event_id.map(|_| J::Null).or(Some(J::Array(vec![]))));
    };
    if let Some(id) = event_id {
        return Ok(S::read_json(&layout.path("receipts", id))?
            .or(S::read_json(&layout.path("events", id))?));
    }
    Ok(Some(J::Array(
        events(&layout)?
            .into_iter()
            .map(|event| {
                let id = event["event_id"].as_str().unwrap_or("");
                S::read_json(&layout.path("receipts", id))
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| S::summary(&event))
            })
            .collect(),
    )))
}

pub fn pending(
    record: Option<&Path>,
    state_dir: Option<&Path>,
    cwd: &Path,
    include_handled: bool,
) -> Result<Vec<J>> {
    let (record, _, _) = selected_record(record, cwd)?;
    let Some(layout) = S::Layout::existing_from(&record, state_dir, cwd)? else {
        return Ok(vec![]);
    };
    let handled = validated_handled(
        S::read_json(&layout.root.join("handled.json"))?
            .unwrap_or_else(|| json!({"signals":{}})),
    )?;
    let mut signals = Vec::new();
    let raw = std::fs::read(&record);
    for entry in std::fs::read_dir(layout.root.join("signals"))? {
        let path = entry?.path();
        if let Some(mut signal) = S::read_json(&path)? {
            let id = signal["id"].as_str().unwrap_or("");
            if let Some(when) = handled["signals"].get(id) {
                if !include_handled {
                    continue;
                }
                signal["handled_at"] = when.clone();
            }
            let names = signal
                .get("newly_fired_judgments")
                .and_then(J::as_array)
                .filter(|names| !names.is_empty())
                .or_else(|| signal.get("actionable_judgments").and_then(J::as_array));
            if let (Some(names), Ok(raw)) = (names, raw.as_ref()) {
                let ids = names
                    .iter()
                    .filter_map(J::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                if !ids.is_empty() {
                    let seeds = signal
                        .get("target")
                        .and_then(J::as_str)
                        .map(|target| vec![target.to_owned()])
                        .unwrap_or_default();
                    if let Ok(graph) = crate::public_update::current_graph(raw, &seeds) {
                        let contradiction = signal["category"] == "contradiction";
                        let live = ids
                            .iter()
                            .filter(|name| {
                                let judgment = &graph["judgments"][name.as_str()];
                                if contradiction {
                                    judgment["evaluation"] == true
                                } else {
                                    matches!(
                                        judgment["tag"].as_str(),
                                        Some(
                                            "MOVED"
                                                | "UNCHECKED"
                                                | "BROKEN"
                                                | "BLOCKED"
                                                | "UNKNOWN"
                                        )
                                    )
                                }
                            })
                            .cloned()
                            .collect::<Vec<_>>();
                        if live.is_empty() {
                            continue;
                        }
                        signal["affected_judgments"] = json!(live);
                        if contradiction {
                            signal["newly_fired_judgments"] = json!(live);
                            signal["reason"] = json!(
                                live.iter()
                                    .filter_map(|name| graph["judgments"][name]["reason"].as_str())
                                    .collect::<Vec<_>>()
                                    .join("; ")
                            );
                        } else {
                            signal["actionable_judgments"] = json!(live);
                            let reason = live
                                .iter()
                                .filter_map(|name| {
                                    graph["judgments"][name]["reason"]
                                        .as_str()
                                        .map(|reason| format!("{name} requires review: {reason}"))
                                })
                                .collect::<Vec<_>>()
                                .join("; ");
                            signal["reason"] = json!(reason);
                            signal["question"] = signal["reason"].clone();
                        }
                    }
                }
            }
            signals.push(signal);
        }
    }
    signals.sort_by(|a, b| {
        (a["event_id"].as_str(), a["id"].as_str()).cmp(&(b["event_id"].as_str(), b["id"].as_str()))
    });
    Ok(signals)
}

pub fn acknowledge(
    ids: &[String],
    record: Option<&Path>,
    state_dir: Option<&Path>,
    cwd: &Path,
) -> Result<J> {
    require(!ids.is_empty(), "one or more signal IDs are required")?;
    let (record, _, _) = selected_record(record, cwd)?;
    let layout = S::Layout::create_from(&record, state_dir, cwd)?;
    let _lock = S::FileLock::acquire(&layout.root.join("delivery.lock"))?;
    for id in ids {
        require(layout.path("signals", id).is_file(), "unknown signal ID")?;
    }
    let path = layout.root.join("handled.json");
    let mut handled = validated_handled(
        S::read_json(&path)?.unwrap_or_else(|| json!({"signals":{}})),
    )?;
    let now = S::now();
    for id in ids {
        handled["signals"].as_object_mut().unwrap()
            .entry(id.clone()).or_insert_with(|| json!(now));
    }
    S::save_json(&path, &handled)?;
    Ok(handled)
}

pub fn worker(record: &Path, state_dir: &Path, cwd: &Path, token: &str) -> Result<()> {
    let layout = S::Layout::create(record, Some(state_dir))?;
    for _ in 0..MAX_WORKER_PASSES {
        {
            let _lock = S::FileLock::acquire(&layout.root.join("capture.lock"))?;
            let lease = S::read_json(&layout.root.join("worker.lease"))?
                .ok_or_else(|| error("worker lease is unavailable"))?;
            if lease["token"] != token {
                return Ok(());
            }
        }
        process(
            Some(&layout.record),
            Some(&layout.root),
            cwd,
            None,
            MAX_EVENTS,
        )?;
        let _lock = S::FileLock::acquire(&layout.root.join("capture.lock"))?;
        let lease = S::read_json(&layout.root.join("worker.lease"))?;
        if lease.as_ref().is_none_or(|lease| lease["token"] != token) {
            return Ok(());
        }
        if !has_work(&layout)? {
            S::remove(&layout.root.join("worker.lease"))?;
            return Ok(());
        }
    }
    let _lock = S::FileLock::acquire(&layout.root.join("capture.lock"))?;
    let lease = S::read_json(&layout.root.join("worker.lease"))?;
    if lease.as_ref().is_some_and(|lease| lease["token"] == token) {
        S::remove(&layout.root.join("worker.lease"))?;
        if has_work(&layout)? {
            start_worker_locked(&layout)?;
        }
    }
    Ok(())
}

pub fn wait_for_terminal(
    record: Option<&Path>,
    state_dir: Option<&Path>,
    cwd: &Path,
    event_id: &str,
    timeout: Duration,
) -> Result<Option<J>> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let value = status(Some(event_id), record, state_dir, cwd)?;
        if value.as_ref().is_some_and(terminal) || std::time::Instant::now() >= deadline {
            return Ok(value);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_lease_deduplicates_and_expiry_allows_a_new_token() {
        let temp = tempfile::tempdir().unwrap();
        let record = temp.path().join("GROUNDING.yaml");
        std::fs::write(&record, "known: {p.a: {v: 1}}\n").unwrap();
        let layout = S::Layout::create(&record, Some(&temp.path().join("state"))).unwrap();
        let first = reserve_worker(&layout).unwrap().unwrap();
        assert!(reserve_worker(&layout).unwrap().is_none());
        let path = layout.root.join("worker.lease");
        let mut lease = S::read_json(&path).unwrap().unwrap();
        lease["started_at"] = json!(S::now() - 6.0);
        S::save_json(&path, &lease).unwrap();
        let second = reserve_worker(&layout).unwrap().unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn spawn_failure_releases_lease_for_retry() {
        let temp = tempfile::tempdir().unwrap();
        let record = temp.path().join("GROUNDING.yaml");
        std::fs::write(&record, "known: {p.a: {v: 1}}\n").unwrap();
        let layout = S::Layout::create(&record, Some(&temp.path().join("state"))).unwrap();
        let missing = temp.path().join("missing-worker");
        assert!(start_worker_with(&layout, &missing).is_err());
        assert!(!layout.root.join("worker.lease").exists());
        assert!(reserve_worker(&layout).unwrap().is_some());
    }

    #[test]
    fn recovery_required_is_not_automatic_work() {
        let temp = tempfile::tempdir().unwrap();
        let record = temp.path().join("GROUNDING.yaml");
        std::fs::write(&record, "known: {p.a: {v: 1}}\n").unwrap();
        let layout = S::Layout::create(&record, Some(&temp.path().join("state"))).unwrap();
        S::save_json(&layout.path("events", "e"), &json!({"event_id":"e","state":"recovery_required","order":1})).unwrap();
        assert!(!has_work(&layout).unwrap());
    }

    #[test]
    fn dead_worker_pid_does_not_hold_lease() {
        let value = json!({"pid": 4_294_967_295u64, "expires_at": S::now() + 300.0});
        assert!(!lease_active(Some(&value)));
    }

    #[test]
    fn live_worker_and_short_spawn_handoff_are_distinct() {
        assert!(lease_active(Some(&json!({"pid": std::process::id()}))));
        assert!(lease_active(Some(&json!({"pid":null,"started_at":S::now()}))));
        assert!(!lease_active(Some(&json!({"pid":null,"started_at":S::now()-6.0,
            "expires_at":S::now()+300.0}))));
    }

    #[test]
    fn windows_tasklist_replies_distinguish_rows_absence_and_unusable_output() {
        assert_eq!(tasklist_liveness("\"a,b.exe\",\"123\",\"Console\",\"1\",\"1,000 K\"\r\n", 123), Some(true));
        assert_eq!(tasklist_liveness("INFO: No tasks are running which match the specified criteria.\r\n", 123), Some(false));
        assert_eq!(tasklist_liveness("", 123), None);
        assert_eq!(tasklist_liveness("query failed or inaccessible", 123), None);
    }

    #[cfg(unix)]
    #[test]
    fn inaccessible_recycled_pid_cannot_hold_an_expired_worker_lease() {
        if nix::sys::signal::kill(nix::unistd::Pid::from_raw(1), None)
            == Err(nix::errno::Errno::EPERM)
        {
            assert!(!lease_active(Some(&json!({"pid":1,"started_at":0,"expires_at":0}))));
        }
    }

    #[test]
    fn only_retained_advanced_capture_io_is_retryable_without_writer_journal() {
        let temp = tempfile::tempdir().unwrap();
        let record = temp.path().join("GROUNDING.yaml");
        std::fs::write(&record, "known: {p.a: {v: 1}}\n").unwrap();
        let layout = S::Layout::create(&record, Some(&temp.path().join("state"))).unwrap();
        let failure = crate::Error("io: Permission denied".into());
        S::save_json(&layout.path("journals", "event"), &json!({"kind":"ordinary-report"})).unwrap();
        assert!(!retryable_capture_io(&layout, "event", &failure));
        S::save_json(&layout.path("journals", "event"), &json!({"kind":"native-advanced-report/v1"})).unwrap();
        assert!(retryable_capture_io(&layout, "event", &failure));
        assert!(!retryable_capture_io(&layout, "event", &crate::Error("stale_baseline".into())));
    }

    #[test]
    fn malformed_handled_state_is_rejected_without_indexing() {
        assert!(validated_handled(json!({"signals": 5})).is_err());
        assert!(validated_handled(json!(5)).is_err());
    }

    #[test]
    fn writer_journal_detection_uses_primary_and_shared_layouts() {
        let temp = tempfile::tempdir().unwrap();
        let record = temp.path().join("GROUNDING.yaml");
        std::fs::write(&record, "known: {p.a: {v: 1}}\n").unwrap();
        let store = crate::history_store::Store::new(&record).unwrap();
        assert!(!record_writer_journal(&record));
        std::fs::create_dir_all(store.root.join(&store.layout.journal).parent().unwrap()).unwrap();
        std::fs::write(store.root.join(&store.layout.journal), b"pending").unwrap();
        assert!(record_writer_journal(&record));
        std::fs::remove_file(store.root.join(&store.layout.journal)).unwrap();
        std::fs::create_dir_all(
            store
                .root
                .join(crate::direct_history::journal(&store))
                .parent()
                .unwrap(),
        )
        .unwrap();
        std::fs::write(
            store.root.join(crate::direct_history::journal(&store)),
            b"pending",
        )
        .unwrap();
        assert!(record_writer_journal(&record));
    }
    #[test]
    fn node_report_recovery_keeps_the_original_target_until_writer_verification() {
        let temp = tempfile::tempdir().unwrap();
        let record = temp.path().join("GROUNDING.yaml");
        std::fs::write(&record, "known: {p.a: {v: 1}}\n").unwrap();
        let layout = S::Layout::create(&record, Some(&temp.path().join("state"))).unwrap();
        let event = json!({"event_id":"e","target_snapshot":{"body_sha256":"captured"}});
        assert!(bound_target(&layout, &event, &json!({}), &record, temp.path()).is_err());
        S::save_json(
            &layout.path("journals", "e"),
            &json!({"kind":"native-node-report/v1"}),
        )
        .unwrap();
        let pending = temp.path().join(".kpopper/.history-node-publication.json");
        std::fs::create_dir_all(pending.parent().unwrap()).unwrap();
        std::fs::write(&pending, b"pending").unwrap();
        assert!(record_writer_journal(&record));
        assert_eq!(
            bound_target(&layout, &event, &json!({}), &record, temp.path()).unwrap(),
            event["target_snapshot"]
        );
        std::fs::remove_file(pending).unwrap();
        assert!(!record_writer_journal(&record));
    }

    #[test]
    fn malformed_pending_node_publication_remains_recovery_required() {
        let temp = tempfile::tempdir().unwrap();
        let record = temp.path().join("GROUNDING.yaml");
        let state = temp.path().join("state");
        std::fs::write(&record, "known: {s.old: {url: 'https://example.test', read: 2026-09-24}, p.a: {v: 1, from: s.old, at: table}}\n").unwrap();
        let envelope = json!({"event_id":"queued","source_quote":"a is 2","date":"2026-09-24","target":"p.a","value":2});
        let captured = capture(
            &serde_json::to_vec(&envelope).unwrap(),
            Some(&record),
            Some(&state),
            temp.path(),
            false,
        )
        .unwrap();
        let id = captured["event_id"].as_str().unwrap();
        let layout = S::Layout::existing_from(&record, Some(&state), temp.path())
            .unwrap()
            .unwrap();
        let event = S::read_json(&layout.path("events", id)).unwrap().unwrap();
        assert_eq!(event["capture_issue"], J::Null);
        S::save_json(
            &layout.path("journals", id),
            &json!({"kind":"native-node-report/v1"}),
        )
        .unwrap();
        std::fs::create_dir_all(temp.path().join(".kpopper")).unwrap();
        std::fs::write(temp.path().join(".kpopper/history.yaml"), "version: 3\nprofile: node-history/v1\nauthority: history\nrecord_id: fixture\ngeneration: 1\nrequires: [node-history/v1]\n").unwrap();
        std::fs::write(
            temp.path().join(".kpopper/.history-node-publication.json"),
            b"malformed",
        )
        .unwrap();
        let outputs = process(Some(&record), Some(&state), temp.path(), Some(id), 1).unwrap();
        assert_eq!(outputs[0]["state"], "recovery_required");
        assert!(
            S::read_json(&layout.path("receipts", id))
                .unwrap()
                .is_none()
        );
        assert_eq!(
            S::read_json(&layout.path("events", id)).unwrap().unwrap()["state"],
            "recovery_required"
        );
    }
}
