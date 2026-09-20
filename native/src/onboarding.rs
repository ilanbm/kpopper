//! Private first-use state.  This module deliberately knows nothing about records' contents.
use crate::{Error, Result, require};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, io::Write, path::{Path, PathBuf}};

pub const MODES: [&str; 3] = ["work", "map", "deep"];
pub const MAPPING_STATES: [&str; 5] = ["requested", "ready", "running", "complete", "failed"];

pub fn state_dir() -> Result<PathBuf> {
    let root = std::env::var_os("XDG_STATE_HOME").filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|v| PathBuf::from(v).join(".local/state")))
        .ok_or_else(|| Error("state directory is unavailable".into()))?;
    Ok(root.join("kpopper/first-use"))
}
pub fn project_key(workspace: &Path) -> String {
    let mut h = Sha256::new(); h.update(workspace.to_string_lossy().as_bytes());
    format!("{:x}", h.finalize())
}
pub fn project_dir(workspace: &Path) -> Result<PathBuf> { Ok(state_dir()?.join("projects").join(project_key(workspace))) }
pub fn project_dir_key(key: &str) -> Result<PathBuf> { Ok(state_dir()?.join("projects").join(key)) }

pub fn read(path: &Path) -> Result<Option<Value>> {
    let raw = match fs::read(path) { Ok(v) => v, Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None), Err(e) => return Err(e.into()) };
    let value: Value = serde_json::from_slice(&raw).map_err(|_| Error(format!("Invalid first-use state; keep it for inspection: {}", path.display())))?;
    require(value.get("schema") == Some(&json!(1)) && value.is_object(), &format!("Invalid first-use state; keep it for inspection: {}", path.display()))?;
    Ok(Some(value))
}
fn private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; fs::set_permissions(path, fs::Permissions::from_mode(0o700))?; }
    Ok(())
}
pub fn write(path: &Path, fields: &Value) -> Result<()> {
    let parent = path.parent().ok_or_else(|| Error("invalid state path".into()))?; private_dir(parent)?;
    let object = fields.as_object().ok_or_else(|| Error("state must be an object".into()))?;
    let mut out = serde_json::Map::new(); out.insert("schema".into(), json!(1));
    out.extend(object.clone());
    let mut tmpfile = tempfile::Builder::new().prefix(".first-use-").tempfile_in(parent)?;
    tmpfile.as_file_mut().write_all(serde_json::to_string_pretty(&Value::Object(out))?.as_bytes())?; tmpfile.as_file_mut().write_all(b"\n")?; tmpfile.as_file_mut().sync_all()?;
    tmpfile.persist(path).map(|_| ()).map_err(|e| e.error.into())
}
pub fn guidance() -> Result<bool> {
    let value = read(&state_dir()?.join("guidance.json"))?.unwrap_or(json!({"enabled":true}));
    value.get("enabled").and_then(Value::as_bool).ok_or_else(|| Error("Invalid first-use guidance preference".into()))
}
pub fn status(workspace: &Path, record: &Path, record_found: bool) -> Result<Value> {
    let dir = project_dir(workspace)?;
    let map = read(&dir.join("mapping.json"))?.or(read(&dir.join("choice.json"))?).unwrap_or(json!({}));
    let mode = map.get("mode").and_then(Value::as_str);
    let mapping = map.get("mapping").and_then(Value::as_str);
    if let Some(m) = mode { require(MODES.contains(&m), "Invalid first-use choice")?; }
    if let Some(m) = mapping { require(MAPPING_STATES.contains(&m), "Invalid first-use choice")?; }
    let mut pending = Vec::new();
    for event in ["record","source","decision","conflict","reuse","review"] {
        if read(&state_dir()?.join("shown").join(format!("{event}.json")))?.is_none() { pending.push(event); }
    }
    Ok(json!({"workspace":workspace,"record":record,"status":if record_found{"found"}else{"missing"},
        "mode":mode,"mapping":mapping,"request":map.get("request"),"owner":map.get("owner"),"report":map.get("report"),"check":map.get("check"),"error":map.get("error"),
        "guidance":guidance()?,"introduced":read(&state_dir()?.join("shown/welcome.json"))?.is_some(),
        "followups_offered":read(&dir.join("followups-offered.json"))?.is_some(),"offered":read(&dir.join("offered.json"))?.is_some() || mode.is_some(),"pending_tips":pending}))
}
pub fn status_at(workspace: &Path, key: &str, record: &Path, status: &str, reason: &str) -> Result<Value> {
    let dir = project_dir_key(key)?;
    let map = read(&dir.join("mapping.json"))?.or(read(&dir.join("choice.json"))?).unwrap_or(json!({}));
    let mode = map.get("mode").and_then(Value::as_str);
    let mapping = map.get("mapping").and_then(Value::as_str);
    if let Some(m) = mode { require(MODES.contains(&m), "Invalid first-use choice")?; }
    if let Some(m) = mapping { require(MAPPING_STATES.contains(&m), "Invalid first-use choice")?; }
    if matches!(mode, Some("map"|"deep")) { require(mapping.is_some() && map.get("request").and_then(Value::as_str).is_some_and(|v| v.len()==32 && v.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())), "Invalid mapping request")?; }
    if matches!(mode, None|Some("work")) { require(mapping.is_none() && map.get("request").is_none(), "Invalid first-use choice")?; }
    let mut pending = Vec::new();
    for event in ["record","source","decision","conflict","reuse","review"] {
        if read(&state_dir()?.join("shown").join(format!("{event}.json")))?.is_none() { pending.push(event); }
    }
    Ok(json!({"workspace":workspace,"record":record,"status":status,"reason":reason,"key":key,"mode":mode,"mapping":mapping,"request":map.get("request"),"owner":map.get("owner"),"report":map.get("report"),"check":map.get("check"),"error":map.get("error"),"guidance":guidance()? ,"introduced":read(&state_dir()?.join("shown/welcome.json"))?.is_some(),"followups_offered":read(&dir.join("followups-offered.json"))?.is_some(),"offered":read(&dir.join("offered.json"))?.is_some() || mode.is_some(),"pending_tips":pending}))
}

pub fn mark(workspace: &Path, event: &str) -> Result<Value> {
    require(event == "welcome" || event == "followups" || ["record","source","decision","conflict","reuse","review"].contains(&event), "invalid event")?;
    let path = if event == "followups" { project_dir(workspace)?.join("followups-offered.json") } else { state_dir()?.join("shown").join(format!("{event}.json")) };
    write(&path, &json!({"shown":true}))?;
    if event == "welcome" { write(&project_dir(workspace)?.join("offered.json"), &json!({"shown":true}))?; }
    status(workspace, &workspace.join("GROUNDING.yaml"), workspace.join("GROUNDING.yaml").is_file())
}
pub fn mark_key(workspace: &Path, key: &str, record: &Path, status_value: &str, event: &str) -> Result<Value> {
    require(event == "welcome" || event == "followups" || ["record","source","decision","conflict","reuse","review"].contains(&event), "invalid event")?;
    let path = if event == "followups" { project_dir_key(key)?.join("followups-offered.json") } else { state_dir()?.join("shown").join(format!("{event}.json")) };
    write(&path, &json!({"shown":true}))?;
    if event == "welcome" { write(&project_dir_key(key)?.join("offered.json"), &json!({"shown":true}))?; }
    status_at(workspace, key, record, status_value, "")
}
