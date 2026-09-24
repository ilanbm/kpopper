// Recorded prose is rendered with the same literal dependency anchors as the
// reference page. Ambiguous values never acquire an invented source.
const CARD_CHARS: usize = 400;
static PROSE_REFS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"\{\{\s*([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+)\s*\}\}").unwrap()
});
static PROSE_IDS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+").unwrap()
});
static PROSE_NUMBER: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"^-?\d+(?:\.\d+)?$").unwrap());
static PROSE_EXPRESSION: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"[<>=!+\-*/()]|\bor\b|\band\b|\bnot\b").unwrap()
});

fn clipped_reasoning(value: &str) -> String {
    if value.chars().count() <= CARD_CHARS {
        return value.into();
    }
    let mut cut = value.chars().take(CARD_CHARS - 1).collect::<String>();
    if cut.rfind("{{") > cut.rfind("}}") {
        cut.truncate(cut.rfind("{{").unwrap());
    }
    if let Some(space) = cut.rfind(' ')
        && cut[..space].chars().count() > CARD_CHARS / 2
    {
        cut.truncate(space);
    }
    format!("{}…", cut.trim_end_matches([' ', ',', ';', ':', '-']))
}
fn reasoning(body: &Map) -> String {
    field(body, &["because", "breaks_if"])
        .map(textish)
        .unwrap_or_default()
}
fn page_number(value: &V) -> String {
    crate::public_ordinary_readers::fmt(value)
}
fn reference_text(context: &RenderContext<'_>, id: &str) -> Option<String> {
    let b = context
        .nodes
        .get(id)
        .and_then(|_| body(context.nodes, id).ok())?;
    if context.judgments.contains(id) {
        return Some(
            field(b, &["verdict", "title"])
                .map(textish)
                .unwrap_or_else(|| id.into()),
        );
    }
    if let Some(v) = b.get("v").or_else(|| b.get("quoted"))
        && !matches!(v, V::Null | V::Map(_) | V::List(_))
    {
        return Some(page_number(v));
    }
    Some(label(id, b, context.labels))
}
fn resolve_references(context: &RenderContext<'_>, value: &str) -> String {
    let refs = &*PROSE_REFS;
    refs.replace_all(value, |m: &regex::Captures<'_>| {
        reference_text(context, &m[1]).unwrap_or_else(|| m[0].into())
    })
    .into_owned()
}
fn value_renderings(value: Option<&V>) -> Vec<String> {
    let Some(value) = value else {
        return vec![];
    };
    let s = textish(value).trim().to_owned();
    let mut out = if let Some(day) = entry_date(&Map::from([("v".into(), value.clone())])) {
        ["%d/%m/%Y", "%d/%m", "%Y-%m-%d", "%d.%m.%Y"]
            .map(|f| day.format(f).to_string())
            .to_vec()
    } else {
        let bare = s.replace(',', "");
        if PROSE_NUMBER.is_match(&bare) {
            let mut values = vec![s.clone(), bare.clone()];
            if let Ok(integer) = crate::value::Integer::new(&bare) {
                values.push(page_number(&V::Integer(integer)));
            }
            values
        } else if s.chars().count() >= 6 {
            vec![s]
        } else {
            vec![]
        }
    };
    out.retain(|s| s.chars().count() >= 3);
    out.sort();
    out.dedup();
    out
}
fn surface_words(context: &RenderContext<'_>, value: &str) -> String {
    let ids = &*PROSE_IDS;
    ids.replace_all(value, |m: &regex::Captures<'_>| {
        context
            .nodes
            .get(&m[0])
            .and_then(|_| body(context.nodes, &m[0]).ok())
            .map(|b| label(&m[0], b, context.labels))
            .unwrap_or_else(|| m[0].into())
    })
    .into_owned()
}
fn anchor_prose(
    context: &RenderContext<'_>,
    value: &str,
    dependencies: &[String],
) -> (String, BTreeSet<String>) {
    let mut owners = BTreeMap::<String, Option<String>>::new();
    for id in dependencies {
        let b = context
            .nodes
            .get(id)
            .and_then(|_| body(context.nodes, id).ok());
        let v = b
            .filter(|_| !context.judgments.contains(id))
            .and_then(|b| b.get("v").or_else(|| b.get("quoted")));
        for candidate in std::iter::once(id.clone()).chain(value_renderings(v)) {
            owners
                .entry(candidate)
                .and_modify(|old| {
                    if old.as_ref() != Some(id) {
                        *old = None;
                    }
                })
                .or_insert_with(|| Some(id.clone()));
        }
    }
    let mut candidates = owners
        .into_iter()
        .filter_map(|(s, id)| id.map(|id| (s, id)))
        .collect::<Vec<_>>();
    candidates.sort_by_key(|(s, _)| std::cmp::Reverse(s.chars().count()));
    let mut hits = vec![];
    for (candidate, id) in candidates {
        for (start, _) in value.match_indices(&candidate) {
            let end = start + candidate.len();
            if !hits.iter().any(|(a, b, _)| start < *b && *a < end) {
                hits.push((start, end, id.clone()));
            }
        }
    }
    hits.sort();
    let mut found = BTreeSet::new();
    let mut out = String::new();
    let mut pos = 0;
    for (start, end, id) in hits {
        out.push_str(&esc(&surface_words(context, &value[pos..start]), false));
        let shown = if value[start..end] == id {
            context
                .nodes
                .get(&id)
                .and_then(|_| body(context.nodes, &id).ok())
                .map(|b| label(&id, b, context.labels))
                .unwrap_or_else(|| id.clone())
        } else {
            value[start..end].into()
        };
        out.push_str(&format!(
            "<span class=\"fx in\" data-id=\"{}\">{}</span>",
            esc(&id, true),
            esc(&surface_words(context, &shown), false)
        ));
        found.insert(id);
        pos = end;
    }
    out.push_str(&esc(&surface_words(context, &value[pos..]), false));
    (out, found)
}
fn judgment_prose(
    context: &RenderContext<'_>,
    value: &str,
    dependencies: &[String],
    movements: &[(String, V, V)],
) -> (String, BTreeSet<String>) {
    let refs = &*PROSE_REFS;
    let mut out = String::new();
    let mut found = BTreeSet::new();
    let mut pos = 0;
    for m in refs.captures_iter(value) {
        let full = m.get(0).unwrap();
        let (part, hits) = anchor_prose(context, &value[pos..full.start()], dependencies);
        out.push_str(&part);
        found.extend(hits);
        if let Some(shown) = reference_text(context, &m[1]) {
            let moved = movements.iter().find(|(id, _, _)| id == &m[1]);
            let title = moved
                .map(|(_, old, _)| {
                    format!(
                        " title=\"{}\"",
                        esc(
                            &context.words["was"]
                                .as_str()
                                .unwrap()
                                .replace("{v}", &crate::public_ordinary_readers::short(old, 60)),
                            true
                        )
                    )
                })
                .unwrap_or_default();
            out.push_str(&format!(
                "<span class=\"fx in{}\" data-id=\"{}\"{title}>{}</span>",
                if moved.is_some() { " mv" } else { "" },
                esc(&m[1], true),
                esc(&surface_words(context, &shown), false)
            ));
            found.insert(m[1].into());
        } else {
            out.push_str(&esc(&m[0], false));
        }
        pos = full.end();
    }
    let (part, hits) = anchor_prose(context, &value[pos..], dependencies);
    out.push_str(&part);
    found.extend(hits);
    (out, found)
}

fn record_groups(
    ids: &BTreeSet<String>,
    judgments: &BTreeSet<String>,
) -> Vec<(String, BTreeSet<String>)> {
    let mut groups = BTreeMap::<String, BTreeSet<String>>::new();
    for id in ids.difference(judgments) {
        groups
            .entry(id.split_once('.').map_or("-", |(p, _)| p).into())
            .or_default()
            .insert(id.clone());
    }
    let mut groups = groups.into_iter().collect::<Vec<_>>();
    groups.sort_by(|(a, x), (b, y)| y.len().cmp(&x.len()).then(a.cmp(b)));
    groups
}
fn prefix_heading(prefix: &str, meta: &Map) -> (String, String) {
    let word = meta
        .get("prefixes")
        .and_then(|v| map(v).ok())
        .and_then(|m| m.get(prefix))
        .and_then(|v| text(v).ok())
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|s| !s.is_empty());
    if let Some(word) = word {
        let word = if word.chars().count() > 40 {
            format!("{}…", word.chars().take(39).collect::<String>())
        } else {
            word
        };
        (
            format!(" title=\"{}.\"", esc(prefix, true)),
            esc(&word, false),
        )
    } else {
        (String::new(), esc(prefix, false))
    }
}
fn record_nav(context: &RenderContext<'_>, ids: &BTreeSet<String>, meta: &Map) -> String {
    let links = record_groups(ids, context.judgments)
        .into_iter()
        .map(|(prefix, entries)| {
            let (hover, word) = prefix_heading(&prefix, meta);
            format!(
                "<a href=\"#g-{}\"{hover}>{word} ({})</a>",
                esc(&prefix, true),
                entries.len()
            )
        })
        .collect::<String>();
    format!("<nav class=\"ns\" dir=\"ltr\">{links}</nav>")
}
fn page_footer(
    context: &RenderContext<'_>,
    brief: &Map,
    tabs: &[Tab],
    words: &J,
    panels: &[(String, String)],
    failures: &mut Vec<String>,
) -> Result<String> {
    let mut out = format!("<footer dir=\"{}\">", words["dir"].as_str().unwrap());
    for field in ["truth", "elsewhere"] {
        if !brief.get(field).is_some_and(truth) {
            continue;
        }
        let mut links = vec![];
        for id in selectors(brief.get(field)) {
            if !context.nodes.contains_key(&id) {
                failures.push(format!(
                    "{field}: name a record entry, not unanchored footer prose"
                ));
                continue;
            }
            let b = body(context.nodes, &id)?;
            let label = format!(
                "<span class=\"fx\" data-id=\"{}\">{}</span>",
                esc(&id, true),
                esc(
                    &surface_words(context, &label(&id, b, context.labels)),
                    false
                )
            );
            links.push(
                if let Some(h) = href(b, context.record_root, context.page_path) {
                    format!("<a href=\"{}\">{label}</a>", esc(&h, true))
                } else {
                    label
                },
            );
        }
        out.push_str(&format!(
            "<div>{}{}</div>",
            words[field].as_str().unwrap(),
            links.join(" &middot; ")
        ));
    }
    if panels
        .iter()
        .any(|(_, panel)| panel.contains("data-countdown=\"") || panel.contains("data-day=\""))
    {
        out.push_str(words["snapshot"].as_str().unwrap());
        out.push(' ');
    }
    out.push_str(words["footer"].as_str().unwrap());
    if !brief.is_empty() {
        out.push_str(
            &words[if tabs.len() == 1 && tabs[0].bare {
                "footer_brief"
            } else {
                "footer_tabs"
            }]
            .as_str()
            .unwrap()
            .replace("{now}", words["tab_now"].as_str().unwrap())
            .replace("{record}", words["tab_record"].as_str().unwrap()),
        );
    }
    out.push_str("</footer>");
    Ok(out)
}

fn group_order(content: Option<&[u8]>) -> Result<(String, Vec<String>)> {
    use crate::history_yaml::SourceValue;
    let Some(content) = content else {
        return Ok((String::new(), vec![]));
    };
    let source = crate::history_yaml::decode_source_value(content)?;
    let Some(SourceValue::Map(groups)) = source.get("groups").or_else(|| source.get("fronts"))
    else {
        return Ok((String::new(), vec![]));
    };
    if !groups.is_empty() && groups.iter().all(|(_, v)| matches!(v, SourceValue::Map(_))) {
        let (name, SourceValue::Map(groups)) = &groups[0] else {
            unreachable!()
        };
        Ok((
            name.clone(),
            groups.iter().map(|(name, _)| name.clone()).collect(),
        ))
    } else {
        Ok((
            "groups".into(),
            groups.iter().map(|(name, _)| name.clone()).collect(),
        ))
    }
}
fn linked_ids(context: &RenderContext<'_>, value: &str) -> String {
    let pattern = &*PROSE_IDS;
    let mut out = String::new();
    let mut pos = 0;
    for m in pattern.find_iter(value) {
        let id = m.as_str();
        if !context.nodes.contains_key(id) {
            continue;
        }
        out.push_str(&esc(&value[pos..m.start()], false));
        out.push_str(&format!(
            "<span class=\"fx in\" data-id=\"{}\">{}</span>",
            esc(id, true),
            esc(
                &label(id, body(context.nodes, id).unwrap(), context.labels),
                false
            )
        ));
        pos = m.end();
    }
    out.push_str(&esc(&value[pos..], false));
    out
}
fn table_value(context: &RenderContext<'_>, id: &str, b: &Map) -> String {
    let value = b
        .get("v")
        .or_else(|| b.get("quoted"))
        .filter(|v| !matches!(v, V::Null));
    let value = value.filter(|v| {
        if let V::Text(s) = v {
            // A value written as an expression is a rule, not a copied result.
            let has_operator = PROSE_EXPRESSION.is_match(s);
            let has_id = PROSE_IDS
                .find_iter(s)
                .any(|m| context.nodes.contains_key(m.as_str()));
            !(has_operator && has_id)
        } else {
            true
        }
    });
    if let Some(value) = value {
        let shown = if let V::Bool(v) = value
            && (context.component_page || context.language != "en")
        {
            context.words[if *v { "yes" } else { "no" }]
                .as_str()
                .unwrap()
                .into()
        } else if let Some(original) = context.source_values.get(id) {
            original.clone()
        } else {
            page_number(value)
        };
        linked_ids(context, &shown)
    } else {
        format!(
            "<span class=\"derived\">{}</span>",
            context.words[if id.starts_with("page.") {
                "uncounted"
            } else {
                "derived"
            }]
            .as_str()
            .unwrap()
        )
    }
}

fn moved_note(context: &RenderContext<'_>, movements: &[(String, V, V)]) -> String {
    if movements.is_empty() {
        return String::new();
    }
    let said = movements
        .iter()
        .map(|(id, old, now)| {
            let short = |v: &V| {
                esc(
                    &crate::public_ordinary_readers::short(
                        &V::Text(surface_words(context, &textish(v))),
                        100,
                    ),
                    false,
                )
            };
            format!(
                "{} {} &rarr; {}",
                esc(
                    &label(id, body(context.nodes, id).unwrap(), context.labels),
                    false
                ),
                short(old),
                short(now)
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "<div class=\"mvd\" dir=\"auto\">{}{said}</div>",
        context.words["moved_reviewed"].as_str().unwrap()
    )
}
fn source_container_values(
    source: &crate::history_yaml::OrdinaryValue,
    nodes: &Map,
) -> BTreeMap<String, String> {
    use crate::history_yaml::OrdinaryValue as S;
    fn repr(value: &S) -> String {
        match value {
            S::Scalar(v) => crate::source_text::ordinary_python_repr(v),
            S::List(values) => format!(
                "[{}]",
                values.iter().map(repr).collect::<Vec<_>>().join(", ")
            ),
            S::Map(values) => format!(
                "{{{}}}",
                values
                    .iter()
                    .map(|(k, v)| format!(
                        "{}: {}",
                        crate::source_text::ordinary_python_repr(k.scalar()),
                        repr(v)
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
    let mut out = BTreeMap::new();
    if let S::Map(collections) = source {
        for (_, collection) in collections {
            if let S::Map(entries) = collection {
                for (key, entry) in entries {
                    let Some(id) = key.text() else {
                        continue;
                    };
                    let Some(value) = entry.get("v").or_else(|| entry.get("quoted")) else {
                        continue;
                    };
                    if matches!(value, S::Map(_) | S::List(_))
                        && nodes.contains_key(id)
                        && body(nodes, id)
                            .ok()
                            .and_then(|b| b.get("v").or_else(|| b.get("quoted")))
                            .is_some_and(|v| *v == value.projected())
                    {
                        out.insert(id.into(), repr(value));
                    }
                }
            }
        }
    }
    out
}
