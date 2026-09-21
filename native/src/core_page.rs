//! Page selectors and layout checks over one retained canonical assessment.
//! No evaluator or source loader runs at this projection boundary.
use crate::{
    Result, history_yaml, reasoning_context::CapturedAssessment, require, value::TypedValue as V,
};
use serde_json::{Value as J, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
const STATES: &[&str] = &[
    "broken",
    "falsified",
    "unchecked",
    "moved",
    "blocked",
    "no_predicate",
    "unknown",
    "reversed",
];
fn truth(v: &J) -> bool {
    match v {
        J::Null => false,
        J::Bool(b) => *b,
        J::String(s) => !s.is_empty(),
        J::Array(a) => !a.is_empty(),
        J::Object(m) => !m.is_empty(),
        J::Number(n) => n.as_f64() != Some(0.0),
    }
}
fn py(v: &J) -> String {
    V::from_json(v)
        .map(|v| crate::reasoning_authoring::py(&v))
        .unwrap_or_else(|_| v.to_string())
}
fn string(v: &J) -> String {
    if truth(v) { py(v) } else { String::new() }
}
fn iterable(v: &J) -> Result<Vec<J>> {
    if !truth(v) {
        return Ok(vec![]);
    }
    match v {
        J::Array(v) => Ok(v.clone()),
        J::Object(m) => Ok(m.keys().cloned().map(J::String).collect()),
        J::String(s) => Ok(s.chars().map(|c| J::String(c.to_string())).collect()),
        _ => Err(crate::Error("page collection is not iterable".into())),
    }
}
fn state_flags(node: &J, view: &J) -> BTreeSet<String> {
    let mut flags = BTreeSet::new();
    let state = &node["state"];
    let status = &view["status"];
    let body = &node["body"];
    let judgment = node["fields"]["deps"]
        .as_str()
        .is_some_and(|d| body.get(d).is_some());
    let issues = state["integrity"]["issues"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|i| i["code"].as_str())
        .collect::<BTreeSet<_>>();
    if !matches!(
        status["integrity"].as_str(),
        Some("assessed" | "not_available")
    ) || issues.contains("missing_dependency")
        || issues.contains("invalid_dependencies")
    {
        flags.insert("broken".into());
    }
    if status["falsifier"]["holds"] == true {
        flags.insert("falsified".into());
    } else if judgment && status["falsifier"]["status"] == "not_declared" {
        flags.insert("no_predicate".into());
    } else if matches!(
        status["falsifier"]["status"].as_str(),
        Some("unknown" | "error" | "unavailable")
    ) {
        flags.insert("unknown".into());
    }
    if judgment
        && (!matches!(
            status["basis"].as_str(),
            Some("assessed" | "not_applicable")
        ) || issues.contains("missing_snapshot")
            || issues.contains("invalid_snapshot"))
    {
        flags.insert("unchecked".into());
    }
    if state["basis"]["dependencies"].as_object().is_some_and(|m| {
        m.values().any(|d| {
            d["comparison"] == "changed"
                || d["basis_comparison"] == "changed"
                || d["rule_changed"] == true
        })
    }) {
        flags.insert("moved".into());
    }
    if ["blocked_on", "blocked", "waiting_for"]
        .iter()
        .any(|f| truth(&body[*f]))
    {
        flags.insert("blocked".into());
    }
    if matches!(
        status["computation"]["status"].as_str(),
        Some("unknown" | "error" | "limit" | "unsupported_capability")
    ) {
        flags.insert("unknown".into());
    }
    flags
}
fn select(
    selector: &J,
    ids: &BTreeSet<String>,
    judgments: &BTreeSet<String>,
    flags: &BTreeMap<String, BTreeSet<String>>,
) -> BTreeSet<String> {
    if let Some(items) = selector.as_array() {
        return items
            .iter()
            .flat_map(|s| select(s, ids, judgments, flags))
            .collect();
    }
    let Some(s) = selector.as_str() else {
        return BTreeSet::new();
    };
    match s {
        "all" => ids.clone(),
        "judgments" => judgments.clone(),
        "flagged" => flags
            .iter()
            .filter(|(_, f)| !f.is_empty())
            .map(|(id, _)| id.clone())
            .collect(),
        s if STATES.contains(&s) => flags
            .iter()
            .filter(|(_, f)| f.contains(s))
            .map(|(id, _)| id.clone())
            .collect(),
        s if ids.contains(s) => BTreeSet::from([s.into()]),
        s => {
            let prefix = format!("{}.", s.strip_suffix('.').unwrap_or(s));
            ids.iter()
                .filter(|id| id.starts_with(&prefix))
                .cloned()
                .collect()
        }
    }
}
fn unresolved(
    selector: &J,
    ids: &BTreeSet<String>,
    judgments: &BTreeSet<String>,
    flags: &BTreeMap<String, BTreeSet<String>>,
) -> BTreeSet<String> {
    if let Some(items) = selector.as_array() {
        return items
            .iter()
            .flat_map(|s| unresolved(s, ids, judgments, flags))
            .collect();
    }
    let Some(s) = selector.as_str() else {
        return BTreeSet::new();
    };
    if ["all", "judgments", "flagged"].contains(&s)
        || STATES.contains(&s)
        || !select(selector, ids, judgments, flags).is_empty()
    {
        BTreeSet::new()
    } else {
        BTreeSet::from([s.into()])
    }
}
fn valid_authority(authority: &str) -> bool {
    use unicode_normalization::UnicodeNormalization;
    if authority.contains('[') || authority.contains(']') {
        if !(authority.contains('[') && authority.contains(']')) {
            return false;
        }
        let host = authority.rsplit('@').next().unwrap_or("");
        let Some(bracketed) = host.strip_prefix('[') else {
            return false;
        };
        let Some((host, port)) = bracketed.split_once(']') else {
            return false;
        };
        if !port.is_empty() && !port.starts_with(':') {
            return false;
        }
        if let Some(future) = host.strip_prefix('v') {
            if !future.split_once('.').is_some_and(|(version, name)| {
                !version.is_empty()
                    && version.bytes().all(|b| b.is_ascii_hexdigit())
                    && !name.is_empty()
            }) {
                return false;
            }
        } else {
            let address = host.split_once('%').map_or(host, |(v, _)| v);
            if address.parse::<std::net::Ipv6Addr>().is_err() {
                return false;
            }
        }
    }
    let stripped = authority.replace(['@', ':', '#', '?'], "");
    let normalized = stripped.nfkc().collect::<String>();
    stripped == normalized || !normalized.contains(['/', '?', '#', '@', ':'])
}
fn has_link(body: &J) -> bool {
    let target = if truth(&body["url"]) {
        &body["url"]
    } else {
        &body["file"]
    };
    let value = string(target);
    let s = value.trim();
    if s.is_empty() || s.chars().any(|c| (c as u32) < 32) || s.starts_with("//") {
        return false;
    }
    if let Some((scheme, tail)) = s.split_once(':').filter(|(s, _)| {
        s.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    }) {
        let authority = tail
            .strip_prefix("//")
            .map(|v| v.split(['/', '?', '#']).next().unwrap_or(""));
        if authority.is_some_and(|a| !valid_authority(a)) {
            return false;
        }
        match scheme.to_ascii_lowercase().as_str() {
            "http" | "https" => authority.is_some_and(|a| !a.is_empty()),
            "mailto" | "file" => true,
            _ => false,
        }
    } else {
        true
    }
}
fn date(v: &J) -> bool {
    let s = py(v);
    let s = s.trim();
    [
        ("%Y-%m-%d", '-', true),
        ("%d/%m/%Y", '/', false),
        ("%d.%m.%Y", '.', false),
    ]
    .iter()
    .any(|(format, separator, first)| {
        let parts = s.split(*separator).collect::<Vec<_>>();
        parts.len() == 3
            && parts[if *first { 0 } else { 2 }].len() == 4
            && chrono::NaiveDate::parse_from_str(s, format).is_ok()
    })
}
fn fits(
    kind: &str,
    keys: &BTreeSet<String>,
    judgments: &BTreeSet<String>,
    nodes: &serde_json::Map<String, J>,
    typed_dates: &BTreeSet<String>,
) -> Option<String> {
    let entries = keys.difference(judgments).collect::<Vec<_>>();
    match kind {
        "table" | "lines" | "cards" | "axis" => None,
        "timeline" => {
            let bad = entries
                .iter()
                .filter(|k| !typed_dates.contains(**k) && !date(&nodes[**k]["body"]["v"]))
                .map(|k| k.as_str())
                .collect::<Vec<_>>();
            if bad.is_empty() {
                None
            } else {
                Some(format!(
                    "timeline needs date values; {} of {} are not dates ({})",
                    bad.len(),
                    keys.len(),
                    bad.iter().take(4).copied().collect::<Vec<_>>().join(", ")
                ))
            }
        }
        "headline" => {
            if !(1..=4).contains(&entries.len()) {
                return Some(format!(
                    "headline carries one to four values, not {}",
                    entries.len()
                ));
            }
            let blank = entries
                .iter()
                .filter(|k| nodes[**k]["body"]["v"].is_null())
                .map(|k| k.as_str())
                .collect::<Vec<_>>();
            if blank.is_empty() {
                None
            } else {
                Some(format!(
                    "headline needs values; {} {}",
                    blank.join(", "),
                    if blank.len() == 1 {
                        "is derived and this reader does not evaluate rules"
                    } else {
                        "are derived"
                    }
                ))
            }
        }
        "grouped" | "fronts" => {
            let groups = keys
                .iter()
                .map(|k| k.split('.').next().unwrap())
                .collect::<BTreeSet<_>>();
            if groups.len() < 2 {
                Some(format!(
                    "grouped lays groups side by side; these are all one group ({})",
                    groups.into_iter().collect::<Vec<_>>().join(", ")
                ))
            } else {
                None
            }
        }
        "alerts" => {
            if entries.is_empty() {
                None
            } else {
                Some(format!(
                    "alerts ranks judgments; {} of these are entries",
                    entries.len()
                ))
            }
        }
        "links" => {
            let bad = keys
                .iter()
                .filter(|k| judgments.contains(*k) || !has_link(&nodes[*k]["body"]))
                .cloned()
                .collect::<Vec<_>>();
            if bad.is_empty() {
                None
            } else {
                Some(format!(
                    "links needs a safe url or file: {}",
                    bad.join(", ")
                ))
            }
        }
        _ => Some(format!("unknown renderer '{kind}'")),
    }
}
fn tabs(brief: &J) -> Result<Vec<J>> {
    let mut out = vec![];
    let sections = iterable(&brief["sections"])?
        .into_iter()
        .filter(J::is_object)
        .collect::<Vec<_>>();
    if !sections.is_empty() || !truth(&brief["tabs"]) {
        out.push(json!({"key":"now","title":"","occasion":"","serves":[],"sections":sections,"shape":brief["shape"]}));
    }
    for tab in iterable(&brief["tabs"])?.into_iter().filter(J::is_object) {
        let serves = iterable(&tab["serves"])?.iter().map(py).collect::<Vec<_>>();
        let sections = iterable(&tab["sections"])?
            .into_iter()
            .filter(J::is_object)
            .collect::<Vec<_>>();
        out.push(json!({"key":if out.is_empty(){"now".into()}else{format!("now{}",out.len()+1)},"title":string(&tab["title"]),"occasion":string(&tab["occasion"]),"serves":serves,"sections":sections,"shape":tab["shape"]}));
    }
    Ok(out)
}
pub fn coverage(context: &CapturedAssessment, content: Option<&[u8]>) -> Result<J> {
    project(context, content, false)
}
pub fn project(
    context: &CapturedAssessment,
    content: Option<&[u8]>,
    with_links: bool,
) -> Result<J> {
    project_with_page_path(context, content, with_links, None, None)
}

/// Project the page envelope with local file links relative to its eventual output path.
pub fn project_for_page(
    context: &CapturedAssessment,
    content: Option<&[u8]>,
    record_entry: &Path,
    page_path: &Path,
) -> Result<J> {
    project_with_page_path(context, content, true, Some(record_entry), Some(page_path))
}

fn project_with_page_path(
    context: &CapturedAssessment,
    content: Option<&[u8]>,
    with_links: bool,
    record_entry: Option<&Path>,
    page_path: Option<&Path>,
) -> Result<J> {
    let brief = if let Some(content) = content {
        require(
            content.len()
                <= crate::public_core_readers::json_value(context.assessment())?["operational_limits"]["input_bytes"]
                    .as_u64()
                    .ok_or_else(|| crate::Error("invalid page input limit".into()))?
                    as usize,
            "page_brief_input_limit",
        )?;
        let v = crate::public_core_readers::json_value(
            &history_yaml::decode_source_value(content)?.typed(),
        )?;
        if truth(&v) {
            require(v.is_object(), "captured core page brief must be a mapping")?;
            v
        } else {
            json!({})
        }
    } else {
        json!({})
    };
    let mut assessment = crate::public_core_readers::json_value(context.assessment())?;
    let typed_dates = crate::history_contract::map(
        &crate::history_contract::map(context.assessment())?["nodes"],
    )?
    .iter()
    .filter_map(|(id, node)| {
        let body = crate::history_contract::map(node).ok()?.get("body")?;
        let value = if let V::Map(body) = body {
            body.get("v")?
        } else {
            body
        };
        matches!(value, V::Date(_) | V::DateTime(_)).then_some(id.clone())
    })
    .collect::<BTreeSet<_>>();
    for node in assessment["nodes"].as_object_mut().unwrap().values_mut() {
        if !node["body"].is_object() {
            node["body"] = json!({"v":node["body"]});
        }
    }
    let view = crate::public_core_readers::json_value(context.view())?;
    let nodes = assessment["nodes"]
        .as_object()
        .ok_or_else(|| crate::Error("invalid core nodes".into()))?;
    let ids = nodes.keys().cloned().collect::<BTreeSet<_>>();
    let judgments = nodes
        .iter()
        .filter(|(_, n)| {
            n["fields"]["deps"]
                .as_str()
                .is_some_and(|d| n["body"].get(d).is_some())
        })
        .map(|(id, _)| id.clone())
        .collect::<BTreeSet<_>>();
    let flags = nodes
        .iter()
        .map(|(id, n)| (id.clone(), state_flags(n, &view["nodes"][id])))
        .collect::<BTreeMap<_, _>>();
    let shape = json!({"entries":ids.difference(&judgments).count(),"judgments":judgments.len(),"flagged":flags.values().filter(|f| !f.is_empty()).count(),"blocked":flags.values().filter(|f| f.contains("blocked")).count()});
    let mut arrangements = vec![];
    let mut selected = BTreeSet::new();
    let mut missing = BTreeSet::new();
    let mut misfits = vec![];
    let mut stale = vec![];
    let tabs = if truth(&brief) { tabs(&brief)? } else { vec![] };
    for tab in &tabs {
        let recorded = &tab["shape"];
        if truth(recorded) {
            let title = if truth(&tab["title"]) {
                string(&tab["title"])
            } else {
                "Now".into()
            };
            if !recorded.is_object() {
                stale.push(format!("{title}: invalid recorded shape"));
            } else {
                let mut moved = vec![];
                for key in ["entries", "judgments", "flagged", "blocked"] {
                    if let Some(old) = recorded.get(key) {
                        let equal = crate::source_clock::python_equal(
                            &V::from_json(old)?,
                            &V::from_json(&shape[key])?,
                        );
                        if !equal {
                            moved.push(format!("{key}: {} -> {}", py(old), py(&shape[key])));
                        }
                    }
                }
                if !moved.is_empty() {
                    stale.push(format!("{title}: {}", moved.join("; ")));
                }
            }
        }
        let mut sections = vec![];
        for section in tab["sections"].as_array().unwrap() {
            let chosen = select(&section["pick"], &ids, &judgments, &flags);
            missing.extend(unresolved(&section["pick"], &ids, &judgments, &flags));
            let kind = string(&section["as"]);
            let kind = kind.trim();
            if !kind.is_empty()
                && !chosen.is_empty()
                && let Some(wrong) = fits(kind, &chosen, &judgments, nodes, &typed_dates)
            {
                let label = if truth(&section["title"]) {
                    string(&section["title"])
                } else if truth(&section["pick"]) {
                    py(&section["pick"])
                } else {
                    "?".into()
                };
                misfits.push(format!("{label}: {wrong}"));
            }
            selected.extend(chosen.iter().cloned());
            sections.push(json!({"title":string(&section["title"]),"why":string(&section["why"]),"shape":if kind.is_empty(){"cards"}else{kind},"ids":chosen}));
        }
        arrangements.push(json!({"key":tab["key"],"title":tab["title"],"occasion":tab["occasion"],"serves":tab["serves"],"sections":sections}));
    }
    let flagged = flags
        .iter()
        .filter(|(_, f)| !f.is_empty())
        .map(|(id, _)| id.clone())
        .collect::<BTreeSet<_>>();
    let spill = if tabs.is_empty() {
        BTreeSet::new()
    } else {
        flagged.difference(&selected).cloned().collect()
    };
    misfits.sort();
    stale.sort();
    let coverage = json!({"picked":selected,"flagged":flagged,"spill":spill,"covered_count":selected.len(),"spill_count":spill.len(),"unresolved_selectors":missing,"renderer_misfits":misfits,"stale_shapes":stale});
    let page = secondary(
        context,
        content,
        &brief,
        &assessment,
        &view,
        &judgments,
        &flags,
        &arrangements,
        &coverage,
        with_links,
        record_entry,
        page_path,
    )?;
    Ok(
        json!({"shape":shape,"flags":flags,"tabs":arrangements,"coverage":coverage,"page_assessment":page}),
    )
}

fn quote(value: &str, safe: &str) -> String {
    let mut output = String::new();
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || b"_.-~".contains(byte) || safe.as_bytes().contains(byte)
        {
            output.push(*byte as char);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}
fn link_for_page(
    body: &J,
    record_entry: Option<&Path>,
    page_path: Option<&Path>,
) -> Result<String> {
    if !has_link(body) {
        return Ok(String::new());
    }
    let is_url = truth(&body["url"]);
    let value = string(if is_url { &body["url"] } else { &body["file"] });
    let value = value.trim();
    let scheme = value.split_once(':').filter(|(s, _)| {
        s.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    });
    if let Some((scheme, tail)) = scheme {
        let scheme = scheme.to_ascii_lowercase();
        let (netloc, tail) = if let Some(rest) = tail.strip_prefix("//") {
            let stop = rest.find(['/', '?', '#']).unwrap_or(rest.len());
            (Some(&rest[..stop]), &rest[stop..])
        } else {
            (None, tail)
        };
        let (path_query, fragment) = tail
            .split_once('#')
            .map_or((tail, None), |(p, f)| (p, Some(f)));
        let (path, query) = path_query
            .split_once('?')
            .map_or((path_query, None), |(p, q)| (p, Some(q)));
        let mut out = scheme.clone() + ":";
        let netloc = netloc.or_else(|| {
            (scheme == "file" && (path.is_empty() || path.starts_with('/'))).then_some("")
        });
        if let Some(netloc) = netloc {
            out.push_str("//");
            out.push_str(netloc);
            if !path.is_empty() && !path.starts_with('/') {
                out.push('/');
            }
        }
        out.push_str(&quote(path, "/.-_~%:@!$&'()*+,;="));
        if let Some(q) = query.filter(|q| !q.is_empty()) {
            out.push('?');
            out.push_str(&quote(q, "/?.-_~%:@!$&'()*+,;="));
        }
        if let Some(f) = fragment.filter(|f| !f.is_empty()) {
            out.push('#');
            out.push_str(&quote(f, "/?.-_~%:@!$&'()*+,;="));
        }
        Ok(out)
    } else if let (Some(record), Some(page)) = (record_entry, page_path) {
        let source = if Path::new(value).is_absolute() {
            Path::new(value).to_path_buf()
        } else {
            record.parent().unwrap_or(record).join(value)
        };
        let source = crate::project_modes::resolved(&source)?;
        let page_dir = page.parent().unwrap_or(Path::new("."));
        let page_dir = crate::project_modes::resolved(page_dir)?;
        let relative = pathdiff(&source, &page_dir);
        Ok(quote(&relative, "/.-_~"))
    } else if is_url && (value.starts_with('#') || value.starts_with('?')) {
        Ok(quote(value, "?.-_~%=&#+@!$'()*+,;:/"))
    } else {
        Ok(quote(value, if is_url { "/.-_~%?=&#+@" } else { "/.-_~" }))
    }
}

fn pathdiff(path: &Path, base: &Path) -> String {
    let path = path.components().collect::<Vec<_>>();
    let base = base.components().collect::<Vec<_>>();
    let common = path.iter().zip(&base).take_while(|(a, b)| a == b).count();
    let mut out = Vec::new();
    for _ in common..base.len() {
        out.push("..".into());
    }
    out.extend(
        path[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    if out.is_empty() {
        ".".into()
    } else {
        out.join("/")
    }
}

fn language(doc: &J) -> String {
    let meta = &doc["meta"];
    if let Some(explicit) = [&meta["language"], &meta["lang"]]
        .into_iter()
        .find(|v| truth(v))
    {
        return py(explicit)
            .to_lowercase()
            .replace('_', "-")
            .split('-')
            .next()
            .unwrap_or("")
            .into();
    }
    fn visit(value: &J, text: &mut String) {
        match value {
            J::Object(m) => {
                for (k, v) in m {
                    if [
                        "name",
                        "title",
                        "label",
                        "what",
                        "desc",
                        "scope",
                        "domain",
                        "because",
                        "verdict",
                        "note",
                        "why",
                        "asked",
                        "blocked_on",
                        "reopened_by",
                    ]
                    .contains(&k.as_str())
                        && v.is_string()
                    {
                        text.push(' ');
                        text.push_str(v.as_str().unwrap());
                    } else {
                        visit(v, text);
                    }
                }
            }
            J::Array(a) => {
                for v in a {
                    visit(v, text);
                }
            }
            _ => {}
        }
    }
    let mut text = String::new();
    visit(doc, &mut text);
    let letters = text.chars().filter(|c| c.is_alphabetic()).count();
    for (name, start, end) in [("he", '\u{590}', '\u{5ff}'), ("ar", '\u{600}', '\u{6ff}')] {
        if letters > 0
            && text.chars().filter(|c| (start..=end).contains(c)).count() as f64 / letters as f64
                > 0.3
        {
            return name.into();
        }
    }
    "en".into()
}
#[allow(clippy::too_many_arguments)]
fn secondary(
    context: &CapturedAssessment,
    content: Option<&[u8]>,
    brief: &J,
    assessment: &J,
    view: &J,
    judgments: &BTreeSet<String>,
    flags: &BTreeMap<String, BTreeSet<String>>,
    arrangements: &[J],
    coverage: &J,
    with_links: bool,
    record_entry: Option<&Path>,
    page_path: Option<&Path>,
) -> Result<J> {
    let snapshot = crate::public_core_readers::json_value(&context.snapshot().to_data())?;
    let document = &snapshot["document"];
    let meta = &document["meta"];
    let lang = language(document);
    let direction = if truth(&meta["direction"]) {
        meta["direction"].clone()
    } else {
        json!(if ["he", "ar"].contains(&lang.as_str()) {
            "rtl"
        } else {
            "ltr"
        })
    };
    require(direction.is_string(), "page direction must be text")?;
    let title = [&brief["title"], &meta["name"], &meta["scope"]]
        .into_iter()
        .find(|v| truth(v))
        .map_or_else(|| "record".into(), py);
    let typed_nodes = crate::history_contract::map(
        &crate::history_contract::map(context.assessment())?["nodes"],
    )?;
    let mut nodes = vec![];
    for (id, node) in assessment["nodes"].as_object().unwrap() {
        let body = &node["body"];
        let projected = &view["nodes"][id];
        let label = if let Some(label) = brief["labels"].get(id) {
            py(label)
        } else {
            ["name", "title", "label", "what", "desc"]
                .iter()
                .map(|k| &body[*k])
                .find(|v| truth(v))
                .map(|v| py(v).trim().to_owned())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| id.rsplit('.').next().unwrap().replace('_', " "))
        };
        let rule = if body["rule"].is_null() {
            J::Null
        } else {
            let raw = crate::history_contract::map(&typed_nodes[id])?["body"].clone();
            let rule = crate::history_contract::map(&raw)?
                .get("rule")
                .unwrap_or(&V::Null);
            json!(
                crate::reasoning_projection::render_expression(rule)
                    .unwrap_or_else(|_| "unsupported expression".into())
            )
        };
        let deps = node["fields"]["deps"]
            .as_str()
            .and_then(|f| body.get(f))
            .and_then(J::as_array)
            .into_iter()
            .flatten()
            .filter_map(J::as_str)
            .chain(
                projected["dependencies"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|d| d["id"].as_str()),
            )
            .collect::<BTreeSet<_>>();
        nodes.push(json!({"id":id,"kind":if judgments.contains(id){"judgment"}else{"entry"},"label":label,"value":projected["status"]["computation"]["value_text"],"rule":rule,"status":projected["status"],"status_text":projected["status_text"],"dependencies":deps,"flags":flags[id],"href":if with_links{json!(link_for_page(body, record_entry, page_path)?)}else{J::Null}}));
    }
    let values = json!({"consumer_view_version":view["version"],"title":title,"language":lang,"direction":direction,"nodes":nodes,"arrangements":arrangements,"coverage":coverage});
    let to_value = |v: &J| V::from_json_bounded(v, 64 * 1024 * 1024 / 8);
    let input_limit = assessment["operational_limits"]["input_bytes"]
        .as_u64()
        .ok_or_else(|| crate::Error("invalid page input limit".into()))?
        as usize;
    let output_limit = assessment["operational_limits"]["output_bytes"]
        .as_u64()
        .ok_or_else(|| crate::Error("invalid page output limit".into()))?
        as usize;
    require(
        history_yaml::compact_json_size(&to_value(&values)?) <= input_limit,
        "page_input_limit",
    )?;
    let bound = json!({"snapshot_id":context.snapshot_id(),"findings_revision":context.findings_revision(),"values":values});
    let revision = crate::reasoning_snapshot::digest(&to_value(&bound)?)?;
    let brief_identity=content.map(|b|json!({"status":"captured","byte_length":b.len(),"sha256":crate::identity::sha256(b)})).unwrap_or_else(||json!({"status":"absent"}));
    let mut envelope = json!({"schema_version":1,"assessment_profile":"page-secondary/v1","snapshot_id":context.snapshot_id(),"findings_revision":context.findings_revision(),"brief_identity":brief_identity,"page_inputs":{"revision":revision,"values":values},"page_projection_version":1});
    envelope["page_assessment_revision"] =
        json!(crate::reasoning_snapshot::digest(&to_value(&envelope)?)?);
    require(
        history_yaml::compact_json_size(&to_value(&envelope)?) <= output_limit,
        "page_assessment_output_limit",
    )?;
    Ok(envelope)
}

#[cfg(test)]
mod page_link_tests {
    use super::link_for_page;
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn output_relative_local_links_match_python_shape() {
        let body = json!({"file":"evidence/space #/%/Δ.txt"});
        assert_eq!(
            link_for_page(
                &body,
                Some(Path::new("/tmp/record/GROUNDING.yaml")),
                Some(Path::new("/tmp/out/pages/index.html")),
            )
            .unwrap(),
            "../../record/evidence/space%20%23/%25/%CE%94.txt"
        );
    }

    #[test]
    fn nonexistent_parent_then_dotdot_keeps_the_correct_source() {
        let body = json!({"file":"missing/../source.html"});
        assert_eq!(
            link_for_page(
                &body,
                Some(Path::new("/tmp/record/GROUNDING.yaml")),
                Some(Path::new("/tmp/out/page.html"))
            )
            .unwrap(),
            "../record/source.html"
        );
    }

    #[test]
    fn url_sources_keep_scheme_and_encoded_components() {
        let body = json!({"url":"https://example.com/a b?q=x y#frag ment"});
        assert_eq!(
            link_for_page(
                &body,
                Some(Path::new("/tmp/record/GROUNDING.yaml")),
                Some(Path::new("/tmp/page.html"))
            )
            .unwrap(),
            "https://example.com/a%20b?q=x%20y#frag%20ment"
        );
    }
}
