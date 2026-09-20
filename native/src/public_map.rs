//! Public map request and the private host-agent receipt protocol.
use crate::{Error, Result, require, onboarding, public_workspace};
use clap::Args;
use serde_json::{Value, json};
use std::{fs, path::{Path, PathBuf}};
use uuid::Uuid;

#[derive(Clone, Debug, Default, Args)]
pub struct Options { #[arg(long)] pub deep: bool }
#[derive(Clone, Debug, Args)]
pub struct AgentOptions { #[command(subcommand)] pub command: AgentCommand }
#[derive(Clone, Debug, clap::Subcommand)]
pub enum AgentCommand { Status, Task, Shown { event: String }, Accept { #[arg(long)] request: String }, Complete { #[arg(long)] request: String, #[arg(long)] report: String }, Fail { #[arg(long)] request: String, #[arg(long)] reason: String } }

fn session() -> Option<String> {
    std::env::var("KPOPPER_AGENT_SESSION").ok().or_else(|| std::env::var("CODEX_THREAD_ID").ok())
        .filter(|v| !v.is_empty() && v.len() <= 200 && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
}
fn read_job(key: &str) -> Result<Option<Value>> { onboarding::read(&onboarding::project_dir_key(key)?.join("mapping.json")) }
fn lock_path(key: &str) -> Result<PathBuf> { Ok(onboarding::project_dir_key(key)?.join("choice.lock")) }
fn with_lock<T>(key: &str, f: impl FnOnce() -> Result<T>) -> Result<T> {
    let path = lock_path(key)?; if let Some(parent) = path.parent() { fs::create_dir_all(parent)?; }
    let file = fs::OpenOptions::new().create(true).read(true).write(true).open(path)?;
    fs2::FileExt::lock_exclusive(&file).map_err(|e| Error(e.to_string()))?;
    let result = f(); let _ = fs2::FileExt::unlock(&file); result
}
pub fn packet(location: &public_workspace::Location, job: &Value) -> Result<Value> {
    let request = job.get("request").and_then(Value::as_str).ok_or_else(|| Error("invalid mapping request".into()))?;
    Ok(json!({"request":request,"status":job["mapping"],"mode":job["mode"],"owner":job["owner"],"workspace":location.workspace,
        "record":location.record,"instructions":"Map existing materials within the agreed scope. Return a report; protocol completion requires an actual nonempty report.",
        "protocol":{"accept":["kpop","_agent","accept","--request",request],"complete":["kpop","_agent","complete","--request",request,"--report","REPORT_PATH"],"fail":["kpop","_agent","fail","--request",request,"--reason","REASON"]}}))
}
pub fn request(workspace: &Path, deep: bool) -> Result<Value> {
    let owner = session().ok_or_else(|| Error("No calling agent session is bound. Mapping runs inside an active agent session; no work was started.".into()))?;
    let loc = public_workspace::locate(workspace, crate::source_capture::ReadMode::Live)?;
    require(loc.status != "unavailable", &loc.reason)?;
    with_lock(&loc.key, || {
        if let Some(old) = read_job(&loc.key)? {
            let state = old.get("mapping").and_then(Value::as_str).unwrap_or("");
            if ["ready","running"].contains(&state) && old.get("owner").and_then(Value::as_str) != Some(&owner) { return Err(Error("Another agent session owns a pending mapping. Finish or fail that task before starting another.".into())); }
            if ["ready","running"].contains(&state) && old.get("owner").and_then(Value::as_str) == Some(&owner) && old.get("mode").and_then(Value::as_str) == Some(if deep{"deep"}else{"map"}) { return packet(&loc, &old); }
        }
        let job = json!({"mode":if deep{"deep"}else{"map"},"mapping":"ready","request":Uuid::new_v4().simple().to_string(),"owner":owner});
        onboarding::write(&onboarding::project_dir_key(&loc.key)?.join("mapping.json"), &job)?; packet(&loc, &job)
    })
}
pub fn transition(workspace: &Path, request: &str, action: &str, report: Option<&str>, reason: Option<&str>) -> Result<Value> {
    let loc = public_workspace::locate(workspace, crate::source_capture::ReadMode::Live)?;
    with_lock(&loc.key, || {
        let mut job = read_job(&loc.key)?.ok_or_else(|| Error("This mapping belongs to a different request or agent session.".into()))?;
        require(job.get("request").and_then(Value::as_str) == Some(request), "This mapping belongs to a different request or agent session.")?;
        require(job.get("owner").and_then(Value::as_str) == session().as_deref(), "This mapping belongs to a different request or agent session.")?;
        let state = job["mapping"].as_str().unwrap_or("");
        match action {
            "accept" => { require(["ready","running"].contains(&state), "This mapping cannot be accepted in its current state.")?; job["mapping"] = json!("running"); }
            "fail" => { require(["ready","running"].contains(&state) && reason.is_some_and(|s| !s.trim().is_empty()), "An active mapping and a failure reason are required.")?; job["mapping"]=json!("failed"); job["error"]=json!(reason.unwrap().trim()); }
            "complete" => { require(state == "running", "Accept the mapping before completing it.")?; let p=PathBuf::from(report.unwrap_or("")).canonicalize().map_err(|_| Error("Completion requires the actual, nonempty report or knowledge record.".into()))?; require(p.is_file() && fs::metadata(&p)?.len()>0, "Completion requires the actual, nonempty report or knowledge record.")?; job["mapping"]=json!("complete"); job["report"]=json!(p); }
            _ => return Err(Error("unknown mapping transition".into()))
        }
        onboarding::write(&onboarding::project_dir_key(&loc.key)?.join("mapping.json"), &job)?; Ok(job)
    })
}
pub fn run(options: &Options, workspace: &Path) -> Result<Value> { request(workspace, options.deep) }
pub fn run_agent(options: &AgentOptions, workspace: &Path) -> Result<Value> {
    let loc = public_workspace::locate(workspace, crate::source_capture::ReadMode::Live)?;
    match &options.command {
        AgentCommand::Status => onboarding::status_at(&loc.workspace, &loc.key, &loc.record, &loc.status),
        AgentCommand::Task => { let job=read_job(&loc.key)?.ok_or_else(|| Error("This mapping belongs to a different request or agent session.".into()))?; require(job.get("owner").and_then(Value::as_str)==session().as_deref(), "This mapping belongs to a different request or agent session.")?; packet(&loc,&job) }
        AgentCommand::Shown { event } => onboarding::mark_key(&loc.workspace, &loc.key, &loc.record, &loc.status, event),
        AgentCommand::Accept { request } => transition(&loc.workspace, request, "accept", None, None),
        AgentCommand::Complete { request, report } => transition(&loc.workspace, request, "complete", Some(report), None),
        AgentCommand::Fail { request, reason } => transition(&loc.workspace, request, "fail", None, Some(reason)),
    }
}

#[cfg(test)]
mod tests {
    use super::*; use tempfile::TempDir;
    #[test] fn project_location_is_stable() { let t=TempDir::new().unwrap(); let loc=public_workspace::locate(t.path(), crate::source_capture::ReadMode::Live).unwrap(); assert_eq!(loc.status, "missing"); assert_eq!(loc.key.len(), 64); }
}
