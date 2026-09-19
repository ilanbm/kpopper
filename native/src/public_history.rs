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
}

fn fresh_id(prefix: &str) -> Result<String> {
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

pub fn run(options: &Options, cwd: &Path, frozen: bool) -> Result<Value> {
    let operation = options
        .operation
        .as_deref()
        .ok_or_else(|| error("history operation required"))?;
    require(
        ["status", "reconcile", "rebuild", "migrate"].contains(&operation),
        "history operation unsupported",
    )?;
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
                _ if frozen => ReadMode::Frozen,
                _ if string_is(&map(&project.config()?)?["mode"], "advanced") => ReadMode::Live,
                _ => ReadMode::Frozen,
            };
            let runtime = public_workspace::runtime()?;
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
    let code = error.0.split(':').next().unwrap_or("history_refused");
    let code = if !code.is_empty() && code.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
        code
    } else {
        "history_refused"
    };
    json!({"state":"refused", "code":code, "detail":error.to_string()})
}
