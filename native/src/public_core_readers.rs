//! Public core readers consume one shared, captured finding set.
use crate::{Result, reasoning_context::CapturedAssessment};
use serde_json::{Value as J, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
#[derive(Debug, PartialEq, Eq)]
pub struct Output {
    pub text: String,
    pub code: i32,
}
pub(crate) fn json_value(value: &crate::value::TypedValue) -> Result<J> {
    use crate::value::TypedValue as V;
    match value {
        V::Date(d) => Ok(json!(d.as_str())),
        V::DateTime(d) => Ok(json!(d.as_str().replacen('T', " ", 1))),
        V::List(a) => Ok(J::Array(a.iter().map(json_value).collect::<Result<_>>()?)),
        V::Map(m) => Ok(J::Object(
            m.iter()
                .map(|(k, v)| Ok((k.clone(), json_value(v)?)))
                .collect::<Result<_>>()?,
        )),
        _ => value.to_json(),
    }
}
fn stamp(context: &CapturedAssessment) -> String {
    format!(
        "core/v1 snapshot {}; findings {}",
        context.snapshot_id(),
        context.findings_revision()
    )
}
pub fn pull(context: &CapturedAssessment, seeds: &[String]) -> Result<Output> {
    let report = json_value(context.assessment())?;
    let available = report["nodes"]
        .as_object()
        .unwrap()
        .keys()
        .chain(report["history_subjects"].as_object().unwrap().keys())
        .collect::<BTreeSet<_>>();
    let selected = available
        .into_iter()
        .filter(|id| {
            seeds
                .iter()
                .any(|s| *id == s || id.starts_with(&format!("{s}.")))
        })
        .cloned()
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Ok(Output {
            text: format!("nothing matching {}\n", seeds.join(", ")),
            code: 1,
        });
    }
    let nodes = selected
        .iter()
        .filter_map(|id| report["nodes"].get(id).map(|v| (id.clone(), v.clone())))
        .collect::<BTreeMap<_, _>>();
    let subjects = selected
        .iter()
        .filter_map(|id| {
            report["history_subjects"]
                .get(id)
                .map(|v| (id.clone(), v.clone()))
        })
        .collect::<BTreeMap<_, _>>();
    let payload = json!({"schema_version":1,"profile":"core/v1-consumer/v1","snapshot_id":context.snapshot_id(),"findings_revision":context.findings_revision(),"selection":selected,"nodes":nodes,"history_subjects":subjects});
    Ok(Output {
        text: serde_json::to_string(&payload)? + "\n",
        code: 0,
    })
}
pub fn affects(context: &CapturedAssessment, changed: &[String]) -> Result<Output> {
    let view = json_value(context.view())?;
    let mut outgoing = BTreeMap::<String, Vec<(String, String)>>::new();
    for edge in view["impacts"].as_array().unwrap() {
        outgoing
            .entry(edge["from"].as_str().unwrap().into())
            .or_default()
            .push((
                edge["to"].as_str().unwrap().into(),
                edge["classification"].as_str().unwrap().into(),
            ));
    }
    for edges in outgoing.values_mut() {
        edges.sort_by(|a, b| a.0.cmp(&b.0));
    }
    let mut queue = changed
        .iter()
        .map(|id| (id.clone(), true))
        .collect::<VecDeque<_>>();
    let mut reached = BTreeMap::<String, bool>::new();
    while let Some((source, executed)) = queue.pop_front() {
        for (target, class) in outgoing.get(&source).into_iter().flatten() {
            let candidate = executed && class == "executed";
            if reached
                .get(target)
                .is_none_or(|current| !*current && candidate)
            {
                reached.insert(target.clone(), candidate);
                queue.push_back((target.clone(), candidate));
            }
        }
    }
    let mut lines = if reached.is_empty() {
        vec![format!("nothing reached from {}", changed.join(", "))]
    } else {
        reached
            .into_iter()
            .map(|(target, executed)| {
                format!(
                    "{} {target}",
                    if executed { "EXECUTED" } else { "POTENTIAL" }
                )
            })
            .collect()
    };
    lines.push(stamp(context));
    Ok(Output {
        text: lines.join("\n") + "\n",
        code: 0,
    })
}
fn codes(items: &J, field: &str, default: &str) -> String {
    items
        .as_array()
        .into_iter()
        .flatten()
        .map(|v| v[field].as_str().unwrap_or(default))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join(", ")
}
pub fn check_findings(context: &CapturedAssessment, page: Result<J>) -> Result<J> {
    findings(context, Some(page))
}
pub fn record_findings(context: &CapturedAssessment) -> Result<J> {
    findings(context, None)
}
fn findings(context: &CapturedAssessment, page: Option<Result<J>>) -> Result<J> {
    let report = json_value(context.assessment())?;
    let mut failures = vec![];
    let mut notes = vec![];
    for (id, node) in report["nodes"].as_object().unwrap() {
        if node["coverage"]["complete"] != true {
            failures.push(format!(
                "{id}: incomplete history coverage ({})",
                codes(&node["coverage"]["findings"], "code", "incomplete")
            ));
        }
        if node["state"]["integrity"]["issues"]
            .as_array()
            .is_some_and(|v| !v.is_empty())
        {
            failures.push(format!(
                "{id}: integrity ({})",
                codes(&node["state"]["integrity"]["issues"], "code", "error")
            ));
        }
        if node["computation"].is_object()
            && !matches!(
                node["computation"]["status"].as_str(),
                Some("ok" | "unknown")
            )
        {
            failures.push(format!(
                "{id}: computation {}",
                node["computation"]["status"].as_str().unwrap_or("error")
            ));
        }
        let falsifier = &node["state"]["falsifier"];
        match falsifier["status"].as_str() {
            Some("holds") => failures.push(format!("{id}: falsifier holds")),
            Some("error") => failures.push(format!("{id}: falsifier unavailable")),
            Some("unknown") => {
                if falsifier["computation"].is_object() {
                    failures.push(format!("{id}: falsifier unknown"));
                } else {
                    notes.push(format!("{id}: falsifier unknown (declared prose)"));
                }
            }
            _ => {}
        }
        match node["temporal"]["status"].as_str() {
            Some("counterexample") if falsifier["status"] != "holds" => {
                failures.push(format!("{id}: historical counterexample"))
            }
            Some("unknown") => failures.push(format!("{id}: historical evidence unknown")),
            _ => {}
        }
        if node["support"]["status"] == "reserved" {
            notes.push(format!(
                "{id}: support reserved ({})",
                codes(&node["support"]["reservations"], "state", "")
            ));
        }
    }
    let falsified = report["nodes"]
        .as_object()
        .unwrap()
        .iter()
        .filter(|(_, node)| node["state"]["falsifier"]["status"] == "holds")
        .map(|(id, _)| id.clone())
        .collect::<BTreeSet<_>>();
    if let Ok(document) = crate::value::TypedValue::from_json(
        &json_value(&context.snapshot().to_data())?["document"],
    ) {
        for (id, flag) in crate::public_amend::document_flags(&document, &falsified) {
            notes.push(format!(
                "{id}: {}",
                flag.text(&|a, b| crate::public_ordinary_readers::apart(a, b, 40))
            ));
        }
    }
    match page {
        Some(Ok(page)) => {
            for (key, label, separator) in [
                ("unresolved_selectors", "page selectors unresolved", ", "),
                ("renderer_misfits", "page renderer mismatch", "; "),
                ("stale_shapes", "page shape moved", "; "),
            ] {
                if let Some(items) = page["coverage"][key].as_array()
                    && !items.is_empty()
                {
                    failures.push(format!(
                        "{label} ({})",
                        items
                            .iter()
                            .filter_map(J::as_str)
                            .collect::<Vec<_>>()
                            .join(separator)
                    ));
                }
            }
        }
        Some(Err(error)) => failures.push(format!("page projection unavailable ({error})")),
        None => {}
    }
    Ok(json!({"failures":failures,"notes":notes}))
}
pub fn check(context: &CapturedAssessment, page: Result<J>) -> Result<Output> {
    let findings = check_findings(context, page)?;
    check_output(context, findings, false)
}
pub const LAYOUT_NOTICE: &str =
    "NOTE page layout not checked; use kpop experimental hub --verify\n";
pub fn record_check(context: &CapturedAssessment, has_brief: bool) -> Result<Output> {
    check_output(context, record_findings(context)?, has_brief)
}
fn check_output(context: &CapturedAssessment, findings: J, has_brief: bool) -> Result<Output> {
    let failures = findings["failures"].as_array().unwrap();
    let mut lines = findings["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| format!("NOTE {}", n.as_str().unwrap()))
        .collect::<Vec<_>>();
    lines.extend(
        failures
            .iter()
            .map(|f| format!("FAIL {}", f.as_str().unwrap())),
    );
    lines.push(stamp(context));
    lines.push(format!(
        "{} nodes, {} problems",
        crate::history_contract::map(
            &crate::history_contract::map(context.assessment())?["nodes"]
        )?
        .len(),
        failures.len()
    ));
    Ok(Output {
        text: if has_brief {
            LAYOUT_NOTICE.to_string()
        } else {
            String::new()
        } + &lines.join("\n")
            + "\n",
        code: i32::from(!failures.is_empty()),
    })
}

pub fn opening(
    context: &CapturedAssessment,
    mut data: J,
    fallback_title: &str,
) -> Result<(J, String)> {
    let report = json_value(context.assessment())?;
    let snapshot = json_value(&context.snapshot().to_data())?;
    let meta = &snapshot["document"]["meta"];
    let title = ["name", "scope"]
        .iter()
        .filter_map(|key| crate::value::TypedValue::from_json(&meta[*key]).ok())
        .find(crate::history_view::truth)
        .map(|v| crate::reasoning_authoring::py(&v))
        .unwrap_or_else(|| fallback_title.into());
    let mut attention = vec![];
    for (id, node) in report["nodes"].as_object().unwrap() {
        let mut reasons = node["attention"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|a| a["reasons"].as_array().into_iter().flatten())
            .filter_map(|r| r["code"].as_str().map(str::to_owned))
            .collect::<BTreeSet<_>>();
        if node["support"]["status"] == "reserved" {
            reasons.extend(
                node["support"]["reservations"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|r| r["state"].as_str())
                    .map(|s| format!("support_{s}")),
            );
        }
        if !reasons.is_empty() {
            attention.push(json!({"id":id,"reasons":reasons}));
        }
    }
    let nodes = report["nodes"].as_object().unwrap().len();
    let subjects = report["history_subjects"].as_object().unwrap().len();
    let mut lines = vec![
        title.chars().take(300).collect(),
        format!("core/v1 snapshot {}", context.snapshot_id()),
        format!("findings {}", context.findings_revision()),
        format!("{nodes} computational nodes; {subjects} history subjects"),
    ];
    let falsified = report["nodes"]
        .as_object()
        .unwrap()
        .iter()
        .filter(|(_, node)| node["state"]["falsifier"]["status"] == "holds")
        .map(|(id, _)| id.clone())
        .collect::<BTreeSet<_>>();
    let answered = crate::value::TypedValue::from_json(&snapshot["document"])
        .map(|document| crate::public_amend::document_flags(&document, &falsified))
        .unwrap_or_default();
    let mut attention_lines = attention
        .iter()
        .map(|a| {
            format!(
                "  {}: {}",
                a["id"].as_str().unwrap(),
                a["reasons"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(J::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
        .collect::<Vec<_>>();
    attention_lines.extend(answered.iter().map(|(id, flag)| {
        format!(
            "  {id}: {}",
            flag.text(&|a, b| crate::public_ordinary_readers::apart(a, b, 28))
        )
    }));
    if attention.is_empty() && answered.is_empty() {
        lines.push(format!(
            "  no attention selected by {}",
            report["attention_policy"].as_str().unwrap()
        ));
    }
    let mut footer =
        vec!["Use `kpop assess ID --profile core/v1 --history` for exact findings.".into()];
    if let Some(mapping) = data["mapping"]["mapping"].as_str() {
        footer.push(format!("Mapping: {mapping}").chars().take(300).collect());
    }
    let mut kept = vec![];
    for line in &attention_lines {
        let omitted = attention_lines.len() - kept.len() - 1;
        let mut candidate = lines.clone();
        candidate.extend(kept.clone());
        candidate.push(line.clone());
        if omitted > 0 {
            candidate.push(format!("  ... {omitted} more attention items omitted"));
        }
        candidate.extend(footer.clone());
        if candidate.join("\n").chars().count() > 2000 {
            break;
        }
        kept.push(line.clone());
    }
    let omitted = attention_lines.len() - kept.len();
    lines.extend(kept);
    if omitted > 0 {
        lines.push(format!("  ... {omitted} more attention items omitted"));
        data["text_omitted_attention"] = json!(omitted);
    }
    lines.extend(footer);
    let fields = json!({"assessment_profile":"core/v1","snapshot_id":context.snapshot_id(),"findings_revision":context.findings_revision(),"nodes":nodes,"history_subjects":subjects,"attention":attention,"followups_status":"not_projected_for_core/v1"});
    data.as_object_mut()
        .unwrap()
        .extend(fields.as_object().unwrap().clone());
    Ok((data, lines.join("\n")))
}
