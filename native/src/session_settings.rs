//! Native session preferences, with read-only fallback to existing Python preferences.
use crate::{Error, Result, project_modes::Project, require, source_inventory::Inventory};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| Error("home_directory_unavailable".into()))
}
pub(crate) fn expand(cwd: &Path, path: &Path) -> Result<PathBuf> {
    let path = if let Ok(tail) = path.strip_prefix("~") {
        home()?.join(tail)
    } else {
        cwd.join(path)
    };
    crate::project_modes::resolved(&path)
}
pub(crate) struct Paths {
    pub native_local: PathBuf,
    pub legacy_local: PathBuf,
    pub native_global: PathBuf,
    pub legacy_global: PathBuf,
}
pub(crate) fn paths(directory: &Path, cwd: &Path) -> Result<Paths> {
    let base = match std::env::var_os("XDG_CONFIG_HOME").filter(|s| !s.is_empty()) {
        Some(path) => PathBuf::from(path),
        None => home()?.join(".config"),
    }
    .join("kpopper");
    let (native_local, legacy_local) =
        if let Some(path) = std::env::var_os("KPOPPER_SESSION_CONFIG").filter(|p| !p.is_empty()) {
            let path = expand(cwd, Path::new(&path))?;
            (path.clone(), path)
        } else if let Some(common) = Project::open(directory)?.common {
            (
                common.join("kpopper-native-session.json"),
                common.join("kpopper-session.json"),
            )
        } else {
            let key = crate::identity::sha256(directory.to_string_lossy().as_bytes());
            (
                base.join("native-projects").join(format!("{key}.json")),
                base.join("projects").join(format!("{key}.json")),
            )
        };
    Ok(Paths {
        native_local,
        legacy_local,
        native_global: base.join("native-session.json"),
        legacy_global: base.join("session.json"),
    })
}
pub(crate) fn read(inventory: &mut Inventory, path: &Path) -> Result<Option<Value>> {
    if !inventory.exists(path)? {
        return Ok(None);
    }
    let raw = inventory.read(path)?;
    require(
        raw.len() <= 64 * 1024,
        "session settings exceed their byte limit",
    )?;
    let value =
        crate::json_ingress::parse_slice(&raw, crate::json_ingress::DuplicateKeys::LastWins)?;
    require(
        value.is_object() && value["schema"] == 1 && value["enabled"].is_boolean(),
        &format!("invalid checked-session settings: {}", path.display()),
    )?;
    if value["enabled"] == true {
        let executable = value.get("native").or_else(|| value.get("python"));
        require(
            executable
                .and_then(Value::as_str)
                .is_some_and(|s| Path::new(s).is_absolute()),
            if value.get("native").is_some() {
                "session settings need an absolute native executable"
            } else {
                "session settings need an absolute Python executable"
            },
        )?;
        require(
            value["tokens"]
                .as_u64()
                .is_some_and(|n| (64..=65536).contains(&n)),
            "session token budget must be 64..65536",
        )?;
    }
    Ok(Some(value))
}
pub(crate) fn current(inventory: &mut Inventory, directory: &Path, cwd: &Path) -> Result<Value> {
    if std::env::var("KPOPPER_SESSION_DISABLE").as_deref() == Ok("1") {
        return Ok(json!({"enabled":false}));
    }
    let paths = paths(directory, cwd)?;
    for path in [
        &paths.native_local,
        &paths.legacy_local,
        &paths.native_global,
        &paths.legacy_global,
    ] {
        if let Some(value) = read(inventory, path)? {
            return Ok(value);
        }
    }
    Ok(json!({}))
}
pub(crate) fn write(path: &Path, value: &Value) -> Result<()> {
    let mut inventory = Inventory::default();
    read(&mut inventory, path)?;
    let parent = path
        .parent()
        .ok_or_else(|| Error("invalid settings path".into()))?;
    fs::create_dir_all(parent)?;
    let _guard = crate::history_transaction_fs::DirectoryGuard::acquire(parent, true)?;
    inventory.verify()?;
    let mut file = tempfile::Builder::new()
        .prefix(".session-")
        .tempfile_in(parent)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.write_all(b"\n")?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| Error(e.error.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_settings_are_preserved_and_native_runtime_is_explicit() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        fs::write(&path, b"unrelated file").unwrap();
        assert!(write(&path, &json!({"schema":1,"enabled":false})).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"unrelated file");
        fs::remove_file(&path).unwrap();
        let value = json!({"schema":1,"enabled":true,"native":std::env::current_exe().unwrap(),"tokens":1000});
        write(&path, &value).unwrap();
        assert_eq!(read(&mut Inventory::default(), &path).unwrap(), Some(value));
    }
}
