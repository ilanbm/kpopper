//! Retained ingestion receipts establish recorded purpose, not source truth.
use crate::{
    Result, history_contract::map, history_yaml::SourceValue, identity::sha256,
    project_modes::resolved, source_capture::CapturedSource, source_inventory::Inventory,
    value::TypedValue,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

/// Supplied only by an internal preparing writer, never record YAML or CLI flags.
#[derive(Clone, Debug)]
pub struct PreparingContext {
    pub record: PathBuf,
    pub state_dir: PathBuf,
    pub event_id: String,
    pub shadow: PathBuf,
}
#[derive(Default)]
pub struct RecordingSources {
    ids: BTreeSet<String>,
    inventory: Inventory,
}
impl RecordingSources {
    pub fn ids(&self) -> &BTreeSet<String> {
        &self.ids
    }
    pub fn verify(&self) -> Result<()> {
        self.inventory.verify()
    }
}
fn load(inventory: &mut Inventory, path: &Path) -> Result<Value> {
    if !inventory.exists(path)? {
        return Ok(Value::Null);
    }
    crate::require(inventory.file(path)?, "ingestion_evidence_not_regular")?;
    Ok(serde_json::from_slice(&inventory.read(path)?)?)
}

/// Read one durably applied ingestion receipt while revalidating its owner and
/// immutable captured inputs. This is the read-only boundary used by consumers
/// that bind a saved result to current record evidence.
pub fn applied_event_binding(
    event_id: &str,
    record: &Path,
    state_dir: Option<&Path>,
) -> Result<Value> {
    crate::require(
        !event_id.is_empty()
            && event_id.len() <= 128
            && event_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
        "invalid_ingestion_event_id",
    )?;
    let record = resolved(record)?;
    let state = match state_dir {
        Some(path) => resolved(path)?,
        None => {
            let base = std::env::var_os("XDG_STATE_HOME")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var_os("HOME").map(|value| PathBuf::from(value).join(".local/state"))
                })
                .ok_or_else(|| crate::Error("home_directory_unavailable".into()))?;
            resolved(
                &base
                    .join("kpopper/ingestion")
                    .join(sha256(record.to_string_lossy().as_bytes())),
            )?
        }
    };
    let mut inventory = Inventory::default();
    crate::require(
        load(&mut inventory, &state.join("record.json"))?
            == json!({"record":record.to_string_lossy()}),
        "ingestion_state_owner_mismatch",
    )?;
    let event = load(
        &mut inventory,
        &state.join("events").join(format!("{event_id}.json")),
    )?;
    crate::require(
        event.is_object() && event["event_id"] == event_id,
        "ingestion_event_unavailable",
    )?;
    let source = event["source_file"]
        .as_str()
        .map(PathBuf::from)
        .ok_or_else(|| crate::Error("ingestion_event_evidence_mismatch".into()))?;
    crate::require(
        source.is_absolute()
            && source.parent() == Some(state.join("sources").as_path())
            && source.file_name().and_then(|v| v.to_str()) == Some(&format!("{event_id}.txt")),
        "ingestion_event_evidence_mismatch",
    )?;
    let envelope = inventory.read(&state.join("envelopes").join(format!("{event_id}.json")))?;
    let source_bytes = inventory.read(&source)?;
    crate::require(
        event["source_sha256"] == sha256(&source_bytes)
            && event["envelope_sha256"] == sha256(&envelope)
            && serde_json::from_slice::<Value>(&envelope).is_ok(),
        "ingestion_event_evidence_mismatch",
    )?;
    let receipt = load(
        &mut inventory,
        &state.join("receipts").join(format!("{event_id}.json")),
    )?;
    crate::require(
        receipt.is_object()
            && receipt["state"] == "applied"
            && receipt["event_id"] == event_id
            && receipt["source"] == format!("s.ingest_{event_id}")
            && ["source_file", "source_sha256", "envelope_sha256"]
                .iter()
                .all(|key| receipt[*key] == event[*key]),
        "ingestion_event_not_applied",
    )?;
    inventory.verify()?;
    Ok(receipt)
}
fn valid(
    id: &str,
    body: &TypedValue,
    record: &Path,
    preparing: Option<&PreparingContext>,
    inventory: &mut Inventory,
) -> Result<bool> {
    let Some(eid) = id.strip_prefix("s.ingest_") else {
        return Ok(false);
    };
    if eid.is_empty()
        || eid.len() > 128
        || !eid
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Ok(false);
    }
    let body = map(body)?;
    let Some(TypedValue::Text(file)) = body.get("file") else {
        return Ok(false);
    };
    let source = PathBuf::from(file);
    let Some(parent) = source.parent() else {
        return Ok(false);
    };
    if !source.is_absolute()
        || parent.file_name().is_none_or(|n| n != "sources")
        || source.file_name().and_then(|n| n.to_str()) != Some(&format!("{eid}.txt"))
    {
        return Ok(false);
    }
    let Some(root) = parent.parent() else {
        return Ok(false);
    };
    let mut owner = resolved(record)?;
    let mut preparing_root = None;
    if let Some(preparing) = preparing {
        let state = resolved(&preparing.state_dir)?;
        let shadow = resolved(&preparing.shadow)?;
        if owner != shadow
            || shadow.parent().and_then(Path::parent) != Some(state.join("drafts").as_path())
            || shadow
                .parent()
                .and_then(Path::file_name)
                .and_then(|p| p.to_str())
                .is_none_or(|p| !p.starts_with(&format!("{}-", preparing.event_id)))
        {
            return Ok(false);
        }
        owner = resolved(&preparing.record)?;
        preparing_root = Some(state);
    }
    let Some(owner) = owner.to_str() else {
        return Ok(false);
    };
    if load(inventory, &root.join("record.json"))? != json!({"record":owner}) {
        return Ok(false);
    }
    let event = load(inventory, &root.join("events").join(format!("{eid}.json")))?;
    if !event.is_object() || event["event_id"] != eid || event["source_file"] != file.as_str() {
        return Ok(false);
    }
    let envelope_path = root.join("envelopes").join(format!("{eid}.json"));
    if !inventory.file(&envelope_path)? || !inventory.file(&source)? {
        return Ok(false);
    }
    let envelope = inventory.read(&envelope_path)?;
    let source_bytes = inventory.read(&source)?;
    if event["source_sha256"] != sha256(&source_bytes)
        || event["envelope_sha256"] != sha256(&envelope)
        || serde_json::from_slice::<Value>(&envelope).is_err()
    {
        return Ok(false);
    }
    let receipt = load(
        inventory,
        &root.join("receipts").join(format!("{eid}.json")),
    )?;
    if receipt.is_object()
        && receipt["state"] == "applied"
        && receipt["event_id"] == eid
        && receipt["source"] == id
        && ["source_file", "source_sha256", "envelope_sha256"]
            .iter()
            .all(|key| receipt[*key] == event[*key])
    {
        return Ok(true);
    }
    Ok(preparing.is_some_and(|p| p.event_id == eid)
        && preparing_root.as_ref() == Some(&resolved(root)?)
        && event["state"] == "processing")
}
/// Inspect effective base bodies in source order. Hypotheses cannot borrow the
/// base record's ingestion ownership marker. Evidence is retained for revalidation.
pub fn capture(
    source: &CapturedSource,
    preparing: Option<&PreparingContext>,
) -> Result<RecordingSources> {
    let mut result = RecordingSources::default();
    let SourceValue::Map(collections) = source.source() else {
        return Ok(result);
    };
    let mut effective = BTreeMap::new();
    for (section, members) in collections {
        if matches!(section.as_str(), "meta" | "schema" | "record" | "also") {
            continue;
        }
        let SourceValue::Map(members) = members else {
            continue;
        };
        for (id, body) in members {
            effective.insert(
                id,
                (
                    body.typed(),
                    source.origins().get(section).and_then(|m| m.get(id)),
                ),
            );
        }
    }
    for (id, (body, origin)) in effective {
        let Some(origin) = origin else { continue };
        let Ok(fields) = map(&body) else { continue };
        if !matches!(fields.get("recorded_for"), Some(TypedValue::Text(s)) if !s.trim().is_empty())
            || ["v", "quoted", "rule", "verdict"]
                .iter()
                .any(|key| fields.contains_key(*key))
        {
            continue;
        }
        if let Ok(true) = valid(id, &body, origin, preparing, &mut result.inventory) {
            result.ids.insert(id.clone());
        }
    }
    if result.verify().is_err() {
        Ok(RecordingSources::default())
    } else {
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source_capture::{ReadMode, capture_source};
    use std::fs;
    fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let state = root.join("ingestion");
        for name in ["sources", "events", "envelopes", "receipts"] {
            fs::create_dir_all(state.join(name)).unwrap();
        }
        let record = root.join("GROUNDING.yaml");
        let source = state.join("sources/test.txt");
        let envelope = b"{\"source_quote\":\"observed\"}";
        fs::write(&source, "observed").unwrap();
        fs::write(state.join("envelopes/test.json"), envelope).unwrap();
        fs::write(
            state.join("record.json"),
            json!({"record":record}).to_string(),
        )
        .unwrap();
        let event = json!({"event_id":"test", "source_file":source, "source_sha256":sha256(b"observed"), "envelope_sha256":sha256(envelope), "state":"processing"});
        fs::write(state.join("events/test.json"), event.to_string()).unwrap();
        let mut receipt = event;
        receipt["state"] = json!("applied");
        receipt["source"] = json!("s.ingest_test");
        fs::write(state.join("receipts/test.json"), receipt.to_string()).unwrap();
        fs::write(&record, format!("known:\n  s.ingest_test:\n    file: '{}'\n    recorded_for: keep the observation\n  p.x: {{v: 1, from: s.ingest_test}}\n", source.display())).unwrap();
        (temp, record, state)
    }
    fn source(record: &Path) -> CapturedSource {
        capture_source(
            &[record.to_owned()],
            record.parent().unwrap(),
            ReadMode::Frozen,
            None,
        )
        .unwrap()
    }
    #[test]
    fn applied_receipt_requires_owner_hashes_and_effective_source_body() {
        let (_temp, record, state) = setup();
        assert!(
            capture(&source(&record), None)
                .unwrap()
                .ids()
                .contains("s.ingest_test")
        );
        fs::write(state.join("sources/test.txt"), "changed").unwrap();
        assert!(capture(&source(&record), None).unwrap().ids().is_empty());
        fs::write(state.join("sources/test.txt"), "observed").unwrap();
        fs::write(
            state.join("record.json"),
            json!({"record":"another record"}).to_string(),
        )
        .unwrap();
        assert!(capture(&source(&record), None).unwrap().ids().is_empty());
    }
    #[test]
    fn receipt_evidence_is_revalidated() {
        let (_temp, record, state) = setup();
        let proof = capture(&source(&record), None).unwrap();
        fs::write(state.join("receipts/test.json"), "{}").unwrap();
        assert!(proof.verify().is_err());
        assert!(capture(&source(&record), None).unwrap().ids().is_empty());
    }

    #[test]
    fn applied_event_api_rejects_changed_wrong_owner_and_queued_state() {
        let (_temp, record, state) = setup();
        let receipt = applied_event_binding("test", &record, Some(&state)).unwrap();
        assert_eq!(receipt["state"], "applied");
        fs::write(state.join("sources/test.txt"), "changed").unwrap();
        assert_eq!(
            applied_event_binding("test", &record, Some(&state))
                .unwrap_err()
                .0,
            "ingestion_event_evidence_mismatch"
        );
        fs::write(state.join("sources/test.txt"), "observed").unwrap();
        fs::write(
            state.join("record.json"),
            json!({"record":"wrong"}).to_string(),
        )
        .unwrap();
        assert_eq!(
            applied_event_binding("test", &record, Some(&state))
                .unwrap_err()
                .0,
            "ingestion_state_owner_mismatch"
        );
        fs::write(
            state.join("record.json"),
            json!({"record":record}).to_string(),
        )
        .unwrap();
        fs::remove_file(state.join("receipts/test.json")).unwrap();
        assert_eq!(
            applied_event_binding("test", &record, Some(&state))
                .unwrap_err()
                .0,
            "ingestion_event_not_applied"
        );
    }

    #[test]
    fn session_gate_only_exempts_a_verified_capture_source() {
        use crate::session_gate::{self, GateOptions};
        let (_temp, record, state) = setup();
        let body = fs::read_to_string(&record).unwrap().replacen(
            "known:\n",
            "known:\n  p.before: {v: 0}\n",
            1,
        );
        fs::write(&record, "known: {p.before: {v: 0}}\n").unwrap();
        let mark = state.join("mark.json");
        let paths = vec![record.clone()];
        let options = GateOptions {
            state_path: &mark,
            paths: &paths,
            workspace: record.parent().unwrap(),
            read_mode: ReadMode::Frozen,
            runtime: None,
            turns: 0,
            host: None,
            nudged_at: None,
            session_id: None,
            private_tmp: None,
        };
        session_gate::mark(&options).unwrap();
        fs::write(&record, body).unwrap();
        assert_eq!(session_gate::gate(&options).unwrap().code, 0);
        fs::write(state.join("receipts/test.json"), "{}").unwrap();
        let result = session_gate::gate(&options).unwrap();
        assert_eq!(result.code, 2);
        assert!(result.text.contains("recorded no intent"));
    }
    #[test]
    fn staged_context_is_bound_to_exact_owner_shadow_event_and_processing_state() {
        let (_temp, record, state) = setup();
        fs::remove_file(state.join("receipts/test.json")).unwrap();
        let shadow = state.join("drafts/test-123/GROUNDING.yaml");
        fs::create_dir_all(shadow.parent().unwrap()).unwrap();
        fs::copy(&record, &shadow).unwrap();
        let source = source(&shadow);
        assert!(capture(&source, None).unwrap().ids().is_empty());
        let mut preparing = PreparingContext {
            record,
            state_dir: state,
            event_id: "test".into(),
            shadow,
        };
        assert!(
            capture(&source, Some(&preparing))
                .unwrap()
                .ids()
                .contains("s.ingest_test")
        );
        preparing.event_id = "different".into();
        assert!(capture(&source, Some(&preparing)).unwrap().ids().is_empty());
    }
}
