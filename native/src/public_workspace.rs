//! Shared public discovery and explicitly bundled computation runtime.
use crate::{
    Result,
    history_contract::error,
    project_modes::Project,
    reasoning_runtime::{OperationalBounds, Runtime},
};
use std::path::{Path, PathBuf};

pub fn records(cwd: &Path) -> Result<Vec<PathBuf>> {
    let cwd = cwd.canonicalize()?;
    let project = Project::open(&cwd)?;
    if project.config_path.exists() {
        return Ok(vec![project.record(None)?]);
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .and_then(|p| p.canonicalize().ok());
    let mut current = cwd.clone();
    loop {
        for name in ["GROUNDING.yaml", "PROVENANCE.yaml"] {
            let path = current.join(name);
            if std::fs::symlink_metadata(&path).is_ok() {
                crate::require(
                    path.is_file(),
                    "The record path exists but is not an accessible file.",
                )?;
                return Ok(vec![path]);
            }
        }
        let Some(parent) = current.parent() else {
            break;
        };
        if project.is_git() && current == project.root
            || home.as_ref() == Some(&current)
            || home.as_deref() == Some(parent)
            || parent.parent().is_none()
        {
            break;
        }
        current = parent.to_owned();
    }
    // Project defaults validate the registered external location, including a
    // refusal for competing in-tree records. Never invent a duplicate beside it.
    let record = project.record(None)?;
    if project
        .common
        .as_ref()
        .is_some_and(|p| p.join("kpopper-record").exists())
    {
        crate::require(
            record.is_file(),
            "The registered record is unavailable. Restore its location before creating another.",
        )?;
    }
    Ok(vec![record])
}

pub fn runtime() -> Result<Option<Runtime>> {
    load_runtime(true)
}
/// A core-only authoring route needs the core archive, independently of ordinary
/// computation. Full distribution bundles still include both programs.
pub fn core_runtime() -> Result<Option<Runtime>> {
    load_runtime(false)
}
pub fn runtime_for_document(document: &crate::value::TypedValue) -> Result<Option<Runtime>> {
    let capabilities = crate::reasoning_fields::capabilities(document, None)?;
    if crate::history_contract::string_is(
        &crate::history_contract::map(&capabilities)?["profile"],
        "core/v1",
    ) {
        core_runtime()
    } else {
        runtime()
    }
}
fn load_runtime(ordinary: bool) -> Result<Option<Runtime>> {
    let configured = std::env::var_os("KPOPPER_NATIVE_RESOURCES").map(PathBuf::from);
    let root = configured.clone().unwrap_or(
        std::env::current_exe()?
            .parent()
            .ok_or_else(|| error("native executable has no parent"))?
            .join("resources"),
    );
    if configured.is_none() && !root.exists() {
        return Ok(None);
    }
    crate::require(root.is_dir(), "native resources directory is unavailable")?;
    let target = crate::reasoning_runtime::target_name()?;
    let cache = if let Some(path) = std::env::var_os("KPOPPER_NATIVE_CACHE") {
        PathBuf::from(path)
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .or_else(|| std::env::var_os("USERPROFILE"))
                    .map(|p| PathBuf::from(p).join(".cache"))
            })
            .ok_or_else(|| error("set KPOPPER_NATIVE_CACHE for the packaged runtime"))?
            .join("kpopper/native")
    };
    let runtime = Runtime::open(
        &root.join("reasoning").join(format!("{target}.zip")),
        &cache,
        OperationalBounds::default(),
    )?;
    if ordinary {
        let program = crate::ordinary_runtime::Program::open(&root.join("ordinary").join(target))?;
        Ok(Some(runtime.with_ordinary_program(program)))
    } else {
        Ok(Some(runtime))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub workspace: PathBuf,
    pub record: PathBuf,
    pub status: String,
    pub reason: String,
    pub key: String,
}
/// Public orientation keeps the deliberate subproject boundary used for host state.
pub fn locate(cwd: &Path, mode: crate::source_capture::ReadMode) -> Result<Location> {
    let cwd = cwd.canonicalize()?;
    let project = Project::open(&cwd)?;
    let mut workspace = if project.is_git() {
        project.root.clone()
    } else {
        cwd.clone()
    };
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .and_then(|p| p.canonicalize().ok());
    let mut current = cwd.clone();
    let mut record = None;
    let mut status = "missing";
    let mut reason = String::new();
    loop {
        if let Some(path) = ["GROUNDING.yaml", "PROVENANCE.yaml"]
            .iter()
            .map(|n| current.join(n))
            .find(|p| p.symlink_metadata().is_ok())
        {
            workspace = current.clone();
            status = if path.is_file() {
                "found"
            } else {
                "unavailable"
            };
            if status == "unavailable" {
                reason = "The record path exists but is not an accessible file.".into();
            }
            record = Some(path);
            break;
        }
        let Some(parent) = current.parent() else {
            break;
        };
        if project.is_git() && current == project.root
            || home.as_ref() == Some(&current)
            || home.as_deref() == Some(parent)
            || parent.parent().is_none()
        {
            break;
        }
        current = parent.into();
    }
    if record.is_none()
        && let Some(common) = &project.common
    {
        let pointer = common.join("kpopper-record");
        if pointer.symlink_metadata().is_ok() {
            let path = project.record(None)?;
            status = if path.is_file() {
                "found"
            } else {
                "unavailable"
            };
            if status == "unavailable" {
                reason="The registered record is unavailable. Restore its location before creating another.".into();
            }
            record = Some(path);
        }
    }
    let config = project.config()?;
    if project.config_path.exists() {
        let path = project.record(Some(&config))?;
        status = if path.is_file() {
            "found"
        } else if path.exists() {
            "unavailable"
        } else {
            "missing"
        };
        reason = if status == "unavailable" {
            "The configured record is unavailable.".into()
        } else {
            String::new()
        };
        record = Some(path);
    }
    if status == "missing"
        && mode == crate::source_capture::ReadMode::Live
        && project.is_git()
        && crate::history_contract::string_is(
            &crate::history_contract::map(&config)?["mode"],
            "advanced",
        )
        && crate::pending_state::Ledger::capture(&project)?
            .head
            .is_some()
    {
        record = Some(project.record(Some(&config))?);
        status = "pending";
    }
    let identity = if let Some(common) = &project.common {
        let relative = workspace
            .strip_prefix(&project.root)
            .map_err(|_| error("workspace outside project"))?;
        format!(
            "{}\0{}",
            common.display(),
            if relative.as_os_str().is_empty() {
                ".".into()
            } else {
                relative.to_string_lossy().into_owned()
            }
        )
    } else {
        workspace.to_string_lossy().into_owned()
    };
    Ok(Location {
        record: record.unwrap_or_else(|| workspace.join("GROUNDING.yaml")),
        workspace,
        status: status.into(),
        reason,
        key: crate::identity::sha256(identity.as_bytes()),
    })
}
