//! Explicit session readiness and native preference management.
use crate::{
    Error, Result,
    public_checked_session::{Operation, Options},
    require, session_settings,
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub struct Output {
    pub text: String,
    pub code: i32,
}
pub fn handles(operation: &Operation) -> bool {
    matches!(
        operation,
        Operation::Status | Operation::Setup | Operation::Enable | Operation::Disable
    )
}
struct Readiness {
    value: Value,
    archive: PathBuf,
    cache: PathBuf,
    cache_entry: PathBuf,
}
fn readiness(check_cache: bool) -> Result<Readiness> {
    let configured = std::env::var_os("KPOPPER_NATIVE_RESOURCES").map(PathBuf::from);
    let selection = crate::public_workspace::select_resources(
        &std::env::current_exe()?,
        configured.as_deref(),
        true,
    )?;
    let root = selection.root.ok_or_else(|| {
        Error(
            "native session resources are unavailable; restore the complete native package".into(),
        )
    })?;
    let ordinary = selection
        .ordinary
        .ok_or_else(|| Error("native ordinary program is unavailable".into()))?;
    let archive = selection
        .core
        .ok_or_else(|| Error("native core archive is unavailable".into()))?;
    let program = crate::ordinary_runtime::Program::inspect(&root, &ordinary)?;
    let core = crate::reasoning_runtime::inspect_archive(&root, &archive, &selection.target)?;
    let archive = root.join(archive);
    let cache = crate::public_workspace::runtime_cache()?;
    let cache_entry = cache
        .join("kpopper/reasoning")
        .join(core["sha256"].as_str().unwrap());
    if check_cache && cache_entry.try_exists()? {
        crate::reasoning_runtime::Runtime::inspect_cache_files(&cache_entry, &core["manifest"])?;
    }
    Ok(Readiness {
        archive,
        cache,
        cache_entry: cache_entry.clone(),
        value: json!({"source_sha256":env!("KPOP_ORDINARY_SOURCE_SHA256"), "cache":root.join(ordinary),
        "ready":true, "build":program["manifest"], "runtime":"native", "runtime_cache":cache_entry,"core":core}),
    })
}
pub fn run(options: &Options, cwd: &Path) -> Result<Output> {
    let cwd = cwd.canonicalize()?;
    let value = match options.operation {
        Operation::Status => match readiness(true) {
            Ok(value) => value.value,
            Err(error) => {
                return Ok(Output {
                    text: serde_json::to_string_pretty(
                        &json!({"source_sha256":env!("KPOP_ORDINARY_SOURCE_SHA256"),"ready":false,"reason":error.to_string(),"runtime":"native"}),
                    )?,
                    code: 1,
                });
            }
        },
        Operation::Setup => {
            let ready = readiness(!options.rebuild)?;
            let parent = ready.cache_entry.parent().unwrap();
            fs::create_dir_all(parent)?;
            let _guard = crate::history_transaction_fs::DirectoryGuard::acquire(parent, true)?;
            let previous = if options.rebuild && ready.cache_entry.try_exists()? {
                let backup = ready.cache_entry.with_file_name(format!(
                    "{}.backup-{}",
                    ready.cache_entry.file_name().unwrap().to_string_lossy(),
                    uuid::Uuid::new_v4().simple()
                ));
                fs::rename(&ready.cache_entry, &backup)?;
                Some(backup)
            } else {
                None
            };
            if let Err(error) = crate::reasoning_runtime::Runtime::open(
                &ready.archive,
                &ready.cache,
                Default::default(),
            ) {
                if let Some(backup) = &previous
                    && !ready.cache_entry.try_exists()?
                {
                    fs::rename(backup, &ready.cache_entry)?;
                }
                return Err(error);
            }
            let mut value = ready.value;
            value["previous_cache"] = json!(previous);
            value
        }
        Operation::Enable | Operation::Disable => {
            require(
                options
                    .assessment_profile
                    .as_deref()
                    .is_none_or(|p| p == "checked-reader/v1"),
                "core/v1 session routing is explicit per invocation; default activation is not enabled",
            )?;
            require(
                !options.global_scope
                    || (options.profile.is_none()
                        && options.project.is_none()
                        && options.state.is_none()),
                "profile, project and state settings require project-scoped enablement",
            )?;
            let enabled = matches!(options.operation, Operation::Enable);
            let mut value = json!({"schema":1,"enabled":enabled});
            if enabled {
                readiness(true)?;
                let tokens = options.tokens.unwrap_or(1000);
                require((64..=65536).contains(&tokens), "tokens must be 64..65536")?;
                value["native"] = json!(std::env::current_exe()?.canonicalize()?);
                value["tokens"] = json!(tokens);
                if let Some(profile) = &options.profile {
                    let profile = session_settings::expand(&cwd, profile)?;
                    let mut inventory = crate::source_inventory::Inventory::default();
                    let raw = inventory.read(&profile)?;
                    let data: Value = serde_json::from_slice(&raw)?;
                    require(
                        data.is_object() && data["groups"].is_object(),
                        "profile must contain declared groups",
                    )?;
                    inventory.verify()?;
                    value["profile"] = json!(profile);
                }
                if let Some(project) = &options.project {
                    value["project"] = json!(project);
                }
                if let Some(state) = &options.state {
                    value["state"] = json!(session_settings::expand(&cwd, state)?);
                }
            }
            let paths = session_settings::paths(&cwd, &cwd)?;
            let path = if options.global_scope {
                paths.native_global
            } else {
                paths.native_local
            };
            session_settings::write(&path, &value)?;
            value["settings"] = json!(path);
            value
        }
        _ => return Err(Error("not a session management operation".into())),
    };
    Ok(Output {
        text: serde_json::to_string_pretty(&value)?,
        code: 0,
    })
}
