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
    let ordinary = crate::ordinary_runtime::Program::open(&root.join("ordinary").join(target))?;
    Ok(Some(runtime.with_ordinary_program(ordinary)))
}
