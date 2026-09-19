//! Public mark/gate commands and native hook dispatch; no hook installation.
use crate::{
    Error, Result,
    public_core_readers::Output,
    public_workspace as W, require, session_activity,
    session_gate::{self, GateOptions},
    source_capture::ReadMode,
};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, clap::Args)]
pub struct Options {
    pub state_path: PathBuf,
    pub paths: Vec<PathBuf>,
    #[arg(long, default_value_t = 0)]
    pub turns: u64,
    #[arg(long,value_parser=["claude","codex"])]
    pub host: Option<String>,
    #[arg(long)]
    pub nudged_at: Option<u64>,
    #[arg(long)]
    pub session: Option<String>,
}
#[derive(Clone, Debug, Default, clap::Args)]
pub struct HookOptions {
    #[arg(long,value_parser=["claude","codex"])]
    pub host: Option<String>,
}
pub fn valid_session(sid: &str) -> bool {
    !sid.is_empty()
        && sid.len() <= 200
        && sid
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
}
pub fn run(kind: &str, options: &Options, cwd: &Path, mode: ReadMode) -> Result<Output> {
    let paths = if options.paths.is_empty() {
        W::records(cwd)?
    } else {
        options.paths.iter().map(|p| cwd.join(p)).collect()
    };
    let state_path = cwd.join(&options.state_path);
    let runtime = W::runtime_for_paths(&paths, cwd, None)?;
    let tmp = session_activity::temporary_directory();
    let request = GateOptions {
        state_path: &state_path,
        paths: &paths,
        workspace: cwd,
        read_mode: mode,
        runtime: runtime.as_ref(),
        turns: options.turns,
        host: options.host.as_deref(),
        nudged_at: options.nudged_at,
        session_id: options.session.as_deref(),
        private_tmp: Some(&tmp),
    };
    if kind == "mark" {
        session_gate::mark(&request)?;
        return Ok(Output {
            text: String::new(),
            code: 0,
        });
    }
    let result = session_gate::gate(&request)?;
    let Some(sid) = &options.session else {
        return Ok(Output {
            text: result.text,
            code: result.code,
        });
    };
    if result.code != 2 {
        return Ok(Output {
            text: String::new(),
            code: result.code,
        });
    }
    let mut issues = result
        .issues
        .into_iter()
        .map(|issue| (issue.kind, issue.subject, issue.text))
        .collect::<Vec<_>>();
    if issues.is_empty() {
        let message = if result.text.trim().is_empty() {
            "The record assessment could not complete."
        } else {
            result.text.trim()
        };
        issues.push(("assessment".into(), message.into(), message.into()));
    }
    let fresh = session_activity::deliver_stop(&tmp, sid, &issues, &paths);
    Ok(Output {
        code: if fresh.is_empty() { 0 } else { 2 },
        text: if fresh.is_empty() {
            String::new()
        } else {
            fresh.join("\n") + "\n"
        },
    })
}

fn private_json(path: &Path) -> Result<Value> {
    let meta = fs::symlink_metadata(path)?;
    require(
        meta.is_file() && !meta.file_type().is_symlink() && meta.len() <= 1024 * 1024,
        "invalid private session state",
    )?;
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    require(bytes.len() <= 1024 * 1024, "private session state limit")?;
    Ok(serde_json::from_slice(&bytes)?)
}
fn empty_mark(path: &Path) -> Result<()> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        require(
            meta.is_file() && !meta.file_type().is_symlink(),
            "invalid session baseline destination",
        )?;
    }
    let parent = path
        .parent()
        .ok_or_else(|| Error("invalid session baseline destination".into()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(
        json!({"fails":0,"failures":[],"unserved":[],"ids":[],"judgments":{}})
            .to_string()
            .as_bytes(),
    )?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|e| Error(e.error.to_string()))?;
    Ok(())
}

/// Preserve an existing opener mark on resume/compaction. Missing records get a
/// private empty mark so their first write can be assessed without creating a record.
pub fn start_mark(cwd: &Path, payload: &Value, mode: ReadMode) -> Result<()> {
    let Some(sid) = payload["session_id"].as_str().filter(|s| valid_session(s)) else {
        return Ok(());
    };
    let state_path = session_activity::temporary_directory().join(format!("kpopper-base-{sid}"));
    if matches!(payload["source"].as_str(), Some("compact" | "resume")) && state_path.exists() {
        return Ok(());
    }
    let location = W::locate(cwd, mode)?;
    if location.status == "unavailable" {
        return Ok(());
    }
    if location.status == "missing" {
        return empty_mark(&state_path);
    }
    run(
        "mark",
        &Options {
            state_path,
            paths: vec![location.record],
            turns: 0,
            host: None,
            nudged_at: None,
            session: None,
        },
        &location.workspace,
        mode,
    )?;
    Ok(())
}

/// Stop always reassesses; the private delivery ledger decides which messages are
/// fresh. An unavailable assessment is reported to the caller, not treated as proof.
pub fn stop(payload: &Value, host: Option<&str>, mode: ReadMode) -> Result<Output> {
    require(payload.is_object(), "invalid_hook_payload")?;
    let Some(sid) = payload["session_id"].as_str().filter(|s| valid_session(s)) else {
        return Ok(Output {
            text: String::new(),
            code: 0,
        });
    };
    let cwd = PathBuf::from(
        payload["cwd"]
            .as_str()
            .ok_or_else(|| Error("missing_workspace".into()))?,
    );
    let tmp = session_activity::temporary_directory();
    let state_path = tmp.join(format!("kpopper-base-{sid}"));
    if !state_path.is_file() {
        return Ok(Output {
            text: String::new(),
            code: 0,
        });
    }
    let location = W::locate(&cwd, mode)?;
    let ground =
        private_json(&tmp.join(format!("kpopper-ground-{sid}.json"))).unwrap_or(Value::Null);
    run(
        "gate",
        &Options {
            state_path,
            paths: vec![location.record],
            turns: ground["turns"].as_u64().unwrap_or(0),
            host: host.map(str::to_owned),
            nudged_at: ground["nudged_turn"].as_u64(),
            session: Some(sid.into()),
        },
        &location.workspace,
        mode,
    )
}
