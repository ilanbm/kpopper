//! Native ordinary-record Hub projection.
//!
//! The ordinary assessment is supplied by `ordinary_assessment_report`; this module
//! only selects and presents that retained result.  The browser behavior and styles
//! remain the long-lived page assets shared with the Python implementation.
use crate::source_text::ordinary_python_str as py;
use crate::{
    Result,
    history_contract::{Map, map, text},
    history_view::{list, truth},
    reasoning_authoring::named,
    reasoning_runtime::Runtime,
    source_capture::CapturedSource,
    value::TypedValue as V,
};
use serde_json::{Map as JsonMap, Value as J, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};

use crate::public_ordinary_readers::{HubData, Projection};
include!("ordinary_hub_page_inputs.rs");
include!("ordinary_hub_page_projection.rs");
fn decode_brief(raw: &[u8]) -> Result<V> {
    Ok(crate::history_yaml::decode_source_value(raw)?.typed())
}

const CSS: &str = include_str!("../shared/page/page.css");
const COMPONENT_CSS: &str = include_str!("../shared/page/components.css");
const JS: &str = include_str!("../shared/page/page.js");
const DATE_JS: &str = include_str!("../shared/page/dates.js");

#[derive(Debug)]
pub struct Page {
    pub html: String,
    pub elements: usize,
    pub entries: usize,
    pub judgments: usize,
    pub tabs: usize,
    pub failures: Vec<String>,
    pub notes: Vec<String>,
    /// Captured arrangement facts consumed by guarded authoring. This is
    /// derived from the same record assessment and brief bytes rendered here.
    pub arrangement_facts: V,
    /// Typed page readings; an undefined reading is Null, not a guessed zero.
    pub page_values: Map,
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
fn field<'a>(m: &'a Map, keys: &[&str]) -> Option<&'a V> {
    keys.iter().find_map(|k| m.get(*k).filter(|v| truth(v)))
}
fn label(id: &str, body: &Map, labels: &Map) -> String {
    labels
        .get(id)
        .map(textish)
        .filter(|s| !s.is_empty())
        .or_else(|| Some(named(&V::Map(body.clone()))).filter(|s| !s.is_empty()))
        .unwrap_or_else(|| id.split('.').next_back().unwrap_or(id).replace('_', " "))
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
fn json_value(v: &V) -> Result<J> {
    crate::json_ingress::parse_str(
        &crate::ordinary_assessment_report::legacy_json(v)?,
        crate::json_ingress::DuplicateKeys::LastWins,
    )
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
    if let Some((scheme, tail)) = raw.split_once(':')
        && scheme
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
    let relative = pathdiff(&source, parent);
    let url = relative
        .iter()
        .map(|part| part.to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    Some(url_path(&url))
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
    build_document(
        PageDocument {
            capture,
            document: capture.ordinary_document(),
        },
        assessment,
        brief_content,
        entry,
        page_path,
        max,
        runtime,
    )
}

struct PageDocument<'a> {
    capture: &'a CapturedSource,
    document: &'a V,
}

/// Calculate page facts from staged values without manufacturing a new source capture.
pub(crate) fn arrangement_projection_for_document(
    capture: &CapturedSource,
    document: &V,
    brief: &[u8],
    entry: &Path,
    page_path: &Path,
    runtime: Option<&Runtime>,
) -> Result<(V, Map)> {
    let assessment = crate::ordinary_assessment_report::assess(
        document,
        map(capture.hypotheses())?,
        &capture.ordinary_context(),
        runtime,
        crate::ordinary_assessment::POLICY,
    )?;
    let page = build_document(
        PageDocument { capture, document },
        &assessment,
        Some(brief),
        entry,
        page_path,
        16 * 1024 * 1024,
        runtime,
    )?;
    Ok((page.arrangement_facts, page.page_values))
}

fn build_document(
    input: PageDocument<'_>,
    assessment: &V,
    brief_content: Option<&[u8]>,
    entry: &Path,
    page_path: &Path,
    max: usize,
    runtime: Option<&Runtime>,
) -> Result<Page> {
    let PageDocument { capture, document } = input;
    let doc = map(document)?;
    let report = map(assessment)?;
    let nodes = &page_nodes(map(&report["nodes"])?)?;
    let fields = crate::reasoning_fields::snapshot_fields(document)?;
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
    let (brief, tabs) = parse_brief(brief_content)?;
    let context = capture.ordinary_context();
    let context = map(&context)?;
    let empty_conflicts = V::Map(Map::new());
    let conflicts = map(context.get("conflicts").unwrap_or(&empty_conflicts))?;
    let mut projection = crate::public_ordinary_readers::Projection::new(
        document,
        map(capture.hypotheses())?,
        conflicts,
        capture.reader_lines()?,
        runtime,
    )?;
    let hub = projection.hub_data()?;
    let states = ids
        .iter()
        .map(|id| (id.clone(), hub.flags.get(id).cloned().unwrap_or_default()))
        .collect::<BTreeMap<_, _>>();
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
            if !e.contains_key("v")
                && let Some(v) = e.get("quoted").cloned()
            {
                e.insert("v".into(), v);
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
    let render = RenderContext {
        judgments: &judgments,
        nodes,
        labels: &labels,
        dependency_field: dep_field,
        record_root: root_of(entry),
        page_path,
        states: &states,
        groups: &group_schemes,
        language,
    };
    let mut failures = vec![];
    let mut notes = vec![];
    let mut panels = vec![];
    let PageProjection {
        mut selected_all,
        recorded_by,
        earned_by_tab,
        page_values,
        arrangement_owns,
        arrangement_facts,
        hub,
    } = page_projection(
        &brief,
        &tabs,
        &ids,
        &judgments,
        &states,
        nodes,
        dep_field,
        &mut projection,
    )?;
    let current_shape = BTreeMap::from([
        (
            "entries",
            ids.iter()
                .filter(|id| {
                    !judgments.contains(*id)
                        && !crate::reasoning_fields::BUILTINS.contains(&id.as_str())
                })
                .count(),
        ),
        ("judgments", judgments.len()),
        ("flagged", states.values().filter(|f| !f.is_empty()).count()),
        (
            "blocked",
            states
                .values()
                .filter(|flags| flags.contains("blocked"))
                .count(),
        ),
    ]);
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
                    intent_date(intent, body(nodes, intent).ok())
                        .is_some_and(|date| born.is_some_and(|born| date > born))
                        && arrangement_owns[arrangement_index]
                            .iter()
                            .any(|index| earned_by_tab[*index].contains(*intent))
                })
                .count();
            let added = recorded_by
                .iter()
                .filter(|(intent, _)| {
                    intent_date(intent, body(nodes, intent).ok())
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
                    "{} recorded a different shape: {}",
                    if tab.bare {
                        "the brief".to_owned()
                    } else {
                        format!(
                            "tab '{}'",
                            if tab.title.is_empty() {
                                "?"
                            } else {
                                &tab.title
                            }
                        )
                    },
                    moved.join("; ")
                );
                notes.push(message.clone());
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
                chosen.extend(select(s, &ids, &judgments, &states));
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
            // A section is judged by what its selectors pick together, as the Python page
            // judges it: one selector may name a prefix the record does not hold yet.
            if chosen.is_empty() && (!sec.pick.is_empty() || sec.text.is_empty()) {
                failures.push(format!(
                    "section {} picks nothing - it is about something the record no longer holds",
                    if tab.bare {
                        format!("'{title}'")
                    } else {
                        format!(
                            "'{title}' (tab '{}')",
                            if tab.title.is_empty() {
                                "?"
                            } else {
                                &tab.title
                            }
                        )
                    }
                ));
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
            panel.push_str(&render_set(&render, &chosen, kind, &sec.by)?);
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
            p.push_str(&render_set(&render, &spill, "alerts", "")?);
        }
    }
    let record = render_record(&render, &ids)?;
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
        tabs: tabs.len() + 1,
        failures,
        notes,
        arrangement_facts: V::Map(arrangement_facts),
        page_values,
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
struct RenderContext<'a> {
    judgments: &'a BTreeSet<String>,
    nodes: &'a Map,
    labels: &'a Map,
    dependency_field: &'a str,
    record_root: &'a Path,
    page_path: &'a Path,
    states: &'a BTreeMap<String, BTreeSet<String>>,
    groups: &'a Groups,
    language: &'a str,
}
/// What a card says about a judgment that needs a person, in the Python page's words:
/// its states, the most urgent first, then what is unverified about it.
fn state_line(flags: &BTreeSet<String>, body: &Map, language: &str) -> Option<String> {
    const URGENT_FIRST: [&str; 8] = [
        "broken",
        "falsified",
        "unchecked",
        "unknown",
        "reversed",
        "moved",
        "blocked",
        "no_predicate",
    ];
    let mut parts = URGENT_FIRST
        .iter()
        .filter(|flag| flags.contains(**flag))
        .filter_map(|flag| says(language, flag))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    parts.extend(
        flags
            .iter()
            .filter(|flag| says(language, flag).is_none())
            .cloned(),
    );
    if let Some(unverified) = body.get("unverified").filter(|v| truth(v)) {
        let word = match language {
            "he" => "לא אומת",
            "ar" => "غير متحقق",
            _ => "unverified",
        };
        parts.push(format!("{word}: {}", textish(unverified)));
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}
fn says(language: &str, flag: &str) -> Option<&'static str> {
    Some(match (language, flag) {
        ("he", "broken") => "נשען על משהו שאינו ברשומה הזו",
        ("he", "falsified") => "התנאי שהוא עצמו הציב לכך שהוא שגוי מתקיים עכשיו",
        ("he", "moved") => "משהו שהוא נשען עליו כבר לא מה שראה לאחרונה",
        ("he", "unchecked") => "מעולם לא נבדק מול אחד הדברים שהוא נשען עליהם",
        ("he", "blocked") => "ממתין למשהו שאיש עוד לא רשם",
        ("he", "no_predicate") => "שום דבר כאן לא יראה שהוא שגוי",
        ("he", "unknown") => "לא ניתן להכריע כרגע בתנאי שלו",
        ("he", "reversed") => "המסקנה שלו הוחלפה תחת המזהה הזה ואיש לא סקר אותה מאז",
        ("ar", "broken") => "يستند إلى شيء غير موجود في السجل",
        ("ar", "falsified") => "تحقق شرط خطئه",
        ("ar", "moved") => "تغير شيء يستند إليه منذ مراجعته",
        ("ar", "unchecked") => "لم يراجع مقابل بعض ما يستند إليه",
        ("ar", "blocked") => "ينتظر شيئاً لم يسجل بعد",
        ("ar", "no_predicate") => "لا يوجد هنا ما يبيّن خطأه",
        ("ar", "unknown") => "لا يمكن تقييم شرطه حاليًا",
        ("ar", "reversed") => "استُبدل حكمه تحت هذا المعرّف ولم يراجعه أحد منذ ذلك الحين",
        (_, "broken") => "rests on something that is not in this record",
        (_, "falsified") => "its own condition for being wrong now holds",
        (_, "moved") => "something it rests on no longer matches what it last saw",
        (_, "unchecked") => "has never been checked against one of the things it rests on",
        (_, "blocked") => "waiting on something nobody has recorded yet",
        (_, "no_predicate") => "nothing here would show it to be wrong",
        (_, "unknown") => "its condition cannot currently be evaluated",
        (_, "reversed") => {
            "its verdict was replaced under this id and nobody has reviewed it since"
        }
        _ => return None,
    })
}
fn render_set(
    context: &RenderContext<'_>,
    ids: &BTreeSet<String>,
    kind: &str,
    by: &str,
) -> Result<String> {
    let jud = context.judgments;
    let nodes = context.nodes;
    let labels = context.labels;
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
                let tone = if context.states[id].contains("falsified")
                    || context.states[id].contains("broken")
                {
                    "stop"
                } else if context.states[id].is_empty() {
                    "ok"
                } else {
                    "warn"
                };
                let status = if context.states[id].is_empty() {
                    "holds".into()
                } else {
                    context.states[id]
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let group = deps(b, context.dependency_field)
                    .into_iter()
                    .flat_map(|dependency| groups_of(&dependency, by, context.groups, nodes))
                    .next();
                o.push_str(&format!("<div class=\"al\"><span class=\"ico {tone}\">{}</span><span class=\"at\"><span class=\"fx\" data-id=\"{}\">{}</span><div class=\"aw\">{}</div></span><span class=\"tag {tone}\">judgment</span>{}</div>",if tone=="stop"{"!"}else if tone=="warn"{"△"}else{"✓"},esc(id,true),esc(&verdict,false),esc(&status,false),group.map(|g|format!("<span class=\"grp\"><i class=\"group-dot\"></i>{}</span>",esc(&g,false))).unwrap_or_default()));
            } else {
                let rest = deps(b, context.dependency_field).len();
                let state = state_line(&context.states[id], b, context.language)
                    .map(|state| {
                        format!(
                            "<div class=\"state\" data-warning=\"true\">{}</div>",
                            esc(&state, true)
                        )
                    })
                    .unwrap_or_default();
                o.push_str(&format!("<div class=\"card\" data-judgment=\"{}\" data-review=\"{}\"><div class=\"cardtop\"><span class=\"judgment-label\">judgment</span></div><div class=\"vd fx\" data-id=\"{}\">{}</div>{state}{}</div>",esc(id,true),if context.states[id].contains("moved"){"moved"}else{"current"},esc(id,true),esc(&verdict,false),if rest>0{format!("<div class=\"rest\">rests on {rest} more — hover the line above</div>")}else{String::new()}));
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
            let names = groups_of(&id, by, context.groups, nodes);
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
            if let Some(target) = href(b, context.record_root, context.page_path) {
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
            let name = if let Some(h) = href(b, context.record_root, context.page_path) {
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
fn render_record(context: &RenderContext<'_>, ids: &BTreeSet<String>) -> Result<String> {
    let jud = context.judgments;
    let mut o = String::new();
    if !jud.is_empty() {
        o.push_str(&format!(
            "<h2>Judgments <span class=\"n\">{}</span></h2>",
            jud.len()
        ));
        o.push_str(&render_set(context, jud, "cards", "")?);
    }
    let entries = ids.difference(jud).cloned().collect();
    o.push_str(&render_set(context, &entries, "table", "")?);
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
