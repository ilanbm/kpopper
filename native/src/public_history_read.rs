//! Explicit, transient display of retained physical history.
use crate::{Result, history_contract::*};
use serde_json::{Value as J, json};

pub(crate) fn render(
    node: Option<&crate::history_node_capture::Capture>,
    legacy: Option<&crate::history_capture::Capture>,
    prefixes: &[String],
    chars: Option<i64>,
    as_json: bool,
) -> Result<String> {
    let mut rows = if let Some(capture) = node {
        capture
            .historical_versions(prefixes)?
            .iter()
            .map(crate::public_core_readers::json_value)
            .collect::<Result<Vec<_>>>()?
    } else if let Some(capture) = legacy {
        capture
            .objects
            .values()
            .filter(|v| {
                map(v)
                    .ok()
                    .and_then(|m| m.get("subject"))
                    .and_then(|v| text(v).ok())
                    .is_some_and(|s| {
                        prefixes.is_empty()
                            || prefixes
                                .iter()
                                .any(|p| s == p || s.starts_with(&format!("{p}.")))
                    })
            })
            .map(crate::public_core_readers::json_value)
            .collect::<Result<Vec<_>>>()?
    } else {
        vec![]
    };
    let identity = |r: &J| {
        r.get("version_id")
            .and_then(J::as_str)
            .or_else(|| r.get("id").and_then(J::as_str))
            .unwrap_or("")
            .to_owned()
    };
    rows.sort_by_key(&identity);
    let all = rows
        .iter()
        .map(|r| identity(r))
        .collect::<std::collections::BTreeSet<_>>();
    let mut emitted = std::collections::BTreeSet::new();
    let mut causal = Vec::with_capacity(rows.len());
    while !rows.is_empty() {
        let ready = rows
            .iter()
            .position(|r| {
                ["parents", "saw"]
                    .iter()
                    .filter_map(|k| r.get(*k).and_then(J::as_array))
                    .flatten()
                    .filter_map(J::as_str)
                    .all(|id| !all.contains(id) || emitted.contains(id))
            })
            .ok_or_else(|| crate::history_contract::error("history_causal_cycle"))?;
        let row = rows.remove(ready);
        emitted.insert(identity(&row));
        causal.push(row);
    }
    let rows = causal;
    let requested = chars.unwrap_or(12000).max(1) as usize;
    let mut kept = Vec::new();
    let mut used = 0usize;
    for row in rows.iter() {
        let s = serde_json::to_string(row)?;
        if used + s.len() + 1 > requested {
            break;
        }
        used += s.len() + 1;
        kept.push(row.clone());
    }
    let omitted = rows.len() - kept.len();
    if as_json {
        Ok(serde_json::to_string_pretty(
            &json!({"schema_version":1,"historical_section":{"source":"captured_committed_history","complete":omitted==0,"versions":kept,"omitted_versions":omitted,"expand_with":"increase --chars"}}),
        )? + "\n")
    } else {
        let mut out =
            String::from("HISTORICAL SECTION (captured committed source; not a computation pin)\n");
        for row in kept {
            out.push_str(&serde_json::to_string(&row)?);
            out.push('\n');
        }
        if omitted > 0 {
            out.push_str(&format!(
                "PARTIAL: {omitted} retained versions omitted; increase --chars to expand.\n"
            ));
        }
        Ok(out)
    }
}
