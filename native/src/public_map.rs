//! Public map request and the private host-agent receipt protocol.
use crate::{Error, Result, onboarding, public_workspace, require};
use clap::Args;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
use uuid::Uuid;

#[derive(Clone, Debug, Default, Args)]
pub struct Options {
    #[arg(long)]
    pub deep: bool,
}
#[derive(Clone, Debug, Args)]
pub struct AgentOptions {
    #[command(subcommand)]
    pub command: Option<AgentCommand>,
}
#[derive(Clone, Debug, clap::Subcommand)]
pub enum AgentCommand {
    Status,
    Guide,
    Task,
    Shown {
        event: String,
    },
    Accept {
        #[arg(long)]
        request: String,
    },
    Complete {
        #[arg(long)]
        request: String,
        #[arg(long)]
        report: String,
    },
    Fail {
        #[arg(long)]
        request: String,
        #[arg(long)]
        reason: String,
    },
}

fn session() -> Option<String> {
    std::env::var("KPOPPER_AGENT_SESSION")
        .ok()
        .or_else(|| std::env::var("CODEX_THREAD_ID").ok())
        .filter(|v| {
            !v.is_empty()
                && v.len() <= 200
                && v.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
}
fn read_job(key: &str) -> Result<Option<Value>> {
    onboarding::read(&onboarding::project_dir_key(key)?.join("mapping.json"))
}
fn valid_hex_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
fn validate_job(job: &Value) -> Result<()> {
    require(
        matches!(
            job.get("mode").and_then(Value::as_str),
            Some("map" | "deep")
        ),
        "Invalid mapping task state",
    )?;
    require(
        ["ready", "running", "complete", "failed"]
            .contains(&job.get("mapping").and_then(Value::as_str).unwrap_or("")),
        "Invalid mapping task state",
    )?;
    require(
        job.get("request")
            .and_then(Value::as_str)
            .is_some_and(valid_hex_id),
        "Invalid mapping request ID",
    )?;
    let owner = job.get("owner").and_then(Value::as_str).unwrap_or("");
    require(
        !owner.is_empty()
            && owner.len() <= 200
            && owner
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
        "Invalid mapping owner",
    )
}
fn lock_path(key: &str) -> Result<PathBuf> {
    Ok(onboarding::project_dir_key(key)?.join("choice.lock"))
}
fn with_lock<T>(key: &str, f: impl FnOnce() -> Result<T>) -> Result<T> {
    let path = lock_path(key)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(path)?;
    fs2::FileExt::lock_exclusive(&file).map_err(|e| Error(e.to_string()))?;
    let result = f();
    let _ = fs2::FileExt::unlock(&file);
    result
}
pub fn packet(location: &public_workspace::Location, job: &Value) -> Result<Value> {
    validate_job(job)?;
    let request = job
        .get("request")
        .and_then(Value::as_str)
        .ok_or_else(|| Error("invalid mapping request".into()))?;
    Ok(
        json!({"request":request,"status":job["mapping"],"mode":job["mode"],"owner":job["owner"],"workspace":location.workspace,
        "record":location.record,"instructions":include_str!("../../scripts/start-guide.md"),
        "protocol":{"accept":protocol_argv(location,"accept",request,None,None),"complete":protocol_argv(location,"complete",request,Some("REPORT_PATH"),None),"fail":protocol_argv(location,"fail",request,None,Some("REASON"))}}),
    )
}
fn protocol_argv(
    location: &public_workspace::Location,
    action: &str,
    request: &str,
    report: Option<&str>,
    reason: Option<&str>,
) -> Vec<String> {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok())
        .unwrap_or_else(|| PathBuf::from("kpop"));
    let mut v = vec![
        exe.to_string_lossy().into_owned(),
        "--workspace".into(),
        location.workspace.to_string_lossy().into_owned(),
        "_agent".into(),
        action.into(),
        "--request".into(),
        request.into(),
    ];
    if let Some(p) = report {
        v.extend(["--report".into(), p.into()]);
    }
    if let Some(r) = reason {
        v.extend(["--reason".into(), r.into()]);
    }
    v
}
pub fn request(workspace: &Path, deep: bool) -> Result<Value> {
    let owner = session().ok_or_else(|| Error("No calling agent session is bound. Mapping runs inside an active agent session; no work was started.".into()))?;
    let loc = public_workspace::locate(workspace, crate::source_capture::ReadMode::Live)?;
    require(loc.status != "unavailable", &loc.reason)?;
    with_lock(&loc.key, || {
        if let Some(old) = read_job(&loc.key)? {
            validate_job(&old)?;
            let state = old.get("mapping").and_then(Value::as_str).unwrap_or("");
            if ["ready", "running"].contains(&state)
                && old.get("owner").and_then(Value::as_str) != Some(&owner)
            {
                return Err(Error("Another agent session owns a pending mapping. Finish or fail that task before starting another.".into()));
            }
            if ["ready", "running"].contains(&state)
                && old.get("owner").and_then(Value::as_str) == Some(&owner)
                && old.get("mode").and_then(Value::as_str)
                    == Some(if deep { "deep" } else { "map" })
            {
                return packet(&loc, &old);
            }
        }
        let job = json!({"mode":if deep{"deep"}else{"map"},"mapping":"ready","request":Uuid::new_v4().simple().to_string(),"owner":owner});
        onboarding::write(
            &onboarding::project_dir_key(&loc.key)?.join("mapping.json"),
            &job,
        )?;
        packet(&loc, &job)
    })
}
pub fn transition(
    workspace: &Path,
    request: &str,
    action: &str,
    report: Option<&str>,
    reason: Option<&str>,
) -> Result<Value> {
    let loc = public_workspace::locate(workspace, crate::source_capture::ReadMode::Live)?;
    with_lock(&loc.key, || {
        let mut job = read_job(&loc.key)?.ok_or_else(|| {
            Error("This mapping belongs to a different request or agent session.".into())
        })?;
        validate_job(&job)?;
        require(
            job.get("request").and_then(Value::as_str) == Some(request),
            "This mapping belongs to a different request or agent session.",
        )?;
        require(
            job.get("owner").and_then(Value::as_str) == session().as_deref(),
            "This mapping belongs to a different request or agent session.",
        )?;
        let state = job["mapping"].as_str().unwrap_or("");
        match action {
            "accept" => {
                require(
                    ["ready", "running"].contains(&state),
                    "This mapping cannot be accepted in its current state.",
                )?;
                job["mapping"] = json!("running");
            }
            "fail" => {
                require(
                    ["ready", "running"].contains(&state)
                        && reason.is_some_and(|s| !s.trim().is_empty()),
                    "An active mapping and a failure reason are required.",
                )?;
                job["mapping"] = json!("failed");
                job["error"] = json!(reason.unwrap().trim());
            }
            "complete" => {
                require(
                    state == "running",
                    "Accept the mapping before completing it.",
                )?;
                complete_job(
                    &mut job,
                    report.unwrap_or(""),
                    &loc,
                    &onboarding::project_dir_key(&loc.key)?.join("mapping.json"),
                    &run_record_check,
                )?;
            }
            _ => return Err(Error("unknown mapping transition".into())),
        }
        onboarding::write(
            &onboarding::project_dir_key(&loc.key)?.join("mapping.json"),
            &job,
        )?;
        Ok(job)
    })
}
fn complete_job(
    job: &mut Value,
    report: &str,
    loc: &public_workspace::Location,
    job_path: &Path,
    check: &dyn Fn(&Path) -> Value,
) -> Result<()> {
    let expanded = if let Some(rest) = report.strip_prefix("~/") {
        std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(rest))
            .ok_or_else(|| Error("home_directory_unavailable".into()))?
    } else {
        PathBuf::from(report)
    };
    let path = std::path::absolute(expanded).map_err(|_| {
        Error("Completion requires the actual, nonempty report or knowledge record.".into())
    })?;
    require(
        path.is_file() && fs::metadata(&path)?.len() > 0,
        "Completion requires the actual, nonempty report or knowledge record.",
    )?;
    if loc.record.is_file() {
        job["check"] = check(&loc.workspace);
        if job["check"]
            .get("exit_code")
            .and_then(Value::as_i64)
            .is_none()
        {
            onboarding::write(job_path, job)?;
            return Err(Error(
                "The record check could not finish; mapping remains running.".into(),
            ));
        }
    } else {
        job["check"] = Value::Null;
    }
    job["mapping"] = json!("complete");
    job["report"] = json!(path);
    Ok(())
}
fn run_command_check(program: &Path, args: &[String], timeout: Duration) -> Value {
    let mut command = Command::new(program);
    command.args(args);
    match crate::reasoning_runtime::run_command_capture(
        &mut command,
        Vec::new(),
        timeout,
        1_048_576,
    ) {
        Ok(output) => {
            json!({"exit_code":output.status.code(), "output":String::from_utf8_lossy(&output.stdout), "error":String::from_utf8_lossy(&output.stderr)})
        }
        Err(error) => json!({"exit_code":null, "error":match error.0.as_str() {
            "runtime_timeout" => "record check timed out",
            "output_limit" => "record check output exceeded 1048576 bytes",
            _ => &error.0,
        }}),
    }
}
fn run_record_check(workspace: &Path) -> Value {
    let exe = match std::env::current_exe().and_then(|p| p.canonicalize()) {
        Ok(v) => v,
        Err(e) => return json!({"exit_code":null,"error":e.to_string()}),
    };
    run_command_check(
        &exe,
        &[
            "--workspace".into(),
            workspace.to_string_lossy().into_owned(),
            "check".into(),
        ],
        Duration::from_secs(60),
    )
}
pub fn run(options: &Options, workspace: &Path) -> Result<Value> {
    request(workspace, options.deep)
}
pub fn run_agent(options: &AgentOptions, workspace: &Path) -> Result<Value> {
    if matches!(options.command, Some(AgentCommand::Guide)) {
        return Ok(Value::String(
            include_str!("../../scripts/start-guide.md").into(),
        ));
    }
    let loc = public_workspace::locate(workspace, crate::source_capture::ReadMode::Live)?;
    match &options.command {
        None => Ok(Value::String(onboarding::context(&loc)?)),
        Some(AgentCommand::Guide) => unreachable!(),
        Some(AgentCommand::Status) => onboarding::status_at(
            &loc.workspace,
            &loc.key,
            &loc.record,
            &loc.status,
            &loc.reason,
        ),
        Some(AgentCommand::Task) => {
            let job = read_job(&loc.key)?.ok_or_else(|| {
                Error("This mapping belongs to a different request or agent session.".into())
            })?;
            require(
                job.get("owner").and_then(Value::as_str) == session().as_deref(),
                "This mapping belongs to a different request or agent session.",
            )?;
            packet(&loc, &job)
        }
        Some(AgentCommand::Shown { event }) => {
            onboarding::mark_key(&loc.workspace, &loc.key, &loc.record, &loc.status, event)
        }
        Some(AgentCommand::Accept { request }) => {
            transition(&loc.workspace, request, "accept", None, None)
        }
        Some(AgentCommand::Complete { request, report }) => {
            transition(&loc.workspace, request, "complete", Some(report), None)
        }
        Some(AgentCommand::Fail { request, reason }) => {
            transition(&loc.workspace, request, "fail", None, Some(reason))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    #[test]
    fn completion_failure_is_persisted_without_completing_mapping() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("GROUNDING.yaml"), "known: {}\n").unwrap();
        let loc =
            public_workspace::locate(temp.path(), crate::source_capture::ReadMode::Live).unwrap();
        let report = temp.path().join("report.md");
        fs::write(&report, "fixture report\n").unwrap();
        let job_path = temp.path().join("state/mapping.json");
        for reason in [
            "record check timed out",
            "record check output exceeded 1048576 bytes",
            "launch failed",
        ] {
            let mut job = json!({"mapping":"running", "owner":"fixture"});
            let checker = |_: &Path| json!({"exit_code":null,"error":reason});
            assert!(
                complete_job(
                    &mut job,
                    report.to_str().unwrap(),
                    &loc,
                    &job_path,
                    &checker
                )
                .is_err()
            );
            let saved = onboarding::read(&job_path).unwrap().unwrap();
            assert_eq!(saved["mapping"], "running");
            assert_eq!(saved["check"]["error"], reason);
            assert!(saved.get("report").is_none());
        }
        let mut job = json!({"mapping":"running"});
        complete_job(
            &mut job,
            report.to_str().unwrap(),
            &loc,
            &job_path,
            &|_| json!({"exit_code":1,"output":"record needs attention","error":""}),
        )
        .unwrap();
        assert_eq!(job["mapping"], "complete");
        assert_eq!(job["check"]["exit_code"], 1);
    }
    #[test]
    fn project_location_is_stable() {
        let t = TempDir::new().unwrap();
        let loc =
            public_workspace::locate(t.path(), crate::source_capture::ReadMode::Live).unwrap();
        assert_eq!(loc.status, "missing");
        assert_eq!(loc.key.len(), 64);
    }
    #[cfg(unix)]
    #[test]
    fn record_check_drains_large_pipe() {
        let v = run_command_check(
            Path::new("/bin/sh"),
            &["-c".into(), "yes x | head -c 2000000".into()],
            Duration::from_secs(5),
        );
        assert!(v["exit_code"].is_null());
        assert_eq!(v["error"], "record check output exceeded 1048576 bytes");
    }
    #[cfg(unix)]
    #[test]
    fn record_check_timeout_reaps_child() {
        let v = run_command_check(
            Path::new("/bin/sh"),
            &["-c".into(), "sleep 2".into()],
            Duration::from_millis(30),
        );
        assert!(v["exit_code"].is_null());
        assert_eq!(v["error"], "record check timed out");
    }
}
