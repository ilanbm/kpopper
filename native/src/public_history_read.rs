//! Explicit, transient display of retained physical history.
use crate::{Result, history_contract::*};
use serde_json::{Value as J, json};
use std::collections::{BTreeMap, BTreeSet};

fn compact_object(value: &crate::value::TypedValue) -> Result<J> {
    // `saw` is cumulative ancestry; displaying it at every revision inflates a compact
    // chain quadratically. The capture has already verified it and supplies event edges.
    let fields = map(value)?
        .iter()
        .filter(|(key, _)| key.as_str() != "saw")
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    crate::public_core_readers::json_value(&crate::value::TypedValue::Map(fields))
}

fn legacy_rows(capture: &crate::history_capture::Capture, prefixes: &[String]) -> Result<Vec<J>> {
    let mut parents = BTreeMap::<String, Vec<String>>::new();
    for (op, raw) in &capture.commits {
        let manifest = crate::history_yaml::decode_document(raw)?;
        let parent_ids = map(&map(&manifest)?["parents"])?.keys().cloned().collect();
        parents.insert(op.clone(), parent_ids);
    }
    let mut rows = Vec::new();
    for object in capture.objects.values() {
        let fields = map(object)?;
        let subject = text(field(fields, "subject")?)?;
        if !prefixes
            .iter()
            .any(|p| subject == p || subject.starts_with(&format!("{p}.")))
        {
            continue;
        }
        let op = fields
            .get("op")
            .and_then(|v| text(v).ok())
            .unwrap_or("root")
            .to_owned();
        let parent_ids = parents.get(&op).cloned().unwrap_or_default();
        let mut row = compact_object(object)?;
        let fields = row.as_object_mut().ok_or_else(|| error("invalid_schema"))?;
        fields.insert("operation_id".into(), json!(op));
        fields.insert("parents".into(), json!(parent_ids));
        rows.push(row);
    }
    Ok(rows)
}

fn identity(row: &J) -> String {
    row.get("id")
        .and_then(J::as_str)
        .or_else(|| row.get("version_id").and_then(J::as_str))
        .unwrap_or("")
        .to_owned()
}
fn event(row: &J) -> String {
    row.get("version_id")
        .and_then(J::as_str)
        .or_else(|| row.get("operation_id").and_then(J::as_str))
        .unwrap_or_else(|| row.get("id").and_then(J::as_str).unwrap_or(""))
        .to_owned()
}

/// Deterministic Kahn ordering over selected captured storage events. Rows produced by
/// one event stay adjacent and stable by semantic identity; time and hash order never pick
/// a version. Runtime is O(V log V + E log V) for selected rows and event edges.
fn causal_order(rows: Vec<J>) -> Result<Vec<J>> {
    let mut by_event = BTreeMap::<String, Vec<J>>::new();
    let mut deps = BTreeMap::<String, BTreeSet<String>>::new();
    for row in rows {
        let id = event(&row);
        let parents = row
            .get("parents")
            .and_then(J::as_array)
            .into_iter()
            .flatten()
            .filter_map(J::as_str)
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        deps.entry(id.clone()).or_default().extend(parents);
        by_event.entry(id).or_default().push(row);
    }
    for rows in by_event.values_mut() {
        rows.sort_by_key(identity);
    }
    let known = by_event.keys().cloned().collect::<BTreeSet<_>>();
    for parents in deps.values_mut() {
        parents.retain(|id| known.contains(id));
    }
    let mut children = BTreeMap::<String, BTreeSet<String>>::new();
    let mut indegree = deps
        .iter()
        .map(|(id, p)| (id.clone(), p.len()))
        .collect::<BTreeMap<_, _>>();
    for (child, parents) in &deps {
        for parent in parents {
            children
                .entry(parent.clone())
                .or_default()
                .insert(child.clone());
        }
    }
    let mut ready = indegree
        .iter()
        .filter(|(_, n)| **n == 0)
        .map(|(id, _)| id.clone())
        .collect::<BTreeSet<_>>();
    let mut ordered = Vec::with_capacity(by_event.len());
    while let Some(id) = ready.pop_first() {
        ordered.extend(by_event.remove(&id).unwrap_or_default());
        for child in children.get(&id).into_iter().flatten() {
            let degree = indegree.get_mut(child).unwrap();
            *degree -= 1;
            if *degree == 0 {
                ready.insert(child.clone());
            }
        }
    }
    crate::require(by_event.is_empty(), "history_causal_cycle")?;
    Ok(ordered)
}

fn human_row(row: &J) -> Result<String> {
    let subject = row
        .get("subject")
        .and_then(J::as_str)
        .unwrap_or("unknown subject");
    let kind = row
        .get("kind")
        .and_then(J::as_str)
        .or_else(|| row.get("collection").and_then(J::as_str))
        .unwrap_or("entry");
    let id = row
        .get("version_id")
        .and_then(J::as_str)
        .or_else(|| row.get("id").and_then(J::as_str))
        .unwrap_or("unknown version");
    let by = row
        .get("by")
        .filter(|v| !v.is_null())
        .map(|v| format!(" by {}", v))
        .unwrap_or_default();
    let on = row
        .get("on")
        .map(|v| format!(" at {}", v))
        .unwrap_or_default();
    let body = row
        .get("body")
        .map(serde_json::to_string)
        .transpose()?
        .unwrap_or_else(|| "{}".into());
    let why = ["because", "request", "reason"]
        .iter()
        .filter_map(|k| {
            row.get(*k)
                .or_else(|| row.get("body").and_then(|b| b.get(*k)))
                .map(|v| (*k, v))
        })
        .filter(|(_, v)| !v.is_null())
        .map(|(key, v)| format!("; {key}={v}"))
        .collect::<String>();
    Ok(format!(
        "{subject} · {kind} · version {id}{by}{on}{why} · {body}"
    ))
}

pub(crate) fn render(
    node: Option<&crate::history_node_capture::Capture>,
    legacy: Option<&crate::history_capture::Capture>,
    prefixes: &[String],
    chars: Option<i64>,
    as_json: bool,
) -> Result<String> {
    if prefixes.is_empty() {
        return Ok(String::new());
    }
    let rows = if let Some(capture) = node {
        capture
            .historical_versions(prefixes)?
            .iter()
            .map(crate::public_core_readers::json_value)
            .collect::<Result<Vec<_>>>()?
    } else if let Some(capture) = legacy {
        legacy_rows(capture, prefixes)?
    } else {
        vec![]
    };
    let rows = causal_order(rows)?;
    let requested = chars.unwrap_or(12000).max(1) as usize;
    let mut kept = Vec::new();
    let mut used = 0usize;
    for row in &rows {
        let size = serde_json::to_string(row)?.len() + 1;
        if used.saturating_add(size) > requested {
            break;
        }
        used += size;
        kept.push(row.clone());
    }
    let omitted = rows.len() - kept.len();
    if as_json {
        Ok(serde_json::to_string_pretty(
            &json!({"schema_version":1,"historical_section":{"source":"captured_committed_history","ordering":"verified_storage_parent_order","fresh_observation":false,"complete":omitted==0,"versions":kept,"omitted_versions":omitted,"expand_with":"increase --chars or --budget"}}),
        )? + "\n")
    } else {
        let mut out = String::from(
            "HISTORICAL SECTION (captured committed source; provenance display, not a fresh observation or computation pin)\n",
        );
        for row in kept {
            out.push_str(&human_row(&row)?);
            out.push('\n');
        }
        if omitted > 0 {
            out.push_str(&format!("PARTIAL: {omitted} retained versions omitted; increase --chars or --budget to expand.\n"));
        }
        Ok(out)
    }
}
