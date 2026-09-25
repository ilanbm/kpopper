//! Source capture and public read command routing.
#[path = "public_branch_read.rs"]
pub(crate) mod branch_read;
use crate::{
    Result,
    history_contract::*,
    public_core_readers as C, public_workspace as W,
    reasoning_context::CapturedAssessment,
    reasoning_runtime::{OperationalBounds, Runtime},
    source_capture::{self, ReadMode},
    source_inventory::{Inventory, absolute},
};
use serde_json::{Value as J, json};
use std::path::{Path, PathBuf};
#[derive(Clone, Debug, Default, clap::Args)]
pub struct Options {
    pub subjects: Vec<String>,
    #[arg(long,value_parser=["core/v1"])]
    pub profile: Option<String>,
    #[arg(long, allow_negative_numbers = true)]
    pub budget: Option<i64>,
    #[arg(long)]
    pub chars: Option<i64>,
    #[arg(long,value_parser=["claude","codex"],hide=true)]
    pub host: Option<String>,
    /// Set by the session hook, never by a flag: the hook names its host whatever the
    /// record, so a core/v1 opening, which has no next moves to name, does not refuse it.
    #[arg(skip)]
    pub from_hook: bool,
    #[arg(long)]
    pub history: bool,
    /// Read a pinned committed branch beside the current record.
    #[arg(long = "from", value_name = "REF")]
    pub from_ref: Option<String>,
}
/// Whether a reader's subject names a record file rather than an entry. pull and affects
/// read the extension as given; the other readers read it in either case.
fn names_a_file(command: &str, subject: &str) -> bool {
    let filename = if ["pull", "affects"].contains(&command) {
        subject.to_owned()
    } else {
        subject.to_lowercase()
    };
    filename.ends_with(".yaml") || filename.ends_with(".yml")
}
/// Where a named record file is looked for: under HOME for `~/`, else in `cwd`.
fn located(name: &str, cwd: &Path) -> Result<PathBuf> {
    Ok(if let Some(name) = name.strip_prefix("~/") {
        PathBuf::from(std::env::var_os("HOME").ok_or_else(|| error("home_directory_unavailable"))?)
            .join(name)
    } else {
        cwd.join(name)
    })
}
const NO_RECORD_HERE: &str = ": no record here. Run this from the directory the record sits in, or name the record file as an argument.";
/// The Python reader's refusal for a record that is not there. It names the first file
/// the command named that is not there, as it was typed, or GROUNDING.yaml when it named
/// none. A live read of a configured Simple project takes its one record for the entry
/// names at the project's root, and names that record in full.
pub(crate) fn no_record_here<'a>(
    named: impl IntoIterator<Item = &'a str>,
    cwd: &Path,
    live: bool,
) -> crate::Error {
    let mut files = named
        .into_iter()
        .map(|name| (name.to_owned(), located(name, cwd).ok()))
        .collect::<Vec<_>>();
    if files.is_empty() {
        files.push(("GROUNDING.yaml".into(), Some(cwd.join("GROUNDING.yaml"))));
    }
    if live
        && let Some(first) = &files[0].1
        && let Ok(Some(record)) =
            crate::project_modes::simple_record(std::slice::from_ref(first), cwd)
    {
        files[0] = (record.display().to_string(), Some(record));
    }
    let missing = files.iter().find(|(_, path)| {
        path.as_ref().is_some_and(|path| {
            !path.exists()
                && crate::source_inventory::glob(path).is_ok_and(|found| found.is_empty())
        })
    });
    let name = &missing.unwrap_or(&files[0]).0;
    crate::Error(format!("{name}{NO_RECORD_HERE}"))
}
/// A capture that found no record file, told as `no_record_here` tells it. Any other
/// failure stands as it is.
pub(crate) fn explain_missing<'a>(
    failure: crate::Error,
    named: impl IntoIterator<Item = &'a str>,
    cwd: &Path,
    live: bool,
) -> crate::Error {
    if failure.0 == "missing_record" {
        no_record_here(named, cwd, live)
    } else {
        failure
    }
}
fn files(options: &Options, cwd: &Path, command: &str) -> Result<(Vec<PathBuf>, Vec<String>)> {
    let mut paths = vec![];
    let mut seeds = vec![];
    for s in &options.subjects {
        if names_a_file(command, s) {
            let expanded = located(s, cwd)?;
            let found = crate::source_inventory::glob(&expanded)?;
            if found.is_empty() {
                paths.push(absolute(&expanded)?);
            } else {
                paths.extend(found);
            }
        } else {
            seeds.push(s.clone());
        }
    }
    if paths.is_empty() {
        paths = W::records(cwd)?;
    }
    Ok((paths, seeds))
}
fn mapping(location: &W::Location, inventory: &mut Inventory) -> Result<Option<J>> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/state")))
        .ok_or_else(|| error("home_directory_unavailable"))?;
    let path = state
        .join("kpopper/first-use/projects")
        .join(&location.key)
        .join("mapping.json");
    if !inventory.exists(&path)? {
        return Ok(None);
    }
    let value: J = crate::json_ingress::parse_slice(
        &inventory.read(&path)?,
        crate::json_ingress::DuplicateKeys::LastWins,
    )
    .map_err(|_| {
        error(&format!(
            "Invalid first-use state; keep it for inspection: {}",
            path.display()
        ))
    })?;
    crate::require(
        value.is_object() && (value["schema"] == 1 || value["schema"] == true),
        &format!(
            "Invalid first-use state; keep it for inspection: {}",
            path.display()
        ),
    )?;
    crate::require(
        matches!(value["mode"].as_str(), Some("map" | "deep"))
            && matches!(
                value["mapping"].as_str(),
                Some("ready" | "running" | "complete" | "failed")
            ),
        "Invalid mapping task state",
    )?;
    crate::require(
        value["request"].as_str().is_some_and(|s| {
            s.len() == 32
                && s.bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        }),
        "Invalid mapping request ID",
    )?;
    crate::require(
        value["owner"].as_str().is_some_and(|s| {
            !s.is_empty()
                && s.len() <= 200
                && s.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
        }),
        "Invalid mapping owner",
    )?;
    Ok(Some(J::Object(
        ["request", "mapping", "mode", "report", "check", "error"]
            .iter()
            .filter_map(|k| value.get(*k).map(|v| ((*k).into(), v.clone())))
            .collect(),
    )))
}
fn private_draft_count(paths: &[PathBuf], cwd: &Path, inventory: &mut Inventory) -> Result<usize> {
    let project = crate::project_modes::project_for(paths, cwd)?;
    let key = crate::identity::sha256(
        project
            .common
            .as_ref()
            .unwrap_or(&project.root)
            .to_string_lossy()
            .as_bytes(),
    );
    let home = std::env::var_os("KPOPPER_PRIVATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/share/kpopper/private"))
        })
        .ok_or_else(|| error("home_directory_unavailable"))?;
    let paths = inventory.glob(&home.join(key).join("*.json"))?;
    let mut count = 0;
    for path in paths {
        if inventory.file(&path)? {
            count += 1;
        }
    }
    Ok(count)
}
fn has_brief(paths: &[PathBuf], inventory: &mut Inventory) -> Result<bool> {
    let first = paths.first().ok_or_else(|| error("record_required"))?;
    let layout = crate::history_transaction::Layout::for_entry(
        first
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| error("invalid_path"))?,
    )?;
    inventory.exists(&first.parent().unwrap().join(layout.view))
}
fn replaced_path(paths: &[PathBuf]) -> Result<(PathBuf, String)> {
    let first = paths.first().ok_or_else(|| error("record_required"))?;
    let path = if first
        .file_name()
        .is_some_and(|name| name == "GROUNDING.yaml")
    {
        first.parent().unwrap().join(".kpopper/replaced.yaml")
    } else {
        let name = first.to_string_lossy();
        let stem = name
            .strip_suffix(".yaml")
            .or_else(|| name.strip_suffix(".yml"))
            .unwrap_or(&name);
        PathBuf::from(format!("{stem}.replaced.yaml"))
    };
    let relative = path
        .strip_prefix(first.parent().unwrap())
        .unwrap_or(&path)
        .to_string_lossy()
        .to_string();
    Ok((path, relative))
}
fn replaced(
    paths: &[PathBuf],
    inventory: &mut Inventory,
) -> Result<(Option<crate::value::TypedValue>, String)> {
    let (path, relative) = replaced_path(paths)?;
    if !inventory.exists(&path)? || !inventory.file(&path)? {
        return Ok((None, relative));
    }
    let value = crate::history_yaml::decode_document(&inventory.read(&path)?)?;
    Ok((Some(value), relative))
}
/// The kept versions of replaced judgments, for the opener. A file the reader cannot
/// parse holds nothing it can name, as in the Python reader.
fn kept_versions(
    paths: &[PathBuf],
    inventory: &mut Inventory,
) -> Result<Option<crate::ordinary_value::Value>> {
    let (path, _) = replaced_path(paths)?;
    if !inventory.exists(&path)? || !inventory.file(&path)? {
        return Ok(None);
    }
    Ok(
        crate::history_yaml::decode_document(&inventory.read(&path)?)
            .ok()
            .map(|value| crate::ordinary_value::Value::from_typed(&value)),
    )
}
/// The opener's line about a record moved by half: files of the record's other layout
/// beside the entry file, which nothing reads, named as the Python reader names them.
fn leftover_head(paths: &[PathBuf], inventory: &mut Inventory) -> Result<Option<String>> {
    let first = paths.first().ok_or_else(|| error("record_required"))?;
    let directory = first.parent().ok_or_else(|| error("invalid_path"))?;
    let legacy = first
        .file_name()
        .is_none_or(|name| name != "GROUNDING.yaml");
    // The other layout's files, in the order the reader names them; hypotheses first.
    let other = if legacy {
        [
            ".kpopper/hypotheses",
            ".kpopper/view.yaml",
            ".kpopper/measure.yaml",
            ".kpopper/session.json",
            ".kpopper/replaced.yaml",
            ".kpopper/history",
            ".kpopper/history-commits",
            ".kpopper/history.yaml",
            ".kpopper/history-cancellations",
        ]
    } else {
        [
            "PROVENANCE.d",
            "PROVENANCE.view.yaml",
            "PROVENANCE.measure.yaml",
            "PROVENANCE.session.json",
            "PROVENANCE.replaced.yaml",
            "PROVENANCE.history",
            "PROVENANCE.history-commits",
            "PROVENANCE.history.yaml",
            "PROVENANCE.history-cancellations",
        ]
    };
    let mut left = vec![];
    for (index, name) in other.into_iter().enumerate() {
        let path = directory.join(name);
        if !inventory.exists(&path)? {
            continue;
        }
        if index == 0 {
            // An empty hypotheses directory holds nothing to lose.
            if inventory.glob(&path.join("*.y*ml"))?.is_empty() {
                continue;
            }
            left.push(format!("{name}/"));
        } else {
            left.push(name.to_owned());
        }
    }
    if left.is_empty() {
        return Ok(None);
    }
    let names = left.join(", ");
    Ok(Some(if legacy {
        format!(
            "{names} not read beside PROVENANCE.yaml - rename the record to GROUNDING.yaml and move its files into .kpopper/"
        )
    } else {
        format!("left under the earlier name, not read: {names} - move into .kpopper/")
    }))
}
fn prefix_order(source: &crate::ordinary_source::Source) -> Vec<String> {
    let Some(crate::ordinary_source::Source::Map(prefixes)) =
        source.get("meta").and_then(|meta| meta.get("prefixes"))
    else {
        return vec![];
    };
    prefixes
        .iter()
        .filter_map(|(key, _)| key.text().map(str::to_owned))
        .collect()
}
fn orientation(source: &crate::ordinary_source::Source) -> Vec<String> {
    use crate::ordinary_source::Source as S;
    let S::Map(fields) = source else {
        return vec![];
    };
    let mut places = Vec::new();
    for (key, value) in fields {
        let Some(key) = key.text() else { continue };
        if !["record", "also", "skill", "entry"].contains(&key) {
            continue;
        }
        let name = if let S::Scalar(crate::ordinary_value::Scalar::Finite(
            crate::value::TypedValue::Text(s),
        )) = value
        {
            s.clone()
        } else {
            let values = match value {
                S::Map(m) => m.iter().map(|(_, v)| v).collect::<Vec<_>>(),
                S::List(a) => a.iter().collect(),
                _ => vec![],
            };
            values
                .into_iter()
                .filter_map(|v| match v {
                    S::Scalar(crate::ordinary_value::Scalar::Finite(
                        crate::value::TypedValue::Text(s),
                    )) if s.ends_with(".yaml") || s.ends_with(".yml") => Some(s.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        if !name.is_empty() {
            places.push(format!(
                "{key}: {}",
                crate::public_ordinary_readers::cut(&name, 90)
            ));
        }
    }
    if places.is_empty() {
        vec![]
    } else {
        vec![format!("  {}", places.join(" | "))]
    }
}
/// Whether a failure is the reader's account of a record it cannot read: one whose field
/// roles it cannot take, one that is not there, or one whose file does not parse. Commands
/// print it as it stands, with exit status 1, as the Python reader does.
pub fn unreadable_record(failure: &crate::Error) -> bool {
    crate::ordinary_fields::explains_unreadable(failure)
        || failure.0.ends_with(NO_RECORD_HERE)
        || crate::ordinary_yaml_diagnostic::explains_record(failure)
}
/// Whether a failure is the ordinary reader's refusal of a record, or of a layer read with
/// it, that only a core/v1 consumer reads, or whose reasoning declaration it cannot read.
/// Commands print it as it stands, with exit status 1, as the Python reader does.
pub fn core_consumer_refusal(failure: &crate::Error) -> bool {
    failure.0 == crate::source_capture::CORE_CONSUMER
        || crate::ordinary_fields::refuses_declaration(failure)
}
/// What a read command prints on stderr when it fails, and its exit status. With
/// --json the same text travels: wrapped for check, pull and affects, as open's `error`.
pub fn failure(command: &str, options: &Options, error: &crate::Error) -> (String, i32) {
    if unreadable_record(error)
        || core_consumer_refusal(error)
        || error.0.starts_with("core_profile_option_unsupported:")
        || error
            .0
            .contains("is not an entry or a prefix in this record.")
        || command == "pull" && options.from_ref.is_some()
    {
        return (format!("{error}\n"), 1);
    }
    (format!("kpop {command}: {error}\n"), 2)
}
/// What a read returns. Only `open` builds its reply differently: the command's text
/// follows the view with the followups and mapping lines, JSON is its own object, and
/// the session opener takes the view alone because it adds its own followups line.
/// The other readers return their text, which the command line wraps for --json.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    Text,
    Json,
    View,
}
pub fn run_auto(
    command: &str,
    options: &Options,
    cwd: &Path,
    mode: ReadMode,
    reply: Reply,
) -> Result<C::Output> {
    let mut taken = None;
    let mut read = || {
        let cwd = cwd.canonicalize()?;
        let runtime =
            |paths: &[PathBuf]| W::runtime_for_paths(paths, &cwd, options.profile.as_deref());
        run(command, options, &cwd, mode, reply, &runtime, &mut taken)
    };
    match read() {
        Err(error) if command == "open" && reply == Reply::Json => {
            unopened(options, cwd, mode, &error, taken.as_deref())
        }
        result => result,
    }
}
/// The files a read takes, and the subjects that name no file. `open` in a workspace
/// whose record is missing or no readable file takes the location it found and reports it.
fn read_paths(
    command: &str,
    options: &Options,
    cwd: &Path,
    location: &W::Location,
) -> Result<(Vec<PathBuf>, Vec<String>)> {
    if command == "open"
        && options.subjects.is_empty()
        && matches!(location.status.as_str(), "missing" | "unavailable")
    {
        return Ok((vec![location.record.clone()], vec![]));
    }
    files(options, cwd, command)
}
/// The fields `open` reports before it reads a record: the workspace, the record it
/// found or the one it was given, and the workspace's mapping task.
fn opening_fields(
    location: &W::Location,
    options: &Options,
    paths: &[PathBuf],
    seeds: &[String],
    inventory: &mut Inventory,
) -> Result<J> {
    crate::require(
        seeds.is_empty(),
        "Record filenames must have a .yaml or .yml extension.",
    )?;
    let mut data =
        json!({"workspace":location.workspace,"record":location.record,"status":location.status});
    if !options.subjects.is_empty() {
        data["record"] = json!(paths[0]);
        data["status"] = json!(if paths[0].is_file() {
            "found"
        } else {
            "unavailable"
        });
    }
    match mapping(location, inventory) {
        Ok(Some(v)) => data["mapping"] = v,
        Err(e) => data["warning"] = json!(e.0),
        _ => {}
    }
    Ok(data)
}
/// `open` on an ordinary record, around the view the reader produced.
fn opened(
    mut data: J,
    view: String,
    digest: Option<String>,
    workspace: &Path,
    reply: Reply,
) -> Result<C::Output> {
    if reply == Reply::View {
        return Ok(C::Output {
            text: view,
            code: 0,
        });
    }
    let mut text = view.clone();
    match crate::session_admin::followup_summary(workspace) {
        Ok(Some(summary)) => {
            text.push_str(&format!("\n{summary}"));
            data["followups"] = json!(summary);
        }
        Ok(None) => {}
        Err(error) => {
            text.push_str(&format!("\nFollowups unavailable: {error}"));
            data["followups_error"] = json!(error.0);
        }
    }
    if let Some(state) = data["mapping"]["mapping"].as_str() {
        text.push_str(&format!("\nMapping: {state}"));
    }
    if reply == Reply::Text {
        return Ok(C::Output {
            text: text.trim_end().to_owned() + "\n",
            code: 0,
        });
    }
    if let Some(digest) = digest {
        data["record_sha256"] = json!(digest);
    }
    data["view"] = json!(view);
    // The view is read from the record itself, never from a checked session.
    data["checked"] = json!(false);
    Ok(C::Output {
        text: serde_json::to_string_pretty(&data)? + "\n",
        code: 0,
    })
}
/// Whether the core/v1 reader takes the record: its capabilities declare that profile.
fn core_record(paths: &[PathBuf]) -> Result<bool> {
    let mut inventory = Inventory::default();
    let document = crate::ordinary_document::load(paths, &mut inventory, true)?;
    let capabilities =
        crate::ordinary_fields::capabilities(&document.source.projected(), None)?.try_typed()?;
    Ok(!document.members.is_empty() && string_is(&map(&capabilities)?["profile"], "core/v1"))
}
/// `open --json` after a failure: the fields known before the read and, once an ordinary
/// record was located, an empty view, with the text-mode failure as `error`. The digest
/// taken before the read is named only while the entry still holds those bytes: a record
/// that changed during the failed read was read by no one. The core/v1 reader refuses the
/// opener's options before it reads anything, so that refusal carries the error alone.
fn unopened(
    options: &Options,
    cwd: &Path,
    mode: ReadMode,
    error: &crate::Error,
    taken: Option<&str>,
) -> Result<C::Output> {
    let (message, code) = failure("open", options, error);
    let located = || -> Result<J> {
        let cwd = cwd.canonicalize()?;
        let location = W::locate(&cwd, mode)?;
        let (paths, seeds) = read_paths("open", options, &cwd, &location)?;
        let mut inventory = Inventory::default();
        let mut data = opening_fields(&location, options, &paths, &seeds, &mut inventory)?;
        if data["status"] == "missing" || data["status"] == "unavailable" {
            return Ok(data);
        }
        if options.profile.is_some() || core_record(&paths).unwrap_or(false) {
            let refused =
                options.chars.is_some() || options.budget.is_some() || options.host.is_some();
            return Ok(if refused { json!({}) } else { data });
        }
        if let (Some(taken), [entry]) = (taken, paths.as_slice())
            && crate::identity::sha256(&inventory.read(entry)?) == taken
        {
            data["record_sha256"] = json!(taken);
        }
        data["view"] = json!("");
        data["checked"] = json!(false);
        Ok(data)
    };
    let mut data = located().unwrap_or_else(|_| json!({}));
    data["error"] = json!(message.trim());
    Ok(C::Output {
        text: serde_json::to_string_pretty(&data)? + "\n",
        code,
    })
}

pub fn run(
    command: &str,
    options: &Options,
    cwd: &Path,
    mode: ReadMode,
    reply: Reply,
    load_runtime: &dyn Fn(&[PathBuf]) -> Result<Option<Runtime>>,
    taken: &mut Option<String>,
) -> Result<C::Output> {
    let as_json = reply == Reply::Json;
    let cwd = cwd.canonicalize()?;
    let location = W::locate(&cwd, mode)?;
    let (paths, seeds) = read_paths(command, options, &cwd, &location)?;
    let mut inventory = Inventory::default();
    let mut data =
        json!({"workspace":location.workspace,"record":location.record,"status":location.status});
    if command == "open" {
        data = opening_fields(&location, options, &paths, &seeds, &mut inventory)?;
        if data["status"] == "unavailable" || data["status"] == "missing" {
            let unavailable = data["status"] == "unavailable";
            let mut message = if unavailable {
                if options.subjects.is_empty() {
                    location.reason.clone()
                } else {
                    format!(
                        "The requested record is unavailable: {}",
                        paths[0].display()
                    )
                }
            } else {
                "No knowledge record yet. Continue your work and keep useful findings as they arise, or run `kpop map` for an initial map (`--deep` for a deeper investigation).".into()
            };
            if !unavailable && let Some(state) = data["mapping"]["mapping"].as_str() {
                message.push_str(&format!("\nMapping: {state}"));
            }
            data[if unavailable { "error" } else { "message" }] = json!(message);
            inventory.verify()?;
            crate::require(
                W::locate(&cwd, mode)? == location,
                "workspace changed while opening it; retry",
            )?;
            return Ok(C::Output {
                text: if as_json {
                    serde_json::to_string_pretty(&data)? + "\n"
                } else {
                    message + "\n"
                },
                code: i32::from(unavailable),
            });
        }
        // `open --json` names the entry bytes it reads. They are taken before the read and
        // verified with everything else after it, so a write that carries the digest back
        // is refused once the record has changed since.
        if as_json && let [entry] = paths.as_slice() {
            *taken = Some(crate::identity::sha256(&inventory.read(entry)?));
        }
    }
    // Resources are selected from the record, once there is one to read.
    let loaded = load_runtime(&paths)?;
    let runtime = loaded.as_ref();
    let capture =
        source_capture::capture_ordinary_source_with_runtime(&paths, &cwd, mode, None, runtime)
            .map_err(|e| {
                let named = options.subjects.iter().filter(|s| names_a_file(command, s));
                explain_missing(e, named.map(String::as_str), &cwd, mode == ReadMode::Live)
            })?;
    if let Some(reference) = options.from_ref.as_deref() {
        crate::require(command == "pull", "--from is available only with pull")?;
        crate::require(!options.history, "--from cannot be combined with --history")?;
        let output = branch_read::pull(
            reference,
            &seeds,
            options.budget.unwrap_or(40),
            &cwd,
            &paths,
            &capture,
            runtime,
        )?;
        capture.verify()?;
        inventory.verify()?;
        crate::require(
            W::locate(&cwd, mode)? == location,
            "workspace changed while reading it; retry",
        )?;
        return Ok(output);
    }
    let capabilities = crate::ordinary_fields::capabilities(
        capture.ordinary_document(),
        options.profile.as_deref(),
    )?
    .try_typed()?;
    let file_notes = if command == "check" {
        crate::source_references::notes(
            capture.ordinary_document(),
            &location.workspace,
            paths[0].parent().unwrap_or(&location.workspace),
            &mut inventory,
        )?
    } else {
        vec![]
    };
    if !string_is(&map(&capabilities)?["profile"], "core/v1") {
        capture.require_ordinary_reader()?;
        let context = capture.ordinary_context();
        let ordinary_context = crate::ordinary_value::Value::from_typed(&context);
        let conflicts = crate::ordinary_value::map(
            &crate::ordinary_value::map(&ordinary_context)?["conflicts"],
        )?;
        let mut knowledge = capture.reader_lines()?;
        knowledge.extend(file_notes.iter().cloned());
        if mode == ReadMode::Live {
            let drafts = private_draft_count(&paths, &cwd, &mut inventory)?;
            if drafts > 0 {
                knowledge.push(format!(
                    "{drafts} private drafts retained; inspect `kpop knowledge status`"
                ));
            }
        }
        let projection = crate::ordinary_views::Projection::new(
            capture.ordinary_document(),
            crate::ordinary_value::map(capture.hypotheses())?,
            conflicts,
            knowledge,
            runtime,
        )?
        .with_history_review(capture.history_projection())?;
        let prefix_order = prefix_order(capture.source());
        let output = match command {
            "open" => {
                // A caller that names no files or budgets of its own gets the slot a
                // session hook fills; one that names any gets exactly what it named.
                let explicit = !options.subjects.is_empty()
                    || options.chars.is_some()
                    || options.budget.is_some();
                let replaced = kept_versions(&paths, &mut inventory)?;
                projection.opening(&crate::ordinary_views::Opening {
                    budget: options.budget.unwrap_or(25),
                    chars: options
                        .chars
                        .or((!explicit).then_some(crate::ordinary_views::OPENING_CHARS)),
                    host: options.host.as_deref(),
                    prefix_order: &prefix_order,
                    orientation: &orientation(capture.source()),
                    replaced: replaced.as_ref(),
                    leftover: leftover_head(&paths, &mut inventory)?,
                })?
            }
            "check" => {
                let (mut text, code) = projection.check(None)?;
                if has_brief(&paths, &mut inventory)? {
                    text.insert_str(0, C::LAYOUT_NOTICE);
                }
                capture.verify()?;
                inventory.verify()?;
                crate::require(
                    W::locate(&cwd, mode)? == location,
                    "workspace changed while reading it; retry",
                )?;
                return Ok(C::Output { text, code });
            }
            "pull" => {
                if options.history {
                    let mut output = projection.pull(&seeds, options.budget.unwrap_or(40))?;
                    output.push('\n');
                    if capture.node_history_capture().is_some()
                        || capture.history_capture().is_some()
                    {
                        output.push_str(&crate::public_history_read::render(
                            capture.node_history_capture(),
                            capture.history_capture(),
                            &seeds,
                            options
                                .chars
                                .or(options.budget.map(|b| b.saturating_mul(256))),
                            as_json,
                        )?);
                    } else {
                        let (history, path) = replaced(&paths, &mut inventory)?;
                        let ordinary_history = history
                            .as_ref()
                            .map(crate::ordinary_value::Value::from_typed);
                        let retained =
                            projection.history(ordinary_history.as_ref(), &path, &seeds)?;
                        if retained.is_empty() {
                            output.push_str(&format!(
                                "no replaced version is kept for {}\n",
                                seeds.join(", ")
                            ));
                        } else {
                            output.push_str(&retained);
                        }
                    }
                    output
                } else {
                    projection.pull(&seeds, options.budget.unwrap_or(40))?
                }
            }
            "affects" => projection.affects(&seeds)?,
            _ => return Err(error("unknown ordinary reader command")),
        };
        capture.verify()?;
        inventory.verify()?;
        crate::require(
            W::locate(&cwd, mode)? == location,
            "workspace changed while reading it; retry",
        )?;
        if command == "open" {
            return opened(data, output, taken.clone(), &location.workspace, reply);
        }
        return Ok(C::Output {
            text: output,
            code: 0,
        });
    }
    let unsupported = [
        ("--chars", options.chars.is_some() && !options.history),
        ("--budget", options.budget.is_some() && !options.history),
        ("--host", options.host.is_some() && !options.from_hook),
    ]
    .into_iter()
    .filter(|(_, v)| *v)
    .map(|(k, _)| k)
    .collect::<Vec<_>>();
    crate::require(
        unsupported.is_empty(),
        &format!(
            "core_profile_option_unsupported: {}",
            unsupported.join(", ")
        ),
    )?;
    let context = CapturedAssessment::from_snapshot(
        capture.snapshot()?.clone(),
        None,
        "focused-review/v1",
        runtime,
        OperationalBounds::default(),
        None,
    )?;
    let output = match command {
        "open" => {
            let (data, output) = C::opening(
                &context,
                data,
                paths[0]
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("record"),
            )?;
            C::Output {
                text: if as_json {
                    serde_json::to_string_pretty(&data)? + "\n"
                } else {
                    output + "\n"
                },
                code: 0,
            }
        }
        "pull" => {
            let mut output = C::pull(&context, &seeds)?;
            if options.history && output.code == 0 {
                let history = crate::public_history_read::render(
                    capture.node_history_capture(),
                    capture.history_capture(),
                    &seeds,
                    options
                        .chars
                        .or(options.budget.map(|b| b.saturating_mul(256))),
                    as_json,
                )?;
                if as_json {
                    let mut payload: J = serde_json::from_str(output.text.trim())?;
                    let historical: J = serde_json::from_str(history.trim())?;
                    payload["historical_section"] = historical["historical_section"].clone();
                    output.text = serde_json::to_string_pretty(&payload)? + "\n";
                } else {
                    output.text.push_str(&history);
                }
            }
            output
        }
        "affects" => C::affects(&context, &seeds)?,
        "check" => {
            let mut output = C::record_check(&context, has_brief(&paths, &mut inventory)?)?;
            let prefix = file_notes
                .iter()
                .map(|note| format!("NOTE {note}\n"))
                .collect::<String>();
            output.text.insert_str(0, &prefix);
            output
        }
        _ => return Err(error("unknown read command")),
    };
    capture.verify()?;
    inventory.verify()?;
    crate::require(
        W::locate(&cwd, mode)? == location,
        "workspace changed while reading it; retry",
    )?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn brief_presence_and_private_file_type_are_revalidated_without_reading_bodies() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let paths = vec![root.join("GROUNDING.yaml")];
        let mut absent = Inventory::default();
        assert!(!has_brief(&paths, &mut absent).unwrap());
        std::fs::create_dir(root.join(".kpopper")).unwrap();
        std::fs::write(root.join(".kpopper/view.yaml"), "title: first\n").unwrap();
        assert!(absent.verify().is_err());
        let mut captured = Inventory::default();
        assert!(has_brief(&paths, &mut captured).unwrap());
        assert!(captured.files.is_empty());
        std::fs::write(root.join(".kpopper/view.yaml"), "title: changed\n").unwrap();
        captured.verify().unwrap();
        let path = root.join("draft.json");
        std::fs::write(&path, b"secret").unwrap();
        let mut draft = Inventory::default();
        assert!(draft.file(&path).unwrap());
        assert!(draft.files.is_empty());
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(draft.verify().is_err());
    }
}
