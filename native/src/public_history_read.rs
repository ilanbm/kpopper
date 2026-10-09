//! Explicit, transient display of retained physical history.
use crate::{Result, history_contract::*};
use serde_json::{Value as J, json};
use std::collections::{BTreeMap, BTreeSet};

fn explanation_for_action(
    action: &crate::value::TypedValue,
    subject: &str,
) -> Result<Option<crate::value::TypedValue>> {
    let action = map(action)?;
    if string_is(&action["kind"], "batch") {
        let mut combined = Map::new();
        for child in crate::history_view::list(&action["actions"])? {
            if let Some(found) = explanation_for_action(child, subject)? {
                combined.extend(map(&found)?.clone());
            }
        }
        return Ok((!combined.is_empty()).then_some(crate::value::TypedValue::Map(combined)));
    }
    if action.get("id").and_then(|v| text(v).ok()) != Some(subject) {
        return Ok(None);
    }
    let mut selected = Map::new();
    for key in ["drops", "because", "why", "reason"] {
        if let Some(value) = action.get(key) {
            selected.insert(key.into(), value.clone());
        }
    }
    Ok((!selected.is_empty()).then_some(crate::value::TypedValue::Map(selected)))
}

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

fn legacy_event_parents(
    capture: &crate::history_capture::Capture,
) -> Result<BTreeMap<String, Vec<String>>> {
    let mut parents = BTreeMap::<String, Vec<String>>::new();
    for (op, raw) in &capture.commits {
        let manifest = crate::history_yaml::decode_document(raw)?;
        let parent_ids = map(&map(&manifest)?["parents"])?.keys().cloned().collect();
        parents.insert(op.clone(), parent_ids);
    }
    Ok(parents)
}

fn legacy_explanations(
    capture: &crate::history_capture::Capture,
    prefixes: &[String],
) -> Result<BTreeMap<String, BTreeMap<String, crate::value::TypedValue>>> {
    let mut out = BTreeMap::new();
    for (op, raw) in &capture.commits {
        let commit = crate::history_yaml::decode_document(raw)?;
        let receipt = map(&map(&commit)?["receipt"])?;
        let Some(authoring) = receipt
            .get("before")
            .and_then(|v| map(v).ok())
            .and_then(|v| v.get("authoring"))
            .and_then(|v| map(v).ok())
        else {
            continue;
        };
        let Some(action) = authoring.get("action") else {
            continue;
        };
        let mut per_subject = BTreeMap::new();
        collect_action_explanations(action, prefixes, &mut per_subject)?;
        if !per_subject.is_empty() {
            out.insert(op.clone(), per_subject);
        }
    }
    Ok(out)
}

fn collect_action_explanations(
    action: &crate::value::TypedValue,
    prefixes: &[String],
    output: &mut BTreeMap<String, crate::value::TypedValue>,
) -> Result<()> {
    let action = map(action)?;
    if string_is(&action["kind"], "batch") {
        for child in crate::history_view::list(&action["actions"])? {
            collect_action_explanations(child, prefixes, output)?;
        }
        return Ok(());
    }
    let Some(subject) = action.get("id").and_then(|v| text(v).ok()) else {
        return Ok(());
    };
    if !prefixes
        .iter()
        .any(|p| subject == p || subject.starts_with(&format!("{p}.")))
    {
        return Ok(());
    }
    if let Some(explanation) =
        explanation_for_action(&crate::value::TypedValue::Map(action.clone()), subject)?
    {
        output.insert(subject.to_owned(), explanation);
    }
    Ok(())
}

fn legacy_rows(
    capture: &crate::history_capture::Capture,
    prefixes: &[String],
    parents: &BTreeMap<String, Vec<String>>,
    explanations: &BTreeMap<String, BTreeMap<String, crate::value::TypedValue>>,
) -> Result<Vec<J>> {
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
        if let Some(explanation) = explanations
            .get(&op)
            .and_then(|subjects| subjects.get(subject))
        {
            let mut fields = map(explanation)?.clone();
            fields.retain(|key, _| ["drops", "because", "why", "reason"].contains(&key.as_str()));
            row.as_object_mut()
                .ok_or_else(|| error("invalid_schema"))?
                .insert(
                    "transition_explanation".into(),
                    crate::public_core_readers::json_value(&crate::value::TypedValue::Map(fields))?,
                );
        }
        let fields = row.as_object_mut().ok_or_else(|| error("invalid_schema"))?;
        fields.insert("operation_id".into(), json!(op));
        fields.insert("storage_event_id".into(), json!(op));
        fields.insert("parents".into(), json!(parent_ids));
        rows.push(row);
    }
    mark_current_heads(&mut rows, &capture.state)?;
    Ok(rows)
}

fn mark_current_heads(rows: &mut [J], state: &crate::value::TypedValue) -> Result<()> {
    let subjects = map(field(map(state)?, "subjects")?)?;
    for row in rows {
        let Some(subject) = row.get("subject").and_then(J::as_str) else {
            continue;
        };
        let Some(id) = row.get("id").and_then(J::as_str) else {
            continue;
        };
        let Some(head_state) = subjects.get(subject).and_then(|state| map(state).ok()) else {
            continue;
        };
        let single_head = match head_state.get("head") {
            Some(crate::value::TypedValue::Null) | None => None,
            Some(head) => Some(text(head)?),
        };
        let mut disputed_heads = false;
        if let Some(heads) = head_state.get("heads") {
            for head in crate::history_view::list(heads)? {
                disputed_heads |= text(head)? == id;
            }
        }
        if single_head == Some(id) || disputed_heads {
            row.as_object_mut()
                .ok_or_else(|| error("invalid_schema"))?
                .insert("current_head".into(), J::Bool(true));
        }
    }
    Ok(())
}

fn node_rows(
    capture: &crate::history_node_capture::Capture,
    prefixes: &[String],
) -> Result<Vec<J>> {
    let mut rows = capture
        .historical_versions(prefixes)?
        .iter()
        .map(crate::public_core_readers::json_value)
        .collect::<Result<Vec<_>>>()?;
    for row in &mut rows {
        let operation = row.get("operation_id").and_then(J::as_str).unwrap_or("");
        let subject = row.get("subject").and_then(J::as_str).unwrap_or("");
        let Some(context) = capture
            .snapshot
            .transactions
            .get(operation)
            .and_then(|tx| tx.context.as_ref())
        else {
            continue;
        };
        if !crate::history_node_transaction::is_context(context) {
            continue;
        }
        let action = crate::history_node_transaction::validate(context)?;
        if let Some(explanation) = explanation_for_action(&action["action"], subject)? {
            row.as_object_mut()
                .ok_or_else(|| error("invalid_schema"))?
                .insert(
                    "transition_explanation".into(),
                    crate::public_core_readers::json_value(&explanation)?,
                );
        }
    }
    mark_current_heads(&mut rows, capture.state())?;
    Ok(rows)
}

fn legacy_archive_rows(
    archive: Option<&crate::history_node_capture::ReplacedArchive>,
    prefixes: &[String],
) -> Result<Vec<J>> {
    let Some(archive) = archive else {
        return Ok(vec![]);
    };
    let document = crate::history_yaml::decode_source_value(&archive.bytes)?.typed();
    // Ordinary replacement archives may be empty or contain annotations rather
    // than version lists. Preserve that reader's interpretation after copying.
    let Ok(document) = map(&document) else {
        return Ok(vec![]);
    };
    let mut rows = Vec::new();
    for (subject, versions) in document {
        if !prefixes
            .iter()
            .any(|p| subject == p || subject.starts_with(&format!("{p}.")))
        {
            continue;
        }
        let Ok(versions) = crate::history_view::list(versions) else {
            continue;
        };
        for entry in versions {
            rows.push(json!({
                "subject": subject,
                "source": archive.source,
                "archive_member": archive.path,
                "member_sha256": archive.member_sha256,
                "entry": crate::public_core_readers::json_value(entry)?
            }));
        }
    }
    Ok(rows)
}

fn identity(row: &J) -> String {
    row.get("id")
        .and_then(J::as_str)
        .or_else(|| row.get("storage_event_id").and_then(J::as_str))
        .unwrap_or("")
        .to_owned()
}
fn event(row: &J) -> String {
    row.get("storage_event_id")
        .and_then(J::as_str)
        .or_else(|| row.get("operation_id").and_then(J::as_str))
        .unwrap_or_else(|| row.get("id").and_then(J::as_str).unwrap_or(""))
        .to_owned()
}

/// Deterministic Kahn ordering over selected captured storage events. Rows produced by
/// one event stay adjacent and stable by semantic identity; time and hash order never pick
/// a version. Runtime is O(V log V + E log V) for selected rows and event edges.
fn causal_order(rows: Vec<J>, mut parents: BTreeMap<String, Vec<String>>) -> Result<Vec<J>> {
    let mut by_event = BTreeMap::<String, Vec<J>>::new();
    for row in rows {
        let id = event(&row);
        parents.entry(id.clone()).or_default();
        by_event.entry(id).or_default().push(row);
    }
    for rows in by_event.values_mut() {
        rows.sort_by_key(identity);
    }
    let known = parents.keys().cloned().collect::<BTreeSet<_>>();
    crate::require(
        parents.values().flatten().all(|id| known.contains(id)),
        "incomplete_history_event_ancestry",
    )?;
    let mut children = BTreeMap::<String, BTreeSet<String>>::new();
    let mut indegree = parents
        .iter()
        .map(|(id, p)| (id.clone(), p.iter().collect::<BTreeSet<_>>().len()))
        .collect::<BTreeMap<_, _>>();
    for (child, event_parents) in &parents {
        for parent in event_parents.iter().collect::<BTreeSet<_>>() {
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
    let mut visited = 0usize;
    while let Some(id) = ready.pop_first() {
        visited += 1;
        ordered.extend(by_event.remove(&id).unwrap_or_default());
        for child in children.get(&id).into_iter().flatten() {
            let degree = indegree.get_mut(child).unwrap();
            *degree -= 1;
            if *degree == 0 {
                ready.insert(child.clone());
            }
        }
    }
    crate::require(visited == parents.len(), "history_causal_cycle")?;
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
        .get("id")
        .and_then(J::as_str)
        .unwrap_or("unknown version");
    let event = row
        .get("storage_event_id")
        .and_then(J::as_str)
        .map(|id| format!(" (storage event {id})"))
        .unwrap_or_default();
    let current = row
        .get("current_head")
        .and_then(J::as_bool)
        .filter(|current| *current)
        .map(|_| " [current head]")
        .unwrap_or("");
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
    let drops = row
        .get("transition_explanation")
        .and_then(|value| value.get("drops"))
        .and_then(J::as_object)
        .map(|drops| {
            drops
                .iter()
                .map(|(id, why)| format!("; dropped {id}: {why}"))
                .collect::<String>()
        })
        .unwrap_or_default();
    Ok(format!(
        "{subject} · {kind} · version {id}{event}{current}{by}{on}{why}{drops} · {body}"
    ))
}

fn clip_history(rows: &[J], archive: &[J], requested: usize) -> (Vec<J>, usize, Vec<J>, usize) {
    let mut keep = vec![false; rows.len()];
    let mut keep_archive = vec![false; archive.len()];
    let mut used = 0usize;
    // Current heads are required context even when later review acts fill the causal tail.
    for index in (0..rows.len())
        .rev()
        .filter(|i| rows[*i]["current_head"] == true)
    {
        let size = serde_json::to_vec(&rows[index]).map_or(usize::MAX, |row| row.len() + 1);
        if used.saturating_add(size) <= requested {
            keep[index] = true;
            used += size;
        }
    }
    for index in (0..rows.len()).rev() {
        if keep[index] {
            continue;
        }
        let size = serde_json::to_vec(&rows[index]).map_or(usize::MAX, |row| row.len() + 1);
        if used.saturating_add(size) <= requested {
            keep[index] = true;
            used += size;
        }
    }
    // Pre-import annotations must not crowd out the actual recorded change that
    // prompted review. They share the remaining budget, without causal ordering.
    for index in (0..archive.len()).rev() {
        let size = serde_json::to_vec(&archive[index]).map_or(usize::MAX, |row| row.len() + 1);
        if used.saturating_add(size) <= requested {
            keep_archive[index] = true;
            used += size;
        }
    }
    let kept = rows
        .iter()
        .zip(keep)
        .filter_map(|(row, selected)| selected.then(|| row.clone()))
        .collect::<Vec<_>>();
    let omitted = rows.len() - kept.len();
    let kept_archive = archive
        .iter()
        .zip(keep_archive)
        .filter_map(|(row, selected)| selected.then(|| row.clone()))
        .collect::<Vec<_>>();
    let omitted_archive = archive.len() - kept_archive.len();
    (kept, omitted, kept_archive, omitted_archive)
}

#[cfg(test)]
fn clip_recent_and_heads(rows: &[J], requested: usize) -> (Vec<J>, usize) {
    let (kept, omitted, _, _) = clip_history(rows, &[], requested);
    (kept, omitted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_archive_shapes_follow_ordinary_history_tolerance() {
        for raw in ["", "# no replacements yet\n", "{}", "a note", "d.done: a note\n", "d.done: {verdict: old}\n", "d.done:\n"] {
            let archive = crate::history_node_capture::ReplacedArchive {
                source: "verified",
                path: ".kpopper/replaced.yaml".into(),
                member_sha256: crate::identity::sha256(raw.as_bytes()),
                bytes: raw.as_bytes().to_vec(),
            };
            assert!(legacy_archive_rows(Some(&archive), &["d.done".into()]).unwrap().is_empty(), "{raw:?}");
        }
        let raw = b"d.note: just a note\nd.done:\n- {verdict: old}\n";
        let archive = crate::history_node_capture::ReplacedArchive {
            source: "verified", path: ".kpopper/replaced.yaml".into(), member_sha256: crate::identity::sha256(raw), bytes: raw.to_vec(),
        };
        let rows = legacy_archive_rows(Some(&archive), &["d".into()]).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["entry"]["verdict"], "old");
    }

    #[test]
    fn legacy_archives_cannot_displace_recent_semantic_versions() {
        let rows = vec![
            json!({"id":"predecessor","kind":"judgment","body":{"verdict":"before"}}),
            json!({"id":"current","current_head":true,"kind":"judgment","body":{"verdict":"after"}}),
            json!({"id":"transition","kind":"act","body":{"because":"new evidence"}}),
        ];
        let archive = (0..60).map(|i| json!({"subject":"d.done","entry":{"because":format!("legacy reason {i}")}})).collect::<Vec<_>>();
        let budget = rows.iter().map(|row| serde_json::to_vec(row).unwrap().len()+1).sum();
        let (kept, omitted, _, archive_omitted) = clip_history(&rows, &archive, budget);
        assert_eq!(kept, rows, "pre-import archives hid the change the review note names");
        assert_eq!(omitted, 0);
        assert_eq!(archive_omitted, archive.len());
    }

    #[test]
    fn causal_order_keeps_order_through_rowless_event_bridge() {
        let rows = vec![
            json!({"id":"claim-1","storage_event_id":"event-1"}),
            json!({"id":"claim-3","storage_event_id":"event-3"}),
        ];
        let parents = BTreeMap::from([
            ("event-1".into(), vec![]),
            ("event-2".into(), vec!["event-1".into()]),
            ("event-3".into(), vec!["event-2".into()]),
        ]);
        let ordered = causal_order(rows, parents).unwrap();
        assert_eq!(ordered[0]["id"], "claim-1");
        assert_eq!(ordered[1]["id"], "claim-3");
    }

    #[test]
    fn clipping_keeps_newest_rows_in_causal_order() {
        let rows = (0..20)
            .map(|i| json!({"id":format!("v{i}"), "body":{"value":i}}))
            .collect::<Vec<_>>();
        let budget = serde_json::to_vec(&rows[10]).unwrap().len() * 5;
        let (kept, omitted) = clip_recent_and_heads(&rows, budget);
        assert!(omitted > 0);
        assert_eq!(kept.last().unwrap()["id"], "v19");
        assert!(kept.first().unwrap()["id"].as_str().unwrap() > "v0");
        assert!(
            kept.windows(2)
                .all(|w| w[0]["id"].as_str() < w[1]["id"].as_str())
        );
    }

    #[test]
    fn clipping_preserves_current_head_after_later_review_acts() {
        let mut rows = vec![json!({"id":"head","current_head":true,"kind":"judgment"})];
        rows.extend((0..30).map(|i| json!({"id":format!("review-{i}"),"kind":"act"})));
        let newest_sizes = rows[26..]
            .iter()
            .map(|row| serde_json::to_vec(row).unwrap().len() + 1)
            .sum::<usize>();
        let budget = serde_json::to_vec(&rows[0]).unwrap().len() + 1 + newest_sizes;
        let (kept, omitted) = clip_recent_and_heads(&rows, budget);
        assert!(omitted > 0);
        assert_eq!(kept.first().unwrap()["id"], "head");
        assert_eq!(kept.last().unwrap()["id"], "review-29");
        assert_eq!(
            kept.iter()
                .filter(|row| row["current_head"] == true)
                .count(),
            1
        );
    }

    #[test]
    fn both_contested_current_heads_receive_priority() {
        let mut rows = vec![
            json!({"subject":"d.contested","id":"head-a"}),
            json!({"subject":"d.contested","id":"head-b"}),
            json!({"subject":"d.contested","id":"old"}),
            json!({"subject":"d.retired","id":"retired-old"}),
        ];
        let state = crate::value::TypedValue::from_json(&json!({
            "subjects":{
                "d.contested":{"head":null,"heads":["head-a","head-b"],"acceptance":"contested"},
                "d.retired":{"head":null,"heads":[],"acceptance":"retired"}
            }
        }))
        .unwrap();
        mark_current_heads(&mut rows, &state).unwrap();
        assert_eq!(rows[0]["current_head"], true);
        assert_eq!(rows[1]["current_head"], true);
        assert!(rows[2].get("current_head").is_none());
        assert!(rows[3].get("current_head").is_none());
    }
}

pub(crate) fn render(
    node: Option<&crate::history_node_capture::Capture>,
    legacy: Option<&crate::history_capture::Capture>,
    imported_archive: Option<&crate::history_node_capture::ReplacedArchive>,
    prefixes: &[String],
    chars: Option<i64>,
    as_json: bool,
) -> Result<String> {
    if prefixes.is_empty() {
        return Ok(String::new());
    }
    let (rows, event_parents, archive_rows) = if let Some(capture) = node {
        let archive = capture.archived_replaced_yaml()?;
        (
            node_rows(capture, prefixes)?,
            capture.historical_event_parents(),
            legacy_archive_rows(archive.as_ref(), prefixes)?,
        )
    } else if let Some(capture) = legacy {
        let parents = legacy_event_parents(capture)?;
        let explanations = legacy_explanations(capture, prefixes)?;
        (
            legacy_rows(capture, prefixes, &parents, &explanations)?,
            parents,
            legacy_archive_rows(imported_archive, prefixes)?,
        )
    } else {
        (vec![], BTreeMap::new(), vec![])
    };
    let rows = causal_order(rows, event_parents)?;
    let requested = chars.unwrap_or(12000).max(1) as usize;
    // Keep current heads, archived source evidence and recent causal rows within one budget.
    let (kept, omitted, kept_archive, omitted_archive) =
        clip_history(&rows, &archive_rows, requested);
    if as_json {
        Ok(serde_json::to_string_pretty(
            &json!({"schema_version":1,"historical_section":{"source":"captured_committed_history","version_ordering":"verified_storage_parent_order","legacy_archive_ordering":"archive source sequence only; no causal order inferred","fresh_observation":false,"complete":omitted==0 && omitted_archive==0,"versions":kept,"omitted_versions":omitted,"legacy_archive_evidence":kept_archive,"omitted_legacy_archive_entries":omitted_archive,"expand_with":"increase --chars or --budget"}}),
        )? + "\n")
    } else {
        let mut out = String::from(
            "HISTORICAL SECTION (captured committed source; provenance display, not a fresh observation or computation pin)\n",
        );
        for row in kept {
            out.push_str(&human_row(&row)?);
            out.push('\n');
        }
        if !kept_archive.is_empty() {
            out.push_str("LEGACY ARCHIVE EVIDENCE (verified archived source data; no semantic IDs or event order)\n");
            for row in kept_archive {
                let subject = row
                    .get("subject")
                    .and_then(J::as_str)
                    .unwrap_or("unknown subject");
                let path = row
                    .get("archive_member")
                    .and_then(J::as_str)
                    .unwrap_or("unknown archive member");
                let digest = row
                    .get("member_sha256")
                    .and_then(J::as_str)
                    .unwrap_or("unknown digest");
                let entry = row
                    .get("entry")
                    .map(serde_json::to_string)
                    .transpose()?
                    .unwrap_or_default();
                out.push_str(&format!(
                    "{subject} · archived legacy entry from {path} member sha256 {digest} · {entry}\n"
                ));
            }
        }
        if omitted > 0 {
            out.push_str(&format!("PARTIAL: {omitted} retained versions omitted; increase --chars or --budget to expand.\n"));
        }
        if omitted_archive > 0 {
            out.push_str(&format!("PARTIAL: {omitted_archive} legacy archive entries omitted; increase --chars or --budget to expand.\n"));
        }
        Ok(out)
    }
}
