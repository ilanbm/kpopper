//! Native session start mark and stop gate over one immutable source capture.
//! Optional page/HTML coverage belongs to Hub and is intentionally absent here.

use crate::{
    Error, Result,
    history_contract::{map, string_is, text},
    public_ordinary_readers::{
        GateData as OrdinaryData, GateJudgment as OrdinaryJudgment, Projection,
    },
    reasoning_context::CapturedAssessment,
    reasoning_runtime::{OperationalBounds, Runtime},
    source_capture::{CapturedSource, ReadMode, capture_source_with_runtime},
    value::TypedValue as V,
};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

const MAX_STATE: usize = 1024 * 1024;
const TREE_FILES: usize = 500;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    pub kind: String,
    pub subject: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GateResult {
    pub code: i32,
    pub text: String,
    pub issues: Vec<Issue>,
    /// Existing, unchanged judgments newly falsified by updated inputs. They
    /// remain findings, but do not by themselves block this session.
    pub allowed_falsifiers: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct GateOptions<'a> {
    pub state_path: &'a Path,
    pub paths: &'a [PathBuf],
    pub workspace: &'a Path,
    pub read_mode: ReadMode,
    pub runtime: Option<&'a Runtime>,
    pub turns: u64,
    pub host: Option<&'a str>,
    pub nudged_at: Option<u64>,
    pub session_id: Option<&'a str>,
    pub private_tmp: Option<&'a Path>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct JudgmentMark {
    shape: String,
    predicate: Option<bool>,
    arrangement: bool,
    inputs: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct TreeMark {
    head: String,
    files: Vec<String>,
    n: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct MarkState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    snapshot_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    findings_revision: Option<String>,
    fails: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    failures: Option<Vec<String>>,
    #[serde(default)]
    unserved: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ids: Option<Vec<String>>,
    #[serde(default)]
    judgments: BTreeMap<String, JudgmentMark>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tree: Option<TreeMark>,
    #[serde(default)]
    nudged: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    nudged_turn: Option<u64>,
}

struct Current {
    profile: &'static str,
    snapshot_id: Option<String>,
    findings_revision: Option<String>,
    failures: Vec<String>,
    ids: BTreeSet<String>,
    judgments: BTreeMap<String, JudgmentMark>,
    intents: BTreeSet<String>,
    attributed: BTreeSet<String>,
}

fn ordinary(
    capture: &CapturedSource,
    runtime: Option<&Runtime>,
    recordings: &BTreeSet<String>,
) -> Result<Current> {
    let context = capture.ordinary_context();
    let projection = Projection::new(
        capture.ordinary_document(),
        map(capture.hypotheses())?,
        map(&map(&context)?["conflicts"])?,
        vec![],
        runtime,
    )?;
    let OrdinaryData {
        failures,
        ids,
        judgments,
        intents,
        attributed,
    } = projection.gate_data_with_source_and_recordings(Some(capture.source()), recordings)?;
    let judgments = judgments
        .into_iter()
        .map(
            |(
                id,
                OrdinaryJudgment {
                    shape,
                    predicate,
                    arrangement,
                    inputs,
                },
            )| {
                (
                    id,
                    JudgmentMark {
                        shape,
                        predicate,
                        arrangement,
                        inputs,
                    },
                )
            },
        )
        .collect();
    Ok(Current {
        profile: "ordinary/v1",
        snapshot_id: None,
        findings_revision: None,
        failures,
        ids,
        judgments,
        intents,
        attributed,
    })
}

fn core(capture: &CapturedSource, runtime: Option<&Runtime>) -> Result<Current> {
    let context = CapturedAssessment::from_snapshot(
        capture.snapshot()?.clone(),
        None,
        "focused-review/v1",
        runtime,
        OperationalBounds::default(),
        None,
    )?;
    let report = map(context.assessment())?;
    let findings = crate::public_core_readers::record_findings(&context)?;
    let failures = findings["failures"]
        .as_array()
        .ok_or_else(|| Error("invalid_core_findings".into()))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| Error("invalid_core_findings".into()))
        })
        .collect::<Result<Vec<_>>>()?;
    let nodes = map(&report["nodes"])?;
    let judgment_ids = nodes
        .iter()
        .filter(|(_, node)| {
            map(node).is_ok_and(|node| {
                map(&node["body"]).is_ok_and(|body| {
                    map(&node["fields"])
                        .ok()
                        .and_then(|fields| fields.get("deps"))
                        .and_then(|field| text(field).ok())
                        .is_some_and(|field| body.contains_key(field))
                })
            })
        })
        .map(|(id, _)| id.clone())
        .collect::<BTreeSet<_>>();
    let mut judgments = BTreeMap::new();
    for (id, node) in nodes {
        let node = map(node)?;
        let Ok(body) = map(&node["body"]) else {
            continue;
        };
        let fields = map(&node["fields"])?;
        let Some(deps_field) = fields.get("deps").and_then(|value| text(value).ok()) else {
            continue;
        };
        let Some(predicate_field) = fields.get("predicate").and_then(|value| text(value).ok())
        else {
            continue;
        };
        if !body.contains_key(deps_field) || !body.contains_key(predicate_field) {
            continue;
        }
        let dependencies = map(&map(&node["state"])?["basis"])?
            .get("dependencies")
            .map(map)
            .transpose()?
            .cloned()
            .unwrap_or_default();
        let inputs = dependencies
            .iter()
            .filter(|(dependency, evidence)| {
                !judgment_ids.contains(*dependency) && map(evidence).is_ok()
            })
            .map(|(dependency, evidence)| {
                Ok((dependency.clone(), map(evidence)?["current"].digest()?))
            })
            .collect::<Result<_>>()?;
        let shape = V::Map(BTreeMap::from([
            ("body".into(), node["body"].clone()),
            ("fields".into(), node["fields"].clone()),
        ]))
        .digest()?;
        let falsifier = map(&map(&node["state"])?["falsifier"])?;
        judgments.insert(
            id.clone(),
            JudgmentMark {
                shape,
                predicate: Some(string_is(&falsifier["status"], "holds")),
                arrangement: body.contains_key("born"),
                inputs,
            },
        );
    }
    let ids = nodes
        .keys()
        .cloned()
        .chain(map(&report["history_subjects"])?.keys().cloned())
        .collect();
    let intents = nodes
        .iter()
        .filter_map(|(id, node)| {
            let body = map(node).ok().and_then(|node| map(&node["body"]).ok())?;
            (!judgment_ids.contains(id)
                && body.get("asked").is_some_and(crate::history_view::truth))
            .then(|| id.clone())
        })
        .collect::<BTreeSet<_>>();
    let attributed = nodes
        .iter()
        .filter_map(|(id, node)| {
            let node = map(node).ok()?;
            let body = map(&node["body"]).ok()?;
            let from_intent = body
                .get("from")
                .and_then(|v| text(v).ok())
                .is_some_and(|v| intents.contains(v));
            let dependencies = map(&map(&node["state"]).ok()?["basis"])
                .ok()?
                .get("dependencies")
                .and_then(|v| map(v).ok());
            (from_intent || dependencies.is_some_and(|d| d.keys().any(|dep| intents.contains(dep))))
                .then(|| id.clone())
        })
        .collect();
    Ok(Current {
        profile: "core/v1",
        snapshot_id: Some(context.snapshot_id().into()),
        findings_revision: Some(context.findings_revision().into()),
        failures,
        ids,
        judgments,
        intents,
        attributed,
    })
}

fn capture(
    options: &GateOptions<'_>,
    check_purpose: bool,
    preparing: Option<&crate::recording_receipt::PreparingContext>,
) -> Result<(
    CapturedSource,
    Current,
    crate::recording_receipt::RecordingSources,
)> {
    let capture = capture_source_with_runtime(
        options.paths,
        options.workspace,
        options.read_mode,
        None,
        options.runtime,
    )?;
    let capabilities = crate::reasoning_fields::capabilities(capture.ordinary_document(), None)?;
    let core_profile = string_is(&map(&capabilities)?["profile"], "core/v1");
    let recordings = if check_purpose && !core_profile {
        crate::recording_receipt::capture(&capture, preparing).unwrap_or_default()
    } else {
        crate::recording_receipt::RecordingSources::default()
    };
    let current = if core_profile {
        core(&capture, options.runtime)?
    } else {
        ordinary(&capture, options.runtime, recordings.ids())?
    };
    capture.verify()?;
    Ok((capture, current, recordings))
}

fn source_digest(capture: &CapturedSource, paths: &[PathBuf]) -> String {
    let mut hash = Sha1::new();
    let mut seen = BTreeSet::new();
    for wanted in paths.iter().chain(capture.members()) {
        let Ok(wanted) = crate::project_modes::resolved(wanted) else {
            continue;
        };
        if !seen.insert(wanted.clone()) {
            continue;
        }
        if let Some((_, bytes)) = capture
            .files()
            .iter()
            .find(|(path, _)| crate::project_modes::resolved(path).ok().as_ref() == Some(&wanted))
        {
            hash.update(bytes);
        }
    }
    for (path, bytes) in capture.files().iter().filter(|(path, _)| {
        path.components()
            .any(|part| part.as_os_str() == "hypotheses")
            || path
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == "PROVENANCE.d")
    }) {
        if crate::project_modes::resolved(path)
            .ok()
            .is_some_and(|path| seen.insert(path))
        {
            hash.update(bytes);
        }
    }
    format!("{:x}", hash.finalize())
}

fn command(workspace: &Path, arguments: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    command.args(arguments).current_dir(workspace);
    crate::reasoning_runtime::run_command_bounded(
        &mut command,
        Vec::new(),
        Duration::from_secs(5),
        2 * 1024 * 1024,
    )
    .ok()
    .and_then(|output| String::from_utf8(output).ok())
}

fn bounded_file_stamp(path: &Path, size: u64) -> String {
    if size > 1 << 20 {
        return format!("size {size}");
    }
    let Ok(file) = fs::File::open(path) else {
        return "unreadable".into();
    };
    let mut bytes = vec![];
    if file.take((1 << 20) + 1).read_to_end(&mut bytes).is_err() || bytes.len() as u64 != size {
        return "unreadable".into();
    }
    format!("{:x}", Sha1::digest(bytes))[..12].to_owned()
}

fn tree_state(workspace: &Path) -> Option<TreeMark> {
    let head = command(workspace, &["rev-parse", "HEAD"])?;
    let changed = command(workspace, &["diff", "HEAD", "--numstat"])?;
    let untracked = command(workspace, &["ls-files", "--others", "--exclude-standard"])?;
    let mut files = changed
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut names = untracked
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    names.sort_unstable();
    for relative in names.into_iter().take(TREE_FILES) {
        let path = workspace.join(relative);
        let stamp = match fs::metadata(&path) {
            Ok(metadata) => bounded_file_stamp(&path, metadata.len()),
            Err(_) => "unreadable".into(),
        };
        files.push(format!("?? {relative} {stamp}"));
    }
    files.sort();
    let n = files.len();
    files.truncate(TREE_FILES);
    Some(TreeMark {
        head: head.trim().into(),
        files,
        n,
    })
}

fn safe_read(path: &Path) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() as usize > MAX_STATE
    {
        return Err(Error("invalid_session_mark".into()));
    }
    let mut bytes = vec![];
    fs::File::open(path)?
        .take((MAX_STATE + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_STATE {
        return Err(Error("session_mark_limit".into()));
    }
    Ok(bytes)
}

fn read_mark(path: &Path) -> Result<MarkState> {
    let bytes = safe_read(path)?;
    if let Ok(value) =
        crate::json_ingress::parse_slice(&bytes, crate::json_ingress::DuplicateKeys::LastWins)
    {
        if value.is_object() {
            return serde_json::from_value(value).map_err(Into::into);
        }
        if let Some(count) = value.as_u64() {
            return Ok(MarkState {
                fails: count as usize,
                ..Default::default()
            });
        }
    }
    let text = String::from_utf8_lossy(&bytes);
    Ok(MarkState {
        fails: text.trim().parse().unwrap_or(0),
        ..Default::default()
    })
}

fn write_mark(path: &Path, state: &MarkState) -> Result<()> {
    let bytes = serde_json::to_vec(state)?;
    if bytes.len() > MAX_STATE {
        return Err(Error("session_mark_limit".into()));
    }
    let parent = path
        .parent()
        .ok_or_else(|| Error("invalid_session_mark".into()))?;
    fs::create_dir_all(parent)?;
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(Error("session mark must not be a symlink".into()));
    }
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    temporary.write_all(&bytes)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| Error(format!("persist: {}", error.error)))?;
    Ok(())
}

fn mark_overlaps_history_namespace<'a>(
    path: &Path,
    entries: impl Iterator<Item = &'a PathBuf>,
) -> bool {
    let Ok(target) = crate::project_modes::resolved(path) else {
        return true;
    };
    // All files in this directory belong to the recovery protocol, including
    // direct-history and bootstrap journal companions and future receipt types.
    let mut entries = entries;
    entries.any(|entry| {
        let parent = entry.parent().unwrap_or_else(|| Path::new("."));
        let name = entry.file_name().and_then(|v| v.to_str()).unwrap_or("");
        crate::history_transaction::Layout::for_entry(name)
            .ok()
            .is_some_and(|layout| {
                crate::project_modes::resolved(&parent.join(layout.home).join(".history-local"))
                    .is_ok_and(|directory| target.starts_with(directory))
            })
    })
}

fn mark_overlaps_capture(path: &Path, capture: &CapturedSource, paths: &[PathBuf]) -> bool {
    let Ok(target) = crate::project_modes::resolved(path) else {
        return true;
    };
    if mark_overlaps_history_namespace(path, paths.iter().chain(capture.members())) {
        return true;
    }
    let mut reserved = paths.iter().chain(capture.members()).flat_map(|entry| {
        let parent = entry.parent().unwrap_or_else(|| Path::new("."));
        let name = entry.file_name().and_then(|v| v.to_str()).unwrap_or("");
        crate::history_transaction::Layout::for_entry(name)
            .ok()
            .into_iter()
            .flat_map(move |layout| {
                [
                    layout.view,
                    layout.replaced,
                    layout.authority,
                    layout.journal,
                    Path::new(&layout.home)
                        .join("project.json")
                        .to_string_lossy()
                        .into_owned(),
                    ".kpopper/project.json".into(),
                ]
                .into_iter()
                .map(move |relative| parent.join(relative))
            })
    });
    reserved.any(|path| crate::project_modes::resolved(&path).ok().as_ref() == Some(&target))
        || paths
            .iter()
            .chain(capture.members())
            .any(|path| crate::project_modes::resolved(path).ok().as_ref() == Some(&target))
        || capture
            .files()
            .keys()
            .any(|path| crate::project_modes::resolved(path).ok().as_ref() == Some(&target))
}

/// Capture and persist the bounded private session-start mark.
pub fn mark(options: &GateOptions<'_>) -> Result<()> {
    if mark_overlaps_history_namespace(options.state_path, options.paths.iter()) {
        return Err(Error("session_mark_overlaps_record".into()));
    }
    let (capture, current, _) = capture(options, false, None)?;
    if mark_overlaps_capture(options.state_path, &capture, options.paths) {
        return Err(Error("session_mark_overlaps_record".into()));
    }
    let state = MarkState {
        profile: (current.profile == "core/v1").then(|| current.profile.into()),
        snapshot_id: current.snapshot_id,
        findings_revision: current.findings_revision,
        fails: current.failures.len(),
        failures: Some(current.failures),
        unserved: vec![],
        ids: Some(current.ids.into_iter().collect()),
        judgments: current.judgments,
        digest: Some(source_digest(&capture, options.paths)),
        tree: tree_state(options.workspace),
        nudged: false,
        nudged_turn: None,
    };
    capture.verify()?;
    write_mark(options.state_path, &state)
}

/// Assess the stop gate from one fresh immutable capture. Source or assessment
/// failure is returned as an error, never converted into a successful gate.
pub fn gate(options: &GateOptions<'_>) -> Result<GateResult> {
    gate_with_recording_context(options, None)
}

/// Internal ingestion preparation may supply a validated staged context. No CLI
/// flag or record field can supply this context.
pub fn gate_with_recording_context(
    options: &GateOptions<'_>,
    preparing: Option<&crate::recording_receipt::PreparingContext>,
) -> Result<GateResult> {
    if mark_overlaps_history_namespace(options.state_path, options.paths.iter()) {
        return Err(Error("session_mark_overlaps_record".into()));
    }
    let saved_base = read_mark(options.state_path)?;
    let mut use_recordings = true;
    loop {
        let base = saved_base.clone();
        let (capture, current, recordings) = capture(options, use_recordings, preparing)?;
        if mark_overlaps_capture(options.state_path, &capture, options.paths) {
            return Err(Error("session_mark_overlaps_record".into()));
        }
        let previous = base
            .failures
            .clone()
            .unwrap_or_default()
            .into_iter()
            .collect::<BTreeSet<_>>();
        let mut allowed = BTreeMap::<String, String>::new();
        for (id, old) in &base.judgments {
            let Some(now) = current.judgments.get(id) else {
                continue;
            };
            if old.shape == now.shape
                && !old.arrangement
                && !now.arrangement
                && old.predicate != Some(true)
                && now.predicate == Some(true)
                && now
                    .inputs
                    .iter()
                    .any(|(dependency, value)| old.inputs.get(dependency) != Some(value))
            {
                let failure = if current.profile == "core/v1" {
                    format!("{id}: falsifier holds")
                } else {
                    current
                        .failures
                        .iter()
                        .find(|failure| failure.starts_with(&format!("{id}: wrong_if holds")))
                        .cloned()
                        .unwrap_or_else(|| format!("{id}: wrong_if holds"))
                };
                allowed.insert(failure, id.clone());
            }
        }
        let added = if current.profile == "ordinary/v1" && base.failures.is_some() {
            current
                .failures
                .iter()
                .filter(|failure| !previous.contains(*failure) && !allowed.contains_key(*failure))
                .cloned()
                .collect::<Vec<_>>()
        } else if current.profile == "core/v1" {
            current
                .failures
                .iter()
                .filter(|failure| {
                    base.profile.as_deref() != Some("core/v1")
                        || (!previous.contains(*failure) && !allowed.contains_key(*failure))
                })
                .cloned()
                .collect()
        } else if current.failures.len() > base.fails {
            current.failures.clone()
        } else {
            vec![]
        };
        let mut lines = vec![];
        let mut issues = vec![];
        if !added.is_empty() {
            let first = options
                .paths
                .first()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "record".into());
            lines.push(if current.profile == "core/v1" {
                format!(
                    "{first} fails core check with {} problems ({} at session start).",
                    current.failures.len(),
                    base.fails
                )
            } else {
                format!(
                    "{first} fails check with {} problems ({} at session start).",
                    current.failures.len(),
                    base.fails
                )
            });
            if current.profile != "core/v1" {
                lines.push(
                    "Fix the record - or declare the hole with blocked_on - before finishing:"
                        .into(),
                );
            }
            for failure in added.iter().take(12) {
                let line = format!("FAIL {failure}");
                lines.push(line.clone());
            }
            for failure in &added {
                let line = format!("FAIL {failure}");
                issues.push(Issue {
                    kind: "failure".into(),
                    subject: failure.clone(),
                    text: line,
                });
            }
        }
        if let Some(was) = &base.ids {
            let mut new = current
                .ids
                .difference(&was.iter().cloned().collect())
                .cloned()
                .collect::<Vec<_>>();
            if let Some(sid) = options.session_id {
                let tmp = options
                    .private_tmp
                    .map(Path::to_path_buf)
                    .unwrap_or_else(crate::session_activity::temporary_directory);
                let owned = crate::session_activity::owned(&tmp, sid, &capture);
                new.retain(|id| owned.contains(id));
            }
            if !new.is_empty()
                && !new
                    .iter()
                    .any(|id| current.intents.contains(id) || current.attributed.contains(id))
            {
                let mut names = new.iter().take(4).cloned().collect::<Vec<_>>().join(", ");
                if new.len() > 4 {
                    names.push_str(&format!(" and {} more", new.len() - 4));
                }
                let message = if current.profile == "core/v1" {
                    format!(
                        "this session wrote {} {} ({names}); verify its recorded intent before finishing",
                        new.len(),
                        if new.len() == 1 { "entry" } else { "entries" }
                    )
                } else {
                    format!(
                        "this session wrote {} {} ({names}) and recorded no intent: add s.<date>_<slug> asked=\"...\" name=\"...\", and from: it on what it wrote",
                        new.len(),
                        if new.len() == 1 { "entry" } else { "entries" }
                    )
                };
                lines.push(message);
                for id in new {
                    let text = if current.profile == "core/v1" {
                        format!(
                            "this session wrote 1 entry ({id}); verify its recorded intent before finishing"
                        )
                    } else {
                        format!(
                            "this session wrote 1 entry ({id}) and recorded no intent: add s.<date>_<slug> asked=\"...\" name=\"...\", and from: it on what it wrote"
                        )
                    };
                    issues.push(Issue {
                        kind: "unattributed".into(),
                        subject: id,
                        text,
                    });
                }
            }
        }
        let allowed_falsifiers = allowed.values().cloned().collect::<Vec<_>>();
        if !allowed_falsifiers.is_empty() {
            lines.push(format!("Updated readings falsified unchanged judgments: {}. They remain flagged for review; check still reports their failed conditions.", allowed_falsifiers.join(", ")));
        }
        let code = if issues.is_empty() { 0 } else { 2 };
        let result = GateResult {
            code,
            text: if lines.is_empty() {
                String::new()
            } else {
                lines.join("\n") + "\n"
            },
            issues,
            allowed_falsifiers,
        };
        capture.verify()?;
        if use_recordings && recordings.verify().is_err() {
            // Optional ingestion evidence grants only a purpose exemption. If it
            // becomes unavailable at final proof, assess again without exemptions;
            // never discard unrelated failures or refresh the session baseline.
            use_recordings = false;
            continue;
        }
        return Ok(result);
    }
}
