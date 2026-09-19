//! Source-span edits of generated capability metadata preserve surrounding YAML.
use crate::{
    Result,
    history_authoring::{empty, obj},
    history_contract::*,
    history_view::list,
    history_yaml as Y, reasoning_authoring as A, reasoning_fields as F, require,
    value::TypedValue as V,
};
use libyaml_safer::{Event, EventData as E, MappingStyle, Parser, SequenceStyle};
struct Node {
    start: usize,
    end: usize,
    flow: bool,
    kind: Kind,
}
enum Kind {
    Scalar(String),
    List(Vec<Node>),
    Map(Vec<(Node, Node)>),
}
impl Node {
    fn scalar(&self) -> Option<&str> {
        if let Kind::Scalar(s) = &self.kind {
            Some(s)
        } else {
            None
        }
    }
    fn member(&self, key: &str) -> Result<(&Node, &Node)> {
        if let Kind::Map(m) = &self.kind {
            m.iter()
                .find(|(k, _)| k.scalar() == Some(key))
                .map(|(k, v)| (k, v))
                .ok_or_else(|| error("invalid_declaration_yaml"))
        } else {
            Err(error("invalid_declaration_yaml"))
        }
    }
}
fn next(p: &mut Parser<&[u8]>) -> Result<Event> {
    p.parse().map_err(|_| error("invalid_declaration_yaml"))
}
fn node(p: &mut Parser<&[u8]>, event: Event, depth: usize, visits: &mut usize) -> Result<Node> {
    *visits += 1;
    require(depth <= 128 && *visits <= 200_000, "history_limit")?;
    let start = event.start_mark.index as usize;
    let mut end = event.end_mark.index as usize;
    let mut flow = false;
    let (kind, _anchor) = match event.data {
        E::Alias { .. } => return Err(error("invalid_declaration_yaml")),
        E::Scalar { value, anchor, .. } => (Kind::Scalar(value), anchor),
        E::MappingStart { anchor, style, .. } => {
            flow = style == MappingStyle::Flow;
            let mut m = vec![];
            loop {
                let e = next(p)?;
                if matches!(e.data, E::MappingEnd) {
                    end = e.end_mark.index as usize;
                    break;
                }
                let k = node(p, e, depth + 1, visits)?;
                let e = next(p)?;
                let v = node(p, e, depth + 1, visits)?;
                m.push((k, v));
            }
            (Kind::Map(m), anchor)
        }
        E::SequenceStart { anchor, style, .. } => {
            flow = style == SequenceStyle::Flow;
            let mut a = vec![];
            loop {
                let e = next(p)?;
                if matches!(e.data, E::SequenceEnd) {
                    end = e.end_mark.index as usize;
                    break;
                }
                a.push(node(p, e, depth + 1, visits)?);
            }
            (Kind::List(a), anchor)
        }
        _ => return Err(error("invalid_declaration_yaml")),
    };
    Ok(Node {
        start,
        end,
        flow,
        kind,
    })
}
fn parse(text: &str) -> Result<Node> {
    let mut p = Parser::new();
    p.set_input(text.as_bytes());
    require(
        matches!(next(&mut p)?.data, E::StreamStart { .. }),
        "invalid_declaration_yaml",
    )?;
    let start = next(&mut p)?;
    if matches!(start.data, E::StreamEnd) {
        return Ok(Node {
            start: 0,
            end: 0,
            flow: false,
            kind: Kind::Scalar(String::new()),
        });
    }
    require(
        matches!(start.data, E::DocumentStart { .. }),
        "invalid_declaration_yaml",
    )?;
    let e = next(&mut p)?;
    let n = node(&mut p, e, 0, &mut 0)?;
    require(
        matches!(next(&mut p)?.data, E::DocumentEnd { .. })
            && matches!(next(&mut p)?.data, E::StreamEnd),
        "invalid_declaration_yaml",
    )?;
    Ok(n)
}
fn line_start(text: &str, index: usize) -> usize {
    text[..index].rfind('\n').map(|i| i + 1).unwrap_or(0)
}
fn line_index(text: &str, index: usize) -> usize {
    text[..index].bytes().filter(|b| *b == b'\n').count()
}
fn render(desired: &V, flow: bool) -> Result<String> {
    let d = map(desired)?;
    let req = list(&d["requires"])?
        .iter()
        .map(text_value)
        .collect::<Result<Vec<_>>>()?;
    if flow {
        Ok(format!(
            "reasoning: {{version: {}, profile: core/v1, requires: [{}]}}",
            A::py(&d["version"]),
            req.join(", ")
        ))
    } else {
        Ok(format!(
            "reasoning:\n  version: {}\n  profile: core/v1\n  requires:\n{}",
            A::py(&d["version"]),
            req.iter()
                .map(|v| format!("  - {v}"))
                .collect::<Vec<_>>()
                .join("\n")
        ))
    }
}
/// Only generated version/requirements are edited. Unsupported or ambiguous YAML
/// refuses before publication; callers retain the original UTF-8 source.
pub fn declare(text: &str, desired: Option<&V>) -> Result<String> {
    require(text.len() <= Y::MAX_DOCUMENT_BYTES, "history_limit")?;
    let parsed = parse(text)?;
    let document = if matches!(&parsed.kind,Kind::Scalar(v) if v.is_empty()) {
        empty()
    } else {
        Y::decode_document(text.as_bytes())?
    };
    let doc = map(&document)?;
    let meta = doc.get("meta").unwrap_or(&V::Null);
    require(
        *meta == V::Null || matches!(meta, V::Map(_)),
        "metadata_requires_explicit_migration",
    )?;
    let desired = desired
        .cloned()
        .map(Ok)
        .unwrap_or_else(|| A::declaration(&document))?;
    F::capabilities(
        &obj([("meta", obj([("reasoning", desired.clone())]))]),
        None,
    )?;
    if map(meta).ok().and_then(|m| m.get("reasoning")) == Some(&desired) {
        return Ok(text.into());
    }
    let encoded = render(&desired, false)?;
    let mut lines = text.split('\n').map(str::to_owned).collect::<Vec<_>>();
    if *meta == V::Null {
        let at = if text.trim().is_empty() {
            0
        } else {
            line_index(text, parse(text)?.start)
        };
        let mut insert = vec!["meta:".into()];
        insert.extend(encoded.lines().map(|s| format!("  {s}")));
        insert.push(String::new());
        lines.splice(at..at, insert);
        let out = lines.join("\n");
        Y::decode_document(out.as_bytes())?;
        return Ok(out);
    }
    let (_, metadata) = parsed.member("meta")?;
    let mut edits = vec![];
    if map(meta)?.contains_key("reasoning") {
        let (key, declaration) = metadata.member("reasoning")?;
        require(
            declaration.start >= key.end,
            "aliased_reasoning_declaration",
        )?;
        let original = map(&map(meta)?["reasoning"])?;
        let wanted = map(&desired)?;
        let (key, version) = declaration.member("version")?;
        if original["version"] != wanted["version"] {
            require(version.start >= key.end, "aliased_reasoning_declaration")?;
            edits.push((version.start, version.end, A::py(&wanted["version"])));
        }
        let (key, requirements) = declaration.member("requires")?;
        if original["requires"] != wanted["requires"] {
            require(
                requirements.start >= key.end,
                "aliased_reasoning_declaration",
            )?;
            let existing = list(&original["requires"])?;
            let desired = list(&wanted["requires"])?;
            if requirements.flow {
                edits.push((
                    requirements.start,
                    requirements.end,
                    format!(
                        "[{}]",
                        desired
                            .iter()
                            .map(text_value)
                            .collect::<Result<Vec<_>>>()?
                            .join(", ")
                    ),
                ));
            } else {
                let Kind::List(nodes) = &requirements.kind else {
                    return Err(error("invalid_declaration_yaml"));
                };
                let indent = " ".repeat(requirements.start - line_start(text, requirements.start));
                for item in desired.iter().filter(|v| !existing.contains(v)) {
                    let value = text_value(item)?;
                    let following = existing
                        .iter()
                        .zip(nodes)
                        .find(|(prior, _)| text_value(prior).is_ok_and(|s| s > value));
                    if let Some((_, following)) = following {
                        let at = line_start(text, following.start);
                        edits.push((at, at, format!("{indent}- {value}\n")));
                    } else {
                        let last = nodes
                            .last()
                            .ok_or_else(|| error("invalid_declaration_yaml"))?;
                        if let Some(newline) = text[last.end..].find('\n') {
                            let at = last.end + newline + 1;
                            edits.push((at, at, format!("{indent}- {value}\n")));
                        } else {
                            edits.push((text.len(), text.len(), format!("\n{indent}- {value}")));
                        }
                    }
                }
            }
        }
    } else if metadata.flow {
        let at = metadata.start + 1;
        let item = render(&desired, true)?;
        edits.push((
            at,
            at,
            format!("{item}{}", if map(meta)?.is_empty() { "" } else { ", " }),
        ));
    } else {
        let at = line_index(text, metadata.start);
        lines.splice(at..at, encoded.lines().map(|s| format!("  {s}")));
        let out = lines.join("\n");
        Y::decode_document(out.as_bytes())?;
        return Ok(out);
    }
    edits.sort_by(|a, b| b.cmp(a));
    let mut result = text.to_owned();
    for (start, end, replacement) in edits {
        require(
            result.is_char_boundary(start) && result.is_char_boundary(end),
            "invalid_declaration_yaml",
        )?;
        result.replace_range(start..end, &replacement);
    }
    Y::decode_document(result.as_bytes())?;
    Ok(result)
}
fn text_value(v: &V) -> Result<&str> {
    crate::history_contract::text(v)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn declarations_preserve_source_spans_against_python() {
        let corpus: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/reasoning-declaration-text.json"
        ))
        .unwrap();
        let mut failures = vec![];
        for case in corpus["cases"].as_array().unwrap() {
            let desired = if case["desired"].is_null() {
                None
            } else {
                Some(V::from_json(&case["desired"]).unwrap())
            };
            let result = declare(case["input"].as_str().unwrap(), desired.as_ref());
            match (case.get("output"), result) {
                (_, Err(_))
                    if case.get("strict_refusal") == Some(&serde_json::Value::Bool(true)) => {}
                (Some(wanted), Ok(actual)) => {
                    if actual != wanted.as_str().unwrap() {
                        failures.push(format!("{}: got {actual:?}; wanted {wanted}", case["name"]));
                    }
                }
                (None, Err(_)) => {}
                (Some(_), Err(e)) => failures.push(format!("{}: refused {e}", case["name"])),
                (None, Ok(_)) => failures.push(format!("{}: accepted", case["name"])),
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}
