//! Exact brief edits retain source formatting and historical evidence.
use crate::{
    Result,
    history_contract::*,
    history_identity_rewrite::tokens,
    history_identity_text::{self as Text, Kind, Node},
    history_view::truth,
    history_yaml as Y,
    reasoning_authoring::py,
    require,
    value::TypedValue as V,
};
use regex::Regex;
use std::sync::LazyLock;
static MEMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^( +)([A-Za-z_][A-Za-z0-9_.]*):(?: |$)").unwrap());
static TOP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Za-z_][A-Za-z0-9_]*):(?: |$)").unwrap());
static ITEM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^( *)- +title:\s*(.+?)\s*$").unwrap());
fn indent(s: &str) -> usize {
    s.len() - s.trim_start_matches(' ').len()
}
fn members(lines: &[String], start: usize, end: usize) -> (Option<usize>, Vec<(String, usize)>) {
    let mut ind = None;
    let mut out = vec![];
    for (i, line) in lines.iter().enumerate().take(end).skip(start + 1) {
        if line.trim().is_empty() || line.trim().starts_with('#') {
            continue;
        }
        let expected = *ind.get_or_insert(indent(line));
        if let Some(m) = MEMBER.captures(line)
            && m[1].len() == expected
        {
            out.push((m[2].into(), i));
        }
    }
    (ind, out)
}
fn block_end(lines: &[String], i: usize, ind: usize, end: usize) -> usize {
    let mut j = i + 1;
    while j < end {
        if lines[j].trim().is_empty() {
            let mut k = j;
            while k < end && lines[k].trim().is_empty() {
                k += 1;
            }
            if k < end && indent(&lines[k]) > ind {
                j = k;
                continue;
            }
            break;
        }
        if indent(&lines[j]) > ind {
            j += 1;
        } else {
            break;
        }
    }
    j
}
fn field_span(lines: &[String], start: usize, end: usize, field: &str) -> Option<(usize, usize)> {
    let (ind, fields) = members(lines, start, end);
    fields
        .into_iter()
        .find(|(name, _)| name == field)
        .map(|(_, i)| (i, block_end(lines, i, ind.unwrap(), end)))
}
fn section_span(lines: &[String], title: &str) -> Option<(usize, usize)> {
    for (i, line) in lines.iter().enumerate() {
        let Some(m) = ITEM.captures(line) else {
            continue;
        };
        let got = m[2].trim();
        let got = if got.starts_with(['\'', '"']) {
            got.get(1..got.len().saturating_sub(1)).unwrap_or("")
        } else {
            got
        };
        if got != title {
            continue;
        }
        let ind = m[1].len();
        let mut j = i + 1;
        while j < lines.len() && (lines[j].trim().is_empty() || indent(&lines[j]) > ind) {
            j += 1;
        }
        while j > i + 1 && lines[j - 1].trim().is_empty() {
            j -= 1;
        }
        if field_span(lines, i, j, "text").is_some() {
            return Some((i, j));
        }
    }
    None
}
fn drop_key(
    lines: &mut Vec<String>,
    start: usize,
    end: usize,
    field: &str,
    key: &str,
) -> Result<()> {
    let Some((i, j)) = field_span(lines, start, end, field) else {
        return Ok(());
    };
    if lines[i]
        .split_once(':')
        .is_some_and(|(_, v)| v.trim().starts_with('{'))
    {
        let block = lines[i..j].join("\n");
        let pattern = format!(
            r#"{}\s*:\s*("(?:[^"\\]|\\.)*"|'(?:[^']|'')*'|[^,}}\n]+)\s*,?\s*"#,
            regex::escape(key)
        );
        let regex = Regex::new(&pattern).map_err(|_| error("invalid_identity_yaml"))?;
        let mut output = String::new();
        let mut last = 0;
        for m in regex.find_iter(&block) {
            if block[..m.start()]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
            {
                continue;
            }
            output.push_str(&block[last..m.start()]);
            last = m.end();
        }
        output.push_str(&block[last..]);
        let output = Regex::new(r",\s*}").unwrap().replace_all(&output, "}");
        lines.splice(i..j, output.split('\n').map(str::to_owned));
    } else if let Some((ki, kj)) = field_span(lines, i, j, key) {
        lines.drain(ki..kj);
    }
    Ok(())
}
fn scalar_spans(node: &Node, out: &mut Vec<(usize, usize)>) {
    match &node.kind {
        Kind::Scalar(_, _) => out.push((node.start, node.end)),
        Kind::Map(pairs) => {
            for (key, value) in pairs {
                scalar_spans(key, out);
                scalar_spans(value, out);
            }
        }
        Kind::List(values) => {
            for value in values {
                scalar_spans(value, out);
            }
        }
    }
}
fn dedupe_flow(text: &str, survivor: &str) -> String {
    let Ok(node) = Text::parse(text) else {
        return text.into();
    };
    let mut protected = vec![];
    scalar_spans(&node, &mut protected);
    Regex::new(r"\[([^\[\]]*)\]")
        .unwrap()
        .replace_all(text, |m: &regex::Captures<'_>| {
            let matched = m.get(0).unwrap();
            if protected
                .iter()
                .any(|(a, b)| *a <= matched.start() && matched.start() < *b)
                || (m[1].len() - tokens(&m[1], survivor, "").len()) / survivor.len() < 2
            {
                return matched.as_str().to_owned();
            }
            let mut out = vec![];
            let mut had = false;
            let inner = m[1].replace('\n', " ");
            for item in inner.split(',').map(str::trim).filter(|v| !v.is_empty()) {
                if item == survivor {
                    if had {
                        continue;
                    }
                    had = true;
                }
                out.push(item);
            }
            format!("[{}]", out.join(", "))
        })
        .into_owned()
}
fn dedupe_distinct(text: &str) -> String {
    let regex =
        Regex::new(r#"(?m)^(\s*distinct_from:\s*)(["']?)([A-Za-z0-9_.,\s]+?)(["']?)\s*$"#).unwrap();
    regex
        .replace_all(text, |m: &regex::Captures<'_>| {
            if m[2] != m[4] {
                return m[0].into();
            }
            let mut out = vec![];
            for id in m[3]
                .split(|c: char| c == ',' || c.is_whitespace())
                .filter(|v| !v.is_empty())
            {
                if !out.contains(&id) {
                    out.push(id);
                }
            }
            let joined = out.join(", ");
            let bare = Regex::new(r"^[A-Za-z_][A-Za-z0-9_.\-]*$")
                .unwrap()
                .is_match(&joined)
                && Y::decode_document(format!("v: {joined}\n").as_bytes()).is_ok_and(|v| {
                    map(&v).is_ok_and(|v| v.get("v").is_some_and(|v| string_is(v, &joined)))
                });
            format!(
                "{}{}",
                &m[1],
                if bare {
                    joined
                } else {
                    serde_json::to_string(&joined).unwrap()
                }
            )
        })
        .into_owned()
}
fn mappings(value: Option<&V>) -> Result<Vec<&V>> {
    match value {
        Some(V::List(values)) => Ok(values.iter().filter(|v| matches!(v, V::Map(_))).collect()),
        Some(V::Text(_) | V::Map(_)) | None => Ok(vec![]),
        Some(v) if !truth(v) => Ok(vec![]),
        _ => Err(error("invalid_identity_brief")),
    }
}
pub fn rewrite(
    raw: Option<&[u8]>,
    retired: &str,
    survivor: &str,
    fields: &Map,
) -> Result<Option<Vec<u8>>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    require(raw.len() <= 16 * 1024 * 1024, "history_limit")?;
    require(
        !retired.is_empty() && !survivor.is_empty(),
        "invalid_identity_subjects",
    )?;
    let text = std::str::from_utf8(raw).map_err(|_| error("invalid_brief_encoding"))?;
    // A brief with no matching token retains even otherwise unsupported source bytes.
    if tokens(text, retired, "") == text {
        return Ok(Some(raw.to_vec()));
    }
    let document = Y::decode_document(raw)?;
    let document = map(&document)?;
    let mut lines = text.split('\n').map(str::to_owned).collect::<Vec<_>>();
    let mut sections = mappings(document.get("sections"))?;
    for tab in mappings(document.get("tabs"))? {
        sections.extend(mappings(map(tab)?.get("sections"))?);
    }
    for section in sections {
        let section = map(section)?;
        if let Some(seen) = section.get("seen").and_then(|v| map(v).ok())
            && seen.contains_key(retired)
            && seen.contains_key(survivor)
            && section.get("text").is_some_and(truth)
        {
            let title = section.get("title").map(py).unwrap_or("None".into());
            let (start, end) = section_span(&lines, &title)
                .ok_or_else(|| error("identity_ambiguous_brief_section"))?;
            drop_key(&mut lines, start, end, "seen", retired)?;
        }
    }
    if let Some(labels) = document.get("labels").and_then(|v| map(v).ok())
        && labels.contains_key(retired)
        && labels.contains_key(survivor)
    {
        let tops = lines
            .iter()
            .enumerate()
            .filter_map(|(i, l)| TOP.captures(l).map(|m| (m[1].to_owned(), i)))
            .collect::<Vec<_>>();
        let i = tops
            .iter()
            .position(|(name, _)| name == "labels")
            .ok_or_else(|| error("identity_ambiguous_brief_labels"))?;
        let start = tops[i].1;
        let end = tops.get(i + 1).map(|(_, i)| *i).unwrap_or(lines.len());
        let (ind, members) = members(&lines, start, end);
        if let Some((_, i)) = members.into_iter().find(|(name, _)| name == retired) {
            let j = block_end(&lines, i, ind.unwrap(), end);
            lines.drain(i..j);
        }
    }
    let rewritten = Text::rewrite(
        &lines.join("\n"),
        retired,
        survivor,
        text_field(fields, "predicate")?,
        text_field(fields, "snapshot")?,
    )?;
    let rewritten = dedupe_distinct(&dedupe_flow(&rewritten, survivor));
    Y::decode_document(rewritten.as_bytes())?;
    Ok(Some(rewritten.into_bytes()))
}
fn text_field<'a>(fields: &'a Map, key: &str) -> Result<&'a str> {
    text(field(fields, key)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history_authoring::s;
    use base64::{Engine, engine::general_purpose::STANDARD};
    #[test]
    fn brief_rewriting_keeps_exact_layout_and_refuses_ambiguous_source() {
        let data: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/history-identity-rewrite.json"
        ))
        .unwrap();
        let fields = Map::from([
            ("predicate".into(), s("wrong_if")),
            ("snapshot".into(), s("seen")),
        ]);
        for case in data["brief"].as_array().unwrap() {
            let raw = case["raw"].as_str().map(|s| STANDARD.decode(s).unwrap());
            let result = rewrite(raw.as_deref(), "p.other", "p.input", &fields);
            if let Some(expected) = case.get("output") {
                assert_eq!(
                    result.unwrap(),
                    expected.as_str().map(|s| STANDARD.decode(s).unwrap()),
                    "{}",
                    case["name"]
                );
            } else {
                assert!(result.is_err(), "{}", case["name"]);
            }
        }
    }
}
