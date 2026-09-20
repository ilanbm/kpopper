//! Public project configuration and guidance preference.
use crate::{
    Error, Result, project_modes::Project, require, value::TypedValue as V,
};
use clap::Args;
use serde_json::{Map as JMap, Value as J, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Default, Args)]
pub struct Options {
    #[arg(long)]
    pub mode: Option<String>,
    #[arg(long)]
    pub record: Option<String>,
    #[arg(long)]
    pub check: bool,
    #[arg(long)]
    pub guidance: Option<String>,
    #[arg(long)]
    pub expected_generation: Option<u64>,
    #[arg(long)]
    pub migration_receipt: Option<PathBuf>,
    #[arg(long)]
    pub rollback: bool,
}

fn state_dir() -> Result<PathBuf> {
    let root = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/state")))
        .ok_or_else(|| Error("state_directory_unavailable".into()))?;
    Ok(root.join("kpopper/first-use"))
}
fn guidance() -> Result<bool> {
    let path = state_dir()?.join("guidance.json");
    let Ok(raw) = fs::read(path) else {
        return Ok(true);
    };
    let v: J =
        serde_json::from_slice(&raw).map_err(|_| Error("invalid_guidance_preference".into()))?;
    require(
        v.get("schema").and_then(J::as_i64) == Some(1),
        "invalid_guidance_preference",
    )?;
    v.get("enabled")
        .and_then(J::as_bool)
        .ok_or_else(|| Error("invalid_guidance_preference".into()))
}
fn set_guidance(enabled: bool) -> Result<()> {
    let path = state_dir()?.join("guidance.json");
    fs::create_dir_all(path.parent().unwrap())?;
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(
        &tmp,
        serde_json::to_vec(&json!({"schema":1,"enabled":enabled}))?,
    )?;
    fs::rename(tmp, path)?;
    Ok(())
}
fn j(v: &V) -> Result<J> {
    Ok(v.to_json()?)
}
fn v(j: &J) -> Result<V> {
    V::from_json(j)
}
fn generation(j: &J) -> Result<u64> {
    j.get("generation")
        .and_then(J::as_u64)
        .ok_or_else(|| Error("invalid_project_config".into()))
}
fn digest(path: &Path) -> Result<Option<Vec<u8>>> {
    if !path.exists() {
        return Ok(None);
    }
    require(path.is_file(), "record_is_not_a_file")?;
    Ok(Some(fs::read(path)?))
}
fn atomic(path: &Path, value: &J) -> Result<()> {
    fs::create_dir_all(path.parent().unwrap())?;
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&tmp, serde_json::to_vec(value)?)?;
    fs::rename(tmp, path)?;
    Ok(())
}

fn transition(
    project: &Project,
    current: &J,
    mode: &str,
    record: &str,
    receipt: Option<&Path>,
    rollback: bool,
) -> Result<J> {
    require(
        mode == "simple" || mode == "advanced",
        "invalid_project_mode",
    )?;
    require(
        project.is_git() || mode == "simple",
        "advanced_requires_git",
    )?;
    if mode == "advanced" {
        crate::history_branch::portable_path(record)?;
    }
    if rollback {
        require(receipt.is_some(), "rollback_requires_migration_receipt")?;
    }
    let changed = current.get("mode").and_then(J::as_str) != Some(mode)
        || current.get("record").and_then(J::as_str) != Some(record);
    if !changed {
        return Ok(
            json!({"from":current,"mode":mode,"record":record,"changed":false,"blockers":[],"snapshots":{}}),
        );
    }
    let old = project.record(None)?;
    let destination = project.root.join(record);
    let old_bytes = digest(&old)?;
    let new_bytes = digest(&destination)?;
    let mut blockers = Vec::new();
    if new_bytes.is_none() && receipt.is_none() {
        blockers.push(format!(
            "destination record is unavailable; prepare it before changing mode: {}",
            destination.display()
        ));
    }
    if old_bytes != new_bytes && old_bytes.is_some() && receipt.is_none() {
        blockers.push(format!(
            "destination record differs; reconcile its knowledge before changing mode: {}",
            destination.display()
        ));
    }
    if let Some(r) = receipt {
        require(r.is_file(), "migration_receipt_unavailable")?;
    }
    // Preview returns blockers as data; applying a blocked transition is refused below.
    Ok(
        json!({"from":current,"mode":mode,"record":record,"changed":true,"blockers":blockers,"snapshots":{},"migration":receipt.map(|p|json!({"receipt":p,"rollback":rollback}))}),
    )
}

pub fn run(options: &Options, cwd: &Path) -> Result<V> {
    let project = Project::open(cwd)?;
    let before_guidance = guidance()?;
    if let Some(g) = &options.guidance {
        if !options.check {
            set_guidance(g == "on")?;
        }
    }
    let guard = project.lock()?;
    let current = j(guard.config())?;
    let mode = options
        .mode
        .as_deref()
        .or_else(|| current.get("mode").and_then(J::as_str))
        .unwrap();
    let record = options
        .record
        .as_deref()
        .or_else(|| current.get("record").and_then(J::as_str))
        .unwrap();
    if let Some(expected) = options.expected_generation {
        require(
            generation(&current)? == expected,
            "project_configuration_changed",
        )?;
    }
    let report = if options.mode.is_some()
        || options.record.is_some()
        || options.check
        || options.migration_receipt.is_some()
    {
        Some(transition(
            &project,
            &current,
            mode,
            record,
            options.migration_receipt.as_deref(),
            options.rollback,
        )?)
    } else {
        None
    };
    if !options.check { if let Some(r) = &report { require(r["blockers"].as_array().map_or(true, |a| a.is_empty()), "configuration_transition_blocked")?; } }
    let policy = if options.check || report.as_ref().is_some_and(|r| r["changed"] == false) {
        current.clone()
    } else if options.mode.is_some() || options.record.is_some() {
        let mut next = current.clone();
        next["mode"] = J::String(mode.into());
        next["record"] = J::String(record.into());
        next["generation"] = (generation(&current)? + 1).into();
        atomic(&project.config_path, &next)?;
        next
    } else {
        current.clone()
    };
    let record_path = project.record(Some(&v(&policy)?))?;
    let mut out = JMap::new();
    out.insert(
        "guidance".into(),
        J::Bool(if options.guidance.is_some() && !options.check {
            guidance()?
        } else {
            before_guidance
        }),
    );
    out.insert("project".into(), policy);
    out.insert(
        "record".into(),
        J::String(record_path.to_string_lossy().into()),
    );
    if let Some(r) = report {
        out.insert("transition".into(), r);
    }
    Ok(v(&J::Object(out))?)
}

pub fn dispatch(
    options: &Options,
    cwd: &Path,
    json_output: bool,
) -> crate::public_pending::CommandOutput {
    match run(options, cwd) {
        Ok(value) => {
            let raw = value.to_json().unwrap();
            let code = if raw.get("transition").and_then(|x| x.get("blockers")).and_then(J::as_array).is_some_and(|a| !a.is_empty()) { 2 } else { 0 };
            crate::public_pending::CommandOutput {
                stdout: if json_output {
                    format!("{}\n", raw)
                } else {
                    format!(
                        "Mode: {}\nRecord: {}\nGuidance: {}\n",
                        raw["project"]["mode"],
                        raw["record"],
                        if raw["guidance"] == true { "on" } else { "off" }
                    )
                },
                stderr: String::new(),
                code,
            }
        }
        Err(e) => crate::public_pending::CommandOutput {
            stdout: if json_output {
                format!("{{\"error\":{}}}\n", serde_json::to_string(&e.0).unwrap())
            } else {
                String::new()
            },
            stderr: format!("{}\n", e),
            code: 2,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn inspect_plain_project_returns_policy_and_record() {
        let dir = tempfile::tempdir().unwrap();
        let result = run(&Options::default(), dir.path()).unwrap().to_json().unwrap();
        assert_eq!(result["project"]["mode"], "simple");
        assert_eq!(result["record"], dir.path().canonicalize().unwrap().join("GROUNDING.yaml").to_string_lossy().as_ref());
        assert_eq!(result["guidance"], true);
    }

    #[test]
    fn check_reports_missing_destination_without_writing_policy() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("GROUNDING.yaml"), b"known: {}\n").unwrap();
        let before = Project::open(dir.path()).unwrap().config_path;
        let result = run(&Options { mode: Some("simple".into()), record: Some("other.yaml".into()), check: true, ..Default::default() }, dir.path()).unwrap().to_json().unwrap();
        assert!(!result["transition"]["blockers"].as_array().unwrap().is_empty());
        assert!(!before.exists());
    }
}
