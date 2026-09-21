//! Public history maintenance uses the same captured authority and exact-image
//! guards as the native store. Migration only materializes a separate copy.
use crate::{
    Result, history_capture,
    history_contract::{error, map, string_is},
    history_migration::{self, Plan},
    history_store::Store,
    project_modes::{self, WriteRoute},
    public_core_readers::json_value,
    public_workspace, require,
    source_capture::ReadMode,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Clone, Default, clap::Args)]
pub struct Options {
    /// status, reconcile, rebuild, or migrate. Feasibility stores accept a subject.
    pub operation: Option<String>,
    #[arg(long)]
    pub record: Option<PathBuf>,
    /// Absent destination directory for a verified history copy.
    #[arg(long)]
    pub to: Option<PathBuf>,
    #[arg(long, value_parser = ["live", "frozen"])]
    pub read_mode: Option<String>,
    #[arg(long)]
    pub subject: Option<String>,
    #[arg(long = "of")]
    pub target: Option<String>,
    #[arg(long)]
    pub over: Vec<String>,
    #[arg(long)]
    pub because: Option<String>,
    #[arg(long)]
    pub by: Option<String>,
    /// Fresh challenge token for a native deployment declaration.
    #[arg(long)]
    pub nonce: Option<String>,
    /// Retained pending contribution revision to adopt.
    #[arg(long)]
    pub revision: Option<String>,
    /// SUBJECT=VERSION for every overlapping subject; repeat as needed.
    #[arg(long)]
    pub choose: Vec<String>,
    /// Show adoption choices without writing.
    #[arg(long)]
    pub preview: bool,
    #[arg(long)]
    pub record_proposals: bool,
    #[arg(long)]
    pub proposal_subject: Vec<String>,
}

impl Options {
    pub fn selects_public_operation(&self) -> bool {
        self.record.is_some()
            || self.to.is_some()
            || self.read_mode.is_some()
            || self.subject.is_some()
            || self.target.is_some()
            || !self.over.is_empty()
            || self.because.is_some()
            || self.by.is_some()
            || self.nonce.is_some()
            || self.revision.is_some()
            || !self.choose.is_empty()
            || self.preview
            || self.record_proposals
            || !self.proposal_subject.is_empty()
    }
}

pub(crate) fn fresh_id(prefix: &str) -> Result<String> {
    let token = tempfile::Builder::new()
        .prefix("kpop-identity-")
        .rand_bytes(24)
        .tempdir()?;
    Ok(format!(
        "{prefix}-{}",
        crate::identity::sha256(token.path().as_os_str().as_encoded_bytes())
    ))
}

fn status(entry: &Path) -> Result<Value> {
    let capture = history_capture::capture(entry, None, None)?;
    let subjects = map(&map(&capture.state)?["subjects"])?
        .iter()
        .map(|(subject, value)| {
            let value = map(value)?;
            Ok((
                subject.clone(),
                json!({
                    "acceptance": value["acceptance"].to_json()?,
                    "heads": value["heads"].to_json()?,
                }),
            ))
        })
        .collect::<Result<serde_json::Map<String, Value>>>()?;
    capture.verify_current()?;
    Ok(json!({"state":"captured", "record":entry,
        "authority":capture.marker.to_json()?, "commits":capture.commits.len(),
        "objects":capture.objects.len(), "subjects":subjects}))
}

pub fn run(options: &Options, cwd: &Path) -> Result<Value> {
    let operation = options
        .operation
        .as_deref()
        .ok_or_else(|| error("history operation required"))?;
    require(
        [
            "status",
            "reconcile",
            "rebuild",
            "migrate",
            "accept",
            "refute",
            "correct",
            "propose",
            "retire",
            "adopt",
            "capabilities",
        ]
        .contains(&operation),
        "history operation unsupported",
    )?;
    if operation == "capabilities" {
        require(
            options.record.is_none() && options.to.is_none(),
            "a runtime declaration does not select or migrate a record",
        )?;
        return crate::history_native_declaration::describe(
            options
                .nonce
                .as_deref()
                .ok_or_else(|| error("invalid_runtime_nonce"))?,
        );
    }
    require(
        options.to.is_none() || operation == "migrate",
        "--to belongs to history migrate",
    )?;
    let cwd = cwd.canonicalize()?;
    let original = match &options.record {
        Some(path) => vec![cwd.join(path)],
        None => public_workspace::records(&cwd)?,
    };
    let paths = project_modes::write_paths(&original, &cwd)?;
    require(paths.len() == 1, "choose one logical record entry")?;
    let entry = &paths[0];
    let result = match operation {
        "adopt" => {
            let revision = options
                .revision
                .as_deref()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| error("adoption requires --revision"))?;
            let choices = crate::public_history_adopt::choices(&options.choose)?;
            json_value(&crate::public_history_adopt::run(
                &original,
                &cwd,
                revision,
                &choices,
                options.by.as_deref(),
                options.preview,
            )?)?
        }
        "accept" | "refute" | "correct" | "propose" | "retire" => {
            require(
                options.subject.as_ref().is_some_and(|s| !s.is_empty())
                    && options.target.as_ref().is_some_and(|s| !s.is_empty())
                    && options.because.as_ref().is_some_and(|s| !s.is_empty()),
                "explicit acts require --subject, --of, and --because",
            )?;
            let action = crate::value::TypedValue::from_json(&json!({
                "kind":operation, "id":options.subject, "of":options.target,
                "over":options.over, "because":options.because,
            }))?;
            json_value(&crate::direct_history::act(&original, &cwd, &action)?)?
        }
        "reconcile" if options.record_proposals => {
            let because = options
                .because
                .as_deref()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| error("recording proposals requires --because"))?;
            let subjects = (!options.proposal_subject.is_empty())
                .then_some(options.proposal_subject.as_slice());
            json_value(&crate::direct_history::proposals(
                &original,
                &cwd,
                subjects,
                because,
                options.by.as_deref(),
            )?)?
        }
        "status" => status(entry)?,
        "reconcile" => json_value(&Store::new(entry)?.prepare_reconciliation(None, true, &[])?)?,
        "rebuild" => {
            let route = WriteRoute::capture(&original, &cwd)?;
            require(route.paths() == paths, "project_route_changed")?;
            route.verify()?;
            let store = Store::new(entry)?;
            store.rebuild(None, true, true, &[])?;
            let captured = store.capture()?;
            let unresolved = map(&map(&captured.state)?["subjects"])?
                .iter()
                .filter_map(|(subject, item)| {
                    let accepted = map(item).ok()?.get("acceptance")?;
                    (!string_is(accepted, "accepted")).then_some(subject.clone())
                })
                .collect::<Vec<_>>();
            captured.verify_current()?;
            route.verify()?;
            json!({"state":"rebuilt", "record":entry,
                "baseline":captured.baseline.to_json()?, "unresolved_subjects":unresolved})
        }
        "migrate" => {
            let project = project_modes::project_for(&paths, &cwd)?;
            let mode = match options.read_mode.as_deref() {
                Some("live") => ReadMode::Live,
                Some("frozen") => ReadMode::Frozen,
                _ if string_is(&map(&project.config()?)?["mode"], "advanced") => ReadMode::Live,
                _ => ReadMode::Frozen,
            };
            let runtime =
                public_workspace::runtime_for_paths(std::slice::from_ref(entry), &cwd, None)?;
            let authority = entry.parent().unwrap().join(
                crate::history_transaction::Layout::for_entry(
                    entry
                        .file_name()
                        .and_then(|s| s.to_str())
                        .ok_or_else(|| error("invalid_path"))?,
                )?
                .authority,
            );
            let record_id = if authority.try_exists()? {
                None
            } else {
                Some(fresh_id("record")?)
            };
            let plan = Plan::prepare(
                entry,
                &cwd,
                history_migration::Options {
                    operation: fresh_id("import")?,
                    recorded_at: chrono::Utc::now().to_rfc3339(),
                    record_id,
                    read_mode: mode,
                    route: false,
                    as_of: None,
                },
                runtime.as_ref(),
            )?;
            require(
                project_modes::write_paths(&original, &cwd)? == paths,
                "project_route_changed",
            )?;
            let result = match &options.to {
                Some(path) => plan.publish(&cwd.join(path))?,
                None => {
                    plan.verify_source()?;
                    plan.summary()?
                }
            };
            json_value(&result)?
        }
        _ => unreachable!(),
    };
    require(
        project_modes::write_paths(&original, &cwd)? == paths,
        "project_route_changed",
    )?;
    Ok(result)
}

pub fn refusal(error: &crate::Error) -> Value {
    if error.0 == "invalid_history_contribution" {
        return json!({
            "state":"refused",
            "code":"history_refused",
            "detail":"stored contribution evidence failed its complete identity check",
        });
    }
    let code = error.0.split(':').next().unwrap_or("history_refused");
    let code = if !code.is_empty() && code.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
        code
    } else {
        "history_refused"
    };
    json!({"state":"refused", "code":code, "detail":error.to_string()})
}

fn key_order(map: &serde_json::Map<String, Value>) -> Vec<&String> {
    let preferred: &[&str] = if map.get("state") == Some(&json!("refused")) {
        &["state", "code", "detail"]
    } else if map.contains_key("artifact_revision") && map.contains_key("target_authority") {
        &[
            "artifact_revision",
            "target_authority",
            "target_baseline",
            "subjects",
        ]
    } else if map.contains_key("requires_choice") && map.contains_key("combined_heads") {
        &[
            "requires_choice",
            "target_heads",
            "incoming_heads",
            "combined_heads",
            "claims",
        ]
    } else if map.get("state") == Some(&json!("captured")) && map.contains_key("authority") {
        &[
            "state",
            "record",
            "authority",
            "commits",
            "objects",
            "subjects",
        ]
    } else if map.contains_key("acceptance") && map.contains_key("heads") {
        &["acceptance", "heads"]
    } else if map.contains_key("authority")
        && map.contains_key("generation")
        && map.contains_key("record_id")
    {
        &["authority", "generation", "profile", "record_id", "version"]
    } else if map.contains_key("authority_generation") && map.contains_key("committed_set_digest") {
        &[
            "version",
            "record_id",
            "authority_generation",
            "committed_set_digest",
            "heads",
            "open_acts",
        ]
    } else if map.get("state") == Some(&json!("adopted")) {
        &["state", "revision", "operation"]
    } else {
        &[]
    };
    let mut keys = preferred
        .iter()
        .filter_map(|key| map.get_key_value(*key).map(|(key, _)| key))
        .collect::<Vec<_>>();
    keys.extend(map.keys().filter(|key| !preferred.contains(&key.as_str())));
    keys
}

fn render_json(value: &Value) -> String {
    match value {
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(render_json)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(map) => format!(
            "{{{}}}",
            key_order(map)
                .into_iter()
                .map(|key| format!(
                    "{}: {}",
                    serde_json::to_string(key).unwrap(),
                    render_json(&map[key])
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => value.to_string(),
    }
}

/// Python-compatible one-line output for the public history command family.
pub fn output(value: &Value) -> String {
    render_json(value) + "\n"
}

#[cfg(test)]
mod output_tests {
    use super::*;

    #[test]
    fn status_preview_adoption_and_refusal_follow_python_field_order() {
        let authority = json!({
            "version":1,"record_id":"record","profile":"history/v1","generation":1,"authority":"history"
        });
        let baseline = json!({
            "open_acts":{},"heads":{},"version":1,"record_id":"record",
            "committed_set_digest":"digest","authority_generation":1
        });
        assert_eq!(
            output(&json!({
                "subjects":{"p.x":{"heads":["claim"],"acceptance":"accepted"}},
                "objects":1,"commits":1,"authority":authority,"record":"/record","state":"captured"
            })),
            "{\"state\": \"captured\", \"record\": \"/record\", \"authority\": {\"authority\": \"history\", \"generation\": 1, \"profile\": \"history/v1\", \"record_id\": \"record\", \"version\": 1}, \"commits\": 1, \"objects\": 1, \"subjects\": {\"p.x\": {\"acceptance\": \"accepted\", \"heads\": [\"claim\"]}}}\n"
        );
        assert_eq!(
            output(&json!({
                "subjects":{"p.x":{"claims":["claim"],"combined_heads":["claim"],
                    "incoming_heads":["claim"],"target_heads":[],"requires_choice":false}},
                "target_baseline":baseline,"target_authority":authority,"artifact_revision":"revision"
            })),
            "{\"artifact_revision\": \"revision\", \"target_authority\": {\"authority\": \"history\", \"generation\": 1, \"profile\": \"history/v1\", \"record_id\": \"record\", \"version\": 1}, \"target_baseline\": {\"version\": 1, \"record_id\": \"record\", \"authority_generation\": 1, \"committed_set_digest\": \"digest\", \"heads\": {}, \"open_acts\": {}}, \"subjects\": {\"p.x\": {\"requires_choice\": false, \"target_heads\": [], \"incoming_heads\": [\"claim\"], \"combined_heads\": [\"claim\"], \"claims\": [\"claim\"]}}}\n"
        );
        assert_eq!(
            output(&json!({"operation":"op","revision":"rev","state":"adopted"})),
            "{\"state\": \"adopted\", \"revision\": \"rev\", \"operation\": \"op\"}\n"
        );
        assert_eq!(
            output(&refusal(&crate::Error(
                "invalid_history_contribution".into()
            ))),
            "{\"state\": \"refused\", \"code\": \"history_refused\", \"detail\": \"stored contribution evidence failed its complete identity check\"}\n"
        );
    }
}
