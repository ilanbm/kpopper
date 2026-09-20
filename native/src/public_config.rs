//! Public project configuration and guidance preference.
use crate::{
    Error, Result,
    history_contract::map,
    history_transaction_fs::DirectoryGuard,
    identity::sha256,
    project_modes::{Project, resolved},
    require,
    source_capture::{ReadMode, capture_source},
    value::TypedValue as V,
};
use clap::Args;
use serde_json::{Map as JMap, Value as J, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, Default, Args)]
pub struct Options {
    #[arg(long, value_parser = ["simple", "advanced"])]
    pub mode: Option<String>,
    #[arg(long)]
    pub record: Option<String>,
    #[arg(long)]
    pub check: bool,
    #[arg(long, value_parser = ["on", "off"])]
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
        .ok_or_else(|| Error("state directory is unavailable".into()))?;
    Ok(root.join("kpopper/first-use"))
}

fn guidance_at(state: &Path) -> Result<bool> {
    let path = state.join("guidance.json");
    let Ok(raw) = fs::read(&path) else {
        return Ok(true);
    };
    let value: J = serde_json::from_slice(&raw).map_err(|_| {
        Error(format!(
            "Invalid first-use state; keep it for inspection: {}",
            path.display()
        ))
    })?;
    require(
        value
            .as_object()
            .is_some_and(|m| m.get("schema") == Some(&json!(1))),
        &format!(
            "Invalid first-use state; keep it for inspection: {}",
            path.display()
        ),
    )?;
    value
        .get("enabled")
        .and_then(J::as_bool)
        .ok_or_else(|| Error(format!("Invalid guidance preference: {}", path.display())))
}

fn private_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    fs::create_dir_all(path)?;
    Ok(())
}

fn atomic_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| Error("invalid path".into()))?;
    private_directory(parent)?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|v| v.to_str())
            .unwrap_or("kpopper"),
        std::process::id()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> Result<()> {
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn set_guidance_at(state: &Path, enabled: bool) -> Result<()> {
    if state.join("guidance.json").exists() {
        guidance_at(state)?;
    }
    atomic_bytes(
        &state.join("guidance.json"),
        format!("{{\"schema\": 1, \"enabled\": {enabled}}}\n").as_bytes(),
    )
}

fn to_json(value: &V) -> Result<J> {
    value.to_json()
}
fn generation(value: &J) -> Result<u64> {
    value
        .get("generation")
        .and_then(J::as_u64)
        .ok_or_else(|| Error("unrecognized project configuration; it was not replaced".into()))
}
fn path_string(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| Error("nonportable project path".into()))
}
fn digest(path: &Path) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    require(
        path.is_file(),
        &format!("record is not a file: {}", path.display()),
    )?;
    Ok(Some(sha256(&fs::read(path)?)))
}
fn hypothesis_dir(record: &Path) -> PathBuf {
    if record.file_name().and_then(|n| n.to_str()) == Some("GROUNDING.yaml") {
        record.parent().unwrap().join(".kpopper/hypotheses")
    } else {
        record.parent().unwrap().join("PROVENANCE.d")
    }
}
fn has_hypotheses(record: &Path) -> Result<bool> {
    let directory = hypothesis_dir(record);
    if !directory.is_dir() {
        return Ok(false);
    }
    for item in fs::read_dir(directory)? {
        let path = item?.path();
        if path.is_file()
            && matches!(
                path.extension().and_then(|v| v.to_str()),
                Some("yaml" | "yml")
            )
        {
            return Ok(true);
        }
    }
    Ok(false)
}
fn truthy(value: Option<&V>) -> bool {
    match value {
        None | Some(V::Null) | Some(V::Bool(false)) => false,
        Some(V::Text(v)) => !v.is_empty(),
        Some(V::List(v)) => !v.is_empty(),
        Some(V::Map(v)) => !v.is_empty(),
        Some(V::Integer(v)) => v.as_str() != "0",
        Some(V::Float(v)) => v.get() != 0.0,
        _ => true,
    }
}
fn is_multifile(path: &Path) -> Result<bool> {
    let document = crate::history_yaml::decode_ordinary_source_value(&fs::read(path)?)?.projected();
    Ok(map(&document).is_ok_and(|m| truthy(m.get("record")) || truthy(m.get("also"))))
}
fn pending_ref_exists(project: &Project) -> Result<bool> {
    if !project.is_git() {
        return Ok(false);
    }
    let status = Command::new("git")
        .args([
            "--no-pager",
            "--no-replace-objects",
            "--no-lazy-fetch",
            "-C",
        ])
        .arg(&project.root)
        .args(["rev-parse", "--verify", "refs/kpopper/pending_grounding"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    Ok(status.success())
}

fn migration_evidence(
    project: &Project,
    current: &J,
    destination: &str,
    receipt: &Path,
    rollback: bool,
) -> Result<(J, BTreeMap<String, Option<String>>)> {
    let mut snapshots = BTreeMap::new();
    let mut receipts = Vec::new();
    let mut signatures = BTreeSet::new();
    for root in project.worktrees()? {
        let current_path = resolved(&root.join(current["record"].as_str().unwrap()))?;
        let destination_path = resolved(&root.join(destination))?;
        let (original, candidate) = if rollback {
            (&destination_path, &current_path)
        } else {
            (&current_path, &destination_path)
        };
        require(
            original.is_file() && candidate.is_file(),
            "migration destination is unavailable in a participating worktree",
        )?;
        let witness = if root == project.root {
            receipt.to_owned()
        } else {
            candidate
                .parent()
                .unwrap()
                .join(".kpopper-migration/receipt.json")
        };
        let proof = crate::public_expressions::validate_configuration_migration(
            original,
            candidate,
            &witness,
            rollback,
            &project.root,
        )?;
        signatures.insert(proof.source_signature);
        for (path, bytes) in proof.snapshots {
            snapshots.insert(path_string(&path)?, Some(sha256(&bytes)));
        }
        receipts.push(J::String(proof.receipt_sha256));
    }
    require(
        signatures.len() == 1,
        "worktrees have different migration source closures; reconcile before changing mode",
    )?;
    Ok((json!({"receipts":receipts,"rollback":rollback}), snapshots))
}

fn destination(project: &Project, mode: &str, record: &str, current: &J) -> Result<String> {
    if mode == "advanced" {
        require(
            project.is_git(),
            "mode needs a compatible Git or Simple project",
        )?;
        crate::history_branch::portable_path(record)
            .map_err(|_| Error("path must stay inside the project, outside .git".into()))?;
        return Ok(record.into());
    }
    let result = path_string(&resolved(&project.root.join(record))?)?;
    let changed = current.get("mode").and_then(J::as_str) != Some(mode)
        || current.get("record").and_then(J::as_str) != Some(result.as_str());
    if project.is_git() && changed {
        for root in project.worktrees()? {
            if Path::new(&result).starts_with(&root) {
                return Err(Error("Simple in Git needs an external shared record; checkout files belong to their branch".into()));
            }
        }
    }
    Ok(result)
}

fn transition_report(
    project: &Project,
    current: &J,
    mode: &str,
    record: Option<&str>,
    migration_receipt: Option<&Path>,
    rollback: bool,
) -> Result<J> {
    require(
        mode == "simple" || mode == "advanced",
        "mode needs a compatible Git or Simple project",
    )?;
    let selected = record
        .or_else(|| current.get("record").and_then(J::as_str))
        .ok_or_else(|| Error("unrecognized project configuration; it was not replaced".into()))?;
    let destination = destination(project, mode, selected, current)?;
    if rollback && migration_receipt.is_none() {
        return Err(Error(
            "rollback requires a verified migration receipt".into(),
        ));
    }
    let worktrees = project.worktrees()?;
    let mut blockers = Vec::<String>::new();
    let mut snapshots = BTreeMap::<String, Option<String>>::new();
    for root in &worktrees {
        let path = resolved(&root.join(current["record"].as_str().unwrap()))?;
        let hash = if path.exists() && !path.is_file() {
            blockers.push(format!("record is not a file: {}", path.display()));
            None
        } else {
            digest(&path)?
        };
        snapshots.insert(path_string(&path)?, hash);
        if has_hypotheses(&path)? {
            blockers.push(format!(
                "hypotheses need reconciliation: {}",
                path.display()
            ));
        }
    }
    if snapshots.values().collect::<BTreeSet<_>>().len() > 1 {
        blockers.push(
            "worktrees have different or missing records; reconcile before changing mode".into(),
        );
    }
    if pending_ref_exists(project)? {
        blockers.push(
            "durable contributions exist; reconcile their publication before changing mode".into(),
        );
    }
    let changed = current.get("mode").and_then(J::as_str) != Some(mode)
        || current.get("record").and_then(J::as_str) != Some(destination.as_str());
    let mut targets = BTreeSet::new();
    for root in &worktrees {
        targets.insert(resolved(&root.join(&destination))?);
    }
    let migration = if changed {
        migration_receipt
            .map(|receipt| migration_evidence(project, current, &destination, receipt, rollback))
            .transpose()?
    } else {
        None
    };
    if changed {
        let old_hashes = snapshots
            .values()
            .filter_map(Clone::clone)
            .collect::<BTreeSet<_>>();
        for target in &targets {
            let hash = digest(target)?;
            if let Some(hash) = &hash {
                if !old_hashes.is_empty() && !old_hashes.contains(hash) && migration.is_none() {
                    blockers.push(format!(
                        "destination record differs; reconcile its knowledge before changing mode: {}",
                        target.display()
                    ));
                }
            } else {
                blockers.push(format!(
                    "destination record is unavailable; prepare it before changing mode: {}",
                    target.display()
                ));
            }
            snapshots.entry(path_string(target)?).or_insert(hash);
            if has_hypotheses(target)? {
                blockers.push(format!(
                    "destination hypotheses need reconciliation: {}",
                    target.display()
                ));
            }
        }
        for (path, hash) in &snapshots {
            if hash.is_some() && migration.is_none() && is_multifile(Path::new(path))? {
                blockers.push(format!(
                    "multi-file record needs explicit reconciliation before changing mode: {path}"
                ));
            }
        }
        for target in &targets {
            if migration.is_none() && target.is_file() {
                let capture = capture_source(
                    std::slice::from_ref(target),
                    &project.root,
                    ReadMode::Frozen,
                    None,
                )?;
                crate::reasoning_capabilities::document_capabilities(&capture.strict_document()?)?;
            }
        }
    }
    if let Some((_, extra)) = &migration {
        snapshots.extend(extra.clone());
    }
    if !changed {
        blockers.clear();
    }
    Ok(
        json!({"from":current,"mode":mode,"record":destination,"changed":changed,
        "blockers":blockers,"snapshots":snapshots,
        "migration":migration.map(|(evidence, _)| evidence).unwrap_or(J::Null)}),
    )
}

fn canonical_json_line(value: &J) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}
fn save_history(project: &Project, current: &J, report: &J) -> Result<()> {
    use base64::Engine;
    let mut records = JMap::new();
    for (path, hash) in report["snapshots"].as_object().unwrap() {
        let value = if hash.is_null() {
            J::Null
        } else {
            J::String(base64::engine::general_purpose::STANDARD.encode(fs::read(path)?))
        };
        records.insert(path.clone(), value);
    }
    let backup = json!({"config":current,"encoding":"base64","migration":report["migration"].clone(),"records":records});
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error("system clock is unavailable".into()))?
        .as_nanos();
    atomic_bytes(
        &project.state.join(format!("mode-history/{stamp}.json")),
        &canonical_json_line(&backup)?,
    )
}
fn verify_snapshots(report: &J) -> Result<()> {
    for (path, expected) in report["snapshots"].as_object().unwrap() {
        let actual = digest(Path::new(path))?.map(J::String).unwrap_or(J::Null);
        require(
            actual == *expected,
            "record closure changed before configuration commit",
        )?;
    }
    Ok(())
}

fn configure(options: &Options, project: &Project) -> Result<(J, Option<J>)> {
    let guard = project.lock()?;
    let current = to_json(guard.config())?;
    if let Some(expected) = options.expected_generation {
        require(
            generation(&current)? == expected,
            "project configuration changed; inspect the current policy",
        )?;
    }
    let mode = options
        .mode
        .as_deref()
        .or_else(|| current.get("mode").and_then(J::as_str))
        .unwrap();
    let mut paths = project
        .worktrees()?
        .into_iter()
        .flat_map(|root| {
            let mut paths = vec![root.join(current["record"].as_str().unwrap())];
            if let Some(record) = &options.record {
                paths.push(root.join(record));
            }
            paths
        })
        .collect::<Vec<_>>();
    if options.migration_receipt.is_some() {
        let preliminary = transition_report(
            project,
            &current,
            mode,
            options.record.as_deref(),
            options.migration_receipt.as_deref(),
            options.rollback,
        )?;
        paths.extend(
            preliminary["snapshots"]
                .as_object()
                .unwrap()
                .keys()
                .map(PathBuf::from),
        );
    }
    let mut directories = paths
        .iter()
        .filter_map(|p| p.parent())
        .filter(|p| p.is_dir())
        .map(Path::canonicalize)
        .collect::<std::io::Result<Vec<_>>>()?;
    directories.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
    directories.dedup();
    let mut directory_guards = Vec::new();
    for directory in directories {
        directory_guards.push(DirectoryGuard::acquire(&directory, true)?);
    }
    let report = transition_report(
        project,
        &current,
        mode,
        options.record.as_deref(),
        options.migration_receipt.as_deref(),
        options.rollback,
    )?;
    let blockers = report["blockers"].as_array().unwrap();
    if !options.check && !blockers.is_empty() {
        return Err(Error(
            blockers
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect::<Vec<_>>()
                .join("; "),
        ));
    }
    if options.check {
        return Ok((current, Some(report)));
    }
    verify_snapshots(&report)?;
    if report["changed"] == true {
        save_history(project, &current, &report)?;
    }
    let mut next = current.clone();
    next["mode"] = report["mode"].clone();
    next["record"] = report["record"].clone();
    next["generation"] = (generation(&current)? + 1).into();
    guard.verify()?;
    atomic_bytes(&project.config_path, &canonical_json_line(&next)?)?;
    drop(directory_guards);
    Ok((next, None))
}

pub fn run(options: &Options, cwd: &Path) -> Result<V> {
    let state = state_dir()?;
    let mut guidance = guidance_at(&state)?;
    if let Some(value) = &options.guidance
        && !options.check
    {
        guidance = value == "on";
        set_guidance_at(&state, guidance)?;
    }
    let project = Project::open(cwd)?;
    let (policy, report) = if options.check || options.mode.is_some() || options.record.is_some() {
        configure(options, &project)?
    } else {
        (to_json(&project.config()?)?, None)
    };
    let record = project.record(Some(&V::from_json(&policy)?))?;
    let mut output = JMap::new();
    output.insert("guidance".into(), J::Bool(guidance));
    output.insert("project".into(), policy);
    output.insert("record".into(), J::String(path_string(&record)?));
    if let Some(report) = report {
        output.insert("transition".into(), report);
    }
    V::from_json(&J::Object(output))
}

fn object_order<'a>(map: &'a JMap<String, J>, current_record: Option<&str>) -> Vec<&'a str> {
    let preferred: &[&str] = if map.contains_key("guidance") && map.contains_key("project") {
        &["guidance", "project", "record", "transition"]
    } else if map.contains_key("version")
        && map.contains_key("mode")
        && map.contains_key("generation")
    {
        &["version", "mode", "record", "publication", "generation"]
    } else if map.contains_key("from") && map.contains_key("changed") {
        &[
            "from",
            "mode",
            "record",
            "changed",
            "blockers",
            "snapshots",
            "migration",
        ]
    } else {
        &[]
    };
    if !map.is_empty()
        && map.keys().all(|key| Path::new(key).is_absolute())
        && map
            .values()
            .all(|value| value.is_null() || value.is_string())
    {
        let mut result = map.keys().map(String::as_str).collect::<Vec<_>>();
        if let Some(current) = current_record
            && let Some(index) = result.iter().position(|key| *key == current)
        {
            result.swap(0, index);
        }
        return result;
    }
    let mut result = preferred
        .iter()
        .copied()
        .filter(|key| map.contains_key(*key))
        .collect::<Vec<_>>();
    result.extend(
        map.keys()
            .map(String::as_str)
            .filter(|key| !preferred.contains(key)),
    );
    result
}
fn render_pretty(
    value: &J,
    indent: usize,
    output: &mut String,
    current_record: Option<&str>,
) -> Result<()> {
    match value {
        J::Array(values) if values.is_empty() => output.push_str("[]"),
        J::Array(values) => {
            output.push_str("[\n");
            for (index, value) in values.iter().enumerate() {
                output.push_str(&" ".repeat(indent + 2));
                render_pretty(value, indent + 2, output, current_record)?;
                output.push_str(if index + 1 == values.len() {
                    "\n"
                } else {
                    ",\n"
                });
            }
            output.push_str(&" ".repeat(indent));
            output.push(']');
        }
        J::Object(map) if map.is_empty() => output.push_str("{}"),
        J::Object(map) => {
            output.push_str("{\n");
            let keys = object_order(map, current_record);
            for (index, key) in keys.iter().enumerate() {
                output.push_str(&" ".repeat(indent + 2));
                output.push_str(&serde_json::to_string(key)?);
                output.push_str(": ");
                render_pretty(&map[*key], indent + 2, output, current_record)?;
                output.push_str(if index + 1 == keys.len() { "\n" } else { ",\n" });
            }
            output.push_str(&" ".repeat(indent));
            output.push('}');
        }
        _ => output.push_str(&serde_json::to_string(value)?),
    }
    Ok(())
}
fn pretty(value: &J) -> Result<String> {
    let mut output = String::new();
    let current_record = value.get("record").and_then(J::as_str);
    render_pretty(value, 0, &mut output, current_record)?;
    output.push('\n');
    Ok(output)
}
fn public_error(error: Error) -> Error {
    Error(
        match error.0.as_str() {
            "invalid_project_config" => {
                "unrecognized project configuration; it was not replaced"
            }
            "advanced_requires_git" => "Advanced mode requires Git",
            "invalid_publication_config" => {
                "invalid publication configuration; no authority was inferred"
            }
            "registered_record_empty" => {
                "registered record path is empty; restore it before configuring"
            }
            "ambiguous_registered_record" => {
                "branch records and a different registered record both exist; reconcile their routing explicitly"
            }
            _ => return error,
        }
        .into(),
    )
}
pub fn dispatch(
    options: &Options,
    cwd: &Path,
    json_output: bool,
) -> crate::public_pending::CommandOutput {
    match run(options, cwd).and_then(|value| value.to_json()) {
        Ok(value) => {
            let blockers = value
                .get("transition")
                .and_then(|v| v.get("blockers"))
                .and_then(J::as_array);
            let code = if blockers.is_some_and(|v| !v.is_empty()) {
                2
            } else {
                0
            };
            let stdout = if json_output {
                pretty(&value).unwrap()
            } else {
                let mut text = format!(
                    "Mode: {}\nRecord: {}\nGuidance: {}",
                    value["project"]["mode"].as_str().unwrap(),
                    value["record"].as_str().unwrap(),
                    if value["guidance"] == true {
                        "on"
                    } else {
                        "off"
                    }
                );
                if let Some(blockers) = blockers {
                    text.push_str("\nTransition: ");
                    if blockers.is_empty() {
                        text.push_str("ready");
                    } else {
                        text.push_str(
                            &blockers
                                .iter()
                                .map(|v| v.as_str().unwrap())
                                .collect::<Vec<_>>()
                                .join("; "),
                        );
                    }
                }
                text.push('\n');
                text
            };
            crate::public_pending::CommandOutput {
                stdout,
                stderr: String::new(),
                code,
            }
        }
        Err(error) if json_output => crate::public_pending::CommandOutput {
            stdout: pretty(&json!({"error":public_error(error).to_string()})).unwrap(),
            stderr: String::new(),
            code: 2,
        },
        Err(error) => crate::public_pending::CommandOutput {
            stdout: String::new(),
            stderr: format!("{}\n", public_error(error)),
            code: 2,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn options(mode: Option<&str>, record: Option<&Path>, check: bool) -> Options {
        Options {
            mode: mode.map(str::to_owned),
            record: record.map(|p| p.to_string_lossy().into_owned()),
            check,
            ..Default::default()
        }
    }
    #[test]
    fn inspect_plain_project_has_no_side_effects() {
        let dir = tempfile::tempdir().unwrap();
        let project = Project::open(dir.path()).unwrap();
        assert_eq!(
            project.config().unwrap().to_json().unwrap()["mode"],
            "simple"
        );
        assert_eq!(
            project.record(None).unwrap(),
            dir.path().canonicalize().unwrap().join("GROUNDING.yaml")
        );
        assert!(!project.config_path.exists());
        assert!(!project.state.exists());
    }
    #[test]
    fn transition_normalizes_external_simple_record_and_keeps_hashes() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir(&repo).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q", "-b", "main"])
                .current_dir(&repo)
                .status()
                .unwrap()
                .success()
        );
        fs::write(repo.join("GROUNDING.yaml"), b"known: {}\n").unwrap();
        let shared = dir.path().join("shared.yaml");
        fs::copy(repo.join("GROUNDING.yaml"), &shared).unwrap();
        let project = Project::open(&repo).unwrap();
        let current = project.config().unwrap().to_json().unwrap();
        let report = transition_report(
            &project,
            &current,
            "simple",
            Some(shared.to_str().unwrap()),
            None,
            false,
        )
        .unwrap();
        assert_eq!(
            report["record"],
            shared.canonicalize().unwrap().to_string_lossy().as_ref()
        );
        assert_eq!(
            report["snapshots"][repo
                .canonicalize()
                .unwrap()
                .join("GROUNDING.yaml")
                .to_string_lossy()
                .as_ref()],
            sha256(b"known: {}\n")
        );
        assert_eq!(report["blockers"], json!([]));
    }
    #[test]
    fn missing_and_different_destinations_keep_snapshot_evidence() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("GROUNDING.yaml"), b"known: {one: {v: 1}}\n").unwrap();
        let project = Project::open(dir.path()).unwrap();
        let current = project.config().unwrap().to_json().unwrap();
        let missing = dir.path().join("missing.yaml");
        let report = transition_report(
            &project,
            &current,
            "simple",
            Some(missing.to_str().unwrap()),
            None,
            false,
        )
        .unwrap();
        assert!(
            report["blockers"][0]
                .as_str()
                .unwrap()
                .contains("unavailable")
        );
        assert!(
            report["snapshots"]
                .as_object()
                .unwrap()
                .contains_key(resolved(&missing).unwrap().to_str().unwrap())
        );
        let other = dir.path().join("other.yaml");
        fs::write(&other, b"known: {one: {v: 2}}\n").unwrap();
        let report = transition_report(
            &project,
            &current,
            "simple",
            Some(other.to_str().unwrap()),
            None,
            false,
        )
        .unwrap();
        assert!(
            report["blockers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v.as_str().unwrap().contains("differs"))
        );
        assert_eq!(
            report["snapshots"][resolved(&other).unwrap().to_str().unwrap()],
            sha256(b"known: {one: {v: 2}}\n")
        );
    }
    #[test]
    fn same_route_advances_generation_without_history() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("GROUNDING.yaml"), b"known: {}\n").unwrap();
        let project = Project::open(dir.path()).unwrap();
        let (policy, report) = configure(&options(Some("simple"), None, false), &project).unwrap();
        assert_eq!(policy["generation"], 1);
        assert!(report.is_none());
        assert!(project.config_path.is_file());
        assert!(!project.state.join("mode-history").exists());
    }
    #[test]
    fn check_reports_blockers_without_writing_policy() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("GROUNDING.yaml"), b"known: {}\n").unwrap();
        let project = Project::open(dir.path()).unwrap();
        let missing = dir.path().join("missing.yaml");
        let (policy, report) =
            configure(&options(Some("simple"), Some(&missing), true), &project).unwrap();
        assert_eq!(policy["generation"], 0);
        assert!(!report.unwrap()["blockers"].as_array().unwrap().is_empty());
        assert!(!project.config_path.exists());
    }
    #[test]
    fn snapshot_guard_refuses_record_bytes_changed_after_preview() {
        let dir = tempfile::tempdir().unwrap();
        let record = dir.path().join("GROUNDING.yaml");
        fs::write(&record, b"known: {}\n").unwrap();
        let project = Project::open(dir.path()).unwrap();
        let current = project.config().unwrap().to_json().unwrap();
        let report = transition_report(
            &project,
            &current,
            "simple",
            current["record"].as_str(),
            None,
            false,
        )
        .unwrap();
        fs::write(&record, b"known: {changed: {v: true}}\n").unwrap();
        assert_eq!(
            verify_snapshots(&report).unwrap_err().0,
            "record closure changed before configuration commit"
        );
    }
    #[test]
    fn rollback_requires_a_receipt() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("GROUNDING.yaml"), b"known: {}\n").unwrap();
        let other = dir.path().join("other.yaml");
        fs::copy(dir.path().join("GROUNDING.yaml"), &other).unwrap();
        let project = Project::open(dir.path()).unwrap();
        let current = project.config().unwrap().to_json().unwrap();
        assert_eq!(
            transition_report(
                &project,
                &current,
                "simple",
                Some(other.to_str().unwrap()),
                None,
                true
            )
            .unwrap_err()
            .0,
            "rollback requires a verified migration receipt"
        );
    }
}
