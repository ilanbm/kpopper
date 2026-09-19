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
    reasoning_runtime::Runtime,
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
    pub notes: Vec<String>,
}

#[derive(Clone)]
struct Tab {
    key: String,
    title: String,
    occasion: String,
    intent: String,
    serves: Vec<String>,
    bare: bool,
    shape: Map,
    sections: Vec<Section>,
}
#[derive(Clone)]
struct Section {
    title: String,
    why_: String,
    text: String,
    pick: Vec<String>,
    kind: String,
    by: String,
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
fn entry_date(body: &Map) -> Option<chrono::NaiveDate> {
    let value = field(body, &["v", "quoted"])?;
    let value = textish(value);
    let value = value.trim();
    for format in ["%Y-%m-%d", "%d/%m/%Y", "%d.%m.%Y"] {
        if let Ok(date) = chrono::NaiveDate::parse_from_str(value, format) {
            return Some(date);
        }
    }
    None
}
fn parse_date(value: &str) -> Option<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d").ok()
}
fn intent_date(body: Option<&Map>) -> Option<chrono::NaiveDate> {
    let body = body?;
    field(body, &["read", "of"])
        .map(textish)
        .and_then(|value| parse_date(&value))
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
type Groups = BTreeMap<String, BTreeMap<String, BTreeSet<String>>>;
fn groups(
    brief: &Map,
    ids: &BTreeSet<String>,
    judgments: &BTreeSet<String>,
    states: &BTreeMap<String, BTreeSet<String>>,
) -> Groups {
    let Some(declaration) = brief
        .get("groups")
        .or_else(|| brief.get("fronts"))
        .and_then(|v| map(v).ok())
    else {
        return Groups::new();
    };
    let nested = !declaration.is_empty() && declaration.values().all(|v| matches!(v, V::Map(_)));
    let schemes = if nested {
        declaration.clone()
    } else {
        BTreeMap::from([("groups".into(), V::Map(declaration.clone()))])
    };
    schemes
        .into_iter()
        .filter_map(|(scheme, value)| {
            let declared = map(&value).ok()?;
            Some((
                scheme,
                declared
                    .iter()
                    .map(|(name, sels)| {
                        let mut members = BTreeSet::new();
                        for selector in selectors(Some(sels)) {
                            members.extend(select(&selector, ids, judgments, states));
                        }
                        (name.clone(), members)
                    })
                    .collect(),
            ))
        })
        .collect()
}
fn groups_of(id: &str, by: &str, schemes: &Groups, nodes: &Map) -> Vec<String> {
    let scheme = if by.is_empty() {
        schemes
            .keys()
            .next()
            .map(String::as_str)
            .unwrap_or("prefix")
    } else {
        by
    };
    if let Some(groups) = schemes.get(scheme) {
        return groups
            .iter()
            .filter(|(_, members)| members.contains(id))
            .map(|(name, _)| name.clone())
            .collect();
    }
    if scheme == "prefix" {
        return id
            .split_once('.')
            .map(|(p, _)| vec![p.into()])
            .unwrap_or_default();
    }
    body(nodes, id)
        .ok()
        .and_then(|b| b.get(scheme))
        .filter(|v| !matches!(v, V::Map(_) | V::List(_)))
        .map(|v| vec![textish(v)])
        .unwrap_or_default()
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
            occasion: String::new(),
            intent: brief.get("intent").map(textish).unwrap_or_default(),
            serves: vec![],
            bare: true,
            shape: brief
                .get("shape")
                .and_then(|v| map(v).ok())
                .cloned()
                .unwrap_or_default(),
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
            occasion: t.get("occasion").map(textish).unwrap_or_default(),
            intent: String::new(),
            serves: selectors(t.get("serves")),
            bare: false,
            shape: t
                .get("shape")
                .and_then(|v| map(v).ok())
                .cloned()
                .unwrap_or_default(),
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
            by: s.get("by").map(textish).unwrap_or_default(),
        })
        .collect()
}
fn json_value(v: &V) -> Result<J> {
    serde_json::from_str(&crate::ordinary_assessment_report::legacy_json(v)?)
        .map_err(|e| crate::Error(format!("ordinary Hub JSON projection: {e}")))
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
fn js_safe(v: &J) -> Result<String> {
    Ok(serde_json::to_string(v)?.replace('<', "\\u003c"))
}

pub fn build(
    capture: &CapturedSource,
    assessment: &V,
    brief_content: Option<&[u8]>,
    entry: &Path,
    page_path: &Path,
    max: usize,
    runtime: Option<&Runtime>,
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
        .map(|id| Ok((id.clone(), flags(map(&nodes[id])?, judgments.contains(id)))))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let (brief, tabs) = parse_brief(brief_content)?;
    let context = capture.ordinary_context();
    let context = map(&context)?;
    let empty_conflicts = V::Map(Map::new());
    let conflicts = map(context.get("conflicts").unwrap_or(&empty_conflicts))?;
    let projection = crate::public_ordinary_readers::Projection::new(
        &document,
        map(capture.hypotheses())?,
        conflicts,
        capture.reader_lines()?,
        runtime,
    )?;
    let hub = projection.hub_data()?;
    let group_schemes = groups(&brief, &ids, &judgments, &states);
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
                    e.insert(k.into(), json_value(v)?);
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
    let mut notes = vec![];
    let mut panels = vec![];
    let mut selected_all = BTreeSet::new();
    let picked_by_tab = tabs
        .iter()
        .map(|tab| {
            tab.sections
                .iter()
                .flat_map(|sec| sec.pick.iter())
                .flat_map(|selector| select(selector, &ids, &judgments, &states))
                .collect::<BTreeSet<_>>()
        })
        .collect::<Vec<_>>();
    selected_all.extend(
        picked_by_tab
            .iter()
            .flat_map(|picked| picked.iter().cloned()),
    );
    let recorded_by = ids
        .iter()
        .filter(|id| body(nodes, id).is_ok_and(|b| b.get("asked").is_some_and(truth)))
        .map(|intent| {
            let recorded = ids
                .iter()
                .filter(|id| {
                    body(nodes, id).is_ok_and(|b| {
                        b.get("from").and_then(|v| text(v).ok()) == Some(intent.as_str())
                            || judgments.contains(*id) && deps(b, dep_field).contains(intent)
                    })
                })
                .cloned()
                .collect::<BTreeSet<_>>();
            (intent.clone(), recorded)
        })
        .collect::<BTreeMap<_, _>>();
    let current_shape = BTreeMap::from([
        ("entries", ids.len() - judgments.len()),
        ("judgments", judgments.len()),
        ("flagged", states.values().filter(|f| !f.is_empty()).count()),
        (
            "blocked",
            ids.iter()
                .filter(|id| {
                    body(nodes, id).is_ok_and(|b| {
                        field(b, &["blocked_on", "blocked", "waiting_for"]).is_some()
                    })
                })
                .count(),
        ),
    ]);
    let earned_by_tab = tabs
        .iter()
        .enumerate()
        .map(|(index, tab)| {
            tab.serves
                .iter()
                .filter(|intent| {
                    recorded_by
                        .get(*intent)
                        .is_some_and(|recorded| !recorded.is_disjoint(&picked_by_tab[index]))
                })
                .cloned()
                .collect::<BTreeSet<_>>()
        })
        .collect::<Vec<_>>();
    let arrangement_owns = hub
        .arrangements
        .iter()
        .map(|arrangement| {
            tabs.iter()
                .enumerate()
                .filter(|(index, tab)| {
                    tabs.len() == 1 && tab.bare
                        || arrangement
                            .sources
                            .iter()
                            .any(|source| earned_by_tab[*index].contains(source))
                })
                .map(|(index, _)| index)
                .collect::<BTreeSet<_>>()
        })
        .collect::<Vec<_>>();
    for left in 0..hub.arrangements.len() {
        for right in left + 1..hub.arrangements.len() {
            if !arrangement_owns[left].is_disjoint(&arrangement_owns[right])
                && hub.arrangements[left]
                    .sources
                    .iter()
                    .all(|source| !hub.arrangements[right].sources.contains(source))
            {
                let tab = *arrangement_owns[left]
                    .intersection(&arrangement_owns[right])
                    .next()
                    .unwrap();
                failures.push(format!(
                    "the brief reads {} and {} on one tab ('{}'), which neither decided",
                    hub.arrangements[left].id,
                    hub.arrangements[right].id,
                    if tabs[tab].title.is_empty() {
                        "Now"
                    } else {
                        &tabs[tab].title
                    }
                ));
            }
        }
    }
    for (index, arrangement) in hub.arrangements.iter().enumerate() {
        let unearned = arrangement
            .sources
            .iter()
            .filter(|source| {
                !arrangement_owns[index]
                    .iter()
                    .any(|tab| earned_by_tab[*tab].contains(*source))
            })
            .cloned()
            .collect::<Vec<_>>();
        if !(tabs.len() == 1 && tabs[0].bare) && !unearned.is_empty() {
            failures.push(format!(
                "the brief does not serve {} together, as {} decided (no tab's sections earn {})",
                arrangement.sources.join(", "),
                arrangement.id,
                unearned.join(", ")
            ));
        }
        if arrangement.fired {
            failures.push(format!(
                "{}: wrong_if holds ({}) - the arrangement fired",
                arrangement.id, arrangement.predicate
            ));
        }
    }
    if !brief.is_empty() && hub.arrangements.is_empty() && !recorded_by.is_empty() {
        notes.push("no arrangement decision is recorded, so the brief is held against none".into());
    }
    for (index, tab) in tabs.iter().enumerate() {
        if !tab.bare
            && !earned_by_tab[index].is_empty()
            && !arrangement_owns.iter().any(|owned| owned.contains(&index))
        {
            notes.push(format!(
                "tab '{}' is an arrangement no decision records",
                tab.title
            ));
        }
    }
    for (tab_index, tab) in tabs.iter().enumerate() {
        let mut panel = String::new();
        panel.push_str(&heading(
            &meta,
            &brief,
            &words,
            ids.len() - judgments.len(),
            judgments.len(),
        ));
        if tab.bare {
            if !tab.intent.is_empty() {
                panel.push_str(&format!("<p class=\"purpose\" dir=\"auto\">Everything in this tab was chosen for one purpose — <b dir=\"auto\">{}</b></p>",esc(&tab.intent,false)));
            } else {
                panel.push_str(&format!("<p class=\"sub\" dir=\"{dir}\">The Record tab has all {} entries and judgments, arranged by nothing.</p>",ids.len()));
            }
        } else {
            panel.push_str(&format!("<p class=\"purpose\" dir=\"auto\">This tab is for one occasion — <b dir=\"auto\">{}</b></p><p class=\"sub\">The Record tab has all {} entries and judgments, arranged by nothing.</p>",esc(if tab.occasion.is_empty(){&tab.title}else{&tab.occasion},false),ids.len()));
        }
        for line in &hub.reader_lines {
            panel.push_str(&format!(
                "<p class=\"meta\" dir=\"auto\" lang=\"en\">{}</p>",
                linked_text(line, &ids, &labels, nodes, true)
            ));
        }
        let mut tab_drift = Vec::new();
        for (arrangement_index, arrangement) in hub.arrangements.iter().enumerate() {
            let owns =
                arrangement_owns[arrangement_index].contains(&tab_index) || tab.serves.is_empty();
            if !owns {
                continue;
            }
            let decided = arrangement.born.as_deref().unwrap_or(&arrangement.id);
            let born = arrangement.born.as_deref().and_then(parse_date);
            let stood = recorded_by
                .keys()
                .filter(|intent| {
                    intent_date(body(nodes, intent).ok())
                        .is_some_and(|date| born.is_some_and(|born| date > born))
                        && arrangement_owns[arrangement_index]
                            .iter()
                            .any(|index| earned_by_tab[*index].contains(*intent))
                })
                .count();
            let added = recorded_by
                .iter()
                .filter(|(intent, _)| {
                    intent_date(body(nodes, intent).ok())
                        .is_some_and(|date| born.is_some_and(|born| date >= born))
                })
                .flat_map(|(_, recorded)| recorded.iter().cloned())
                .collect::<BTreeSet<_>>();
            let drift = (!added.is_empty()).then(|| {
                ((added.difference(&selected_all).count() as f64 / added.len() as f64) * 100.0)
                    .round()
                    / 100.0
            });
            tab_drift.push((arrangement.id.clone(), drift));
            panel.push_str(&format!("<p class=\"sub\" dir=\"auto\">decided <span class=\"fx\" data-id=\"{}\">{}</span>{}{}</p>",esc(&arrangement.id,true),esc(decided,false),if stood>0{format!(" &middot; stood {stood} session{}",if stood==1{""}else{"s"})}else{String::new()},arrangement.request.as_ref().map(|request|{let asked=body(nodes,request).ok().and_then(|b|b.get("asked")).map(textish).unwrap_or_else(||request.clone());format!(" &middot; on the word of <span class=\"fx\" data-id=\"{}\" data-request=\"{}\">{}</span>",esc(request,true),esc(request,true),esc(&asked,false))}).unwrap_or_default()));
            if arrangement.fired {
                panel.push_str(&format!(
                    "<p class=\"sub\" data-warning=\"true\">{}: wrong_if holds ({})</p>",
                    esc(&arrangement.id, false),
                    esc(&arrangement.predicate, false)
                ));
            }
            for (who, claim) in &arrangement.contested {
                panel.push_str(&format!(
                    "<p class=\"sub\">{} is contested by {}: {}</p>",
                    esc(&arrangement.id, false),
                    esc(who, false),
                    esc(claim, false)
                ));
            }
            for (dependency, old, now, state) in &arrangement.moved {
                panel.push_str(&format!(
                    "<p class=\"sub\">{} moved since review: {} {} &rarr; {} ({})</p>",
                    esc(&arrangement.id, false),
                    esc(dependency, false),
                    esc(&textish(old), false),
                    esc(&textish(now), false),
                    state
                ));
            }
        }
        if !tab.shape.is_empty() {
            let moved = tab
                .shape
                .iter()
                .filter_map(|(key, old)| {
                    current_shape
                        .get(key.as_str())
                        .filter(|now| textish(old) != now.to_string())
                        .map(|now| format!("{key}: {} -> {now}", textish(old)))
                })
                .collect::<Vec<_>>();
            if !moved.is_empty() {
                let message = format!(
                    "tab '{}': shape moved ({})",
                    if tab.title.is_empty() {
                        "Now"
                    } else {
                        &tab.title
                    },
                    moved.join("; ")
                );
                failures.push(message.clone());
                let drift = tab_drift
                    .iter()
                    .filter_map(|(id, value)| {
                        value.map(|value| format!("; since {id} was decided {value}"))
                    })
                    .collect::<String>();
                panel.push_str(&format!(
                    "<div class=\"banner\">{}{}</div>",
                    esc(&message, false),
                    esc(&drift, false)
                ));
            }
        }
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
            let entry_ids = chosen.difference(&judgments).cloned().collect::<Vec<_>>();
            match kind {
                "timeline" => {
                    let bad = entry_ids
                        .iter()
                        .filter(|id| body(nodes, id).ok().and_then(entry_date).is_none())
                        .collect::<Vec<_>>();
                    if !bad.is_empty() {
                        failures.push(format!(
                            "section '{title}': timeline needs date values; {} of {} are not dates",
                            bad.len(),
                            chosen.len()
                        ));
                    }
                }
                "headline" if !(1..=4).contains(&entry_ids.len()) => failures.push(format!(
                    "section '{title}': headline carries one to four values, not {}",
                    entry_ids.len()
                )),
                "alerts" if !entry_ids.is_empty() => failures.push(format!(
                    "section '{title}': alerts ranks judgments; {} of these are entries",
                    entry_ids.len()
                )),
                "links" => {
                    let bad = entry_ids
                        .iter()
                        .filter(|id| {
                            body(nodes, id)
                                .ok()
                                .and_then(|body| href(body, root_of(entry), page_path))
                                .is_none()
                        })
                        .collect::<Vec<_>>();
                    if !bad.is_empty() {
                        failures.push(format!(
                            "section '{title}': links needs a safe url or file: {}",
                            bad.into_iter()
                                .map(|s| s.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                }
                "grouped" | "fronts" => {
                    let found = chosen
                        .iter()
                        .flat_map(|id| groups_of(id, &sec.by, &group_schemes, nodes))
                        .collect::<BTreeSet<_>>();
                    if found.len() < 2 {
                        failures.push(format!("section '{title}': grouped lays groups side by side; these are all one group ({})",found.into_iter().collect::<Vec<_>>().join(", ")));
                    }
                }
                "axis" if sec.text.trim().is_empty() => failures.push(format!(
                    "section '{title}': axis needs a written sequence in text"
                )),
                _ => {}
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
                let content = if kind == "axis" {
                    sec.text
                        .lines()
                        .filter(|line| !line.trim().is_empty())
                        .map(|line| format!("<div class=\"axis-step\">{}</div>", esc(line, false)))
                        .collect::<String>()
                } else {
                    linked_text(&sec.text, &ids, &labels, nodes, false)
                };
                panel.push_str(&format!("<div data-prose=\"connective\" data-review=\"current\"><div class=\"txt\" dir=\"auto\"><div class=\"{}\">{content}</div></div></div>",if kind=="axis"{"axis"}else{"prose"}));
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
                &group_schemes,
                &sec.by,
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
                &group_schemes,
                "",
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
        &group_schemes,
    )?;
    let tree = render_tree(&ids, &judgments, nodes, dep_field, &labels)?;
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
    out.push_str(&js_safe(&words)?);
    out.push_str(";window.__E=");
    out.push_str(&js_safe(&J::Object(entries))?);
    out.push_str(";window.__J=");
    out.push_str(&js_safe(&J::Object(decisions))?);
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
        notes,
    })
}
fn root_of(entry: &Path) -> &Path {
    entry.parent().unwrap_or_else(|| Path::new("."))
}
fn linked_text(
    value: &str,
    ids: &BTreeSet<String>,
    labels: &Map,
    nodes: &Map,
    axis: bool,
) -> String {
    let re = regex::Regex::new(r"\{\{\s*([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+)\s*\}\}|([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+)").unwrap();
    let mut out = String::new();
    let mut at = 0;
    for captures in re.captures_iter(value) {
        let m = captures.get(0).unwrap();
        let id = captures
            .get(1)
            .or_else(|| captures.get(2))
            .unwrap()
            .as_str();
        if !ids.contains(id) {
            continue;
        }
        out.push_str(&esc(&value[at..m.start()], false));
        if captures.get(1).is_some() {
            let b = body(nodes, id).ok();
            let mut shown = b
                .and_then(|b| field(b, &["v", "quoted", "because", "verdict", "title"]))
                .map(textish)
                .unwrap_or_else(|| b.map(|b| label(id, b, labels)).unwrap_or_else(|| id.into()));
            if axis && shown.parse::<f64>().is_ok_and(|n| n > 0.0) {
                shown = format!("+{shown}")
            }
            out.push_str(&format!(
                "<span class=\"fx in{}\" data-id=\"{}\">{}</span>",
                if axis { " signed" } else { "" },
                esc(id, true),
                esc(&shown, false)
            ));
        } else {
            let shown = if axis {
                id.into()
            } else {
                body(nodes, id)
                    .ok()
                    .map(|b| label(id, b, labels))
                    .unwrap_or_else(|| id.into())
            };
            out.push_str(&esc(&shown, false));
        }
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
    schemes: &Groups,
    by: &str,
) -> Result<String> {
    let mut o = String::new();
    let judgment_ids = ids.intersection(jud).cloned().collect::<Vec<_>>();
    let entries = ids.difference(jud).cloned().collect::<Vec<_>>();
    if !judgment_ids.is_empty() {
        o.push_str(if kind == "alerts" {
            "<div class=\"alerts\">"
        } else {
            "<div class=\"cards\">"
        });
        for id in &judgment_ids {
            let b = body(nodes, id)?;
            let verdict = field(b, &["verdict", "title"])
                .map(textish)
                .unwrap_or_else(|| id.clone());
            if kind == "alerts" {
                let tone = if states[id].contains("falsified") || states[id].contains("broken") {
                    "stop"
                } else if states[id].is_empty() {
                    "ok"
                } else {
                    "warn"
                };
                let status = if states[id].is_empty() {
                    "holds".into()
                } else {
                    states[id].iter().cloned().collect::<Vec<_>>().join(", ")
                };
                let group = deps(b, _dep)
                    .into_iter()
                    .flat_map(|dependency| groups_of(&dependency, by, schemes, nodes))
                    .next();
                o.push_str(&format!("<div class=\"al\"><span class=\"ico {tone}\">{}</span><span class=\"at\"><span class=\"fx\" data-id=\"{}\">{}</span><div class=\"aw\">{}</div></span><span class=\"tag {tone}\">judgment</span>{}</div>",if tone=="stop"{"!"}else if tone=="warn"{"△"}else{"✓"},esc(id,true),esc(&verdict,false),esc(&status,false),group.map(|g|format!("<span class=\"grp\"><i class=\"group-dot\"></i>{}</span>",esc(&g,false))).unwrap_or_default()));
            } else {
                let rest = deps(b, _dep).len();
                o.push_str(&format!("<div class=\"card\" data-judgment=\"{}\" data-review=\"{}\"><div class=\"cardtop\"><span class=\"judgment-label\">judgment</span></div><div class=\"vd fx\" data-id=\"{}\">{}</div>{}</div>",esc(id,true),if states[id].contains("moved"){"moved"}else{"current"},esc(id,true),esc(&verdict,false),if rest>0{format!("<div class=\"rest\">rests on {rest} more — hover the line above</div>")}else{String::new()}));
            }
        }
        o.push_str("</div>");
    }
    if entries.is_empty() {
        return Ok(o);
    }
    if kind == "lines" {
        o.push_str("<div class=\"deps\">");
        for id in entries {
            o.push_str(&format!(
                "<span class=\"fx dep\" data-id=\"{}\" dir=\"auto\">{}</span>",
                esc(&id, true),
                esc(&label(&id, body(nodes, &id)?, labels), false)
            ));
        }
        o.push_str("</div>");
        return Ok(o);
    }
    if kind == "timeline" {
        let today = chrono::Local::now().date_naive();
        let mut rows = entries
            .into_iter()
            .filter_map(|id| entry_date(body(nodes, &id).ok()?).map(|d| (d, id)))
            .collect::<Vec<_>>();
        rows.sort();
        o.push_str("<div class=\"tl\">");
        for (date, id) in rows {
            let hot = if date == today {
                " hot"
            } else if date < today {
                " past"
            } else {
                ""
            };
            o.push_str(&format!("<div class=\"day{hot}\" data-day=\"{date}\"><div class=\"when\"><span class=\"fx\" data-id=\"{}\">{}</span></div><div class=\"day-label\">{}</div><div class=\"day-item\"><span class=\"fx\" data-id=\"{}\">{}</span></div></div>",esc(&id,true),date.format("%d/%m/%Y"),if date==today{"today"}else{""},esc(&id,true),esc(&label(&id,body(nodes,&id)?,labels),false)));
        }
        o.push_str("</div>");
        return Ok(o);
    }
    if kind == "headline" {
        o.push_str("<div class=\"heads\">");
        for id in entries {
            let b = body(nodes, &id)?;
            let value = field(b, &["v", "quoted"]).map(textish).unwrap_or_default();
            let date = entry_date(b);
            o.push_str(&format!("<div class=\"head\"><div class=\"big\"{}><span class=\"fx\" data-id=\"{}\">{}</span></div><div class=\"cap\" dir=\"auto\">{}</div>{}</div>",date.map(|d|format!(" data-countdown=\"{d}\"")).unwrap_or_default(),esc(&id,true),esc(&value,false),esc(&label(&id,b,labels),false),date.map(|d|format!("<div class=\"nt\"><span class=\"fx\" data-id=\"{}\">{d}</span></div>",esc(&id,true))).unwrap_or_default()));
        }
        o.push_str("</div>");
        return Ok(o);
    }
    if matches!(kind, "grouped" | "fronts") {
        let mut grouped = BTreeMap::<String, Vec<String>>::new();
        for id in entries {
            let names = groups_of(&id, by, schemes, nodes);
            for name in if names.is_empty() {
                vec![id.split('.').next().unwrap_or("-").into()]
            } else {
                names
            } {
                grouped.entry(name).or_default().push(id.clone())
            }
        }
        o.push_str("<div class=\"grid\">");
        for (name, members) in grouped {
            o.push_str(&format!(
                "<div class=\"group\"><h3 dir=\"auto\">{}</h3>",
                esc(&name, false)
            ));
            for id in members {
                let b = body(nodes, &id)?;
                let value = field(b, &["v", "quoted", "rule"])
                    .map(textish)
                    .unwrap_or_else(|| "derived".into());
                o.push_str(&format!("<div class=\"kv fx\" data-id=\"{}\"><span class=\"kl\">{}</span><span class=\"kvv\">{}</span></div>",esc(&id,true),esc(&label(&id,b,labels),false),esc(&value,false)));
            }
            o.push_str("</div>");
        }
        o.push_str("</div>");
        return Ok(o);
    }
    if kind == "links" {
        o.push_str("<div class=\"links\">");
        for id in entries {
            let b = body(nodes, &id)?;
            if let Some(target) = href(b, root, page) {
                o.push_str(&format!(
                    "<a class=\"lk\" href=\"{}\"><span class=\"fx\" data-id=\"{}\">{}</span></a>",
                    esc(&target, true),
                    esc(&id, true),
                    esc(&label(&id, b, labels), false)
                ));
            }
        }
        o.push_str("</div>");
        return Ok(o);
    }
    {
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
    schemes: &Groups,
) -> Result<String> {
    let mut o = String::new();
    if !jud.is_empty() {
        o.push_str(&format!(
            "<h2>Judgments <span class=\"n\">{}</span></h2>",
            jud.len()
        ));
        o.push_str(&render_set(
            jud, jud, nodes, labels, dep, root, page, "cards", states, schemes, "",
        )?);
    }
    let entries = ids.difference(jud).cloned().collect();
    o.push_str(&render_set(
        &entries, jud, nodes, labels, dep, root, page, "table", states, schemes, "",
    )?);
    Ok(o)
}
fn render_tree(
    ids: &BTreeSet<String>,
    jud: &BTreeSet<String>,
    nodes: &Map,
    dep: &str,
    labels: &Map,
) -> Result<String> {
    let n = ids.len().max(1);
    let mut pos = BTreeMap::new();
    for (i, id) in ids.iter().enumerate() {
        let a = std::f64::consts::TAU * i as f64 / n as f64;
        pos.insert(id, (480.0 + 310.0 * a.cos(), 350.0 + 250.0 * a.sin()));
    }
    let mut o = "<svg viewBox=\"0 0 960 700\" role=\"img\">".to_string();
    for id in jud {
        for d in deps(body(nodes, id)?, dep) {
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
    Ok(o + "</svg>")
}
fn words(lang: &str, dir: &str) -> J {
    let he = lang == "he";
    let ar = lang == "ar";
    json!({"dir":dir,"untitled":if he{"מה ידוע כאן"}else if ar{"ما المعروف هنا"}else{"What is known here"},"concludes":if he{"מסקנה"}else if ar{"يستنتج"}else{"concludes"},"rests_on":if he{"נשען על"}else if ar{"يستند إلى"}else{"rests on"},"wrong_if":if he{"שגוי אם"}else if ar{"خطأ إذا"}else{"wrong if"},"blocked":"blocked","reopened_by":"reopened by","because":"because","value":"value","rule":"rule","measure":"measure","source":"from","at":"at","file":"file","url":"url","as_of":"as of","used_by":"used by","back":if he||ar{"→"}else{"←"},"tree_btn":"tree","tree_btn_title":"prune the tree to what this touches","whole_tree":if he{"כל העץ ⟶"}else if ar{"الشجرة كاملة ⟶"}else{"⟵ the whole tree"},"asked":"asked","today":if he{"היום"}else if ar{"اليوم"}else{"today"},"days_left_one":if he{"נותר יום אחד"}else if ar{"بقي يوم واحد"}else{"1 day left"},"days_ago_one":if he{"לפני יום אחד"}else if ar{"منذ يوم واحد"}else{"1 day ago"},"days_left_two":if he{"נותרו יומיים"}else if ar{"بقي يومان"}else{"{n} days left"},"days_left_many":if he{"נותרו {n} ימים"}else if ar{"بقي {n} يوماً"}else{"{n} days left"},"days_left_other":if he{"נותרו {n} ימים"}else if ar{"بقي {n} يوم"}else{"{n} days left"},"days_ago_two":if he{"לפני יומיים"}else if ar{"منذ يومين"}else{"{n} days ago"},"days_ago_many":if he{"לפני {n} ימים"}else if ar{"منذ {n} يوماً"}else{"{n} days ago"},"days_ago_other":if he{"לפני {n} ימים"}else if ar{"منذ {n} يوم"}else{"{n} days ago"},"days_left":"{n} days left","days_ago":"{n} days ago"})
}
