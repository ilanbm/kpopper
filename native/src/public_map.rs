//! Public map request and the private host-agent receipt protocol.
use crate::{Error, Result, require, onboarding, public_workspace};
use clap::Args;
use serde_json::{Value, json};
use std::{fs, io::Read, path::{Path, PathBuf}, process::{Command, Stdio}, thread, time::{Duration, Instant}};
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
fn valid_hex_id(value: &str) -> bool { value.len() == 32 && value.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) }
fn validate_job(job: &Value) -> Result<()> {
    require(matches!(job.get("mode").and_then(Value::as_str), Some("map"|"deep")), "Invalid mapping task state")?;
    require(onboarding::MAPPING_STATES.contains(&job.get("mapping").and_then(Value::as_str).unwrap_or("")), "Invalid mapping task state")?;
    require(job.get("request").and_then(Value::as_str).is_some_and(valid_hex_id), "Invalid mapping request ID")?;
    let owner=job.get("owner").and_then(Value::as_str).unwrap_or("");
    require(!owner.is_empty() && owner.len()<=200 && owner.bytes().all(|b| b.is_ascii_alphanumeric()||b==b'_'||b==b'-'), "Invalid mapping owner")
}
fn lock_path(key: &str) -> Result<PathBuf> { Ok(onboarding::project_dir_key(key)?.join("choice.lock")) }
fn with_lock<T>(key: &str, f: impl FnOnce() -> Result<T>) -> Result<T> {
    let path = lock_path(key)?; if let Some(parent) = path.parent() { fs::create_dir_all(parent)?; }
    let file = fs::OpenOptions::new().create(true).read(true).write(true).open(path)?;
    fs2::FileExt::lock_exclusive(&file).map_err(|e| Error(e.to_string()))?;
    let result = f(); let _ = fs2::FileExt::unlock(&file); result
}
pub fn packet(location: &public_workspace::Location, job: &Value) -> Result<Value> {
    validate_job(job)?; let request = job.get("request").and_then(Value::as_str).ok_or_else(|| Error("invalid mapping request".into()))?;
    Ok(json!({"request":request,"status":job["mapping"],"mode":job["mode"],"owner":job["owner"],"workspace":location.workspace,
        "record":location.record,"instructions":include_str!("../../scripts/start-guide.md"),
        "protocol":{"accept":protocol_argv(location,"accept",request,None,None),"complete":protocol_argv(location,"complete",request,Some("REPORT_PATH"),None),"fail":protocol_argv(location,"fail",request,None,Some("REASON"))}}))
}
fn protocol_argv(location: &public_workspace::Location, action: &str, request: &str, report: Option<&str>, reason: Option<&str>) -> Vec<String> {
    let exe=std::env::current_exe().ok().and_then(|p| p.canonicalize().ok()).unwrap_or_else(|| PathBuf::from("kpop"));
    let mut v=vec![exe.to_string_lossy().into_owned(),"--workspace".into(),location.workspace.to_string_lossy().into_owned(),"_agent".into(),action.into(),"--request".into(),request.into()];
    if let Some(p)=report { v.extend(["--report".into(),p.into()]); } if let Some(r)=reason { v.extend(["--reason".into(),r.into()]); } v
}
pub fn request(workspace: &Path, deep: bool) -> Result<Value> {
    let owner = session().ok_or_else(|| Error("No calling agent session is bound. Mapping runs inside an active agent session; no work was started.".into()))?;
    let loc = public_workspace::locate(workspace, crate::source_capture::ReadMode::Live)?;
    require(loc.status != "unavailable", &loc.reason)?;
    with_lock(&loc.key, || {
        if let Some(old) = read_job(&loc.key)? { validate_job(&old)?;
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
        let mut job = read_job(&loc.key)?.ok_or_else(|| Error("This mapping belongs to a different request or agent session.".into()))?; validate_job(&job)?;
        require(job.get("request").and_then(Value::as_str) == Some(request), "This mapping belongs to a different request or agent session.")?;
        require(job.get("owner").and_then(Value::as_str) == session().as_deref(), "This mapping belongs to a different request or agent session.")?;
        let state = job["mapping"].as_str().unwrap_or("");
        match action {
            "accept" => { require(["ready","running"].contains(&state), "This mapping cannot be accepted in its current state.")?; job["mapping"] = json!("running"); }
            "fail" => { require(["ready","running"].contains(&state) && reason.is_some_and(|s| !s.trim().is_empty()), "An active mapping and a failure reason are required.")?; job["mapping"]=json!("failed"); job["error"]=json!(reason.unwrap().trim()); }
            "complete" => { require(state == "running", "Accept the mapping before completing it.")?; let raw=report.unwrap_or(""); let expanded=if let Some(rest)=raw.strip_prefix("~/") { std::env::var_os("HOME").map(|h|PathBuf::from(h).join(rest)).ok_or_else(||Error("home_directory_unavailable".into()))? } else { PathBuf::from(raw) }; let p=expanded.canonicalize().map_err(|_| Error("Completion requires the actual, nonempty report or knowledge record.".into()))?; require(p.is_file() && fs::metadata(&p)?.len()>0, "Completion requires the actual, nonempty report or knowledge record.")?; if loc.record.is_file() { let check=run_record_check(&loc.workspace); if !check.get("exit_code").is_some_and(Value::is_number) { job["check"]=check; onboarding::write(&onboarding::project_dir_key(&loc.key)?.join("mapping.json"), &job)?; return Err(Error("The record check could not finish; mapping remains running.".into())); } job["check"]=check; } job["mapping"]=json!("complete"); job["report"]=json!(p); }
            _ => return Err(Error("unknown mapping transition".into()))
        }
        onboarding::write(&onboarding::project_dir_key(&loc.key)?.join("mapping.json"), &job)?; Ok(job)
    })
}
struct Drained { bytes: Vec<u8>, overflow: bool }
fn drain(mut stream: impl Read + Send + 'static) -> thread::JoinHandle<Drained> {
    thread::spawn(move || { let mut out=Vec::new(); let mut overflow=false; let mut buf=[0u8;8192]; while let Ok(n)=stream.read(&mut buf) { if n==0 { break; } if out.len()<1_048_576 { let take=(1_048_576-out.len()).min(n); out.extend_from_slice(&buf[..take]); if take<n { overflow=true; } } else { overflow=true; } } Drained { bytes:out, overflow } })
}
fn run_command_check(program: &Path, args: &[String], timeout: Duration) -> Value {
    let mut command=Command::new(program); command.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)] std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child=match command.spawn() { Ok(v)=>v, Err(e)=>return json!({"exit_code":null,"error":e.to_string()}) };
    let stdout=drain(child.stdout.take().unwrap()); let stderr=drain(child.stderr.take().unwrap()); let started=Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => { let out=stdout.join().unwrap_or(Drained{bytes:Vec::new(),overflow:true}); let err=stderr.join().unwrap_or(Drained{bytes:Vec::new(),overflow:true}); if out.overflow || err.overflow { return json!({"exit_code":Value::Null,"output":String::from_utf8_lossy(&out.bytes),"error":"record check output exceeded 1048576 bytes"}); } return json!({"exit_code":status.code(),"output":String::from_utf8_lossy(&out.bytes),"error":String::from_utf8_lossy(&err.bytes)}); }
            Ok(None) if started.elapsed() <= timeout => thread::sleep(Duration::from_millis(10)),
            Ok(None) => { kill_process_group(&mut child); let out=stdout.join().unwrap_or(Drained{bytes:Vec::new(),overflow:false}); let err=stderr.join().unwrap_or(Drained{bytes:Vec::new(),overflow:false}); return json!({"exit_code":Value::Null,"error":"record check timed out","output":String::from_utf8_lossy(&out.bytes),"stderr":String::from_utf8_lossy(&err.bytes)}); }
            Err(_) => { kill_process_group(&mut child); let out=stdout.join().unwrap_or(Drained{bytes:Vec::new(),overflow:false}); let err=stderr.join().unwrap_or(Drained{bytes:Vec::new(),overflow:false}); return json!({"exit_code":Value::Null,"error":"record check failed while polling","output":String::from_utf8_lossy(&out.bytes),"stderr":String::from_utf8_lossy(&err.bytes)}); }
        }
    }
}
fn kill_process_group(child: &mut std::process::Child) { #[cfg(unix)] { let pid=child.id() as i32; let _=nix::sys::signal::kill(nix::unistd::Pid::from_raw(-pid), nix::sys::signal::Signal::SIGKILL); } let _=child.kill(); let _=child.wait(); }
fn run_record_check(workspace: &Path) -> Value {
    let exe=match std::env::current_exe().and_then(|p| p.canonicalize()) { Ok(v)=>v, Err(e)=>return json!({"exit_code":null,"error":e.to_string()}) };
    run_command_check(&exe, &["--workspace".into(),workspace.to_string_lossy().into_owned(),"--json".into(),"check".into()], Duration::from_secs(60))
}
pub fn run(options: &Options, workspace: &Path) -> Result<Value> { request(workspace, options.deep) }
pub fn run_agent(options: &AgentOptions, workspace: &Path) -> Result<Value> {
    let loc = public_workspace::locate(workspace, crate::source_capture::ReadMode::Live)?;
    match &options.command {
        AgentCommand::Status => onboarding::status_at(&loc.workspace, &loc.key, &loc.record, &loc.status, &loc.reason),
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
    #[test] fn record_check_drains_large_pipe() { let v=run_command_check(Path::new("/bin/sh"), &["-c".into(),"yes x | head -c 2000000".into()], Duration::from_secs(5)); assert!(v["exit_code"].is_null()); assert_eq!(v["error"],"record check output exceeded 1048576 bytes"); assert!(v["output"].as_str().unwrap().len() <= 1_048_576); }
    #[test] fn record_check_timeout_reaps_child() { let v=run_command_check(Path::new("/bin/sh"), &["-c".into(),"sleep 2".into()], Duration::from_millis(30)); assert!(v["exit_code"].is_null()); assert_eq!(v["error"],"record check timed out"); }
}
