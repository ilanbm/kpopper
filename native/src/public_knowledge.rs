//! Public local knowledge status and immutable contribution materialization.
//! Neither operation activates a record or publishes remote state.
use crate::{
    Error, Result,
    history_contract::{field, is_int, map},
    history_transaction_fs as guarded_fs,
    pending_state::{self, Ledger},
    project_modes::{self, Project},
    require,
    source_capture::{ReadMode, capture_source_with_runtime},
    value::TypedValue as V,
};
use clap::Subcommand;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

fn s(value: &str) -> V {
    V::Text(value.into())
}
fn empty() -> V {
    V::Map(BTreeMap::new())
}
fn object(items: impl IntoIterator<Item = (&'static str, V)>) -> V {
    V::Map(
        items
            .into_iter()
            .map(|(key, value)| (key.into(), value))
            .collect(),
    )
}

#[derive(Clone, Debug, clap::Args)]
pub struct Options {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Clone, Debug, Subcommand)]
pub enum Command {
    /// Inspect the effective local knowledge view and its explicit alternatives.
    Status(StatusOptions),
    /// Materialize one immutable contribution into a new frozen snapshot directory.
    Materialize(MaterializeOptions),
}

#[derive(Clone, Debug, Default, clap::Args)]
pub struct StatusOptions {}

#[derive(Clone, Debug, clap::Args)]
pub struct MaterializeOptions {
    pub revision: String,
    #[arg(long)]
    pub out: PathBuf,
    #[arg(long = "ref")]
    pub reference: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

fn private_drafts(project: &Project) -> Result<V> {
    let home = std::env::var_os("KPOPPER_PRIVATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|home| PathBuf::from(home).join(".local/share/kpopper/private"))
        })
        .ok_or_else(|| Error("private_home_unavailable".into()))?;
    let owner = project.common.as_ref().unwrap_or(&project.root);
    let directory = home.join(crate::identity::sha256(
        owner
            .to_str()
            .ok_or_else(|| Error("nonportable_project_path".into()))?
            .as_bytes(),
    ));
    let mut paths = match fs::read_dir(&directory) {
        Ok(entries) => entries
            .map(|entry| entry.map(|value| value.path()))
            .collect::<std::io::Result<Vec<_>>>()?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => vec![],
        Err(error) => return Err(error.into()),
    };
    paths.sort();
    let drafts = paths
        .into_iter()
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        .filter_map(|path| match path.is_file() {
            true => Some(path),
            false => None,
        })
        .map(|path| {
            let event = path
                .file_stem()
                .and_then(|value| value.to_str())
                .ok_or_else(|| Error("nonportable_project_path".into()))?;
            Ok(object([
                ("event_id", s(event)),
                ("state", s("private draft")),
                (
                    "path",
                    s(path
                        .to_str()
                        .ok_or_else(|| Error("nonportable_project_path".into()))?),
                ),
            ]))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(V::List(drafts))
}

pub fn status(workspace: &Path, mode: ReadMode, _options: &StatusOptions) -> Result<V> {
    let project = Project::open(workspace)?;
    let config = project.config()?;
    let record = project.record(Some(&config))?;
    let mut context = object([
        ("contributions", V::List(vec![])),
        ("conflicts", empty()),
        ("publication", V::Null),
        ("target_unavailable", V::Null),
    ]);
    if record.exists()
        || mode == ReadMode::Live && project.is_git() && Ledger::capture(&project)?.head.is_some()
    {
        let mut capture = capture_source_with_runtime(
            std::slice::from_ref(&record),
            workspace,
            mode,
            None,
            None,
        )?;
        let target_needs_runtime = map(&capture.knowledge_status_context())?["target_unavailable"]
            == s("target_runtime_required");
        if mode == ReadMode::Live
            && target_needs_runtime
            && let Some(runtime) = crate::public_workspace::core_runtime()?
        {
            capture = capture_source_with_runtime(
                std::slice::from_ref(&record),
                workspace,
                mode,
                None,
                Some(&runtime),
            )?;
        }
        context = capture.knowledge_status_context();
        capture.verify()?;
    } else if mode == ReadMode::Live && project.is_git() && map(&config)?["mode"] == s("advanced") {
        context = object([
            ("contributions", V::List(vec![])),
            ("conflicts", empty()),
            (
                "publication",
                pending_state::Observation::capture(&project, &config)?.publication,
            ),
            ("target_unavailable", V::Null),
        ]);
    }
    let fields = map(&context)?;
    Ok(object([
        ("mode", map(&config)?["mode"].clone()),
        (
            "record",
            s(record
                .to_str()
                .ok_or_else(|| Error("nonportable_project_path".into()))?),
        ),
        (
            "read_mode",
            s(if mode == ReadMode::Live {
                "live"
            } else {
                "frozen"
            }),
        ),
        ("contributions", fields["contributions"].clone()),
        ("conflicts", fields["conflicts"].clone()),
        ("publication", fields["publication"].clone()),
        (
            "private_drafts",
            if mode == ReadMode::Live {
                private_drafts(&project)?
            } else {
                V::List(vec![])
            },
        ),
        ("target_unavailable", fields["target_unavailable"].clone()),
    ]))
}

fn snapshot_bytes(
    revision: &str,
    ledger_ref: Option<&str>,
    manifest: Option<&V>,
) -> Result<Vec<u8>> {
    let mut metadata = serde_json::Map::from_iter([
        (
            "version".into(),
            serde_json::json!(if manifest.is_some() { 3 } else { 1 }),
        ),
        ("revision".into(), serde_json::json!(revision)),
        ("ledger_ref".into(), serde_json::json!(ledger_ref)),
        ("read_mode".into(), serde_json::json!("frozen")),
    ]);
    if let Some(manifest) = manifest {
        metadata.insert("contribution".into(), manifest.to_tagged()?);
    }
    serde_json::to_vec(&serde_json::Value::Object(metadata)).map_err(Into::into)
}

fn populate(
    root: &Path,
    revision: &str,
    ledger_ref: Option<&str>,
    bundle: &pending_state::Bundle,
) -> Result<()> {
    crate::pending_bundle::validate(&bundle.value, &bundle.files)?;
    for (name, raw) in &bundle.files {
        crate::history_branch::portable_path(name)?;
        require(
            !matches!(name.as_str(), "GROUNDING.yaml" | "snapshot.json"),
            "evidence conflicts with snapshot metadata",
        )?;
        let target = guarded_fs::target(root, name)?;
        fs::create_dir_all(target.parent().unwrap())?;
        fs::write(target, raw)?;
    }
    let manifest = field(map(&bundle.value)?, "manifest")?;
    let version = field(map(manifest)?, "version")?;
    let document = field(map(manifest)?, "document")?;
    if is_int(version, "3") {
        let captured = crate::history_bundle::validate_contribution(&bundle.value, &bundle.files)?;
        let layout = crate::history_capture::Layout::for_entry("GROUNDING.yaml")?;
        for (name, raw) in &bundle.files {
            let Some(relative) = name.strip_prefix("history-closure/") else {
                continue;
            };
            let destination = if relative == "authority.yaml" {
                Some(layout.authority.clone())
            } else if let Some(name) = relative.strip_prefix("commits/") {
                Some(format!("{}/{name}", layout.commits))
            } else if let Some(name) = relative.strip_prefix("cancellations/") {
                Some(format!("{}/{name}", layout.cancellations))
            } else if let Some(name) = relative.strip_prefix("objects/") {
                Some(format!("{}/{name}", layout.objects))
            } else {
                None
            };
            if let Some(destination) = destination {
                let path = guarded_fs::target(root, &destination)?;
                if path.exists() {
                    require(
                        path.is_file() && fs::read(&path)? == *raw,
                        "evidence conflicts with history snapshot authority",
                    )?;
                } else {
                    fs::create_dir_all(path.parent().unwrap())?;
                    fs::write(path, raw)?;
                }
            }
        }
        fs::write(
            guarded_fs::target(root, "GROUNDING.yaml")?,
            crate::history_emit::encode_document(captured.document())?,
        )?;
    } else {
        fs::write(
            guarded_fs::target(root, "GROUNDING.yaml")?,
            crate::history_emit::encode_document(document)?,
        )?;
    }
    fs::write(
        guarded_fs::target(root, "snapshot.json")?,
        snapshot_bytes(
            revision,
            ledger_ref,
            is_int(version, "3").then_some(manifest),
        )?,
    )?;
    if is_int(version, "3") {
        let capture = capture_source_with_runtime(
            &[root.join("GROUNDING.yaml")],
            root,
            ReadMode::Frozen,
            None,
            None,
        )?;
        require(
            capture.strict_document()?.digest()? == document.digest()?,
            "materialized history does not reproduce captured contribution",
        )?;
        capture.verify()?;
    }
    Ok(())
}

fn atomic_tree(
    destination: &Path,
    populate: &mut dyn FnMut(&Path) -> Result<()>,
) -> Result<PathBuf> {
    let destination = project_modes::resolved(destination)?;
    require(
        !destination.exists() && destination.symlink_metadata().is_err(),
        "snapshot destination already exists; select a new directory",
    )?;
    let parent = destination
        .parent()
        .ok_or_else(|| Error("invalid_path".into()))?;
    fs::create_dir_all(parent)?;
    let parent = parent.canonicalize()?;
    let destination = parent.join(
        destination
            .file_name()
            .ok_or_else(|| Error("invalid_path".into()))?,
    );
    let temporary = tempfile::Builder::new()
        .prefix(".knowledge-")
        .tempdir_in(&parent)?;
    let staging = temporary.path().join("snapshot");
    fs::create_dir(&staging)?;
    populate(&staging)?;
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    rustix::fs::renameat_with(
        rustix::fs::CWD,
        &staging,
        rustix::fs::CWD,
        &destination,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(std::io::Error::from)
    .map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            Error("snapshot destination already exists; select a new directory".into())
        } else {
            error.into()
        }
    })?;
    #[cfg(windows)]
    fs::rename(&staging, &destination).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            Error("snapshot destination already exists; select a new directory".into())
        } else {
            error.into()
        }
    })?;
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    return Err(Error(
        "atomic absent-destination publication is unavailable on this platform".into(),
    ));
    Ok(destination)
}

pub fn materialize(workspace: &Path, options: &MaterializeOptions) -> Result<V> {
    let project = Project::open(workspace)?;
    require(
        options.revision.len() == 64
            && options
                .revision
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "invalid contribution revision",
    )?;
    let head = match &options.reference {
        Some(reference) => pending_state::resolve(&project.root, reference)?,
        None => pending_state::resolve(&project.root, pending_state::REF)?,
    };
    let ledger = Ledger::at(&project.root, head.clone())?;
    let bundle = ledger.bundles.get(&options.revision).ok_or_else(|| {
        Error("contribution is unavailable at the selected ledger revision".into())
    })?;
    let destination = atomic_tree(&options.out, &mut |root| {
        populate(root, &options.revision, head.as_deref(), bundle)
    })?;
    Ok(object([
        ("state", s("materialized")),
        ("revision", s(&options.revision)),
        (
            "record",
            s(destination
                .join("GROUNDING.yaml")
                .to_str()
                .ok_or_else(|| Error("nonportable_project_path".into()))?),
        ),
        ("read_mode", s("frozen")),
        ("ledger_ref", head.map(|value| s(&value)).unwrap_or(V::Null)),
    ]))
}

pub fn run(options: &Options, workspace: &Path, mode: ReadMode) -> Result<V> {
    match &options.command {
        Command::Status(status_options) => status(workspace, mode, status_options),
        Command::Materialize(materialize_options) => materialize(workspace, materialize_options),
    }
}

/// CLI boundary matching `knowledge_cli`: structured results and refusals are
/// written to stdout, and operational refusals exit 2.
pub fn dispatch(options: &Options, workspace: &Path, mode: ReadMode) -> CommandOutput {
    let value = match run(options, workspace, mode) {
        Ok(value) => (value.to_json(), 0),
        Err(error) => (
            Ok(serde_json::json!({"state":"needs attention","error":error.to_string()})),
            2,
        ),
    };
    match value
        .0
        .and_then(|value| serde_json::to_string(&value).map_err(Into::into))
    {
        Ok(stdout) => CommandOutput {
            stdout: stdout + "\n",
            stderr: String::new(),
            code: value.1,
        },
        Err(error) => CommandOutput {
            stdout: format!(
                "{{\"state\":\"needs attention\",\"error\":{}}}\n",
                serde_json::json!(error.to_string())
            ),
            stderr: String::new(),
            code: 2,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guarded_staging_refuses_evidence_escape_without_publishing() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("snapshot");
        let outside = temp.path().join("escape");
        let error = atomic_tree(&destination, &mut |root| {
            let path = guarded_fs::target(root, "../escape")?;
            fs::write(path, b"escaped")?;
            Ok(())
        })
        .unwrap_err();
        assert_eq!(error.0, "invalid_path");
        assert!(!destination.exists());
        assert!(!outside.exists());
    }
}
