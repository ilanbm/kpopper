//! Strict authored-HTML boundary for standalone annotated documents.
//!
//! The HTML5 tree establishes browser semantics.  A second, deliberately strict
//! source collector retains the author's exact character spans and rejects any
//! document whose browser tree would require recovery.

use cssparser::{Parser, ParserInput, Token};
use html5ever::{parse_document, tendril::TendrilSink};
use markup5ever_rcdom::{Handle, NodeData, RcDom};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use crate::{Error, Result};

const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];
const RAW: &[&str] = &[
    "script",
    "style",
    "title",
    "textarea",
    "xmp",
    "iframe",
    "noembed",
    "noframes",
    "plaintext",
];
const INVISIBLE: &[&str] = &[
    "head", "script", "style", "template", "noscript", "title", "link", "meta", "base",
];
const CONTEXT: &[&str] = &[
    "p",
    "li",
    "td",
    "th",
    "figcaption",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
];
const BLOCK: &[&str] = &[
    "p",
    "li",
    "td",
    "th",
    "figcaption",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "body",
    "div",
    "section",
    "article",
    "aside",
    "header",
    "footer",
    "main",
    "nav",
    "blockquote",
    "pre",
    "address",
    "dt",
    "dd",
    "caption",
    "summary",
    "legend",
];
static VALUE_PATTERN: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#""[^"\n]+"|“[^”\n]+”|「[^」\n]+」|(?:^|[^\pL\pN_])\d{4}-\d{1,2}-\d{1,2}(?:$|[^\pL\pN_])|(?:^|[^\pL\pN_])[+−-]?\d+(?:[,.]\d+)*(?:%|°)?"#,
    )
    .unwrap()
});

fn error(message: impl Into<String>) -> Error {
    Error(format!("document: {}", message.into()))
}

#[derive(Clone, Debug)]
struct SourceNode {
    tag: String,
    start: usize,
    end: usize,
    close_end: usize,
    self_closed: bool,
    parent: Option<usize>,
    children: Vec<usize>,
}

fn char_to_byte(source: &str, offset: usize) -> usize {
    source
        .char_indices()
        .nth(offset)
        .map_or(source.len(), |(i, _)| i)
}
fn slice_chars(source: &str, start: usize, end: usize) -> &str {
    &source[char_to_byte(source, start)..char_to_byte(source, end)]
}
fn chars(source: &str) -> Vec<char> {
    source.chars().collect()
}

fn skip_space(c: &[char], mut i: usize) -> usize {
    while i < c.len() && c[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}
fn name(c: &[char], mut i: usize) -> (String, usize) {
    let start = i;
    while i < c.len() && (c[i].is_ascii_alphanumeric() || matches!(c[i], ':' | '-' | '_' | '.')) {
        i += 1;
    }
    (
        c[start..i].iter().collect::<String>().to_ascii_lowercase(),
        i,
    )
}
fn tag_end(c: &[char], mut i: usize) -> Result<usize> {
    let mut quote = None;
    while i < c.len() {
        match (quote, c[i]) {
            (Some(q), ch) if q == ch => quote = None,
            (None, '\'' | '"') => quote = Some(c[i]),
            (None, '>') => return Ok(i + 1),
            _ => {}
        }
        i += 1;
    }
    Err(error("Incomplete HTML token at the end of the document."))
}

fn collect_source(source: &str) -> Result<(Vec<SourceNode>, usize, usize, usize)> {
    let c = chars(source);
    let mut nodes = Vec::<SourceNode>::new();
    let mut stack = Vec::<usize>::new();
    let mut roots = Vec::new();
    let mut i = 0;
    let mut doctype = false;
    while i < c.len() {
        if c[i] != '<' {
            let start = i;
            while i < c.len() && c[i] != '<' {
                i += 1;
            }
            if stack.len() < 2 && c[start..i].iter().any(|ch| !ch.is_ascii_whitespace()) {
                return Err(error(
                    "Place authored text inside the explicit <head> or <body>.",
                ));
            }
            continue;
        }
        let tail: String = c[i..c.len().min(i + 12)].iter().collect();
        if tail.starts_with("<!--") {
            let rest: String = c[i + 4..].iter().collect();
            let Some(pos) = rest.find("-->") else {
                return Err(error("Incomplete HTML comment."));
            };
            i += 4 + rest[..pos].chars().count() + 3;
            continue;
        }
        if tail.to_ascii_lowercase().starts_with("<!doctype") {
            if doctype || !nodes.is_empty() || !stack.is_empty() {
                return Err(error(
                    "Only one HTML doctype before the document is supported.",
                ));
            }
            let end = tag_end(&c, i + 2)?;
            let raw: String = c[i + 2..end - 1].iter().collect();
            if !raw.trim().eq_ignore_ascii_case("doctype html") {
                return Err(error("Only an HTML doctype is supported."));
            }
            doctype = true;
            i = end;
            continue;
        }
        if tail.starts_with("<![CDATA[")
            && stack
                .iter()
                .any(|n| matches!(nodes[*n].tag.as_str(), "svg" | "math"))
        {
            let rest: String = c[i + 9..].iter().collect();
            let Some(pos) = rest.find("]]>") else {
                return Err(error("Incomplete CDATA section."));
            };
            i += 9 + rest[..pos].chars().count() + 3;
            continue;
        }
        if tail.starts_with("<?") || tail.starts_with("<![") {
            return Err(error(
                "Processing instructions and ambiguous declarations are not supported.",
            ));
        }
        if i + 1 < c.len() && c[i + 1] == '/' {
            let (tag, p) = name(&c, skip_space(&c, i + 2));
            let end = tag_end(&c, p)?;
            if skip_space(&c, p) != end - 1 {
                return Err(error(format!("Malformed closing tag </{tag}>.")));
            }
            let Some(index) = stack.pop() else {
                return Err(error(format!("unexpected </{tag}>.")));
            };
            if nodes[index].tag != tag {
                return Err(error(format!(
                    "HTML requires explicit, balanced closing tags; unexpected </{tag}>."
                )));
            }
            nodes[index].end = i;
            nodes[index].close_end = end;
            i = end;
            continue;
        }
        let (tag, p) = name(&c, i + 1);
        if tag.is_empty() {
            return Err(error("Malformed HTML start tag."));
        }
        if tag == "plaintext" {
            return Err(error("<plaintext> has no bounded HTML closing span."));
        }
        let end = tag_end(&c, p)?;
        let self_closed = c[i..end - 1]
            .iter()
            .rev()
            .find(|x| !x.is_ascii_whitespace())
            == Some(&'/');
        if self_closed
            && !VOID.contains(&tag.as_str())
            && !stack
                .iter()
                .any(|n| matches!(nodes[*n].tag.as_str(), "svg" | "math"))
            && !matches!(tag.as_str(), "svg" | "math")
        {
            return Err(error(format!("Use an explicit closing tag for <{tag}>.")));
        }
        // Attribute names are checked lexically so duplicate case variants cannot
        // disappear in the HTML5 tree.
        let mut attrs = BTreeSet::new();
        let mut q = p;
        while q < end - 1 {
            q = skip_space(&c, q);
            if q >= end - 1 || c[q] == '/' {
                break;
            }
            let (attr, next) = name(&c, q);
            if attr.is_empty() {
                return Err(error(format!("Malformed attribute on <{tag}>.")));
            }
            if !attrs.insert(attr.clone()) {
                return Err(error(format!("Duplicate HTML attribute on <{tag}>.")));
            }
            q = skip_space(&c, next);
            if q < end - 1 && c[q] == '=' {
                q = skip_space(&c, q + 1);
                if q >= end - 1 {
                    return Err(error(format!("Missing attribute value on <{tag}>.")));
                }
                if matches!(c[q], '\'' | '"') {
                    let quote = c[q];
                    q += 1;
                    while q < end - 1 && c[q] != quote {
                        q += 1;
                    }
                    if q >= end - 1 {
                        return Err(error(format!("Unterminated attribute on <{tag}>.")));
                    }
                    q += 1;
                } else {
                    while q < end - 1 && !c[q].is_ascii_whitespace() && c[q] != '>' {
                        q += 1;
                    }
                }
            }
        }
        let parent = stack.last().copied();
        let index = nodes.len();
        nodes.push(SourceNode {
            tag: tag.clone(),
            start: end,
            end,
            close_end: end,
            self_closed,
            parent,
            children: vec![],
        });
        if let Some(parent) = parent {
            nodes[parent].children.push(index);
        } else {
            roots.push(index);
        }
        if !VOID.contains(&tag.as_str()) && !self_closed {
            stack.push(index);
            if RAW.contains(&tag.as_str()) {
                let lower: String = c[end..].iter().collect::<String>().to_ascii_lowercase();
                let needle = format!("</{tag}");
                let Some(rel) = lower.find(&needle) else {
                    return Err(error(format!("Missing explicit closing tag for <{tag}>.")));
                };
                i = end + lower[..rel].chars().count();
                continue;
            }
        }
        i = end;
    }
    if let Some(index) = stack.last() {
        return Err(error(format!(
            "Missing explicit closing tag for <{}>.",
            nodes[*index].tag
        )));
    }
    if roots.len() != 1 || nodes[roots[0]].tag != "html" {
        return Err(error(
            "Author a complete document with explicit <html>, <head>, and <body> elements.",
        ));
    }
    let root = roots[0];
    let element_children = nodes[root].children.clone();
    if element_children.len() != 2
        || nodes[element_children[0]].tag != "head"
        || nodes[element_children[1]].tag != "body"
    {
        return Err(error(
            "The explicit <html> element must contain <head> followed by <body>.",
        ));
    }
    Ok((nodes, root, element_children[0], element_children[1]))
}

fn dom_tag(handle: &Handle) -> Option<String> {
    match &handle.data {
        NodeData::Element { name, .. } => Some(name.local.to_string().to_ascii_lowercase()),
        _ => None,
    }
}
fn dom_attrs(handle: &Handle) -> BTreeMap<String, String> {
    match &handle.data {
        NodeData::Element { attrs, .. } => attrs
            .borrow()
            .iter()
            .map(|a| {
                let prefix = a
                    .name
                    .prefix
                    .as_ref()
                    .map(|p| format!("{p}:"))
                    .unwrap_or_default();
                (
                    format!("{prefix}{}", a.name.local).to_ascii_lowercase(),
                    a.value
                        .to_string()
                        .replace("\r\n", "\n")
                        .replace('\r', "\n"),
                )
            })
            .collect(),
        _ => BTreeMap::new(),
    }
}
fn dom_text(handle: &Handle, visible: bool) -> String {
    if visible && dom_tag(handle).is_some_and(|tag| hidden(&tag, &dom_attrs(handle))) {
        return String::new();
    }
    let mut out = String::new();
    for child in handle.children.borrow().iter() {
        match &child.data {
            NodeData::Text { contents } => out.push_str(&contents.borrow()),
            NodeData::Element { .. } => out.push_str(&dom_text(child, visible)),
            _ => {}
        }
    }
    out
}
fn cross_check(
    index: usize,
    handle: &Handle,
    nodes: &[SourceNode],
    map: &mut BTreeMap<usize, Handle>,
) -> Result<()> {
    if dom_tag(handle).as_deref() != Some(nodes[index].tag.as_str()) {
        return Err(error(format!(
            "HTML5 parsing changes authored element structure near <{}>.",
            nodes[index].tag
        )));
    }
    map.insert(index, handle.clone());
    let source_children = &nodes[index].children;
    let mut dom_children = Vec::new();
    let mut position = 0usize;
    for child in handle
        .children
        .borrow()
        .iter()
        .filter(|h| matches!(h.data, NodeData::Element { .. }))
    {
        let tag = dom_tag(child).unwrap_or_default();
        let expected = source_children
            .get(position)
            .map(|i| nodes[*i].tag.as_str());
        if matches!(tag.as_str(), "tbody" | "colgroup")
            && expected != Some(tag.as_str())
            && dom_attrs(child).is_empty()
        {
            let nested = child
                .children
                .borrow()
                .iter()
                .filter(|h| matches!(h.data, NodeData::Element { .. }))
                .cloned()
                .collect::<Vec<_>>();
            position += nested.len();
            dom_children.extend(nested);
        } else {
            dom_children.push(child.clone());
            position += 1;
        }
    }
    if source_children.len() != dom_children.len() {
        return Err(error(format!(
            "HTML5 parsing changes the number of authored elements near <{}>.",
            nodes[index].tag
        )));
    }
    for (child, dom) in source_children.iter().zip(dom_children) {
        cross_check(*child, &dom, nodes, map)?;
    }
    Ok(())
}

fn css_is_hidden(style: &str) -> bool {
    // cssparser normalizes escapes before returning identifiers. Track the last
    // declaration, with !important winning, for the five visibility properties.
    let mut chosen = BTreeMap::<String, (String, bool)>::new();
    for declaration in style.split(';') {
        let Some((raw_name, raw_value)) = declaration.split_once(':') else {
            continue;
        };
        let name = css_unescape(raw_name.trim()).to_ascii_lowercase();
        if !matches!(
            name.as_str(),
            "display" | "visibility" | "content-visibility" | "opacity" | "font-size"
        ) {
            continue;
        }
        let mut value = css_unescape(raw_value.trim()).to_ascii_lowercase();
        let important = value.ends_with("!important");
        if important {
            value.truncate(value.len() - "!important".len());
            value = value.trim().into();
        }
        if chosen.get(&name).is_none_or(|(_, old)| important || !*old) {
            chosen.insert(name, (value, important));
        }
    }
    chosen.iter().any(|(name, (value, _))| match name.as_str() {
        "display" => value == "none",
        "visibility" => matches!(value.as_str(), "hidden" | "collapse"),
        "content-visibility" => value == "hidden",
        "opacity" => matches!(value.as_str(), "0" | "0.0" | "0%"),
        "font-size" => value
            .strip_prefix("-0")
            .or_else(|| value.strip_prefix('0'))
            .is_some_and(|tail| {
                tail.is_empty()
                    || tail == ".0"
                    || tail == "%"
                    || tail.chars().all(|c| c.is_ascii_alphabetic() || c == '.')
            }),
        _ => false,
    })
}
fn css_unescape(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '\\' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        i += 1;
        let start = i;
        while i < chars.len() && i - start < 6 && chars[i].is_ascii_hexdigit() {
            i += 1;
        }
        if i > start {
            let hex: String = chars[start..i].iter().collect();
            if let Ok(code) = u32::from_str_radix(&hex, 16) {
                out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
            }
            if i < chars.len() && chars[i].is_ascii_whitespace() {
                i += 1;
            }
        } else if i < chars.len() {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}
fn hidden(tag: &str, attrs: &BTreeMap<String, String>) -> bool {
    INVISIBLE.contains(&tag)
        || attrs.contains_key("hidden")
        || attrs.contains_key("inert")
        || attrs
            .get("aria-hidden")
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("true"))
        || (tag == "input"
            && attrs
                .get("type")
                .is_some_and(|v| v.trim().eq_ignore_ascii_case("hidden")))
        || attrs.get("style").is_some_and(|v| css_is_hidden(v))
}
fn asset_url(value: &str, where_: &str) -> Result<()> {
    let v = value.trim();
    if v.is_empty() || v.starts_with('#') || v.to_ascii_lowercase().starts_with("data:") {
        Ok(())
    } else {
        Err(error(format!(
            "External resource in {where_}; use an inline or data-URI asset: {}",
            &v[..v.len().min(120)]
        )))
    }
}
fn audit_css(css: &str, where_: &str) -> Result<()> {
    fn walk(parser: &mut Parser<'_, '_>, where_: &str) -> Result<()> {
        while let Ok(token) = parser.next_including_whitespace_and_comments() {
            match token {
                Token::AtKeyword(name) if name.eq_ignore_ascii_case("import") => {
                    return Err(error(
                        "CSS @import is not supported; inline the stylesheet in <style>.",
                    ));
                }
                Token::UnquotedUrl(url) => asset_url(url.as_ref(), where_)?,
                Token::Function(name)
                    if matches!(
                        name.to_ascii_lowercase().as_ref(),
                        "url" | "src" | "image" | "image-set" | "-webkit-image-set"
                    ) =>
                {
                    let function = name.to_string();
                    let strict_single =
                        matches!(function.to_ascii_lowercase().as_str(), "url" | "src");
                    parser
                        .parse_nested_block(|nested| {
                            let mut resources = 0usize;
                            while let Ok(inner) = nested.next_including_whitespace_and_comments() {
                                match inner {
                                    Token::WhiteSpace(_) | Token::Comment(_) | Token::Comma => {}
                                    Token::QuotedString(value) | Token::UnquotedUrl(value) => {
                                        asset_url(value.as_ref(), where_)
                                            .map_err(|_| nested.new_custom_error::<(), ()>(()))?;
                                        resources += 1;
                                    }
                                    Token::Function(inner)
                                        if matches!(
                                            inner.to_ascii_lowercase().as_ref(),
                                            "var" | "attr" | "env"
                                        ) =>
                                    {
                                        return Err(nested.new_custom_error::<(), ()>(()));
                                    }
                                    _ if strict_single => {
                                        return Err(nested.new_custom_error::<(), ()>(()));
                                    }
                                    _ => {}
                                }
                            }
                            if strict_single && resources != 1 {
                                return Err(nested.new_custom_error::<(), ()>(()));
                            }
                            Ok(())
                        })
                        .map_err(|_| error(format!("Ambiguous CSS {function}() in {where_}.")))?;
                }
                Token::Function(_)
                | Token::ParenthesisBlock
                | Token::SquareBracketBlock
                | Token::CurlyBracketBlock => {
                    parser
                        .parse_nested_block(|nested| {
                            walk(nested, where_).map_err(|_| nested.new_custom_error::<(), ()>(()))
                        })
                        .map_err(|_| error(format!("Malformed CSS in {where_}.")))?;
                }
                Token::BadUrl(_) | Token::BadString(_) => {
                    return Err(error(format!("Malformed CSS in {where_}.")));
                }
                _ => {}
            }
        }
        Ok(())
    }
    let mut input = ParserInput::new(css);
    walk(&mut Parser::new(&mut input), where_)
}
fn audit_element(tag: &str, attrs: &BTreeMap<String, String>, text: &str) -> Result<()> {
    if matches!(tag, "base" | "iframe" | "object" | "embed") {
        return Err(error(format!(
            "<{tag}> is not supported in a standalone authored document."
        )));
    }
    if attrs.contains_key("xml:base") {
        return Err(error(
            "Remove xml:base; document assets must keep their inline/data references.",
        ));
    }
    if tag == "meta"
        && attrs.get("http-equiv").is_some_and(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "refresh" | "content-security-policy" | "content-security-policy-report-only"
            )
        })
    {
        return Err(error(
            "Remove authored refresh/CSP meta; the document supplies its isolation policy.",
        ));
    }
    if attrs.contains_key("srcset") || attrs.contains_key("imagesrcset") {
        return Err(error("Use one inline/data-URI src instead of srcset."));
    }
    if tag == "link"
        && attrs.get("rel").is_some_and(|v| {
            v.split_whitespace()
                .any(|x| x.eq_ignore_ascii_case("stylesheet"))
        })
    {
        return Err(error(
            "Inline the stylesheet in <style> instead of a stylesheet <link>.",
        ));
    }
    for key in ["src", "poster", "background", "href", "xlink:href"] {
        if let Some(value) = attrs.get(key) {
            if !(tag == "a" && matches!(key, "href" | "xlink:href")) {
                asset_url(value, &format!("<{tag}> {key}"))?;
            }
        }
    }
    if let Some(style) = attrs.get("style") {
        audit_css(style, &format!("<{tag}> style"))?;
    }
    if tag == "style" {
        audit_css(text, "<style>")?;
    }
    Ok(())
}

fn escape_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn patch_html(source: &str, edits: &[Value]) -> Result<String> {
    let length = source.chars().count();
    let mut checked = Vec::new();
    for edit in edits {
        let object = edit.as_object().ok_or_else(|| {
            error("Each HTML edit requires start, end, before_raw and after_raw.")
        })?;
        let start = object
            .get("start")
            .and_then(Value::as_u64)
            .ok_or_else(|| error("HTML edit span is outside the source bounds."))?
            as usize;
        let end = object
            .get("end")
            .and_then(Value::as_u64)
            .ok_or_else(|| error("HTML edit span is outside the source bounds."))?
            as usize;
        let before = object
            .get("before_raw")
            .and_then(Value::as_str)
            .ok_or_else(|| error("HTML edit before_raw and after_raw must be text."))?;
        let after = object
            .get("after_raw")
            .and_then(Value::as_str)
            .ok_or_else(|| error("HTML edit before_raw and after_raw must be text."))?;
        if start > end || end > length || slice_chars(source, start, end) != before {
            return Err(error(
                "HTML edit no longer matches its expected before_raw.",
            ));
        }
        checked.push((start, end, after));
    }
    checked.sort_by_key(|(s, e, _)| (*s, *e));
    for pair in checked.windows(2) {
        if pair[1].0 < pair[0].1 || pair[1].0 == pair[0].0 {
            return Err(error(
                "HTML edit spans overlap or have an ambiguous shared insertion point.",
            ));
        }
    }
    let mut result = source.to_owned();
    for (start, end, after) in checked.into_iter().rev() {
        result.replace_range(
            char_to_byte(&result, start)..char_to_byte(&result, end),
            after,
        );
    }
    Ok(result)
}

pub fn parse_html(source: &str, claim_ids: &[String]) -> Result<Value> {
    if source.len() > super::annotated_document::MAX_ARTIFACT {
        return Err(error("Input exceeds the supported size limit"));
    }
    let mut requested = BTreeSet::new();
    for id in claim_ids {
        if !super::annotated_document::valid_id(id) || !requested.insert(id.clone()) {
            return Err(error("Claim IDs must be unique stable identifiers."));
        }
    }
    let (nodes, source_root, source_head, source_body) = collect_source(source)?;
    let dom: RcDom = parse_document(RcDom::default(), Default::default()).one(source);
    let dom_root = dom
        .document
        .children
        .borrow()
        .iter()
        .find(|h| dom_tag(h).as_deref() == Some("html"))
        .cloned()
        .ok_or_else(|| error("HTML5 parsing did not produce an html element."))?;
    let mut source_to_dom = BTreeMap::new();
    cross_check(source_root, &dom_root, &nodes, &mut source_to_dom)?;
    let mut anchors = Map::new();
    for (i, node) in nodes.iter().enumerate() {
        let handle = &source_to_dom[&i];
        let attrs = dom_attrs(handle);
        let text = dom_text(handle, false);
        audit_element(&node.tag, &attrs, &text)?;
        let Some(id) = attrs.get("data-kpopper-claim") else {
            continue;
        };
        if !super::annotated_document::valid_id(id) {
            return Err(error(format!(
                "Invalid data-kpopper-claim identifier: {id:?}"
            )));
        }
        if anchors.contains_key(id) {
            return Err(error(format!("Duplicate claim anchor: {id}")));
        }
        let mut p = Some(i);
        let mut inside_body = false;
        while let Some(index) = p {
            let h = &source_to_dom[&index];
            let a = dom_attrs(h);
            inside_body |= index == source_body;
            if hidden(&nodes[index].tag, &a) || RAW.contains(&nodes[index].tag.as_str()) {
                return Err(error(format!(
                    "Claim {id} must be visible body text, outside hidden/raw-text/template elements."
                )));
            }
            if index != i && a.contains_key("data-kpopper-claim") {
                return Err(error(format!(
                    "Nested claim anchors are not supported: {id}"
                )));
            }
            p = nodes[index].parent;
        }
        if !inside_body || VOID.contains(&node.tag.as_str()) || node.self_closed {
            return Err(error(format!(
                "Claim {id} requires an explicit body element with an inner text span."
            )));
        }
        let mut descendants = node.children.clone();
        while let Some(child) = descendants.pop() {
            let h = &source_to_dom[&child];
            if hidden(&nodes[child].tag, &dom_attrs(h)) || RAW.contains(&nodes[child].tag.as_str())
            {
                return Err(error(format!(
                    "Claim {id} contains hidden/raw-text/template content."
                )));
            }
            descendants.extend(nodes[child].children.iter().copied());
        }
        let mut context = i;
        let mut p = Some(i);
        while let Some(index) = p {
            if index == source_body {
                break;
            }
            if CONTEXT.contains(&nodes[index].tag.as_str()) {
                context = index;
                break;
            }
            p = nodes[index].parent;
        }
        let context_handle = &source_to_dom[&context];
        let raw = slice_chars(source, node.start, node.end);
        let text_only = node.children.is_empty() && !raw.contains('<');
        anchors.insert(id.clone(), json!({"start":node.start,"end":node.end,"raw":raw,"text":text,"text_only":text_only,"tag":node.tag,"context_start":nodes[context].start,"context_end":nodes[context].end,"context_text":dom_text(context_handle,true)}));
    }
    let found: BTreeSet<_> = anchors.keys().cloned().collect();
    if found != requested {
        return Err(error(format!(
            "Claim anchors must exactly match the manifest (missing: {}; undeclared: {}).",
            requested
                .difference(&found)
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
            found
                .difference(&requested)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    fn coverage(handle: &Handle, owner: Option<usize>, blocks: &mut Vec<String>) {
        let tag = dom_tag(handle).unwrap_or_default();
        let attrs = dom_attrs(handle);
        if hidden(&tag, &attrs) {
            return;
        }
        if attrs.contains_key("data-kpopper-claim") {
            if let Some(owner) = owner {
                blocks[owner].push(' ');
            }
            return;
        }
        let owner = if BLOCK.contains(&tag.as_str()) {
            blocks.push(String::new());
            Some(blocks.len() - 1)
        } else {
            owner
        };
        for child in handle.children.borrow().iter() {
            match &child.data {
                NodeData::Text { contents } => {
                    if let Some(owner) = owner {
                        blocks[owner].push_str(&contents.borrow());
                    }
                }
                NodeData::Element { .. } => coverage(child, owner, blocks),
                _ => {}
            }
        }
    }
    let mut blocks = Vec::new();
    coverage(&source_to_dom[&source_body], None, &mut blocks);
    let blocks: Vec<String> = blocks
        .into_iter()
        .map(|text| text.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|text| !text.is_empty())
        .collect();
    let unmarked_values: usize = blocks
        .iter()
        .map(|text| VALUE_PATTERN.find_iter(text).count())
        .sum();
    let excerpts: Vec<_> = blocks
        .iter()
        .take(8)
        .map(|t| t.chars().take(240).collect::<String>())
        .collect();
    Ok(
        json!({"head_end":nodes[source_head].start,"anchors":anchors,"coverage":{"anchored_claims":anchors.len(),"unmarked_blocks":blocks.len(),"unmarked_values":unmarked_values,"excerpts":excerpts},"_source_root":source_root}),
    )
}

pub fn text_content(fragment: &str) -> String {
    let wrapped = format!("<!doctype html><html><head></head><body>{fragment}</body></html>");
    let dom: RcDom = parse_document(RcDom::default(), Default::default()).one(wrapped);
    fn body(h: &Handle) -> Option<Handle> {
        if dom_tag(h).as_deref() == Some("body") {
            return Some(h.clone());
        }
        h.children.borrow().iter().find_map(body)
    }
    body(&dom.document).map_or_default(|b| dom_text(&b, true))
}

pub fn escaped_replacement(value: &str) -> String {
    escape_text(value)
}
