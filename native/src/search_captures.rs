//! Read-only ingestion ownership and byte-integrity checks for search captures.
use super::search_corpus::{expanded, py_error};
use crate::{
    Error, Result,
    identity::sha256,
    source_inventory::{Inventory, name},
};
use serde_json::{Value as J, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
pub(super) struct Captures {
    pub rows: Vec<J>,
    pub diagnostics: Vec<J>,
    pub files: BTreeMap<String, J>,
    pub invalid: BTreeSet<String>,
    pub inventory: Inventory,
}
fn read_json(path: &Path, inventory: &mut Inventory) -> Result<Option<J>> {
    if !inventory.exists(path)? {
        return Ok(None);
    }
    let raw = inventory.read(path)?;
    Ok(Some(
        crate::json_ingress::parse_slice(&raw, crate::json_ingress::DuplicateKeys::LastWins)
            .map_err(|e| Error(e.to_string()))?,
    ))
}
fn state(
    record: &Path,
    cwd: &Path,
    requested: Option<&Path>,
    inventory: &mut Inventory,
) -> Result<(PathBuf, bool)> {
    let selected = crate::project_modes::write_paths(&[record.to_owned()], cwd)?[0].clone();
    let record = if let Some(root) = requested {
        let root = expanded(cwd, root)?;
        if read_json(&root.join("record.json"), inventory)? == Some(json!({"record":record})) {
            record.to_owned()
        } else {
            selected
        }
    } else {
        selected
    };
    let root = if let Some(path) = requested {
        expanded(cwd, path)?
    } else {
        let base = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state")
            });
        expanded(
            cwd,
            &base
                .join("kpopper/ingestion")
                .join(sha256(name(&record)?.as_bytes())),
        )?
    };
    let marker = root.join("record.json");
    if !marker.is_file() {
        return Ok((root, false));
    }
    crate::require(
        read_json(&marker, inventory)? == Some(json!({"record":record})),
        "state directory belongs to another provenance record",
    )?;
    Ok((root, true))
}
pub(super) fn read(record: &Path, cwd: &Path, requested: Option<&Path>) -> Result<Captures> {
    let mut out = Captures {
        rows: vec![],
        diagnostics: vec![],
        files: BTreeMap::new(),
        invalid: BTreeSet::new(),
        inventory: Inventory::default(),
    };
    let (root, exists) = state(record, cwd, requested, &mut out.inventory)?;
    if !exists {
        return Ok(out);
    }
    let paths = out.inventory.glob(&root.join("events/*.json"))?;
    for path in paths {
        let event = read_json(&path, &mut out.inventory)?
            .ok_or_else(|| Error(format!("invalid captured event: {}", path.display())))?;
        let event = event
            .as_object()
            .ok_or_else(|| Error(format!("invalid captured event: {}", path.display())))?;
        let eid = event
            .get("event_id")
            .and_then(J::as_str)
            .ok_or_else(|| Error(format!("invalid captured event: {}", path.display())))?;
        crate::require(
            !eid.is_empty()
                && eid.len() <= 128
                && eid
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
            &format!("invalid captured event: {}", path.display()),
        )?;
        let envelope_path = root.join("envelopes").join(format!("{eid}.json"));
        let source_path = root.join("sources").join(format!("{eid}.txt"));
        let capture = (|| -> std::result::Result<J, String> {
            let envelope = fs::read(&envelope_path).map_err(|e| {
                format!(
                    "captured input is unavailable: {}",
                    py_error(&e, &envelope_path)
                )
            })?;
            let source = fs::read(&source_path).map_err(|e| {
                format!(
                    "captured input is unavailable: {}",
                    py_error(&e, &source_path)
                )
            })?;
            if event.get("source_file") != Some(&json!(source_path)) {
                return Err("captured source path changed after capture".into());
            }
            if event.get("envelope_sha256") != Some(&json!(sha256(&envelope))) {
                return Err("captured envelope changed after capture".into());
            }
            if event.get("source_sha256") != Some(&json!(sha256(&source))) {
                return Err("captured source text changed after capture".into());
            }
            crate::json_ingress::parse_slice(
                &envelope,
                crate::json_ingress::DuplicateKeys::LastWins,
            )
            .map_err(|e| {
                format!(
                    "captured envelope cannot be read: {}",
                    e.to_string()
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                )
            })
        })();
        let envelope = match capture {
            Ok(v) => v,
            Err(reason) => {
                if let Some(file) = event.get("source_file").and_then(J::as_str) {
                    out.invalid.insert(file.into());
                }
                out.diagnostics
                    .push(json!({"ref":format!("capture:{eid}"),"reason":reason}));
                continue;
            }
        };
        let object = envelope
            .as_object()
            .ok_or_else(|| Error(format!("invalid captured event: {}", path.display())))?;
        let content = object
            .get("source_quote")
            .and_then(J::as_str)
            .ok_or_else(|| Error(format!("invalid captured event: {}", path.display())))?;
        let file = event.get("source_file").and_then(J::as_str).unwrap();
        let status = event
            .get("state")
            .cloned()
            .ok_or_else(|| Error(format!("invalid captured event: {}", path.display())))?;
        let targets = if let Some(updates) = object.get("updates") {
            let updates = updates
                .as_array()
                .ok_or_else(|| Error("invalid captured updates".into()))?;
            J::Array(
                updates
                    .iter()
                    .map(|op| {
                        op.get("id")
                            .cloned()
                            .ok_or_else(|| Error("invalid captured update".into()))
                    })
                    .collect::<Result<_>>()?,
            )
        } else {
            object.get("target").cloned().unwrap_or(J::Null)
        };
        let question = object.get("question").cloned().unwrap_or(J::Null);
        out.files.insert(file.into(), status.clone());
        out.rows.push(json!({"ref":format!("capture:{eid}"),"id":eid,"kind":"capture","scope":"captured_report","status":status,"name":if question.as_str().is_some_and(|s|!s.is_empty()){question.clone()}else{json!("Captured report")},"content":content,"file":file,"sources":[],"dependencies":[],"targets":targets,"question":question,"reason":event.get("reason").cloned().unwrap_or(J::Null)}));
    }
    Ok(out)
}
