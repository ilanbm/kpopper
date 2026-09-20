//! Native synchronous application of one explicit source report.
use crate::{
    Result,
    history_contract::{error, map},
    history_transaction_fs as F,
    history_yaml::SourceValue as Source,
    legacy_batch,
    project_modes::WriteRoute,
    source_inventory::Inventory,
    value::TypedValue as V,
};
use base64::Engine as _;
use clap::Args;
use serde_json::{Map as JsonMap, Value as J, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

const FIELDS: &[&str] = &[
    "event_id",
    "session_id",
    "source_quote",
    "target",
    "value",
    "date",
    "kind",
    "question",
    "reason",
    "updates",
    "record_sha256",
    "shareability",
    "privacy",
    "scope",
    "source",
    "at",
    "profile",
    "disclosed_locators",
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

#[derive(Debug)]
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
    let top = value
        .as_object()
        .ok_or_else(|| error("capture envelope must be a JSON object"))?;
    let unknown = top
        .keys()
        .filter(|k| !FIELDS.contains(&k.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    crate::require(
        unknown.is_empty(),
        &format!("unknown envelope fields: {}", unknown.join(", ")),
    )?;
    let quote = required_text(top, "source_quote")?;
    let date = required_text(top, "date")?;
    crate::value::Date::new(&date)?;
    if let Some(profile) = top.get("profile").filter(|v| !v.is_null()) {
        crate::require(profile == "core/v1", "unsupported writer profile")?;
    }
    for key in ["source", "at"] {
        if top.contains_key(key) {
            required_text(top, key)?;
        }
    }
    if let Some(hash) = top.get("record_sha256").filter(|v| !v.is_null()) {
        let hash = hash.as_str().unwrap_or("");
        crate::require(
            hash.len() == 64
                && hash
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            "record_sha256 must be the hash returned by the prior record read",
        )?;
    }
    let batch = top.contains_key("updates");
    let updates = if let Some(updates) = top.get("updates").and_then(J::as_array) {
        crate::require(
            (1..=32).contains(&updates.len()),
            "updates must contain 1..32 operations",
        )?;
        crate::require(
            !top.contains_key("target") && !top.contains_key("value"),
            "use updates or target/value, not both",
        )?;
        updates.clone()
    } else {
        let target = required_text(top, "target")?;
        let value = top
            .get("value")
            .ok_or_else(|| error("the report needs target and value"))?;
        vec![json!({"kind":"set","id":target,"value":value})]
    };
    let mut names = BTreeSet::new();
    for update in &updates {
        let item = update
            .as_object()
            .ok_or_else(|| error("each update needs kind=set or kind=add"))?;
        let kind = item.get("kind").and_then(J::as_str).unwrap_or("");
        crate::require(
            ["set", "add"].contains(&kind),
            "each update needs kind=set or kind=add",
        )?;
        let allowed: BTreeSet<&str> = if kind == "set" {
            ["kind", "id", "at", "value"].into_iter().collect()
        } else {
            ["kind", "id", "at", "body", "into", "drops"]
                .into_iter()
                .collect()
        };
        crate::require(
            item.keys().all(|k| allowed.contains(k.as_str())),
            &format!("invalid fields in {kind} update"),
        )?;
        crate::require(
            item.contains_key(if kind == "set" { "value" } else { "body" }),
            &format!("invalid fields in {kind} update"),
        )?;
        let id = required_text(item, "id")?;
        crate::history_paths::subject(&id)
            .map_err(|_| error("each update needs a valid entry id"))?;
        crate::require(
            names.insert(id.clone()),
            &format!("one update per id in a batch: {id}"),
        )?;
        if kind == "add" {
            crate::require(item["body"].is_object(), "add body must be a mapping")?;
            let body = item["body"].as_object().unwrap();
            crate::require(
                !["from", "at", "of", "src", "source", "seen"]
                    .iter()
                    .any(|k| body.contains_key(*k)),
                "batch citations and snapshots are supplied by the report writer",
            )?;
        }
        for key in ["at", "into"] {
            if item.contains_key(key) {
                required_text(item, key)?;
            }
        }
    }
    Ok(Report {
        raw: value,
        date,
        quote,
        updates,
        batch,
    })
}

fn private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
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
    if let Some(path) = selected {
        return crate::project_modes::resolved(path);
    }
    let base = std::env::var_os("XDG_STATE_HOME")
        .filter(|p| Path::new(p).is_absolute())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/state")))
        .ok_or_else(|| error("state home is unavailable"))?;
    Ok(base
        .join("kpopper/ingestion")
        .join(crate::identity::sha256(record.to_string_lossy().as_bytes())))
}

fn prepare_state(record: &Path, selected: Option<&Path>) -> Result<PathBuf> {
    let root = state_root(record, selected)?;
    if record.starts_with(&root) {
        return Err(error(
            "state directory cannot contain the provenance record",
        ));
    }
    if root.exists() && !root.is_dir() {
        return Err(error("state path exists and is not a directory"));
    }
    private_dir(&root)?;
    let marker = root.join("record.json");
    let expected = json!({"record": record});
    if let Some(raw) = F::read(&marker)? {
        crate::require(
            crate::json_ingress::parse_slice(&raw, crate::json_ingress::DuplicateKeys::Reject)?
                == expected,
            "state directory belongs to another provenance record",
        )?;
    } else {
        crate::require(
            fs::read_dir(&root)?.next().is_none(),
            "existing non-empty state directory is not owned by ingestion",
        )?;
        save(&marker, &expected)?;
    }
    for name in [
        "envelopes",
        "sources",
        "receipts",
        "results",
        "journals",
        "signals",
    ] {
        private_dir(&root.join(name))?;
    }
    Ok(root)
}

fn event_id(record: &Path, report: &Report) -> Result<String> {
    if let Some(value) = report.raw.get("event_id") {
        let supplied = value
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| error("event_id must be non-empty when supplied"))?;
        Ok(
            crate::identity::sha256(format!("{}\0{supplied}", record.display()).as_bytes())[..32]
                .into(),
        )
    } else {
        crate::public_history::fresh_id("report")
    }
}

fn source_collection(document: &V, report: &Report) -> Result<String> {
    let collections = crate::reasoning_fields::collections(document)?;
    let is_source = |body: &V| {
        map(body).is_ok_and(|body| {
            !["v", "quoted", "rule"]
                .iter()
                .any(|k| body.contains_key(*k))
                && ["asked", "file", "url", "of", "read"]
                    .iter()
                    .any(|k| body.get(*k).is_some_and(crate::history_view::truth))
        })
    };
    if let Some(source) = report.raw.get("source").and_then(J::as_str) {
        for (name, members) in &collections {
            if let Some(body) = members.get(source) {
                crate::require(is_source(body), "explicit source is not a recorded source")?;
                return Ok(name.clone());
            }
        }
        return Err(error("explicit source is not a recorded source"));
    }
    let candidates = collections
        .iter()
        .filter(|(_, members)| members.values().any(is_source))
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    crate::require(
        candidates.len() == 1,
        "the batch needs one unambiguous existing source collection",
    )?;
    Ok(candidates[0].clone())
}

fn actions(
    report: &Report,
    event: &str,
    source_file: &Path,
    collection: &str,
) -> Result<(Vec<V>, Vec<Option<Source>>)> {
    let source_id = format!("s.ingest_{event}");
    let cited = report
        .raw
        .get("source")
        .and_then(J::as_str)
        .unwrap_or(&source_id);
    crate::require(
        report.raw.get("source").and_then(J::as_str) != Some(source_id.as_str()),
        "a report cannot cite its own capture as an existing source",
    )?;
    let ids = report
        .updates
        .iter()
        .map(|u| u["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    let mut body = JsonMap::from_iter([
        ("name".into(), json!("Captured report")),
        ("file".into(), json!(source_file)),
        ("read".into(), json!(report.date)),
        (
            "recorded_for".into(),
            json!(format!(
                "Update {} from this captured report.",
                ids.join(", ")
            )),
        ),
    ]);
    if let Some(source) = report.raw.get("source") {
        body.insert("from".into(), source.clone());
    }
    if let Some(at) = report.raw.get("at") {
        body.insert("at".into(), at.clone());
    }
    let ordered_source = Source::Map(
        ["name", "file", "read", "recorded_for", "from", "at"]
            .into_iter()
            .filter_map(|key| {
                body.get(key).map(|value| {
                    V::from_json(value).map(|value| (key.into(), Source::from_typed(&value)))
                })
            })
            .collect::<Result<Vec<_>>>()?,
    );
    let mut planned = vec![
        json!({"kind":"add","id":source_id,"body":body,"as_of":report.date,"into":collection}),
    ];
    let mut source_bodies = vec![Some(ordered_source)];
    let mut ordered = report.updates.clone();
    ordered.sort_by_key(|u| u["kind"] != "set");
    for mut update in ordered {
        let item = update.as_object_mut().unwrap();
        item.insert("as_of".into(), json!(report.date));
        item.insert(
            "why".into(),
            report.raw.get("reason").cloned().unwrap_or(J::Null),
        );
        let location = item
            .remove("at")
            .or_else(|| report.raw.get("at").cloned())
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
        source_bodies.push(None);
    }
    Ok((
        planned
            .iter()
            .map(V::from_json)
            .collect::<Result<Vec<_>>>()?,
        source_bodies,
    ))
}

fn graph(
    raw: &[u8],
    seeds: &[String],
    supplied_runtime: Option<&crate::reasoning_runtime::Runtime>,
) -> Result<J> {
    let source = crate::history_yaml::decode_ordinary_source_value(raw)?;
    let document = source.projected();
    graph_document(raw, &document, seeds, supplied_runtime)
}

fn graph_document(
    raw: &[u8],
    document: &V,
    seeds: &[String],
    supplied_runtime: Option<&crate::reasoning_runtime::Runtime>,
) -> Result<J> {
    let capabilities = crate::reasoning_fields::capabilities(document, None)?;
    if map(&capabilities)?.get("profile") == Some(&V::Text("core/v1".into())) {
        return core_graph(raw, document, seeds, supplied_runtime);
    }
    let owned_runtime = if supplied_runtime.is_none() {
        crate::public_workspace::runtime_for_document(document)?
    } else {
        None
    };
    let runtime = supplied_runtime.or(owned_runtime.as_ref());
    let projection = crate::public_ordinary_readers::Projection::new(
        document,
        &BTreeMap::new(),
        &BTreeMap::new(),
        vec![],
        runtime,
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
        judgments.insert(
            id.clone(),
            json!({
                "tag":tag, "reason":reason, "evaluation":world.reader.predicate(&pred)?,
                "verdict":map(body).ok().and_then(|b| b.get("verdict")).map(V::to_json).transpose()?
            }),
        );
    }
    let profile = capabilities.to_json()?["profile"].clone();
    Ok(json!({
        "hash":crate::identity::sha256(raw), "assessment_profile":profile,
        "judgments":judgments,
        "reach":{"judgments":via.keys().cloned().collect::<Vec<_>>(),"via":via,
                 "touched":touched,"derived":derived}
    }))
}

type Reach = (BTreeMap<String, String>, BTreeSet<String>, Vec<String>);

fn reach(document: &V, seeds: &[String]) -> Result<Reach> {
    let entries = crate::reasoning_snapshot::entries(document)?;
    let fields = crate::reasoning_fields::snapshot_fields(document)?;
    let deps = fields["deps"].to_json()?.as_str().unwrap().to_owned();
    let mut feeds = BTreeMap::<String, Vec<String>>::new();
    let mut judgments = BTreeMap::<String, Vec<String>>::new();
    for (id, (_, body)) in &entries {
        let Ok(body) = map(body) else { continue };
        if let Some(V::List(values)) = body.get(&deps) {
            for dep in values.iter().filter_map(|v| match v {
                V::Text(s) => Some(s),
                _ => None,
            }) {
                judgments.entry(dep.clone()).or_default().push(id.clone());
            }
        } else if let Some(rule) = body.get("rule") {
            for dep in crate::reasoning_language::legacy_references(rule) {
                if entries.contains_key(&dep) && dep != *id {
                    feeds.entry(dep).or_default().push(id.clone());
                }
            }
        }
    }
    let mut via = BTreeMap::new();
    let mut moved = seeds.iter().cloned().collect::<BTreeSet<_>>();
    let mut derived = Vec::new();
    let mut frontier = seeds.to_vec();
    while let Some(source) = frontier.pop() {
        for id in feeds.get(&source).into_iter().flatten() {
            if moved.insert(id.clone()) {
                derived.push(id.clone());
                frontier.push(id.clone());
            }
        }
        for id in judgments.get(&source).into_iter().flatten() {
            if !via.contains_key(id) {
                via.insert(id.clone(), source.clone());
                frontier.push(id.clone());
            }
        }
    }
    for seed in seeds {
        if entries
            .get(seed)
            .is_some_and(|(_, body)| map(body).is_ok_and(|b| b.contains_key(&deps)))
        {
            via.entry(seed.clone()).or_insert_with(|| seed.clone());
        }
    }
    moved.extend(via.keys().cloned());
    Ok((via, moved, derived))
}

fn core_graph(
    raw: &[u8],
    document: &V,
    seeds: &[String],
    runtime: Option<&crate::reasoning_runtime::Runtime>,
) -> Result<J> {
    let mut world = crate::reasoning_authoring::World::new(
        document,
        None,
        runtime,
        crate::reasoning_runtime::OperationalBounds::default(),
    )?;
    let report = world.assessment()?;
    let fields = crate::reasoning_fields::snapshot_fields(document)?;
    let deps = fields["deps"].to_json()?.as_str().unwrap().to_owned();
    let entries = crate::reasoning_snapshot::entries(document)?;
    let (via, touched, derived) = reach(document, seeds)?;
    let mut judgments = JsonMap::new();
    for (id, (_, body)) in &entries {
        if !map(body).is_ok_and(|b| b.contains_key(&deps)) {
            continue;
        }
        let state = world.state(id)?;
        let V::List(state) = state else {
            return Err(error("invalid core state"));
        };
        let tag = state[0].to_json()?;
        let reason = state[1].to_json()?;
        let status = map(&map(&map(&report)?["nodes"])?[id])?["state"].clone();
        let evaluation = match map(&map(&status)?["falsifier"])?["status"]
            .to_json()?
            .as_str()
        {
            Some("holds") => J::Bool(true),
            Some("does_not_hold") => J::Bool(false),
            _ => J::Null,
        };
        judgments.insert(
            id.clone(),
            json!({"tag":tag,"reason":reason,"evaluation":evaluation,
            "verdict":map(body)?.get("verdict").map(V::to_json).transpose()?}),
        );
    }
    Ok(
        json!({"hash":crate::identity::sha256(raw),"assessment_profile":"core/v1",
        "judgments":judgments,"reach":{"judgments":via.keys().cloned().collect::<Vec<_>>(),
        "via":via,"touched":touched,"derived":derived}}),
    )
}

fn mutation_graphs(
    mutation: &crate::history_transaction::PreparedMutation,
    report: &Report,
    runtime: Option<&crate::reasoning_runtime::Runtime>,
) -> Result<(J, J)> {
    let image = mutation
        .files()
        .iter()
        .find(|f| f.role == "record")
        .ok_or_else(|| error("report mutation has no record image"))?;
    let before = image
        .before
        .as_deref()
        .ok_or_else(|| error("report mutation has no record preimage"))?;
    let after = image
        .after
        .as_deref()
        .ok_or_else(|| error("report mutation has no record result"))?;
    let seeds = report
        .updates
        .iter()
        .map(|v| v["id"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    Ok((
        graph(before, &seeds, runtime)?,
        graph(after, &seeds, runtime)?,
    ))
}

fn verify_requested_profile(
    report: &Report,
    mutation: &crate::history_transaction::PreparedMutation,
    runtime: Option<&crate::reasoning_runtime::Runtime>,
) -> Result<()> {
    if let Some(requested) = report.raw.get("profile").and_then(J::as_str) {
        let (_, after) = mutation_graphs(mutation, report, runtime)?;
        crate::require(
            after["assessment_profile"] == requested,
            "requested writer profile requires explicit record migration",
        )?;
    }
    Ok(())
}

fn target_after_sha256(
    mutation: &crate::history_transaction::PreparedMutation,
    report: &Report,
) -> Result<String> {
    let raw = mutation
        .files()
        .iter()
        .find(|f| f.role == "record")
        .and_then(|f| f.after.as_deref())
        .ok_or_else(|| error("report mutation has no record result"))?;
    let document = crate::history_yaml::decode_ordinary_source_value(raw)?.projected();
    let entries = crate::reasoning_snapshot::entries(&document)?;
    let value = if report.batch {
        V::Map(
            report
                .updates
                .iter()
                .map(|item| {
                    let id = item["id"].as_str().unwrap();
                    (
                        id.to_owned(),
                        entries
                            .get(id)
                            .map(|(_, body)| body.clone())
                            .unwrap_or(V::Null),
                    )
                })
                .collect(),
        )
    } else {
        entries
            .get(report.updates[0]["id"].as_str().unwrap())
            .map(|(_, body)| body.clone())
            .unwrap_or(V::Null)
    };
    let ordinary = crate::history_yaml::OrdinaryValue::from_typed(&value);
    Ok(crate::identity::sha256(
        &crate::public_ordinary_readers::python_safe_dump_unicode(&ordinary)?,
    ))
}

fn classification(before: &J, after: &J) -> (Vec<String>, Vec<String>) {
    let mut fired = Vec::new();
    let mut actionable = Vec::new();
    for id in after["reach"]["judgments"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(J::as_str)
    {
        let old = &before["judgments"][id];
        let now = &after["judgments"][id];
        if old["evaluation"] != true && now["evaluation"] == true {
            fired.push(id.to_owned());
        } else if ["MOVED", "UNCHECKED", "BROKEN", "BLOCKED", "UNKNOWN"]
            .contains(&now["tag"].as_str().unwrap_or(""))
            && (old["tag"] != now["tag"] || old["reason"] != now["reason"])
        {
            actionable.push(id.to_owned());
        }
    }
    (fired, actionable)
}

fn signal_id(event: &str, category: &str, names: &[String]) -> String {
    crate::identity::sha256(format!("{event}\0{category}\0{}", names.join("\0")).as_bytes())[..32]
        .into()
}

#[allow(clippy::too_many_arguments)]
fn receipt(
    report: &Report,
    record: &Path,
    root: &Path,
    event: &str,
    source: &Path,
    envelope_sha: &str,
    state: &str,
    reason: Option<&str>,
    recovered: bool,
    mutation: Option<&crate::history_transaction::PreparedMutation>,
    supplied_graphs: Option<&(J, J)>,
    runtime: Option<&crate::reasoning_runtime::Runtime>,
) -> Result<(J, Vec<J>)> {
    let source_id = format!("s.ingest_{event}");
    let diagnostics = mutation
        .and_then(|mutation| {
            mutation.to_data().to_json().ok().and_then(|v| {
                v["receipt"]["after"]["batch"]["diagnostics"]
                    .as_array()
                    .cloned()
            })
        })
        .unwrap_or_default();
    let (before, after, fired, actionable) = if let Some(mutation) = mutation {
        let (before, after) = supplied_graphs
            .cloned()
            .map(Ok)
            .unwrap_or_else(|| mutation_graphs(mutation, report, runtime))?;
        let (fired, actionable) = classification(&before, &after);
        (Some(before), Some(after), fired, actionable)
    } else {
        (None, None, vec![], vec![])
    };
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
            let question = actionable
                .iter()
                .map(|id| {
                    format!(
                        "{id} requires review: {}",
                        after["judgments"][id]["reason"].as_str().unwrap_or("")
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
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
        "recovered": recovered, "diagnostics": diagnostics
    });
    if report.batch {
        value["updates"] = J::Array(report.updates.clone());
    }
    if let Some(cited) = report.raw.get("source") {
        value["cited_source"] = if state == "applied" {
            cited.clone()
        } else {
            J::Null
        };
        value["date"] = json!(report.date);
    }
    if let Some(at) = report.raw.get("at") {
        value["at"] = at.clone();
    }
    if let Some(mutation) = mutation {
        let data = mutation.to_data().to_json()?;
        value["mutation"] = json!({"operation":data["operation"],"digest":data["digest"],"receipt":data["receipt"]});
        value["graph_before"] = before.unwrap()["hash"].clone();
        value["graph_after"] = after.unwrap()["hash"].clone();
        value["target_after_sha256"] = json!(target_after_sha256(mutation, report)?);
    }
    Ok((value, signals))
}

fn retained_journal(
    mutation: &crate::history_transaction::PreparedMutation,
    history: bool,
    phase: &str,
    graphs: Option<&(J, J)>,
) -> Result<J> {
    let mut value = json!({
        "version": 1, "kind": "native-source-report/v1", "history": history, "phase": phase,
        "mutation": base64::engine::general_purpose::STANDARD.encode(mutation.to_bytes()?)
    });
    if let Some((before, after)) = graphs {
        value["graph_before"] = before.clone();
        value["graph_after"] = after.clone();
    }
    Ok(value)
}

fn journal_mutation(value: &J) -> Result<crate::history_transaction::PreparedMutation> {
    crate::require(
        value["version"] == 1 && value["kind"] == "native-source-report/v1",
        "invalid report journal",
    )?;
    let encoded = value["mutation"]
        .as_str()
        .ok_or_else(|| error("invalid report journal"))?;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| error("invalid report journal"))?;
    crate::history_transaction::PreparedMutation::from_bytes(&raw)
}

fn private_reason(report: &Report) -> Result<Option<&'static str>> {
    if crate::recording_privacy::private_marker(&V::from_json(&report.raw)?)
        || report.raw.get("privacy").is_some_and(|v| match v {
            J::Null | J::Bool(false) => false,
            J::String(s) => !s.is_empty(),
            _ => true,
        })
    {
        return Ok(Some(
            "private or unclear original source permission; report retained privately",
        ));
    }
    if report
        .raw
        .get("scope")
        .and_then(|v| v.get("kind"))
        .and_then(J::as_str)
        == Some("unclear")
    {
        return Ok(Some("unclear report scope; retained privately"));
    }
    Ok(None)
}

pub fn run(options: &Options, cwd: &Path, stdin: Option<&[u8]>) -> Result<Output> {
    run_with_probe(options, cwd, stdin, None, &mut |_| Ok(()))
}

fn run_with_probe(
    options: &Options,
    cwd: &Path,
    stdin: Option<&[u8]>,
    supplied_runtime: Option<&crate::reasoning_runtime::Runtime>,
    probe: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<Output> {
    let raw = if options.file == "-" {
        stdin
            .ok_or_else(|| error("missing standard input"))?
            .to_vec()
    } else {
        fs::read(cwd.join(&options.file))?
    };
    let report = parse(&raw)?;
    let original = if let Some(record) = &options.record {
        vec![cwd.join(record)]
    } else {
        crate::public_workspace::records(cwd)?
    };
    let route = WriteRoute::capture(&original, cwd)?;
    crate::require(
        route.paths().len() == 1,
        "ingestion requires one record path",
    )?;
    let record = route.paths()[0].clone();
    let root = prepare_state(&record, options.state_dir.as_deref())?;
    let event = event_id(&record, &report)?;
    let envelope_path = root.join("envelopes").join(format!("{event}.json"));
    let source_path = root.join("sources").join(format!("{event}.txt"));
    let receipt_path = root.join("receipts").join(format!("{event}.json"));
    let journal_path = root.join("journals").join(format!("{event}.json"));
    let canonical = {
        let mut b = serde_json::to_vec(&report.raw)?;
        b.push(b'\n');
        b
    };
    let envelope_sha = crate::identity::sha256(&canonical);
    let _record_lock = F::DirectoryGuard::acquire(record.parent().unwrap(), true)?;
    let _state_lock = F::DirectoryGuard::acquire(&root, true)?;
    if let Some(existing) = F::read(&envelope_path)? {
        crate::require(
            existing == canonical,
            "event_id was reused for different input",
        )?;
        if let Some(receipt) = F::read(&receipt_path)? {
            let value: J = crate::json_ingress::parse_slice(
                &receipt,
                crate::json_ingress::DuplicateKeys::Reject,
            )?;
            let code = if value["state"] == "applied" { 0 } else { 1 };
            return Ok(Output {
                text: String::from_utf8(receipt).map_err(|_| error("invalid receipt"))?,
                code,
            });
        }
    } else {
        F::replace(&source_path, Some(report.quote.as_bytes()))?;
        F::replace(&envelope_path, Some(&canonical))?;
    }

    let retained_report = F::read(&journal_path)?;
    if retained_report.is_some()
        && let Some(reason) = private_reason(&report)?
    {
        let (answer, signals) = receipt(
            &report,
            &record,
            &root,
            &event,
            &source_path,
            &envelope_sha,
            "needs_primary",
            Some(reason),
            false,
            None,
            None,
            supplied_runtime,
        )?;
        save(&receipt_path, &answer)?;
        save(
            &root.join("results").join(format!("{event}.json")),
            &json!({"receipt":answer,"signals":signals}),
        )?;
        return Ok(Output {
            text: format!("{}\n", serde_json::to_string(&answer)?),
            code: 1,
        });
    }
    if let Some(raw) = retained_report {
        let mut journal =
            crate::json_ingress::parse_slice(&raw, crate::json_ingress::DuplicateKeys::Reject)?;
        let mutation = journal_mutation(&journal)?;
        let history = journal["history"]
            .as_bool()
            .ok_or_else(|| error("invalid report journal"))?;
        drop(_state_lock);
        drop(_record_lock);
        drop(route);
        let recovery = if history {
            let mut committed = |phase: &str| {
                if phase == "committed" {
                    journal["phase"] = json!("committed");
                    save(&journal_path, &journal)?;
                }
                probe(phase)
            };
            crate::direct_history::recover_expected(
                &original,
                cwd,
                supplied_runtime,
                &mutation,
                &mut committed,
            )
            .map(|_| ())
        } else if crate::legacy_authoring::recovery_pending(&original, cwd)? {
            crate::legacy_authoring::recover_expected(&original, cwd, &mutation).map(|_| ())
        } else if journal["phase"] == "prepared" {
            let mut committed = |_: &V| {
                journal["phase"] = json!("committed");
                save(&journal_path, &journal)
            };
            crate::legacy_authoring::publish_expected(&original, cwd, &mutation, &mut committed)
        } else {
            Ok(())
        };
        if let Err(failure) = recovery {
            if failure.0.starts_with("report_preparation_stale:") {
                let _record_lock = F::DirectoryGuard::acquire(record.parent().unwrap(), true)?;
                let layout = crate::history_transaction::Layout::for_entry(
                    record
                        .file_name()
                        .and_then(|name| name.to_str())
                        .ok_or_else(|| error("invalid_path"))?,
                )?;
                let writer_pending = F::read(&record.parent().unwrap().join(&layout.journal))?
                    .is_some()
                    || F::read(
                        &record
                            .parent()
                            .unwrap()
                            .join(format!("{}.history", layout.journal)),
                    )?
                    .is_some();
                if writer_pending {
                    return Err(failure);
                }
                let _state_lock = F::DirectoryGuard::acquire(&root, true)?;
                let reason = failure.0.trim_start_matches("report_preparation_stale: ");
                let (answer, signals) = receipt(
                    &report,
                    &record,
                    &root,
                    &event,
                    &source_path,
                    &envelope_sha,
                    "needs_primary",
                    Some(reason),
                    true,
                    None,
                    None,
                    supplied_runtime,
                )?;
                save(&receipt_path, &answer)?;
                save(
                    &root.join("results").join(format!("{event}.json")),
                    &json!({"receipt":answer,"signals":signals}),
                )?;
                return Ok(Output {
                    text: format!("{}\n", serde_json::to_string(&answer)?),
                    code: 1,
                });
            }
            return Err(failure);
        }
        probe("recovered")?;
        let _record_lock = F::DirectoryGuard::acquire(record.parent().unwrap(), true)?;
        let _state_lock = F::DirectoryGuard::acquire(&root, true)?;
        if let Some(receipt) = F::read(&receipt_path)? {
            let value = crate::json_ingress::parse_slice(
                &receipt,
                crate::json_ingress::DuplicateKeys::Reject,
            )?;
            let code = if value["state"] == "applied" { 0 } else { 1 };
            return Ok(Output {
                text: String::from_utf8(receipt).map_err(|_| error("invalid receipt"))?,
                code,
            });
        }
        journal["phase"] = json!("committed");
        save(&journal_path, &journal)?;
        let graphs = match (journal.get("graph_before"), journal.get("graph_after")) {
            (Some(before), Some(after)) => Some((before.clone(), after.clone())),
            _ => None,
        };
        let (answer, signals) = receipt(
            &report,
            &record,
            &root,
            &event,
            &source_path,
            &envelope_sha,
            "applied",
            None,
            true,
            Some(&mutation),
            graphs.as_ref(),
            supplied_runtime,
        )?;
        for signal in &signals {
            save(
                &root
                    .join("signals")
                    .join(format!("{}.json", signal["id"].as_str().unwrap())),
                signal,
            )?;
        }
        save(&receipt_path, &answer)?;
        save(
            &root.join("results").join(format!("{event}.json")),
            &json!({"receipt":answer,"signals":signals}),
        )?;
        return Ok(Output {
            text: format!("{}\n", serde_json::to_string(&answer)?),
            code: 0,
        });
    }
    drop(_state_lock);

    let outcome = (|| {
        if let Some(reason) = private_reason(&report)? {
            return Err(error(reason));
        }
        route.verify()?;
        let authority = crate::legacy_authoring::route(&record, route.config())?;
        let context = V::from_json(&json!({
            "kind":"source-report/v1", "event_id":event,
            "source_sha256":crate::identity::sha256(report.quote.as_bytes()),
            "envelope_sha256":envelope_sha, "record":record, "state_dir":root,
            "policy":route.config().to_json()?,
            "routing":crate::source_capture::routing_observation(
                route.paths(), &route.project().root,
            )?.to_json()?
        }))?;
        if authority == crate::legacy_authoring::AuthorityRoute::Legacy {
            let mut inventory = Inventory::default();
            let captured =
                crate::source_document::load(std::slice::from_ref(&record), &mut inventory, false)?;
            crate::require(
                captured.members == [record.clone()],
                "multi-file and pointer records require primary review",
            )?;
            crate::require(
                map(&captured.hypotheses)?.is_empty(),
                "a record with hypothesis context requires primary review",
            )?;

            let document = captured.source.projected();
            if report.raw.get("source").is_some()
                || report.updates.iter().any(|u| u["kind"] == "add")
            {
                let expected = report.raw.get("record_sha256").and_then(J::as_str)
                    .ok_or_else(|| error("new entries and existing source citations require record_sha256 from the primary's prior read"))?;
                let bytes = inventory
                    .files
                    .get(&record)
                    .ok_or_else(|| error("snapshot_changed"))?;
                crate::require(
                    crate::identity::sha256(bytes) == expected,
                    "record changed since the primary read it; reread the premises before resubmitting",
                )?;
            }
            crate::require(
                !crate::recording_privacy::private_marker(&document),
                "private or unclear original source permission; report retained privately",
            )?;
            let collection = source_collection(&document, &report)?;
            let (planned, source_bodies) = actions(&report, &event, &source_path, &collection)?;
            let prepared = legacy_batch::prepare(
                &planned,
                &route,
                &legacy_batch::Options {
                    operation: format!("report-{event}"),
                    context,
                },
                inventory,
                None,
                &source_bodies,
            )?;
            let mutation = prepared.mutation.clone();
            verify_requested_profile(&report, &mutation, supplied_runtime)?;
            let graphs = mutation_graphs(&mutation, &report, supplied_runtime)?;
            save(
                &journal_path,
                &retained_journal(&mutation, false, "prepared", Some(&graphs))?,
            )?;
            probe("prepared")?;
            let mut committed = |_: &V| {
                save(
                    &journal_path,
                    &retained_journal(&mutation, false, "committed", Some(&graphs))?,
                )?;
                probe("committed")
            };
            crate::legacy_authoring::publish_prepared_with_committed(
                prepared,
                &route,
                &mut committed,
            )?;
            return Ok((mutation, graphs));
        }
        let entry_name = record
            .file_name()
            .and_then(|v| v.to_str())
            .ok_or_else(|| error("invalid_path"))?;
        let layout = crate::history_transaction::Layout::for_entry(entry_name)?;
        let portable = if layout.home.is_empty() {
            format!("evidence/reports/{event}.txt")
        } else {
            format!("{}/evidence/reports/{event}.txt", layout.home)
        };
        let store = crate::history_store::Store::new(&record)?;
        let captured = store.capture()?;
        let document = crate::history_authoring::document(&captured)?;
        if report.raw.get("source").is_some() || report.updates.iter().any(|u| u["kind"] == "add") {
            let expected = report.raw.get("record_sha256").and_then(J::as_str)
                .ok_or_else(|| error("new entries and existing source citations require record_sha256 from the primary's prior read"))?;
            crate::require(
                crate::identity::sha256(&captured.entry_bytes) == expected,
                "record changed since the primary read it; reread the premises before resubmitting",
            )?;
        }
        crate::require(
            !crate::recording_privacy::private_marker(&document),
            "private or unclear source permission in history report",
        )?;
        let collection = source_collection(&document, &report)
            .map_err(|e| error(&format!("history source collection: {e}")))?;
        let (planned, _) = actions(&report, &event, Path::new(&portable), &collection)
            .map_err(|e| error(&format!("history report actions: {e}")))?;
        let owned_runtime = if supplied_runtime.is_none() {
            crate::public_workspace::runtime_for_document(&document)?
        } else {
            None
        };
        let runtime = supplied_runtime.or(owned_runtime.as_ref());
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
            evidence: std::collections::BTreeMap::from([(
                portable,
                report.quote.as_bytes().to_vec(),
            )]),
        };
        let mutation = crate::history_authoring_batch::prepare_batch(
            &store, &captured, &planned, &batch, runtime,
        )
        .map_err(|e| error(&format!("history report preparation: {e}")))?;
        verify_requested_profile(&report, &mutation, runtime)?;
        let candidate = crate::history_authoring::candidate(&captured, &mutation)
            .map_err(|e| error(&format!("history report candidate: {e}")))?;
        let seeds = report
            .updates
            .iter()
            .map(|v| v["id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        let before_document = crate::history_authoring::document(&captured)
            .map_err(|e| error(&format!("history report before document: {e}")))?;
        let after_document = crate::history_authoring::document(&candidate)
            .map_err(|e| error(&format!("history report after document: {e}")))?;
        let graphs = (
            graph_document(&captured.entry_bytes, &before_document, &seeds, runtime)
                .map_err(|e| error(&format!("history report before graph: {e}")))?,
            graph_document(&candidate.entry_bytes, &after_document, &seeds, runtime)
                .map_err(|e| error(&format!("history report after graph: {e}")))?,
        );
        let authored = V::List(
            mutation
                .files()
                .iter()
                .filter(|f| f.role == "history_object")
                .map(|f| crate::history_yaml::decode_document(f.after.as_ref().unwrap()))
                .collect::<Result<Vec<_>>>()?,
        );
        crate::require(
            !crate::recording_privacy::private_marker(&authored),
            "private or unclear prepared source permission",
        )?;
        save(
            &journal_path,
            &retained_journal(&mutation, true, "prepared", Some(&graphs))?,
        )?;
        probe("prepared")?;
        crate::direct_history::publish_prepared(
            &store,
            &mutation,
            &route,
            &original,
            runtime,
            &mut |phase| {
                if phase == "committed" {
                    save(
                        &journal_path,
                        &retained_journal(&mutation, true, "committed", Some(&graphs))?,
                    )?;
                }
                probe(phase)
            },
        )?;
        Ok((mutation, graphs))
    })();
    let (state, reason, mutation, graphs, code) = match outcome {
        Ok((mutation, graphs)) => ("applied", None, Some(mutation), Some(graphs), 0),
        Err(e) => {
            if journal_path.is_file() {
                return Err(e);
            }
            ("needs_primary", Some(e.to_string()), None, None, 1)
        }
    };
    let (receipt, signals) = receipt(
        &report,
        &record,
        &root,
        &event,
        &source_path,
        &envelope_sha,
        state,
        reason.as_deref(),
        false,
        mutation.as_ref(),
        graphs.as_ref(),
        supplied_runtime,
    )?;
    for signal in &signals {
        save(
            &root
                .join("signals")
                .join(format!("{}.json", signal["id"].as_str().unwrap())),
            signal,
        )?;
    }
    save(&receipt_path, &receipt)?;
    save(
        &root.join("results").join(format!("{event}.json")),
        &json!({"receipt":receipt,"signals":signals}),
    )?;
    Ok(Output {
        text: format!("{}\n", serde_json::to_string(&receipt)?),
        code,
    })
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

    #[test]
    fn retained_publish_failure_is_recovered_before_a_terminal_receipt() {
        let temp = tempfile::tempdir().unwrap();
        let entry = temp.path().join("GROUNDING.yaml");
        fs::write(&entry, "sources:\n  s.old: {url: 'https://example.test', read: 2026-09-01}\nknown:\n  p.x: {v: 1, from: s.old, of: 2026-09-01}\n").unwrap();
        let state = temp.path().join("state");
        let report = json!({"event_id":"recover-me","date":"2026-09-20","source_quote":"x is 2",
            "updates":[{"kind":"set","id":"p.x","value":2}]});
        let bytes = serde_json::to_vec(&report).unwrap();
        let options = Options {
            file: "-".into(),
            record: None,
            state_dir: Some(state.clone()),
        };
        let first = run_with_probe(&options, temp.path(), Some(&bytes), None, &mut |phase| {
            if phase == "committed" {
                Err(error("injected completion failure"))
            } else {
                Ok(())
            }
        });
        assert_eq!(first.unwrap_err().0, "injected completion failure");
        assert!(
            fs::read_dir(state.join("receipts"))
                .unwrap()
                .next()
                .is_none()
        );
        let recovered = run(&options, temp.path(), Some(&bytes)).unwrap();
        assert_eq!(recovered.code, 0);
        let receipt: J = serde_json::from_str(&recovered.text).unwrap();
        assert_eq!(receipt["state"], "applied");
        assert_eq!(receipt["recovered"], true);
        assert!(fs::read_to_string(entry).unwrap().contains("v: 2"));
    }

    #[test]
    fn retained_prepared_intent_resumes_or_becomes_an_explicit_question() {
        for changed in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let entry = temp.path().join("GROUNDING.yaml");
            fs::write(&entry, "sources:\n  s.old: {url: 'https://example.test', read: 2026-09-01}\nknown:\n  p.x: {v: 1, from: s.old, of: 2026-09-01}\n").unwrap();
            let state = temp.path().join("state");
            let report = json!({"event_id":format!("prepared-{changed}"),"date":"2026-09-20",
                "source_quote":"x is 2","updates":[{"kind":"set","id":"p.x","value":2}]});
            let bytes = serde_json::to_vec(&report).unwrap();
            let options = Options {
                file: "-".into(),
                record: None,
                state_dir: Some(state.clone()),
            };
            let stopped = run_with_probe(&options, temp.path(), Some(&bytes), None, &mut |phase| {
                if phase == "prepared" {
                    Err(error("injected before writer journal"))
                } else {
                    Ok(())
                }
            });
            assert_eq!(stopped.unwrap_err().0, "injected before writer journal");
            assert!(
                fs::read_dir(state.join("receipts"))
                    .unwrap()
                    .next()
                    .is_none()
            );
            if changed {
                fs::write(&entry, "sources:\n  s.old: {url: 'https://example.test', read: 2026-09-01}\nknown:\n  p.x: {v: 99, from: s.old, of: 2026-09-01}\n").unwrap();
            }
            let resumed = run(&options, temp.path(), Some(&bytes)).unwrap();
            let receipt: J = serde_json::from_str(&resumed.text).unwrap();
            assert_eq!(resumed.code, if changed { 1 } else { 0 });
            assert_eq!(
                receipt["state"],
                if changed { "needs_primary" } else { "applied" }
            );
            if !changed {
                assert_eq!(receipt["recovered"], true);
                assert!(fs::read_to_string(&entry).unwrap().contains("v: 2"));
            } else {
                assert!(!receipt["reason"].as_str().unwrap().is_empty());
                assert!(fs::read_to_string(&entry).unwrap().contains("v: 99"));
            }
        }
    }

    #[test]
    fn receipt_found_after_recovery_keeps_its_terminal_exit_code() {
        let temp = tempfile::tempdir().unwrap();
        let entry = temp.path().join("GROUNDING.yaml");
        fs::write(&entry, "sources:\n  s.old: {url: 'https://example.test', read: 2026-09-01}\nknown:\n  p.x: {v: 1, from: s.old, of: 2026-09-01}\n").unwrap();
        let state = temp.path().join("state");
        let raw = serde_json::to_vec(&json!({"event_id":"receipt-race","date":"2026-09-20",
            "source_quote":"x is 2","updates":[{"kind":"set","id":"p.x","value":2}]}))
        .unwrap();
        let options = Options {
            file: "-".into(),
            record: None,
            state_dir: Some(state.clone()),
        };
        assert!(
            run_with_probe(&options, temp.path(), Some(&raw), None, &mut |phase| {
                if phase == "committed" {
                    Err(error("injected completion failure"))
                } else {
                    Ok(())
                }
            })
            .is_err()
        );
        let parsed = parse(&raw).unwrap();
        let id = event_id(&entry.canonicalize().unwrap(), &parsed).unwrap();
        let receipt_path = state.join("receipts").join(format!("{id}.json"));
        let result = run_with_probe(&options, temp.path(), Some(&raw), None, &mut |phase| {
            if phase == "recovered" {
                save(
                    &receipt_path,
                    &json!({"state":"needs_primary","reason":"concurrent finisher"}),
                )?;
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(result.code, 1);
        assert_eq!(
            serde_json::from_str::<J>(&result.text).unwrap()["state"],
            "needs_primary"
        );
    }

    #[test]
    fn retained_prepared_private_envelope_is_never_republished() {
        let temp = tempfile::tempdir().unwrap();
        let entry = temp.path().join("GROUNDING.yaml");
        fs::write(&entry, "sources:\n  s.old: {url: 'https://example.test', read: 2026-09-01}\nknown:\n  p.x: {v: 1, from: s.old, of: 2026-09-01}\n").unwrap();
        let state = temp.path().join("state");
        let public = json!({"event_id":"old-private","date":"2026-09-20","source_quote":"x is 2",
            "updates":[{"kind":"set","id":"p.x","value":2}]});
        let public_bytes = serde_json::to_vec(&public).unwrap();
        let options = Options {
            file: "-".into(),
            record: None,
            state_dir: Some(state.clone()),
        };
        assert!(
            run_with_probe(
                &options,
                temp.path(),
                Some(&public_bytes),
                None,
                &mut |phase| if phase == "prepared" {
                    Err(error("old writer stopped"))
                } else {
                    Ok(())
                }
            )
            .is_err()
        );
        let private = json!({"event_id":"old-private","date":"2026-09-20","source_quote":"x is 2",
            "shareability":"personal","updates":[{"kind":"set","id":"p.x","value":2}]});
        let mut private_bytes = serde_json::to_vec(&private).unwrap();
        private_bytes.push(b'\n');
        let id = event_id(
            &entry.canonicalize().unwrap(),
            &parse(&private_bytes).unwrap(),
        )
        .unwrap();
        F::replace(
            &state.join("envelopes").join(format!("{id}.json")),
            Some(&private_bytes),
        )
        .unwrap();
        let retained = run(&options, temp.path(), Some(&private_bytes)).unwrap();
        assert_eq!(retained.code, 1);
        assert_eq!(
            serde_json::from_str::<J>(&retained.text).unwrap()["state"],
            "needs_primary"
        );
        assert!(fs::read_to_string(entry).unwrap().contains("v: 1"));
        assert!(state.join("journals").join(format!("{id}.json")).is_file());
    }

    #[test]
    fn captured_hash_privacy_and_collection_reads_are_rechecked_before_publish() {
        let temp = tempfile::tempdir().unwrap();
        let entry = temp.path().join("GROUNDING.yaml");
        fs::write(&entry, "sources:\n  s.old: {url: 'https://example.test', read: 2026-09-01}\nknown:\n  p.x: {v: 1, from: s.old, of: 2026-09-01}\n").unwrap();
        let report = json!({"event_id":"race","record_sha256":crate::identity::sha256(&fs::read(&entry).unwrap()),
            "date":"2026-09-20","source_quote":"new value","updates":[{"kind":"add","id":"p.y","body":{"v":2}}]});
        let bytes = serde_json::to_vec(&report).unwrap();
        let state = temp.path().join("state");
        let options = Options {
            file: "-".into(),
            record: None,
            state_dir: Some(state.clone()),
        };
        let result = run_with_probe(&options, temp.path(), Some(&bytes), None, &mut |phase| {
            if phase == "prepared" {
                fs::write(
                    &entry,
                    "sources:\n  s.changed: {url: 'https://changed.test', read: 2026-09-20}\nknown:\n  p.x: {v: 99}\n",
                )?;
            }
            Ok(())
        });
        assert!(result.is_err());
        assert!(
            fs::read_dir(state.join("receipts"))
                .unwrap()
                .next()
                .is_none()
        );
        assert!(fs::read_to_string(entry).unwrap().contains("p.x: {v: 99}"));
    }

    #[test]
    fn active_history_update_publishes_before_building_its_receipt() {
        let temp = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .current_dir(temp.path())
                .status()
                .unwrap()
                .success()
        );
        let entry = temp.path().join("GROUNDING.yaml");
        let policy = crate::project_modes::Project::open(temp.path())
            .unwrap()
            .config()
            .unwrap();
        let cache = tempfile::tempdir().unwrap();
        let runtime = crate::history_authoring::tests::runtime(cache.path())
            .with_ordinary_program(crate::ordinary_reader::tests::program());
        let initial = V::from_json(&json!({"kind":"add","id":"p.x",
            "body":{"v":1},"as_of":"2026-09-01"}))
        .unwrap();
        let bootstrap = crate::history_bootstrap::prepare(
            &entry,
            &initial,
            &policy,
            &crate::history_bootstrap::BootstrapOptions {
                operation: "bootstrap-update-test".into(),
                recorded_at: "2026-09-01T00:00:00Z".into(),
                recording_day: "2026-09-01".into(),
                record_id: "update-test".into(),
                by: V::Null,
            },
            Some(&runtime),
        )
        .unwrap();
        crate::history_bootstrap::publish(&entry, &bootstrap, &policy, Some(&runtime), &mut |_| {
            Ok(())
        })
        .unwrap();
        for (index, action) in [
            json!({"kind":"add","id":"s.old","into":"known","body":{"url":"https://example.test","read":"2026-09-01"},"as_of":"2026-09-01"}),
            json!({"kind":"add","id":"d.x","body":{"rests_on":["p.x"],"verdict":"ok","wrong_if":"p.x > 1"},"as_of":"2026-09-01"}),
        ].into_iter().enumerate() {
            let route = WriteRoute::capture(std::slice::from_ref(&entry), temp.path()).unwrap();
            let store = crate::history_store::Store::new(&entry).unwrap();
            let _guard = F::DirectoryGuard::acquire(&store.root, true).unwrap();
            let captured = store.capture().unwrap();
            let mutation = crate::history_authoring::prepare(&store, &captured,
                &V::from_json(&action).unwrap(), &crate::history_authoring::Options {
                    operation:format!("seed-{index}"), recorded_at:"2026-09-01T00:00:00Z".into(),
                    recording_day:"2026-09-01".into(), by:V::Null, strict:true,
                    paths:crate::history_paths::Scheme::Hashed, receipt_version:None,
                }, Some(&runtime)).unwrap();
            crate::direct_history::publish_prepared(&store, &mutation, &route,
                std::slice::from_ref(&entry), Some(&runtime), &mut |_| Ok(())).unwrap();
        }
        let report = json!({"event_id":"history-update","date":"2026-09-20","source_quote":"x is 2",
            "updates":[{"kind":"set","id":"p.x","value":2}]});
        let bytes = serde_json::to_vec(&report).unwrap();
        let output = run_with_probe(
            &Options {
                file: "-".into(),
                record: None,
                state_dir: Some(temp.path().join("state")),
            },
            temp.path(),
            Some(&bytes),
            Some(&runtime),
            &mut |_| Ok(()),
        )
        .unwrap();
        assert_eq!(output.code, 0, "{}", output.text);
        let receipt: J = serde_json::from_str(&output.text).unwrap();
        assert_eq!(receipt["state"], "applied");
        assert_eq!(receipt["newly_fired_judgments"], json!(["d.x"]));
        let initial_event =
            event_id(&entry.canonicalize().unwrap(), &parse(&bytes).unwrap()).unwrap();
        let initial_journal: J = crate::json_ingress::parse_slice(
            &fs::read(
                temp.path()
                    .join("state/journals")
                    .join(format!("{initial_event}.json")),
            )
            .unwrap(),
            crate::json_ingress::DuplicateKeys::Reject,
        )
        .unwrap();
        let initial_mutation = journal_mutation(&initial_journal).unwrap();
        let store = crate::history_store::Store::new(&entry).unwrap();
        let general = store.root.join(&store.layout.journal);
        fs::create_dir_all(general.parent().unwrap()).unwrap();
        fs::write(&general, bootstrap.to_bytes().unwrap()).unwrap();
        assert_eq!(
            crate::direct_history::recover_expected(
                std::slice::from_ref(&entry),
                temp.path(),
                Some(&runtime),
                &initial_mutation,
                &mut |_| Ok(())
            )
            .unwrap_err()
            .0,
            "history report recovery journal mismatch"
        );
        fs::remove_file(&general).unwrap();
        let adoption = store.root.join(format!("{}.history", store.layout.journal));
        fs::write(&adoption, "kind: history-adoption/v1\n").unwrap();
        assert_eq!(
            crate::direct_history::recover_expected(
                std::slice::from_ref(&entry),
                temp.path(),
                Some(&runtime),
                &initial_mutation,
                &mut |_| Ok(())
            )
            .unwrap_err()
            .0,
            "history report recovery journal mismatch"
        );
        fs::remove_file(&adoption).unwrap();
        let captured = crate::history_store::Store::new(&entry)
            .unwrap()
            .capture()
            .unwrap();
        assert!(
            map(&map(&captured.state).unwrap()["subjects"])
                .unwrap()
                .contains_key("p.x")
        );

        for (event, value, day, interrupted) in [
            ("history-prepared", 3, "2026-09-21", "prepared"),
            ("history-committed", 4, "2026-09-22", "committed"),
        ] {
            let report = json!({"event_id":event,"date":day,"source_quote":format!("x is {value}"),
                "updates":[{"kind":"set","id":"p.x","value":value}]});
            let bytes = serde_json::to_vec(&report).unwrap();
            let state = temp.path().join(format!("state-{value}"));
            let options = Options {
                file: "-".into(),
                record: None,
                state_dir: Some(state.clone()),
            };
            let stopped = run_with_probe(
                &options,
                temp.path(),
                Some(&bytes),
                Some(&runtime),
                &mut |phase| {
                    if phase == interrupted {
                        Err(error("injected history interruption"))
                    } else {
                        Ok(())
                    }
                },
            );
            assert_eq!(stopped.unwrap_err().0, "injected history interruption");
            assert!(
                fs::read_dir(state.join("receipts"))
                    .unwrap()
                    .next()
                    .is_none()
            );
            let store = crate::history_store::Store::new(&entry).unwrap();
            let writer_journal = store.root.join(format!("{}.history", store.layout.journal));
            assert_eq!(writer_journal.is_file(), interrupted == "committed");
            if interrupted == "committed" {
                assert_eq!(
                    crate::direct_history::recover_expected(
                        std::slice::from_ref(&entry),
                        temp.path(),
                        Some(&runtime),
                        &initial_mutation,
                        &mut |_| Ok(())
                    )
                    .unwrap_err()
                    .0,
                    "history report recovery journal mismatch"
                );
                assert!(writer_journal.is_file());
            }
            let resumed = run_with_probe(
                &options,
                temp.path(),
                Some(&bytes),
                Some(&runtime),
                &mut |_| Ok(()),
            )
            .unwrap();
            let receipt: J = serde_json::from_str(&resumed.text).unwrap();
            assert_eq!(resumed.code, 0);
            assert_eq!(receipt["state"], "applied");
            assert_eq!(receipt["recovered"], true);
            assert!(!writer_journal.exists());
        }
        let final_report = json!({"event_id":"history-after-recovery","date":"2026-09-23",
            "source_quote":"x is 5","updates":[{"kind":"set","id":"p.x","value":5}]});
        let final_bytes = serde_json::to_vec(&final_report).unwrap();
        let final_write = run_with_probe(
            &Options {
                file: "-".into(),
                record: None,
                state_dir: Some(temp.path().join("state-final")),
            },
            temp.path(),
            Some(&final_bytes),
            Some(&runtime),
            &mut |_| Ok(()),
        )
        .unwrap();
        assert_eq!(final_write.code, 0, "{}", final_write.text);

        let stale_report = json!({"event_id":"history-stale-preimage","date":"2026-09-24",
            "source_quote":"x is 6","updates":[{"kind":"set","id":"p.x","value":6}]});
        let stale_bytes = serde_json::to_vec(&stale_report).unwrap();
        let stale_options = Options {
            file: "-".into(),
            record: None,
            state_dir: Some(temp.path().join("state-stale")),
        };
        assert!(
            run_with_probe(
                &stale_options,
                temp.path(),
                Some(&stale_bytes),
                Some(&runtime),
                &mut |phase| if phase == "prepared" {
                    Err(error("stop before history journal"))
                } else {
                    Ok(())
                }
            )
            .is_err()
        );
        let valid = fs::read(&entry).unwrap();
        let mut changed = valid.clone();
        changed.extend_from_slice(b"# concurrent edit\n");
        fs::write(&entry, &changed).unwrap();
        let stale = run_with_probe(
            &stale_options,
            temp.path(),
            Some(&stale_bytes),
            Some(&runtime),
            &mut |_| Ok(()),
        )
        .unwrap();
        assert_eq!(stale.code, 1);
        assert_eq!(
            serde_json::from_str::<J>(&stale.text).unwrap()["state"],
            "needs_primary"
        );
        assert_eq!(fs::read(&entry).unwrap(), changed);
        fs::write(&entry, valid).unwrap();

        let route_report = json!({"event_id":"history-route-change","date":"2026-09-25",
            "source_quote":"x is 7","updates":[{"kind":"set","id":"p.x","value":7}]});
        let route_bytes = serde_json::to_vec(&route_report).unwrap();
        let route_options = Options {
            file: "-".into(),
            record: None,
            state_dir: Some(temp.path().join("state-route")),
        };
        assert!(
            run_with_probe(
                &route_options,
                temp.path(),
                Some(&route_bytes),
                Some(&runtime),
                &mut |phase| if phase == "prepared" {
                    Err(error("stop before history journal"))
                } else {
                    Ok(())
                }
            )
            .is_err()
        );
        let project = crate::project_modes::Project::open(temp.path()).unwrap();
        fs::create_dir_all(project.config_path.parent().unwrap()).unwrap();
        fs::write(
            &project.config_path,
            r#"{"version":1,"mode":"advanced","generation":1,"record":"GROUNDING.yaml"}"#,
        )
        .unwrap();
        let changed_route = run_with_probe(
            &route_options,
            temp.path(),
            Some(&route_bytes),
            Some(&runtime),
            &mut |_| Ok(()),
        )
        .unwrap();
        assert_eq!(changed_route.code, 1);
        assert!(
            serde_json::from_str::<J>(&changed_route.text).unwrap()["reason"]
                .as_str()
                .unwrap()
                .contains("project")
        );
    }
}
