//! Native ordinary-record Hub projection.
//!
//! The ordinary assessment is supplied by `ordinary_assessment_report`; this module
//! only selects and presents that retained result.  The browser behavior and styles
//! remain the long-lived page assets shared with the Python implementation.
use crate::{
    Result,
    history_contract::{Map, map, text},
    history_view::{list, truth},
    reasoning_authoring::{named, py},
    source_capture::CapturedSource,
    value::TypedValue as V,
};
use serde_json::{Map as JsonMap, Value as J, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};

const CSS: &str = include_str!("../../scripts/page/page.css");
const COMPONENT_CSS: &str = include_str!("../../scripts/page/components.css");
const JS: &str = include_str!("../../scripts/page/page.js");
const DATE_JS: &str = include_str!("../../scripts/page/dates.js");

#[derive(Debug)]
pub struct Page {
    pub html: String,
    pub elements: usize,
    pub entries: usize,
    pub judgments: usize,
    pub tabs: usize,
    pub failures: Vec<String>,
}

#[derive(Clone)]
struct Tab {
    key: String,
    title: String,
    sections: Vec<Section>,
}
#[derive(Clone)]
struct Section {
    title: String,
    why_: String,
    text: String,
    pick: Vec<String>,
    kind: String,
}

fn esc(value: &str, attribute: bool) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attribute => out.push_str("&quot;"),
            '\'' if attribute => out.push_str("&#x27;"),
            _ => out.push(c),
        }
    }
    out
}
fn body<'a>(nodes: &'a Map, id: &str) -> Result<&'a Map> {
    map(&map(&nodes[id])?["body"])
}
fn textish(v: &V) -> String {
    if matches!(v, V::Text(_)) {
        text(v).unwrap_or("").into()
    } else {
        py(v)
    }
}
fn field<'a>(m: &'a Map, keys: &[&str]) -> Option<&'a V> {
    keys.iter().find_map(|k| m.get(*k).filter(|v| truth(v)))
}
fn label(id: &str, body: &Map, labels: &Map) -> String {
    labels
        .get(id)
        .map(textish)
        .filter(|s| !s.is_empty())
        .or_else(|| Some(named(&V::Map(body.clone()))).filter(|s| !s.is_empty()))
        .unwrap_or_else(|| id.split('.').last().unwrap_or(id).replace('_', " "))
}
fn deps(body: &Map, dep_field: &str) -> Vec<String> {
    body.get(dep_field)
        .and_then(|v| list(v).ok())
        .map(|a| a.iter().map(textish).collect())
        .or_else(|| {
            body.get(dep_field)
                .and_then(|v| text(v).ok())
                .map(|s| s.chars().map(|c| c.to_string()).collect())
        })
        .unwrap_or_default()
}
fn flags(node: &Map, judgment: bool) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let Ok(state) = map(&node["state"]) else {
        out.insert("broken".into());
        return out;
    };
    let basis = state.get("basis").and_then(|v| map(v).ok());
    let falsifier = state.get("falsifier").and_then(|v| map(v).ok());
    let integrity = state.get("integrity").and_then(|v| map(v).ok());
    if integrity
        .and_then(|m| m.get("status"))
        .and_then(|v| text(v).ok())
        .is_some_and(|s| !matches!(s, "assessed" | "unassessed"))
    {
        out.insert("broken".into());
    }
    if falsifier
        .and_then(|m| m.get("status"))
        .and_then(|v| text(v).ok())
        == Some("holds")
    {
        out.insert("falsified".into());
    }
    if judgment
        && falsifier
            .and_then(|m| m.get("status"))
            .and_then(|v| text(v).ok())
            == Some("not_declared")
    {
        out.insert("no_predicate".into());
    }
    if matches!(
        falsifier
            .and_then(|m| m.get("status"))
            .and_then(|v| text(v).ok()),
        Some("unknown" | "error" | "unavailable")
    ) {
        out.insert("unknown".into());
    }
    if judgment
        && !matches!(
            basis
                .and_then(|m| m.get("status"))
                .and_then(|v| text(v).ok()),
            Some("assessed" | "not_applicable")
        )
    {
        out.insert("unchecked".into());
    }
    if basis
        .and_then(|m| m.get("dependencies"))
        .and_then(|v| map(v).ok())
        .is_some_and(|ds| {
            ds.values().any(|d| {
                map(d).ok().is_some_and(|d| {
                    d.get("comparison").and_then(|v| text(v).ok()) == Some("changed")
                        || d.get("rule_changed").is_some_and(truth)
                })
            })
        })
    {
        out.insert("moved".into());
    }
    out
}
fn selectors(v: Option<&V>) -> Vec<String> {
    match v {
        Some(V::List(a)) => a.iter().map(textish).collect(),
        Some(v) if truth(v) => vec![textish(v)],
        _ => vec![],
    }
}
fn select(
    selector: &str,
    ids: &BTreeSet<String>,
    judgments: &BTreeSet<String>,
    states: &BTreeMap<String, BTreeSet<String>>,
) -> BTreeSet<String> {
    let s = selector.trim();
    match s {
        "all" => ids.clone(),
        "judgments" => judgments.clone(),
        "flagged" => states
            .iter()
            .filter(|(_, f)| !f.is_empty())
            .map(|(k, _)| k.clone())
            .collect(),
        s if [
            "broken",
            "falsified",
            "unchecked",
            "moved",
            "blocked",
            "no_predicate",
            "unknown",
            "reversed",
        ]
        .contains(&s) =>
        {
            states
                .iter()
                .filter(|(_, f)| f.contains(s))
                .map(|(k, _)| k.clone())
                .collect()
        }
        s if ids.contains(s) => BTreeSet::from([s.into()]),
        s => {
            let p = format!("{}.", s.trim_end_matches('.'));
            ids.iter()
                .filter(|id| id.starts_with(&p))
                .cloned()
                .collect()
        }
    }
}
fn parse_brief(content: Option<&[u8]>) -> Result<(Map, Vec<Tab>)> {
    let brief = match content {
        Some(b) => crate::history_yaml::decode_source_value(b)?.typed(),
        None => V::Map(Map::new()),
    };
    let brief = if truth(&brief) {
        map(&brief)?.clone()
    } else {
        Map::new()
    };
    let mut tabs = vec![];
    let section_values = brief
        .get("sections")
        .and_then(|v| list(v).ok())
        .map(<[V]>::to_vec)
        .unwrap_or_default();
    if !section_values.is_empty() || !brief.get("tabs").is_some_and(truth) {
        tabs.push(Tab {
            key: "now".into(),
            title: String::new(),
            sections: parse_sections(&section_values),
        });
    }
    for v in brief
        .get("tabs")
        .and_then(|v| list(v).ok())
        .into_iter()
        .flatten()
    {
        let Ok(t) = map(v) else { continue };
        let key = if tabs.is_empty() {
            "now".into()
        } else {
            format!("now{}", tabs.len() + 1)
        };
        tabs.push(Tab {
            key,
            title: t.get("title").map(textish).unwrap_or_default(),
            sections: parse_sections(t.get("sections").and_then(|v| list(v).ok()).unwrap_or(&[])),
        });
    }
    Ok((brief, tabs))
}
fn parse_sections(values: &[V]) -> Vec<Section> {
    values
        .iter()
        .filter_map(|v| map(v).ok())
        .map(|s| Section {
            title: s.get("title").map(textish).unwrap_or_default(),
            why_: s.get("why").map(textish).unwrap_or_default(),
            text: s.get("text").map(textish).unwrap_or_default(),
            pick: selectors(s.get("pick")),
            kind: s.get("as").map(textish).unwrap_or_default(),
        })
        .collect()
}
fn json_value(v: &V) -> J {
    serde_json::from_str(&crate::ordinary_assessment_report::legacy_json(v).unwrap()).unwrap()
}
fn url_path(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"/.-_~".contains(b) {
                (*b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
fn url_full(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"/.-_~%:@!$&'()*+,;=?#[]".contains(b) {
                (*b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
fn pathdiff(path: &Path, base: &Path) -> PathBuf {
    let a = path.components().collect::<Vec<_>>();
    let b = base.components().collect::<Vec<_>>();
    let n = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let mut o = PathBuf::new();
    for c in &b[n..] {
        if matches!(c, Component::Normal(_)) {
            o.push("..")
        }
    }
    for c in &a[n..] {
        o.push(c.as_os_str())
    }
    o
}
fn href(body: &Map, root: &Path, page: &Path) -> Option<String> {
    let (raw, is_url) = body
        .get("url")
        .filter(|v| truth(v))
        .map(|v| (textish(v), true))
        .or_else(|| {
            body.get("file")
                .filter(|v| truth(v))
                .map(|v| (textish(v), false))
        })?;
    let raw = raw.trim();
    if raw.is_empty() || raw.chars().any(|c| c < ' ') || raw.starts_with("//") {
        return None;
    }
    if let Some((scheme, tail)) = raw.split_once(':') {
        if scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
        {
            let scheme = scheme.to_ascii_lowercase();
            if matches!(scheme.as_str(), "http" | "https") && !tail.starts_with("//") {
                return None;
            }
            return matches!(scheme.as_str(), "http" | "https" | "mailto" | "file")
                .then(|| url_full(raw));
        }
    }
    if is_url && raw.starts_with('#') {
        return Some(raw.into());
    }
    let source = if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        root.join(raw)
    };
    let source = crate::project_modes::resolved(&source).ok()?;
    let parent = page.parent()?;
    Some(url_path(&pathdiff(&source, parent).to_string_lossy()))
}
fn js_safe(v: &J) -> String {
    serde_json::to_string(v).unwrap().replace('<', "\\u003c")
}

pub fn build(
    capture: &CapturedSource,
    assessment: &V,
    brief_content: Option<&[u8]>,
    entry: &Path,
    page_path: &Path,
    max: usize,
) -> Result<Page> {
    let document = capture.document();
    let doc = map(&document)?;
    let report = map(assessment)?;
    let nodes = map(&report["nodes"])?;
    let fields = crate::reasoning_fields::snapshot_fields(&document)?;
    let dep_field = fields
        .get("deps")
        .and_then(|v| text(v).ok())
        .unwrap_or("rests_on");
    let pred_field = fields
        .get("predicate")
        .and_then(|v| text(v).ok())
        .unwrap_or("wrong_if");
    let ids = nodes.keys().cloned().collect::<BTreeSet<_>>();
    let judgments = ids
        .iter()
        .filter(|id| body(nodes, id).is_ok_and(|b| b.contains_key(dep_field)))
        .cloned()
        .collect::<BTreeSet<_>>();
    let states = ids
        .iter()
        .map(|id| {
            (
                id.clone(),
                flags(map(&nodes[id]).unwrap(), judgments.contains(id)),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let (brief, tabs) = parse_brief(brief_content)?;
    let labels = brief
        .get("labels")
        .and_then(|v| map(v).ok())
        .cloned()
        .unwrap_or_default();
    let meta = doc
        .get("meta")
        .and_then(|v| map(v).ok())
        .cloned()
        .unwrap_or_default();
    let language = meta
        .get("language")
        .and_then(|v| text(v).ok())
        .filter(|s| ["en", "he", "ar"].contains(s))
        .unwrap_or("en");
    let dir = if language == "he" || language == "ar" {
        "rtl"
    } else {
        "ltr"
    };
    let words = words(language, dir);
    let mut used = BTreeMap::<String, Vec<String>>::new();
    for id in &judgments {
        for d in deps(body(nodes, id)?, dep_field) {
            used.entry(d).or_default().push(id.clone())
        }
    }
    let mut entries = JsonMap::new();
    let mut decisions = JsonMap::new();
    for id in &ids {
        let b = body(nodes, id)?;
        if judgments.contains(id) {
            decisions.insert(id.clone(),json!({"deps":deps(b,dep_field),"used":used.get(id).cloned().unwrap_or_default(),"pred":b.get(pred_field).map(crate::public_ordinary_readers::predicate_text).unwrap_or_default(),"verdict":field(b,&["verdict","title"]).map(textish).unwrap_or_default(),"because":field(b,&["because","why"]).map(textish).unwrap_or_default(),"blocked":field(b,&["blocked_on","blocked","waiting_for"]).map(textish).unwrap_or_default(),"reopened":""}));
        } else {
            let mut e = JsonMap::new();
            for k in [
                "v", "rule", "from", "at", "of", "read", "quoted", "url", "file", "asked",
                "measure",
            ] {
                if let Some(v) = b.get(k).filter(|v| truth(v)) {
                    e.insert(k.into(), json_value(v));
                }
            }
            if !e.contains_key("v") {
                if let Some(v) = e.get("quoted").cloned() {
                    e.insert("v".into(), v);
                }
            }
            let mut parents = Vec::new();
            if let Some(parent) = b.get("from").and_then(|v| text(v).ok())
                && ids.contains(parent)
                && parent != id
            {
                parents.push(parent.to_owned());
            }
            if !parents.is_empty() {
                e.insert("par".into(), json!(parents));
            }
            e.insert(
                "used".into(),
                json!(used.get(id).cloned().unwrap_or_default()),
            );
            let own_name = named(&V::Map(b.clone()));
            if !own_name.is_empty() {
                e.insert("name".into(), json!(own_name));
            }
            entries.insert(id.clone(), J::Object(e));
        }
    }
    let mut failures = vec![];
    let mut panels = vec![];
    let mut selected_all = BTreeSet::new();
    for tab in &tabs {
        let mut panel = String::new();
        panel.push_str(&heading(
            &meta,
            &brief,
            &words,
            ids.len() - judgments.len(),
            judgments.len(),
        ));
        for sec in &tab.sections {
            let mut chosen = BTreeSet::new();
            for s in &sec.pick {
                let got = select(s, &ids, &judgments, &states);
                if got.is_empty() {
                    failures.push(format!(
                        "section '{}': selector {s} picks nothing",
                        if sec.title.is_empty() {
                            "?"
                        } else {
                            &sec.title
                        }
                    ));
                }
                chosen.extend(got);
            }
            selected_all.extend(chosen.clone());
            let title = if sec.title.is_empty() {
                sec.pick.join(",")
            } else {
                sec.title.clone()
            };
            let kind = if sec.kind.is_empty() {
                if chosen.iter().any(|id| judgments.contains(id)) {
                    "cards"
                } else {
                    "table"
                }
            } else {
                sec.kind.as_str()
            };
            if !matches!(
                kind,
                "table"
                    | "lines"
                    | "cards"
                    | "headline"
                    | "timeline"
                    | "grouped"
                    | "fronts"
                    | "alerts"
                    | "axis"
                    | "links"
            ) {
                failures.push(format!("section '{title}': unknown renderer '{kind}'"));
            }
            if sec.pick.is_empty() && sec.text.is_empty() {
                failures.push(format!("section '{title}' is empty"));
            }
            panel.push_str(&format!(
                "<div data-component=\"{}\"><h2 dir=\"auto\">{} <span class=\"n\">{}</span></h2>",
                esc(kind, true),
                esc(&title, false),
                chosen.len()
            ));
            if !sec.why_.is_empty() {
                panel.push_str(&format!(
                    "<div class=\"why\" dir=\"auto\">{}</div>",
                    esc(&sec.why_, false)
                ));
            }
            if !sec.text.is_empty() {
                panel.push_str(&format!(
                    "<div class=\"prose\" dir=\"auto\">{}</div>",
                    linked_text(&sec.text, &ids, &labels, nodes)
                ));
            }
            panel.push_str(&render_set(
                &chosen,
                &judgments,
                nodes,
                &labels,
                dep_field,
                root_of(entry),
                page_path,
                kind,
                &states,
            )?);
            panel.push_str("</div>");
        }
        panels.push((tab.key.clone(), panel));
    }
    let spill = states
        .iter()
        .filter(|(id, f)| !f.is_empty() && !selected_all.contains(*id))
        .map(|(id, _)| id.clone())
        .collect::<BTreeSet<_>>();
    if !spill.is_empty() {
        for (_, p) in &mut panels {
            p.push_str(&format!("<h2 class=\"spill\">Flagged outside the arrangement <span class=\"n\">{}</span></h2>",spill.len()));
            p.push_str(&render_set(
                &spill,
                &judgments,
                nodes,
                &labels,
                dep_field,
                root_of(entry),
                page_path,
                "alerts",
                &states,
            )?);
        }
    }
    let record = render_record(
        &ids,
        &judgments,
        nodes,
        &labels,
        dep_field,
        root_of(entry),
        page_path,
        &states,
    )?;
    let tree = render_tree(&ids, &judgments, nodes, dep_field, &labels);
    let title = brief
        .get("title")
        .map(textish)
        .filter(|s| !s.is_empty())
        .or_else(|| Some(named(&V::Map(meta.clone()))).filter(|s| !s.is_empty()))
        .unwrap_or_else(|| {
            if language == "he" {
                "מה ידוע כאן".into()
            } else {
                "What is known here".into()
            }
        });
    let mut out = format!(
        "<!doctype html><html lang=\"{language}\" dir=\"{dir}\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{}</title><style>\n{}\n{}\n</style></head><body><div class=\"wrap\" dir=\"{dir}\"><div class=\"tabs\" role=\"tablist\">",
        esc(&title.chars().take(60).collect::<String>(), false),
        CSS,
        COMPONENT_CSS
    );
    for (i, t) in tabs.iter().enumerate() {
        out.push_str(&format!(
            "<button type=\"button\" data-tab=\"{}\" aria-selected=\"{}\">{}</button>",
            t.key,
            i == 0,
            esc(
                if t.title.is_empty() {
                    if language == "he" {
                        "עכשיו"
                    } else {
                        "Now"
                    }
                } else {
                    &t.title
                },
                false
            )
        ));
    }
    out.push_str(&format!("<button type=\"button\" data-tab=\"record\" aria-selected=\"{}\">{} <span class=\"n\">{}</span></button><button type=\"button\" data-tab=\"tree\" aria-selected=\"false\">{}</button></div>",tabs.is_empty(),if language=="he"{"הרשומה"}else{"Record"},ids.len(),if language=="he"{"העץ"}else{"Tree"}));
    for (i, (key, panel)) in panels.iter().enumerate() {
        out.push_str(&format!(
            "<section id=\"panel-{key}\"{}>{panel}</section>",
            if i == 0 { "" } else { " hidden" }
        ));
    }
    out.push_str(&format!("<section id=\"panel-record\"{}>{}{record}</section><section id=\"panel-tree\" hidden>{}<div class=\"treewrap\">{tree}</div></section>",if tabs.is_empty(){""}else{" hidden"},heading(&meta,&brief,&words,ids.len()-judgments.len(),judgments.len()),esc(&title,false)));
    out.push_str("<footer>Hover or click any reference to see where it came from. This page is generated from the record.</footer></div><script>window.__T=");
    out.push_str(&js_safe(&words));
    out.push_str(";window.__E=");
    out.push_str(&js_safe(&J::Object(entries)));
    out.push_str(";window.__J=");
    out.push_str(&js_safe(&J::Object(decisions)));
    out.push_str(";</script><script>");
    out.push_str(JS);
    out.push_str("</script><script>");
    out.push_str(DATE_JS);
    out.push_str("</script></body></html>");
    crate::require(out.len() <= max, "Hub HTML output exceeds its byte limit")?;
    Ok(Page {
        html: out,
        elements: ids.len(),
        entries: ids.len() - judgments.len(),
        judgments: judgments.len(),
        tabs: tabs.len() + 2,
        failures,
    })
}
fn root_of(entry: &Path) -> &Path {
    entry.parent().unwrap_or_else(|| Path::new("."))
}
fn linked_text(value: &str, ids: &BTreeSet<String>, labels: &Map, nodes: &Map) -> String {
    let re = regex::Regex::new(r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+").unwrap();
    let mut out = String::new();
    let mut at = 0;
    for m in re.find_iter(value) {
        if !ids.contains(m.as_str()) {
            continue;
        }
        out.push_str(&esc(&value[at..m.start()], false));
        let id = m.as_str();
        let shown = body(nodes, id)
            .ok()
            .map(|b| label(id, b, labels))
            .unwrap_or_else(|| id.into());
        out.push_str(&format!(
            "<span class=\"fx in\" data-id=\"{}\">{}</span>",
            esc(id, true),
            esc(&shown, false)
        ));
        at = m.end();
    }
    out.push_str(&esc(&value[at..], false));
    out
}
fn heading(meta: &Map, _brief: &Map, words: &J, e: usize, j: usize) -> String {
    let title = Some(named(&V::Map(meta.clone())))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| words["untitled"].as_str().unwrap().into());
    format!(
        "<h1 dir=\"auto\">{}</h1><p class=\"meta\">{} entries and {} judgments</p>",
        esc(&title, false),
        e,
        j
    )
}
fn render_set(
    ids: &BTreeSet<String>,
    jud: &BTreeSet<String>,
    nodes: &Map,
    labels: &Map,
    _dep: &str,
    root: &Path,
    page: &Path,
    kind: &str,
    states: &BTreeMap<String, BTreeSet<String>>,
) -> Result<String> {
    let mut o = String::new();
    let cards = matches!(kind, "cards" | "alerts") || ids.iter().any(|id| jud.contains(id));
    if cards {
        o.push_str(if kind == "alerts" {
            "<div class=\"alerts\">"
        } else {
            "<div class=\"cards\">"
        });
        for id in ids {
            if jud.contains(id) {
                let b = body(nodes, id)?;
                let verdict = field(b, &["verdict", "title"])
                    .map(textish)
                    .unwrap_or_else(|| id.clone());
                o.push_str(&format!("<article class=\"card\" data-flags=\"{}\"><div class=\"vd fx\" data-id=\"{}\">{}</div></article>",states[id].iter().cloned().collect::<Vec<_>>().join(" "),esc(id,true),esc(&verdict,false)));
            }
        }
        o.push_str("</div>");
    }
    let entries = ids
        .iter()
        .filter(|id| !jud.contains(*id))
        .cloned()
        .collect::<Vec<_>>();
    if !entries.is_empty() {
        o.push_str("<table><tbody>");
        for id in entries {
            let b = body(nodes, &id)?;
            let val = field(b, &["v", "quoted", "rule"])
                .map(textish)
                .unwrap_or_default();
            let name = label(&id, b, labels);
            let cell = format!(
                "<span class=\"fx\" data-id=\"{}\">{}</span>",
                esc(&id, true),
                esc(&val, false)
            );
            let name = if let Some(h) = href(b, root, page) {
                format!("<a href=\"{}\">{}</a>", esc(&h, true), esc(&name, false))
            } else {
                esc(&name, false)
            };
            o.push_str(&format!(
                "<tr><th>{name}</th><td class=\"v\" dir=\"auto\">{cell}</td></tr>"
            ));
        }
        o.push_str("</tbody></table>");
    }
    Ok(o)
}
fn render_record(
    ids: &BTreeSet<String>,
    jud: &BTreeSet<String>,
    nodes: &Map,
    labels: &Map,
    dep: &str,
    root: &Path,
    page: &Path,
    states: &BTreeMap<String, BTreeSet<String>>,
) -> Result<String> {
    let mut o = String::new();
    if !jud.is_empty() {
        o.push_str(&format!(
            "<h2>Judgments <span class=\"n\">{}</span></h2>",
            jud.len()
        ));
        o.push_str(&render_set(
            jud, jud, nodes, labels, dep, root, page, "cards", states,
        )?);
    }
    let entries = ids.difference(jud).cloned().collect();
    o.push_str(&render_set(
        &entries, jud, nodes, labels, dep, root, page, "table", states,
    )?);
    Ok(o)
}
fn render_tree(
    ids: &BTreeSet<String>,
    jud: &BTreeSet<String>,
    nodes: &Map,
    dep: &str,
    labels: &Map,
) -> String {
    let n = ids.len().max(1);
    let mut pos = BTreeMap::new();
    for (i, id) in ids.iter().enumerate() {
        let a = std::f64::consts::TAU * i as f64 / n as f64;
        pos.insert(id, (480.0 + 310.0 * a.cos(), 350.0 + 250.0 * a.sin()));
    }
    let mut o = "<svg viewBox=\"0 0 960 700\" role=\"img\">".to_string();
    for id in jud {
        for d in deps(body(nodes, id).unwrap(), dep) {
            if let (Some(&(x1, y1)), Some(&(x2, y2))) = (pos.get(&d), pos.get(id)) {
                o.push_str(&format!("<path class=\"tlimb\" data-lf=\"{}\" data-lt=\"{}\" d=\"M{x1:.1} {y1:.1} C{x1:.1} {:.1} {x2:.1} {:.1} {x2:.1} {y2:.1}\"/>",esc(&d,true),esc(id,true),(y1+y2)/2.0,(y1+y2)/2.0));
            }
        }
    }
    for id in ids {
        let (x, y) = pos[id];
        let cls = if jud.contains(id) { "crown" } else { "root" };
        let lbl = body(nodes, id)
            .ok()
            .map(|b| label(id, b, labels))
            .unwrap_or_else(|| id.clone());
        o.push_str(&format!("<g class=\"tn {cls}\" data-id=\"{}\"><circle cx=\"{x:.1}\" cy=\"{y:.1}\" r=\"15\"/><text x=\"{x:.1}\" y=\"{:.1}\" text-anchor=\"middle\">{}</text></g>",esc(id,true),y+34.0,esc(&lbl.chars().take(18).collect::<String>(),false)));
    }
    o + "</svg>"
}
fn words(lang: &str, dir: &str) -> J {
    let he = lang == "he";
    json!({"dir":dir,"untitled":if he{"מה ידוע כאן"}else{"What is known here"},"concludes":if he{"מסקנה"}else{"concludes"},"rests_on":if he{"נשען על"}else{"rests on"},"wrong_if":if he{"שגוי אם"}else{"wrong if"},"blocked":"blocked","reopened_by":"reopened by","because":"because","value":"value","rule":"rule","measure":"measure","source":"from","at":"at","file":"file","url":"url","as_of":"as of","used_by":"used by","back":if he{"→"}else{"←"},"tree_btn":"tree","tree_btn_title":"prune the tree to what this touches","whole_tree":if he{"כל העץ ⟶"}else{"⟵ the whole tree"},"asked":"asked","today":"today","days_left_one":"1 day left","days_ago_one":"1 day ago","days_left":"{n} days left","days_ago":"{n} days ago"})
}
