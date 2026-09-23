// Shared layout interpretation for page rendering and read-only previews.
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

fn body<'a>(nodes: &'a Map, id: &str) -> Result<&'a Map> {
    map(&map(&nodes[id])?["body"])
}
/// The assessment keeps each body as written. The page reads a body that is not a
/// mapping - an open question written as a bare string - as the value `{v: body}`,
/// the way the Python page does.
fn page_nodes(nodes: &Map) -> Result<Map> {
    nodes
        .iter()
        .map(|(id, node)| {
            let mut node = map(node)?.clone();
            if let Some(body) = node.get_mut("body")
                && !matches!(body, V::Map(_))
            {
                let value = std::mem::replace(body, V::Null);
                *body = V::Map(Map::from([("v".into(), value)]));
            }
            Ok((id.clone(), V::Map(node)))
        })
        .collect()
}
fn textish(v: &V) -> String {
    if matches!(v, V::Text(_)) {
        text(v).unwrap_or("").into()
    } else {
        py(v)
    }
}

fn parse_date(value: &str) -> Option<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d").ok()
}
fn intent_date(id: &str, body: Option<&Map>) -> Option<chrono::NaiveDate> {
    let body = body?;
    for field in ["read", "of"] {
        if let Some(day) = body
            .get(field)
            .and_then(|value| parse_date(&textish(value)))
        {
            return Some(day);
        }
    }
    let pattern = regex::Regex::new(r"(\d{4})_(\d{2})_(\d{2})").unwrap();
    let found = pattern.captures(id)?;
    parse_date(&crate::history_yaml::numeric_text(&format!(
        "{}-{}-{}",
        &found[1], &found[2], &found[3]
    )))
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
    // Without a brief there is no session tab, and the page opens on Record.
    let Some(content) = content else {
        return Ok((Map::new(), vec![]));
    };
    let brief = decode_brief(content)?;
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
