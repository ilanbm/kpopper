//! Public map request and the private host-agent receipt protocol.
use crate::{Error, Result, require, onboarding};
use clap::Args;
use serde_json::{Value, json};
use std::{fs, path::{Path, PathBuf}};
use uuid::Uuid;

#[derive(Clone, Debug, Default, Args)]
pub struct Options { #[arg(long)] pub deep: bool }

fn session() -> Option<String> {
    std::env::var("KPOPPER_AGENT_SESSION").ok().or_else(|| std::env::var("CODEX_THREAD_ID").ok())
        .filter(|v| !v.is_empty() && v.len() <= 200 && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
}
fn location(workspace: &Path) -> (PathBuf, bool) {
    let root = workspace.canonicalize().unwrap_or_else(|_| workspace.to_path_buf());
    let record = ["GROUNDING.yaml", "PROVENANCE.yaml"].iter().map(|n| root.join(n)).find(|p| p.exists()).unwrap_or_else(|| root.join("GROUNDING.yaml"));
    let found = record.is_file(); (record, found)
}
fn read_job(workspace: &Path) -> Result<Option<Value>> { onboarding::read(&onboarding::project_dir(workspace)?.join("mapping.json")) }
fn lock_path(workspace: &Path) -> Result<PathBuf> { Ok(onboarding::project_dir(workspace)?.join("choice.lock")) }
fn with_lock<T>(workspace: &Path, f: impl FnOnce() -> Result<T>) -> Result<T> {
    let path = lock_path(workspace)?; if let Some(parent) = path.parent() { fs::create_dir_all(parent)?; }
    let file = fs::OpenOptions::new().create(true).read(true).write(true).open(path)?;
    fs2::FileExt::lock_exclusive(&file).map_err(|e| Error(e.to_string()))?;
    let result = f(); let _ = fs2::FileExt::unlock(&file); result
}
pub fn packet(workspace: &Path, job: &Value) -> Result<Value> {
    let request = job.get("request").and_then(Value::as_str).ok_or_else(|| Error("invalid mapping request".into()))?;
    Ok(json!({"request":request,"status":job["mapping"],"mode":job["mode"],"owner":job["owner"],"workspace":workspace,
        "record":location(workspace).0,"instructions":"Map existing materials within the agreed scope. Return a report; protocol completion requires an actual nonempty report.",
        "protocol":{"accept":["kpop","_agent","accept","--request",request],"complete":["kpop","_agent","complete","--request",request,"--report","REPORT_PATH"],"fail":["kpop","_agent","fail","--request",request,"--reason","REASON"]}}))
}
pub fn request(workspace: &Path, deep: bool) -> Result<Value> {
    let owner = session().ok_or_else(|| Error("No calling agent session is bound. Mapping runs inside an active agent session; no work was started.".into()))?;
    let (record, found) = location(workspace); require(!record.exists() || found, "The record path exists but is not an accessible file.")?;
    with_lock(workspace, || {
        if let Some(old) = read_job(workspace)? {
            let state = old.get("mapping").and_then(Value::as_str).unwrap_or("");
            if ["ready","running"].contains(&state) && old.get("owner").and_then(Value::as_str) != Some(&owner) { return Err(Error("Another agent session owns a pending mapping. Finish or fail that task before starting another.".into())); }
            if ["ready","running"].contains(&state) && old.get("owner").and_then(Value::as_str) == Some(&owner) && old.get("mode").and_then(Value::as_str) == Some(if deep{"deep"}else{"map"}) { return packet(workspace, &old); }
        }
        let job = json!({"mode":if deep{"deep"}else{"map"},"mapping":"ready","request":Uuid::new_v4().simple().to_string(),"owner":owner});
        onboarding::write(&onboarding::project_dir(workspace)?.join("mapping.json"), &job)?; packet(workspace, &job)
    })
}
pub fn transition(workspace: &Path, request: &str, action: &str, report: Option<&str>, reason: Option<&str>) -> Result<Value> {
    with_lock(workspace, || {
        let mut job = read_job(workspace)?.ok_or_else(|| Error("This mapping belongs to a different request or agent session.".into()))?;
        require(job.get("request").and_then(Value::as_str) == Some(request), "This mapping belongs to a different request or agent session.")?;
        require(job.get("owner").and_then(Value::as_str) == session().as_deref(), "This mapping belongs to a different request or agent session.")?;
        let state = job["mapping"].as_str().unwrap_or("");
        match action {
            "accept" => { require(["ready","running"].contains(&state), "This mapping cannot be accepted in its current state.")?; job["mapping"] = json!("running"); }
            "fail" => { require(["ready","running"].contains(&state) && reason.is_some_and(|s| !s.trim().is_empty()), "An active mapping and a failure reason are required.")?; job["mapping"]=json!("failed"); job["error"]=json!(reason.unwrap().trim()); }
            "complete" => { require(state == "running", "Accept the mapping before completing it.")?; let p=PathBuf::from(report.unwrap_or("")).canonicalize().map_err(|_| Error("Completion requires the actual, nonempty report or knowledge record.".into()))?; require(p.is_file() && fs::metadata(&p)?.len()>0, "Completion requires the actual, nonempty report or knowledge record.")?; job["mapping"]=json!("complete"); job["report"]=json!(p); }
            _ => return Err(Error("unknown mapping transition".into()))
        }
        onboarding::write(&onboarding::project_dir(workspace)?.join("mapping.json"), &job)?; Ok(job)
    })
}
pub fn run(options: &Options, workspace: &Path) -> Result<Value> { request(workspace, options.deep) }

#[cfg(test)]
mod tests {
    use super::*; use tempfile::TempDir;
    #[test] fn project_location_is_stable() { let t=TempDir::new().unwrap(); let (path, found)=location(t.path()); assert!(!found); assert_eq!(path, t.path().canonicalize().unwrap().join("GROUNDING.yaml")); assert_eq!(onboarding::project_key(t.path()).len(), 64); }
}
