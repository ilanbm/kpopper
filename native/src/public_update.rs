//! Native synchronous application of one explicit source report.
use crate::{
    Result,
    history_contract::{error, map},
    history_transaction_fs as F,
    legacy_batch,
    project_modes::WriteRoute,
    source_inventory::Inventory,
    value::TypedValue as V,
};
use clap::Args;
use base64::Engine as _;
use serde_json::{Map as JsonMap, Value as J, json};
use std::{collections::{BTreeMap, BTreeSet}, fs, path::{Path, PathBuf}};

const FIELDS: &[&str] = &[
    "event_id", "session_id", "source_quote", "target", "value", "date", "kind",
    "question", "reason", "updates", "record_sha256", "shareability", "privacy", "scope",
    "source", "at", "profile", "disclosed_locators",
];

#[derive(Clone, Debug, Args)]
pub struct Options {
    /// Report JSON file, or - for standard input.
    #[arg(long)]
    pub file: String,
    #[arg(long)]
    pub record: Option<PathBuf>,
    #[arg(long)]
    pub state_dir: Option<PathBuf>,
}

pub struct Output {
    pub text: String,
    pub code: i32,
}

#[derive(Clone, Debug)]
struct Report {
    raw: J,
    date: String,
    quote: String,
    updates: Vec<J>,
    batch: bool,
}

fn required_text(map: &JsonMap<String, J>, key: &str) -> Result<String> {
    map.get(key)
        .and_then(J::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| error(&format!("{key} must be non-empty text")))
}

fn parse(raw: &[u8]) -> Result<Report> {
    let value = crate::json_ingress::parse_slice(raw, crate::json_ingress::DuplicateKeys::Reject)?;
    let top = value.as_object().ok_or_else(|| error("capture envelope must be a JSON object"))?;
    let unknown = top.keys().filter(|k| !FIELDS.contains(&k.as_str())).cloned().collect::<Vec<_>>();
    crate::require(unknown.is_empty(), &format!("unknown envelope fields: {}", unknown.join(", ")))?;
    let quote = required_text(top, "source_quote")?;
    let date = required_text(top, "date")?;
    crate::value::Date::new(&date)?;
    if let Some(profile) = top.get("profile").filter(|v| !v.is_null()) {
        crate::require(profile == "core/v1", "unsupported writer profile")?;
    }
    for key in ["source", "at"] {
        if top.contains_key(key) { required_text(top, key)?; }
    }
    if let Some(hash) = top.get("record_sha256").filter(|v| !v.is_null()) {
        let hash = hash.as_str().unwrap_or("");
        crate::require(hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            "record_sha256 must be the hash returned by the prior record read")?;
    }
    let batch = top.contains_key("updates");
    let updates = if let Some(updates) = top.get("updates").and_then(J::as_array) {
        crate::require((1..=32).contains(&updates.len()), "updates must contain 1..32 operations")?;
        crate::require(!top.contains_key("target") && !top.contains_key("value"),
            "use updates or target/value, not both")?;
        updates.clone()
    } else {
        let target = required_text(top, "target")?;
        let value = top.get("value").ok_or_else(|| error("the report needs target and value"))?;
        vec![json!({"kind":"set","id":target,"value":value})]
    };
    let mut names = BTreeSet::new();
    for update in &updates {
        let item = update.as_object().ok_or_else(|| error("each update needs kind=set or kind=add"))?;
        let kind = item.get("kind").and_then(J::as_str).unwrap_or("");
        crate::require(["set", "add"].contains(&kind), "each update needs kind=set or kind=add")?;
        let allowed: BTreeSet<&str> = if kind == "set" {
            ["kind", "id", "at", "value"].into_iter().collect()
        } else {
            ["kind", "id", "at", "body", "into", "drops"].into_iter().collect()
        };
        crate::require(item.keys().all(|k| allowed.contains(k.as_str())), &format!("invalid fields in {kind} update"))?;
        crate::require(item.contains_key(if kind == "set" { "value" } else { "body" }),
            &format!("invalid fields in {kind} update"))?;
        let id = required_text(item, "id")?;
        crate::history_paths::subject(&id).map_err(|_| error("each update needs a valid entry id"))?;
        crate::require(names.insert(id.clone()), &format!("one update per id in a batch: {id}"))?;
        if kind == "add" {
            crate::require(item["body"].is_object(), "add body must be a mapping")?;
            let body = item["body"].as_object().unwrap();
            crate::require(!["from", "at", "of", "src", "source", "seen"].iter().any(|k| body.contains_key(*k)),
                "batch citations and snapshots are supplied by the report writer")?;
        }
        for key in ["at", "into"] {
            if item.contains_key(key) { required_text(item, key)?; }
        }
    }
    Ok(Report { raw: value, date, quote, updates, batch })
}

fn private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn save(path: &Path, value: &J) -> Result<()> {
    let mut raw = serde_json::to_vec(value)?;
    raw.push(b'\n');
    F::replace(path, Some(&raw))
}

fn state_root(record: &Path, selected: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = selected { return Ok(std::path::absolute(path)?); }
    let base = std::env::var_os("XDG_STATE_HOME")
        .filter(|p| Path::new(p).is_absolute())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/state")))
        .ok_or_else(|| error("state home is unavailable"))?;
    Ok(base.join("kpopper/ingestion").join(crate::identity::sha256(record.to_string_lossy().as_bytes())))
}

fn prepare_state(record: &Path, selected: Option<&Path>) -> Result<PathBuf> {
    let root = state_root(record, selected)?;
    if record.starts_with(&root) { return Err(error("state directory cannot contain the provenance record")); }
    if root.exists() && !root.is_dir() { return Err(error("state path exists and is not a directory")); }
    private_dir(&root)?;
    let marker = root.join("record.json");
    let expected = json!({"record": record});
    if let Some(raw) = F::read(&marker)? {
        crate::require(crate::json_ingress::parse_slice(&raw, crate::json_ingress::DuplicateKeys::Reject)? == expected,
            "state directory belongs to another provenance record")?;
    } else {
        crate::require(fs::read_dir(&root)?.next().is_none(), "existing non-empty state directory is not owned by ingestion")?;
        save(&marker, &expected)?;
    }
    for name in ["envelopes", "sources", "receipts", "results", "journals", "signals"] { private_dir(&root.join(name))?; }
    Ok(root)
}

fn event_id(record: &Path, report: &Report) -> Result<String> {
    if let Some(value) = report.raw.get("event_id") {
        let supplied = value.as_str().filter(|s| !s.trim().is_empty())
            .ok_or_else(|| error("event_id must be non-empty when supplied"))?;
        Ok(crate::identity::sha256(format!("{}\0{supplied}", record.display()).as_bytes())[..32].into())
    } else {
        crate::public_history::fresh_id("report")
    }
}

fn source_collection(entry: &Path, report: &Report) -> Result<String> {
    let mut inventory = Inventory::default();
    let document = crate::source_document::load(&[entry.to_owned()], &mut inventory, false)?;
    crate::require(document.members == [entry], "multi-file and pointer records require primary review")?;
    crate::require(map(&document.hypotheses)?.is_empty(), "a record with hypothesis context requires primary review")?;
    let projected = document.source.projected();
    let collections = crate::reasoning_fields::collections(&projected)?;
    let is_source = |body: &V| map(body).is_ok_and(|body| {
        !["v", "quoted", "rule"].iter().any(|k| body.contains_key(*k))
            && ["asked", "file", "url", "of", "read"].iter().any(|k| body.get(*k).is_some_and(crate::history_view::truth))
    });
    if let Some(source) = report.raw.get("source").and_then(J::as_str) {
        for (name, members) in &collections {
            if let Some(body) = members.get(source) {
                crate::require(is_source(body), "explicit source is not a recorded source")?;
                return Ok(name.clone());
            }
        }
        return Err(error("explicit source is not a recorded source"));
    }
    let candidates = collections.iter().filter(|(_, members)| members.values().any(is_source))
        .map(|(name, _)| name.clone()).collect::<Vec<_>>();
    crate::require(candidates.len() == 1, "the batch needs one unambiguous existing source collection")?;
    Ok(candidates[0].clone())
}

fn actions(report: &Report, event: &str, source_file: &Path, collection: &str) -> Result<Vec<V>> {
    let source_id = format!("s.ingest_{event}");
    let cited = report.raw.get("source").and_then(J::as_str).unwrap_or(&source_id);
    crate::require(report.raw.get("source").and_then(J::as_str) != Some(source_id.as_str()),
        "a report cannot cite its own capture as an existing source")?;
    let ids = report.updates.iter().map(|u| u["id"].as_str().unwrap()).collect::<Vec<_>>();
    let mut body = JsonMap::from_iter([
        ("name".into(), json!("Captured report")),
        ("file".into(), json!(source_file)),
        ("read".into(), json!(report.date)),
        ("recorded_for".into(), json!(format!("Update {} from this captured report.", ids.join(", ")))),
    ]);
    if let Some(source) = report.raw.get("source") { body.insert("from".into(), source.clone()); }
    if let Some(at) = report.raw.get("at") { body.insert("at".into(), at.clone()); }
    let mut planned = vec![json!({"kind":"add","id":source_id,"body":body,"as_of":report.date,"into":collection})];
    let mut ordered = report.updates.clone();
    ordered.sort_by_key(|u| u["kind"] != "set");
    for mut update in ordered {
        let item = update.as_object_mut().unwrap();
        item.insert("as_of".into(), json!(report.date));
        item.insert("why".into(), report.raw.get("reason").cloned().unwrap_or(J::Null));
        let location = item.remove("at").or_else(|| report.raw.get("at").cloned())
            .unwrap_or_else(|| json!("entire captured report"));
        if item["kind"] == "set" {
            item.insert("source".into(), json!(cited));
            item.insert("at".into(), location);
        } else {
            let body = item.get_mut("body").unwrap().as_object_mut().unwrap();
            if body.contains_key("v") || body.contains_key("quoted") {
                body.insert("from".into(), json!(cited));
                body.insert("at".into(), location);
                body.insert("of".into(), json!(report.date));
            }
        }
        planned.push(update);
    }
    planned.iter().map(V::from_json).collect()
}

fn graph(raw: &[u8], seeds: &[String]) -> Result<J> {
    let source = crate::history_yaml::decode_ordinary_source_value(raw)?;
    let document = source.projected();
    let runtime = crate::public_workspace::runtime_for_document(&document)?;
    let projection = crate::public_ordinary_readers::Projection::new(
        &document, &BTreeMap::new(), &BTreeMap::new(), vec![], runtime.as_ref(),
    )?;
    let world = &projection.base;
    let (mut hit, touched, derived) = world.write_reach(seeds)?;
    for seed in seeds {
        if world.judgments.contains_key(seed) && !hit.iter().any(|(id, _)| id == seed) {
            hit.push((seed.clone(), seed.clone()));
        }
    }
    let via = hit.iter().cloned().collect::<BTreeMap<_, _>>();
    let mut judgments = JsonMap::new();
    for (id, body) in &world.judgments {
        let (tag, reason) = world.state_with_touched(id, &touched)?;
        let pred = crate::ordinary_reader::predicate_of(body, world.reader.fields());
        judgments.insert(id.clone(), json!({
            "tag":tag, "reason":reason, "evaluation":world.reader.predicate(&pred)?,
            "verdict":map(body).ok().and_then(|b| b.get("verdict")).map(V::to_json).transpose()?
        }));
    }
    let profile = crate::reasoning_fields::capabilities(&document, None)?.to_json()?["profile"].clone();
    Ok(json!({
        "hash":crate::identity::sha256(raw), "assessment_profile":profile,
        "judgments":judgments,
        "reach":{"judgments":via.keys().cloned().collect::<Vec<_>>(),"via":via,
                 "touched":touched,"derived":derived}
    }))
}

fn mutation_graphs(mutation: &crate::history_transaction::PreparedMutation, report: &Report) -> Result<(J, J)> {
    let image = mutation.files().iter().find(|f| f.role == "record")
        .ok_or_else(|| error("report mutation has no record image"))?;
    let before = image.before.as_deref().ok_or_else(|| error("report mutation has no record preimage"))?;
    let after = image.after.as_deref().ok_or_else(|| error("report mutation has no record result"))?;
    let seeds = report.updates.iter().map(|v| v["id"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
    Ok((graph(before, &seeds)?, graph(after, &seeds)?))
}

fn classification(before: &J, after: &J) -> (Vec<String>, Vec<String>) {
    let mut fired = Vec::new();
    let mut actionable = Vec::new();
    for id in after["reach"]["judgments"].as_array().into_iter().flatten().filter_map(J::as_str) {
        let old = &before["judgments"][id];
        let now = &after["judgments"][id];
        if old["evaluation"] != true && now["evaluation"] == true {
            fired.push(id.to_owned());
        } else if ["MOVED", "UNCHECKED", "BROKEN", "BLOCKED", "UNKNOWN"].contains(&now["tag"].as_str().unwrap_or(""))
            && (old["tag"] != now["tag"] || old["reason"] != now["reason"])
        {
            actionable.push(id.to_owned());
        }
    }
    (fired, actionable)
}

fn signal_id(event: &str, category: &str, names: &[String]) -> String {
    crate::identity::sha256(format!("{event}\0{category}\0{}", names.join("\0")).as_bytes())[..32].into()
}

fn receipt(report: &Report, record: &Path, root: &Path, event: &str, source: &Path,
           envelope_sha: &str, state: &str, reason: Option<&str>, mutation: Option<&crate::history_transaction::PreparedMutation>) -> Result<(J, Vec<J>)> {
    let source_id = format!("s.ingest_{event}");
    let (before, after, fired, actionable) = if let Some(mutation) = mutation {
        let (before, after) = mutation_graphs(mutation, report)?;
        let (fired, actionable) = classification(&before, &after);
        (Some(before), Some(after), fired, actionable)
    } else { (None, None, vec![], vec![]) };
    let mut signals = Vec::new();
    if let Some(after) = &after {
        if !fired.is_empty() {
            signals.push(json!({
                "id":signal_id(event,"contradiction",&fired), "event_id":event,
                "category":"contradiction", "target":J::Null, "source_quote":report.quote,
                "affected_judgments":after["reach"]["judgments"], "newly_fired_judgments":fired,
                "actionable_judgments":[], "reason":fired.iter().filter_map(|id| after["judgments"][id]["reason"].as_str()).collect::<Vec<_>>().join("; ")
            }));
        }
        if !actionable.is_empty() {
            let question = actionable.iter().map(|id| format!("{id} requires review: {}", after["judgments"][id]["reason"].as_str().unwrap_or(""))).collect::<Vec<_>>().join("; ");
            signals.push(json!({
                "id":signal_id(event,"question",&actionable), "event_id":event,
                "category":"question", "target":J::Null, "source_quote":report.quote,
                "affected_judgments":after["reach"]["judgments"], "newly_fired_judgments":[],
                "actionable_judgments":actionable, "reason":question, "question":question
            }));
        }
    }
    let mut value = json!({
        "record": record, "state_dir": root,
        "id": &crate::identity::sha256(format!("receipt\0{event}").as_bytes())[..32],
        "event_id": event, "state": state, "target": report.raw.get("target").cloned().unwrap_or(J::Null),
        "value": report.raw.get("value").cloned().unwrap_or(J::Null),
        "source": if state == "applied" { J::String(source_id) } else { J::Null },
        "source_file": source, "reason": reason, "source_sha256": crate::identity::sha256(report.quote.as_bytes()),
        "envelope_sha256": envelope_sha, "signal_ids": signals.iter().map(|v| v["id"].clone()).collect::<Vec<_>>(),
        "reach": after.as_ref().map(|v| v["reach"].clone()).unwrap_or_else(||json!({})),
        "newly_fired_judgments": fired, "actionable_judgments": actionable,
        "recovered": false, "diagnostics": []
    });
    if report.batch { value["updates"] = J::Array(report.updates.clone()); }
    if let Some(cited) = report.raw.get("source") {
        value["cited_source"] = if state == "applied" { cited.clone() } else { J::Null };
        value["date"] = json!(report.date);
    }
    if let Some(at) = report.raw.get("at") { value["at"] = at.clone(); }
    if let Some(mutation) = mutation {
        let data = mutation.to_data().to_json()?;
        value["mutation"] = json!({"operation":data["operation"],"digest":data["digest"],"receipt":data["receipt"]});
        value["graph_before"] = before.unwrap()["hash"].clone();
        value["graph_after"] = after.unwrap()["hash"].clone();
    }
    Ok((value, signals))
}

fn retained_journal(mutation: &crate::history_transaction::PreparedMutation, history: bool, phase: &str) -> Result<J> {
    Ok(json!({
        "version": 1, "kind": "native-source-report/v1", "history": history, "phase": phase,
        "mutation": base64::engine::general_purpose::STANDARD.encode(mutation.to_bytes()?)
    }))
}

fn journal_mutation(value: &J) -> Result<crate::history_transaction::PreparedMutation> {
    crate::require(value["version"] == 1 && value["kind"] == "native-source-report/v1",
        "invalid report journal")?;
    let encoded = value["mutation"].as_str().ok_or_else(|| error("invalid report journal"))?;
    let raw = base64::engine::general_purpose::STANDARD.decode(encoded)
        .map_err(|_| error("invalid report journal"))?;
    crate::history_transaction::PreparedMutation::from_bytes(&raw)
}

pub fn run(options: &Options, cwd: &Path, stdin: Option<&[u8]>) -> Result<Output> {
    let raw = if options.file == "-" {
        stdin.ok_or_else(|| error("missing standard input"))?.to_vec()
    } else {
        fs::read(cwd.join(&options.file))?
    };
    let report = parse(&raw)?;
    let original = if let Some(record) = &options.record { vec![cwd.join(record)] }
        else { crate::public_workspace::records(cwd)? };
    let route = WriteRoute::capture(&original, cwd)?;
    crate::require(route.paths().len() == 1, "ingestion requires one record path")?;
    let record = route.paths()[0].clone();
    let root = prepare_state(&record, options.state_dir.as_deref())?;
    let event = event_id(&record, &report)?;
    let envelope_path = root.join("envelopes").join(format!("{event}.json"));
    let source_path = root.join("sources").join(format!("{event}.txt"));
    let receipt_path = root.join("receipts").join(format!("{event}.json"));
    let journal_path = root.join("journals").join(format!("{event}.json"));
    let canonical = { let mut b = serde_json::to_vec(&report.raw)?; b.push(b'\n'); b };
    let envelope_sha = crate::identity::sha256(&canonical);
    let _record_lock = F::DirectoryGuard::acquire(record.parent().unwrap(), true)?;
    let _state_lock = F::DirectoryGuard::acquire(&root, true)?;
    if let Some(existing) = F::read(&envelope_path)? {
        crate::require(existing == canonical, "event_id was reused for different input")?;
        if let Some(receipt) = F::read(&receipt_path)? {
            let value: J = crate::json_ingress::parse_slice(&receipt, crate::json_ingress::DuplicateKeys::Reject)?;
            let code = if value["state"] == "applied" { 0 } else { 1 };
            return Ok(Output { text: String::from_utf8(receipt).map_err(|_| error("invalid receipt"))?, code });
        }
    } else {
        F::replace(&source_path, Some(report.quote.as_bytes()))?;
        F::replace(&envelope_path, Some(&canonical))?;
    }

    if let Some(raw) = F::read(&journal_path)? {
        let mut journal = crate::json_ingress::parse_slice(&raw, crate::json_ingress::DuplicateKeys::Reject)?;
        let mutation = journal_mutation(&journal)?;
        let history = journal["history"].as_bool().ok_or_else(|| error("invalid report journal"))?;
        if history {
            if journal["phase"] != "committed" {
                crate::direct_history::recover(&original, cwd, false)?;
            }
        } else if crate::legacy_authoring::recovery_pending(&original, cwd)? {
            crate::legacy_authoring::recover(&original, cwd, false)?;
        } else {
            crate::require(journal["phase"] == "committed", "unfinished report lost its recovery journal")?;
        }
        journal["phase"] = json!("committed");
        save(&journal_path, &journal)?;
        let (answer, signals) = receipt(&report, &record, &root, &event, &source_path, &envelope_sha,
            "applied", None, Some(&mutation))?;
        for signal in &signals {
            save(&root.join("signals").join(format!("{}.json", signal["id"].as_str().unwrap())), signal)?;
        }
        save(&receipt_path, &answer)?;
        save(&root.join("results").join(format!("{event}.json")), &json!({"receipt":answer,"signals":signals}))?;
        return Ok(Output { text: format!("{}\n", serde_json::to_string(&answer)?), code: 0 });
    }

    let outcome = (|| {
        if report.raw.get("privacy").is_some_and(|v| match v {
            J::Null | J::Bool(false) => false,
            J::String(s) => !s.is_empty(),
            _ => true,
        }) {
            return Err(error("private or unclear original source permission; report retained privately"));
        }
        if report.raw.get("scope").and_then(|v| v.get("kind")).and_then(J::as_str) == Some("unclear") {
            return Err(error("unclear report scope; retained privately"));
        }
        let needs_hash = report.raw.get("source").is_some() || report.updates.iter().any(|u| u["kind"] == "add");
        if needs_hash {
            let expected = report.raw.get("record_sha256").and_then(J::as_str)
                .ok_or_else(|| error("new entries and existing source citations require record_sha256 from the primary's prior read"))?;
            let current = F::read(&record)?.ok_or_else(|| error("record not found"))?;
            crate::require(crate::identity::sha256(&current) == expected,
                "record changed since the primary read it; reread the premises before resubmitting")?;
        }
        let collection = source_collection(&record, &report)?;
        route.verify()?;
        let authority = crate::legacy_authoring::route(&record, route.config())?;
        let context = V::from_json(&json!({
            "kind":"source-report/v1", "event_id":event,
            "source_sha256":crate::identity::sha256(report.quote.as_bytes()),
            "envelope_sha256":envelope_sha, "record":record, "state_dir":root,
            "policy":route.config().to_json()?
        }))?;
        if authority == crate::legacy_authoring::AuthorityRoute::Legacy {
            let planned = actions(&report, &event, &source_path, &collection)?;
            let prepared = legacy_batch::prepare(&planned, &route, &legacy_batch::Options {
                operation: format!("report-{event}"), context,
            })?;
            let mutation = prepared.mutation.clone();
            save(&journal_path, &retained_journal(&mutation, false, "prepared")?)?;
            let mut committed = |_: &V| {
                save(&journal_path, &retained_journal(&mutation, false, "committed")?)
            };
            crate::legacy_authoring::publish_prepared_with_committed(prepared, &route, &mut committed)?;
            return Ok(mutation);
        }
        let entry_name = record.file_name().and_then(|v| v.to_str()).ok_or_else(|| error("invalid_path"))?;
        let layout = crate::history_transaction::Layout::for_entry(entry_name)?;
        let portable = if layout.home.is_empty() {
            format!("evidence/reports/{event}.txt")
        } else {
            format!("{}/evidence/reports/{event}.txt", layout.home)
        };
        let planned = actions(&report, &event, Path::new(&portable), &collection)?;
        let store = crate::history_store::Store::new(&record)?;
        let captured = store.capture()?;
        let document = crate::history_adapter::from_store_capture(&captured)?.document().clone();
        crate::require(!crate::recording_privacy::private_marker(&document),
            "private or unclear source permission in history report")?;
        let runtime = crate::public_workspace::runtime_for_document(&document)?;
        let now = chrono::Utc::now();
        let write = crate::history_authoring::Options {
            operation: format!("report-{event}"),
            recorded_at: now.to_rfc3339(),
            recording_day: report.date.clone(),
            by: V::Null,
            strict: true,
            paths: crate::history_paths::Scheme::Hashed,
            receipt_version: None,
        };
        let batch = crate::history_authoring_batch::BatchOptions {
            authoring: write,
            receipt_version: 8,
            context,
            evidence: std::collections::BTreeMap::from([(portable, report.quote.as_bytes().to_vec())]),
        };
        let mutation = crate::history_authoring_batch::prepare_batch(
            &store, &captured, &planned, &batch, runtime.as_ref(),
        )?;
        let authored = V::List(mutation.files().iter().filter(|f| f.role == "history_object")
            .map(|f| crate::history_yaml::decode_document(f.after.as_ref().unwrap())).collect::<Result<Vec<_>>>()?);
        crate::require(!crate::recording_privacy::private_marker(&authored),
            "private or unclear prepared source permission")?;
        save(&journal_path, &retained_journal(&mutation, true, "prepared")?)?;
        crate::direct_history::publish_prepared(
            &store, &mutation, &route, &original, runtime.as_ref(), &mut |phase| {
                if phase == "committed" {
                    save(&journal_path, &retained_journal(&mutation, true, "committed")?)?;
                }
                Ok(())
            },
        )?;
        Ok(mutation)
    })();
    let (state, reason, mutation, code) = match outcome {
        Ok(mutation) => ("applied", None, Some(mutation), 0),
        Err(e) => ("needs_primary", Some(e.to_string()), None, 1),
    };
    let (receipt, signals) = receipt(&report, &record, &root, &event, &source_path, &envelope_sha,
        state, reason.as_deref(), mutation.as_ref())?;
    for signal in &signals {
        save(&root.join("signals").join(format!("{}.json", signal["id"].as_str().unwrap())), signal)?;
    }
    save(&receipt_path, &receipt)?;
    save(&root.join("results").join(format!("{event}.json")), &json!({"receipt":receipt,"signals":signals}))?;
    Ok(Output { text: format!("{}\n", serde_json::to_string(&receipt)?), code })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parser_rejects_duplicate_ids_and_accepts_dependent_actions() {
        let report = parse(br#"{"date":"2026-09-20","source_quote":"x","updates":[{"kind":"add","id":"p.x","body":{"v":1}},{"kind":"add","id":"c.x","body":{"rests_on":["p.x"],"verdict":"x","wrong_if":"p.x > 1"}}]}"#).unwrap();
        assert_eq!(report.updates.len(), 2);
        assert!(parse(br#"{"date":"2026-09-20","source_quote":"x","updates":[{"kind":"set","id":"p.x","value":1},{"kind":"set","id":"p.x","value":2}]}"#).is_err());
    }
}
