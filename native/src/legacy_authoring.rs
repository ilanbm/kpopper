//! Byte-preserving writes for existing Simple records without active history.
use crate::{
    Result, history_authority,
    history_contract::*,
    history_transaction::{self as T, FileImage, Layout, PreparedMutation},
    history_transaction_fs as F,
    history_view::map_mut,
    history_yaml::SourceValue as Source,
    ordinary_reader::{Reader, same_legacy},
    project_modes::WriteRoute,
    recording_privacy as Privacy, require, source_document,
    source_inventory::{Inventory, absolute, name},
    value::{Integer, TypedValue as V},
};
use regex::Regex;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::LazyLock,
};

#[path = "legacy_named.rs"]
mod legacy_named;
#[path = "legacy_replaced.rs"]
pub(crate) mod legacy_replaced;
#[path = "legacy_arrangement.rs"]
mod legacy_arrangement;

static TOP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Za-z_][A-Za-z0-9_]*):(?: |$)").unwrap());
static MEMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^( +)([A-Za-z_][A-Za-z0-9_.]*):(?: |$)").unwrap());
static BARE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_.-]*$").unwrap());
static DATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}$").unwrap());

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuthorityRoute {
    History,
    Legacy,
}

fn s(value: &str) -> V {
    V::Text(value.into())
}

fn n(value: &str) -> V {
    V::Integer(Integer::new(value).expect("literal integer"))
}

fn object(items: impl IntoIterator<Item = (&'static str, V)>) -> V {
    V::Map(items.into_iter().map(|(k, v)| (k.into(), v)).collect())
}

fn entry_layout(entry: &Path) -> Result<Layout> {
    Layout::for_entry(name(Path::new(
        entry.file_name().ok_or_else(|| error("invalid_path"))?,
    ))?)
}

/// Resolve only from durable policy and authority state. Invalid or interrupted
/// authority must never fall through to the ordinary writer.
pub(crate) fn authority_route(entry: &Path) -> Result<AuthorityRoute> {
    let layout = entry_layout(entry)?;
    let authority_path = entry
        .parent()
        .ok_or_else(|| error("invalid_path"))?
        .join(layout.authority);
    let authority = F::read(&authority_path)?;
    if let Some(raw) = authority {
        let marker = crate::history_yaml::decode_document(&raw)?;
        history_authority::validate_authority(&marker)?;
        return match text(field(map(&marker)?, "authority")?)? {
            "history" => Ok(AuthorityRoute::History),
            "legacy" => Ok(AuthorityRoute::Legacy),
            _ => Err(error("unsupported_authority")),
        };
    }
    Ok(AuthorityRoute::Legacy)
}

pub(crate) fn route(entry: &Path, config: &V) -> Result<AuthorityRoute> {
    let route = authority_route(entry)?;
    if route == AuthorityRoute::Legacy {
        require(
            string_is(field(map(config)?, "mode")?, "simple"),
            "legacy_authoring_requires_simple_project",
        )?;
    }
    Ok(route)
}

#[derive(Clone, Debug)]
struct Collection {
    name: String,
    start: usize,
    end: usize,
}

#[derive(Clone, Debug)]
struct Member {
    name: String,
    indent: usize,
    start: usize,
    end: usize,
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

fn blank(line: &str) -> bool {
    let line = line.trim();
    line.is_empty() || line.starts_with('#')
}

fn separator_blank(line: &str) -> bool {
    line.trim().is_empty()
}

fn inline(line: &str) -> &str {
    line.split_once(':').map_or("", |(_, v)| v.trim())
}

fn collections(lines: &[String]) -> Vec<Collection> {
    let mut out = Vec::new();
    let mut current: Option<(String, usize)> = None;
    for (index, line) in lines.iter().enumerate() {
        if let Some(found) = TOP.captures(line) {
            if let Some((name, start)) = current.take() {
                out.push(Collection {
                    name,
                    start,
                    end: index,
                });
            }
            current = Some((found[1].into(), index));
        }
    }
    if let Some((name, start)) = current {
        out.push(Collection {
            name,
            start,
            end: lines.len(),
        });
    }
    out
}

fn members(lines: &[String], collection: &Collection) -> Vec<Member> {
    let mut member_indent = None;
    let mut starts = Vec::<(String, usize, usize)>::new();
    for (index, line) in lines
        .iter()
        .enumerate()
        .take(collection.end)
        .skip(collection.start + 1)
    {
        if blank(line) {
            continue;
        }
        let this_indent = indent(line);
        let expected = *member_indent.get_or_insert(this_indent);
        if this_indent == expected
            && let Some(found) = MEMBER.captures(line)
            && found[1].len() == expected
        {
            starts.push((found[2].into(), expected, index));
        }
    }
    starts
        .iter()
        .enumerate()
        .map(|(position, (name, ind, start))| {
            let mut end = starts
                .get(position + 1)
                .map(|(_, _, start)| *start)
                .unwrap_or(collection.end);
            while end > start + 1 && blank(&lines[end - 1]) {
                end -= 1;
            }
            Member {
                name: name.clone(),
                indent: *ind,
                start: *start,
                end,
            }
        })
        .collect()
}

fn locate(lines: &[String], id: &str) -> Option<(Collection, Member)> {
    for collection in collections(lines) {
        if let Some(member) = members(lines, &collection)
            .into_iter()
            .find(|member| member.name == id)
        {
            return Some((collection, member));
        }
    }
    None
}

fn field_span(lines: &[String], member: &Member, field: &str) -> Option<Member> {
    let collection = Collection {
        name: String::new(),
        start: member.start,
        end: member.end,
    };
    members(lines, &collection)
        .into_iter()
        .find(|item| item.name == field)
}

#[derive(Clone, Copy)]
enum Style {
    Bare,
    Single,
    Double,
    Date,
}

fn style(raw: &str) -> Style {
    let raw = raw.trim();
    if raw.starts_with('\'') {
        Style::Single
    } else if raw.starts_with('"') {
        Style::Double
    } else if DATE.is_match(raw) {
        Style::Date
    } else {
        Style::Bare
    }
}

fn bare_ok(value: &str) -> bool {
    BARE.is_match(value)
        && !matches!(
            value.to_ascii_lowercase().as_str(),
            "null" | "true" | "false" | "yes" | "no" | "on" | "off" | "~"
        )
}

fn scalar(value: &V, style: Style) -> Result<String> {
    scalar_at(value, style, 0, false)
}

fn scalar_at(value: &V, style: Style, col: usize, fold: bool) -> Result<String> {
    Ok(match value {
        V::Null => "null".into(),
        V::Bool(value) => value.to_string(),
        V::Integer(value) => value.as_str().into(),
        V::Float(value) => crate::identity::python_float(value.get()),
        V::Text(value) => text_scalar(value, style, col, fold),
        V::Date(value) => match style {
            Style::Date => value.as_str().into(),
            _ => text_scalar(value.as_str(), style, col, fold),
        },
        V::DateTime(value) => text_scalar(value.as_str(), style, col, fold),
        V::List(_) | V::Map(_) => return Err(error("container_requires_field_emission")),
    })
}

fn text_scalar(value: &str, style: Style, col: usize, fold: bool) -> String {
    if matches!(style, Style::Date) && DATE.is_match(value) {
        return value.into();
    }
    if matches!(style, Style::Single) && !value.contains('\n') {
        return format!("'{}'", value.replace('\'', "''"));
    }
    if matches!(style, Style::Bare | Style::Date) && bare_ok(value) {
        return value.into();
    }
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    let quoted = format!("\"{escaped}\"");
    if !fold || col + quoted.chars().count() <= 100 {
        return quoted;
    }
    let chars = quoted.char_indices().collect::<Vec<_>>();
    let mut words = Vec::new();
    let mut start = 0;
    for (i, (at, ch)) in chars.iter().enumerate() {
        if *ch == ' '
            && i > 0
            && i + 1 < chars.len()
            && !python_whitespace(chars[i - 1].1)
            && !python_whitespace(chars[i + 1].1)
        {
            words.push(&quoted[start..*at]);
            start = *at + 1;
        }
    }
    words.push(&quoted[start..]);
    let mut lines = Vec::new();
    let mut current = words[0].to_owned();
    for word in &words[1..] {
        if col + current.chars().count() + 1 + word.chars().count() <= 100 {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(current);
            current = (*word).into();
        }
    }
    lines.push(current);
    lines.join(&format!("\n{}", " ".repeat(col + 1)))
}

fn safe_key(key: &str) -> Result<String> {
    require(!key.starts_with('\0'), "invalid_ordinary_authored_key")?;
    Ok(if bare_ok(key) {
        key.into()
    } else {
        text_scalar(key, Style::Double, 0, false)
    })
}

fn python_whitespace(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// Preserve source order through the existing SafeDumper emitter.
pub(crate) fn dump_ordered_yaml(
    value: &crate::history_yaml::SourceValue,
    width: usize,
) -> Result<String> {
    String::from_utf8(crate::history_emit::encode_source(value, width)?)
        .map_err(|_| error("nontext_yaml_output"))
}

fn field_lines_ordered(
    field: &str,
    value: &crate::history_yaml::SourceValue,
    ind: usize,
) -> Result<Vec<String>> {
    use crate::history_yaml::SourceValue as S;
    let key = safe_key(field)?;
    let prefix = format!("{}{key}: ", " ".repeat(ind));
    let nested = match value {
        S::List(values) => values.iter().any(|v| !matches!(v, S::Scalar(_))),
        S::Map(values) => values.iter().any(|(_, v)| !matches!(v, S::Scalar(_))),
        _ => false,
    };
    if nested {
        let document = S::Map(vec![(field.into(), value.clone())]);
        return Ok(dump_ordered_yaml(&document, 100usize.saturating_sub(ind))?
            .trim_end()
            .lines()
            .map(|line| format!("{}{line}", " ".repeat(ind)))
            .collect());
    }
    match value {
        S::Scalar(value) => Ok(format!(
            "{prefix}{}",
            scalar_at(value, Style::Bare, prefix.chars().count(), true)?
        )
        .split('\n')
        .map(str::to_owned)
        .collect()),
        S::List(values) => Ok(vec![format!(
            "{prefix}[{}]",
            values
                .iter()
                .map(|v| scalar(&v.typed(), Style::Bare))
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        )]),
        S::Map(values) => {
            let pairs = values
                .iter()
                .map(|(key, value)| {
                    Ok(format!(
                        "{}: {}",
                        safe_key(key)?,
                        scalar(&value.typed(), Style::Bare)?
                    ))
                })
                .collect::<Result<Vec<_>>>()?;
            let flow = format!("{prefix}{{{}}}", pairs.join(", "));
            if flow.chars().count() <= 100 {
                return Ok(vec![flow]);
            }
            let mut lines = vec![format!("{}{key}:", " ".repeat(ind))];
            for (key, value) in values {
                lines.extend(field_lines_ordered(key, value, ind + 2)?);
            }
            Ok(lines)
        }
    }
}

pub(crate) fn entry_lines_ordered(
    id: &str,
    body: &crate::history_yaml::SourceValue,
    ind: usize,
    field_indent: usize,
    flow: bool,
) -> Result<Vec<String>> {
    use crate::history_yaml::SourceValue as S;
    safe_key(id)?;
    let S::Map(fields) = body else {
        return field_lines_ordered(id, body, ind);
    };
    if flow && fields.iter().all(|(_, v)| matches!(v, S::Scalar(_))) {
        let pairs = fields
            .iter()
            .map(|(key, value)| {
                Ok(format!(
                    "{}: {}",
                    safe_key(key)?,
                    scalar(&value.typed(), Style::Bare)?
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        let line = format!("{}{id}: {{{}}}", " ".repeat(ind), pairs.join(", "));
        if line.chars().count() <= 100 {
            return Ok(vec![line]);
        }
    }
    let mut lines = vec![format!("{}{id}:", " ".repeat(ind))];
    for (key, value) in fields {
        lines.extend(field_lines_ordered(key, value, field_indent)?);
    }
    Ok(lines)
}

/// Replace just the fields changed by explicit ordinary expression migration.
/// Source order, untouched fields and newline handling remain the caller's inputs.
pub(crate) fn replace_fields(
    lines: &mut Vec<String>,
    id: &str,
    body: &Source,
    was: &Source,
) -> Result<()> {
    let (_, mut member) =
        locate(lines, id).ok_or_else(|| error(&format!("no file of the record holds {id}")))?;
    let group = Collection {
        name: String::new(),
        start: member.start,
        end: member.end,
    };
    let field_indent = members(lines, &group)
        .first()
        .map(|m| m.indent)
        .unwrap_or(member.indent + 2);
    if inline(&lines[member.start]).starts_with('{') {
        let comments = lines[member.start + 1..member.end]
            .iter()
            .filter(|l| l.trim().starts_with('#'))
            .cloned()
            .collect::<Vec<_>>();
        let mut replacement = entry_lines_ordered(id, body, member.indent, field_indent, true)?;
        replacement.extend(comments);
        lines.splice(member.start..member.end, replacement);
        return Ok(());
    }
    let (Source::Map(body), Source::Map(was)) = (body, was) else {
        return Err(error("migration body must be a mapping"));
    };
    for (field, _) in was {
        if field != "also"
            && !body.iter().any(|(key, _)| key == field)
            && let Some(span) = field_span(lines, &member, field)
        {
            let removed = span.end - span.start;
            lines.drain(span.start..span.end);
            member.end -= removed;
        }
    }
    for (field, value) in body {
        if was
            .iter()
            .any(|(key, old)| key == field && old.typed() == value.typed())
        {
            continue;
        }
        let replacement = field_lines_ordered(field, value, field_indent)?;
        if let Some(span) = field_span(lines, &member, field) {
            let end = member.end + replacement.len() - (span.end - span.start);
            lines.splice(span.start..span.end, replacement);
            member.end = end;
        } else {
            let end = member.end + replacement.len();
            lines.splice(member.end..member.end, replacement);
            member.end = end;
        }
    }
    Ok(())
}

fn common_prefix(left: &str, right: &str) -> usize {
    left.split('.')
        .zip(right.split('.'))
        .take_while(|(left, right)| left == right)
        .count()
}

fn insert_entry(
    lines: &mut Vec<String>,
    collection_name: &str,
    id: &str,
    body: &Source,
) -> Result<String> {
    let collection = collections(lines)
        .into_iter()
        .find(|collection| collection.name == collection_name)
        .ok_or_else(|| error(&format!("no collection {collection_name} in this file")))?;
    let existing = members(lines, &collection);
    if existing.is_empty() {
        let new = entry_lines_ordered(id, body, 2, 4, false)?;
        lines.splice(collection.start + 1..collection.start + 1, new);
        return Ok(format!("{id} into {collection_name}, its first entry"));
    }
    let best = existing
        .iter()
        .map(|member| common_prefix(&member.name, id))
        .max()
        .unwrap_or(0);
    let siblings = existing
        .iter()
        .filter(|member| best == 0 || common_prefix(&member.name, id) == best)
        .collect::<Vec<_>>();
    let (anchor, before) = siblings
        .iter()
        .find(|member| member.name.as_str() > id)
        .map(|member| (*member, true))
        .unwrap_or((*siblings.last().unwrap(), false));
    let field_indent = field_span(lines, anchor, "v")
        .or_else(|| field_span(lines, anchor, "quoted"))
        .map_or(anchor.indent + 2, |field| field.indent);
    let flow = inline(&lines[anchor.start]).starts_with('{');
    let mut new = entry_lines_ordered(id, body, anchor.indent, field_indent, flow)?;
    let position = if before {
        if anchor.start > collection.start + 1
            && separator_blank(&lines[anchor.start - 1])
            && lines[anchor.start - 1].trim().is_empty()
        {
            new.push(String::new());
        }
        anchor.start
    } else {
        let mut position = anchor.end;
        if position < collection.end && separator_blank(&lines[position]) {
            position += 1;
            new.push(String::new());
        }
        position
    };
    lines.splice(position..position, new);
    Ok(format!(
        "{id} into {collection_name}, {} {}",
        if before { "before" } else { "after" },
        anchor.name
    ))
}

fn replace_entry(lines: &mut Vec<String>, id: &str, body: &Source) -> Result<()> {
    let (_, member) = locate(lines, id).ok_or_else(|| error("entry_not_found"))?;
    let flow = inline(&lines[member.start]).starts_with('{');
    let field_indent = field_span(lines, &member, "v")
        .or_else(|| field_span(lines, &member, "quoted"))
        .map_or(member.indent + 2, |field| field.indent);
    let replacement = entry_lines_ordered(id, body, member.indent, field_indent, flow)?;
    lines.splice(member.start..member.end, replacement);
    Ok(())
}

fn ensure_collection(lines: &mut Vec<String>, collection: &str) {
    if collections(lines)
        .iter()
        .any(|item| item.name == collection)
    {
        return;
    }
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    if !lines.is_empty() {
        lines.push(String::new());
    }
    lines.push(format!("{collection}:"));
    lines.push(String::new());
}

fn replace_field(
    lines: &mut Vec<String>,
    member: &Member,
    field: &str,
    value: &V,
) -> Result<usize> {
    replace_field_ordered(lines, member, field, &Source::from_typed(value))
}

fn replace_field_ordered(
    lines: &mut Vec<String>,
    member: &Member,
    field: &str,
    value: &Source,
) -> Result<usize> {
    if let Some(span) = field_span(lines, member, field) {
        let replacement = field_lines_ordered(field, value, span.indent)?;
        let new_end = member.end + replacement.len() - (span.end - span.start);
        lines.splice(span.start..span.end, replacement);
        Ok(new_end)
    } else {
        let replacement = field_lines_ordered(field, value, member.indent + 2)?;
        let len = replacement.len();
        let mut insertion = member.start + 1;
        for candidate in ["v", "quoted", "of", "from", "at"] {
            if let Some(span) = field_span(lines, member, candidate) {
                insertion = insertion.max(span.end);
            }
        }
        lines.splice(insertion..insertion, replacement);
        Ok(insertion + len)
    }
}

fn replace_date_field(
    lines: &mut Vec<String>,
    member: &Member,
    field: &str,
    stamp: &str,
) -> Result<usize> {
    if let Some(span) = field_span(lines, member, field) {
        require(span.end == span.start + 1, "invalid_date_field")?;
        let re = Regex::new(&format!(
            r"^(\s+{}:\s*)(\S+)(\s*(?:#.*)?)$",
            regex::escape(field)
        ))
        .unwrap();
        let found = re
            .captures(&lines[span.start])
            .ok_or_else(|| error("invalid_date_field"))?;
        lines[span.start] = format!(
            "{}{}{}",
            &found[1],
            scalar(&s(stamp), style(&found[2]))?,
            &found[3]
        );
        Ok(member.end)
    } else {
        let replacement = vec![format!(
            "{}{}: \"{stamp}\"",
            " ".repeat(member.indent + 2),
            field
        )];
        lines.splice(member.end..member.end, replacement);
        Ok(member.end + 1)
    }
}

fn flow_scalar_span(block: &str, field: &str) -> Result<(usize, usize)> {
    use libyaml_safer::{Scanner, TokenData};
    let mut input = block.as_bytes();
    let mut scanner = Scanner::new();
    scanner.set_input_string(&mut input);
    let tokens = scanner
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| error("invalid_history_yaml"))?;
    let mut depth = 0_i32;
    for index in 0..tokens.len() {
        match tokens[index].data {
            TokenData::BlockMappingStart | TokenData::FlowMappingStart => depth += 1,
            TokenData::BlockEnd | TokenData::FlowMappingEnd => depth -= 1,
            TokenData::Key if depth == 2 => {
                let Some(key) = tokens.get(index + 1) else {
                    continue;
                };
                let TokenData::Scalar {
                    value: ref key_value,
                    ..
                } = key.data
                else {
                    continue;
                };
                if key_value != field {
                    continue;
                }
                let Some(old) = tokens.get(index + 3) else {
                    break;
                };
                let TokenData::Scalar { .. } = old.data else {
                    return Err(error(&format!("{field} is not a scalar")));
                };
                let start =
                    usize::try_from(old.start_mark.index).map_err(|_| error("source_limit"))?;
                let end = usize::try_from(old.end_mark.index).map_err(|_| error("source_limit"))?;
                return Ok((start, end));
            }
            _ => {}
        }
    }
    Err(error(&format!("field_not_found:{field}")))
}

fn replace_scalar_in_flow(block: &str, field: &str, value: &V) -> Result<(String, String)> {
    let (start, end) = flow_scalar_span(block, field)?;
    let before = &block[start..end];
    let replacement = scalar(value, style(before))?;
    Ok((
        format!("{}{}{}", &block[..start], replacement, &block[end..]),
        before.into(),
    ))
}

fn upsert_scalar_in_flow(block: &str, field: &str, value: &V) -> Result<String> {
    if let Ok((block, _)) = replace_scalar_in_flow(block, field, value) {
        return Ok(block);
    }
    use libyaml_safer::{Scanner, TokenData};
    let mut input = block.as_bytes();
    let mut scanner = Scanner::new();
    scanner.set_input_string(&mut input);
    let tokens = scanner
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| error("invalid_history_yaml"))?;
    let opening = tokens
        .iter()
        .find(|token| matches!(token.data, TokenData::FlowMappingStart))
        .ok_or_else(|| error("invalid_history_yaml"))?;
    let at = usize::try_from(opening.end_mark.index).map_err(|_| error("source_limit"))?;
    let separator = if block[at..].trim_start().starts_with('}') {
        ""
    } else {
        ", "
    };
    Ok(format!(
        "{}{field}: {}{separator}{}",
        &block[..at],
        scalar(value, Style::Bare)?,
        &block[at..]
    ))
}

fn set_entry(
    lines: &mut Vec<String>,
    id: &str,
    value: &V,
    stamp: &str,
    why: Option<&str>,
    source: Option<&str>,
    at: Option<&str>,
) -> Result<String> {
    let (_, member) = locate(lines, id)
        .ok_or_else(|| error(&format!("refused - no file of the record holds {id}")))?;
    if inline(&lines[member.start]).starts_with('{') {
        let block = lines[member.start..member.end].join("\n");
        let (mut block, old) = replace_scalar_in_flow(&block, "v", value)
            .or_else(|_| replace_scalar_in_flow(&block, "quoted", value))?;
        let date = s(stamp);
        block = match replace_scalar_in_flow(&block, "of", &date) {
            Ok((block, _)) => block,
            Err(_) => {
                let (_, end) = flow_scalar_span(&block, "v")
                    .or_else(|_| flow_scalar_span(&block, "quoted"))?;
                format!("{}, of: \"{stamp}\"{}", &block[..end], &block[end..])
            }
        };
        if source.is_some() || at.is_some() {
            let source = source.ok_or_else(|| error("--source and --at go together"))?;
            let at = at.ok_or_else(|| error("--source and --at go together"))?;
            block = upsert_scalar_in_flow(&block, "from", &s(source))?;
            block = upsert_scalar_in_flow(&block, "at", &s(at))?;
        }
        let mut replacement = block.split('\n').map(str::to_owned).collect::<Vec<_>>();
        if let Some(why) = why {
            replacement.push(format!(
                "{}# set {stamp}: {why}",
                " ".repeat(member.indent + 2)
            ));
        }
        lines.splice(member.start..member.end, replacement);
        return Ok(old);
    }
    let field = field_span(lines, &member, "v")
        .or_else(|| field_span(lines, &member, "quoted"))
        .ok_or_else(|| error(&format!("{id} carries no v: this reader can rewrite")))?;
    let mut old_parts = inline(&lines[field.start])
        .split(python_whitespace)
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    old_parts.extend(
        lines[field.start + 1..field.end]
            .iter()
            .map(|line| line.trim().to_owned()),
    );
    let old = old_parts.join(" ");
    let value_field = lines[field.start]
        .trim()
        .split_once(':')
        .map(|(name, _)| name)
        .ok_or_else(|| error("invalid_history_yaml"))?
        .to_owned();
    let replacement = format!(
        "{}{}: {}",
        " ".repeat(field.indent),
        value_field,
        scalar_at(
            value,
            style(&old),
            field.indent + value_field.chars().count() + 2,
            true
        )?
    );
    let mut replacement_lines = replacement
        .split('\n')
        .map(str::to_owned)
        .collect::<Vec<_>>();
    replacement_lines.extend(
        lines[field.start..field.end]
            .iter()
            .filter(|line| line.trim_start().starts_with('#'))
            .cloned(),
    );
    lines.splice(field.start..field.end, replacement_lines);
    let (_, member) = locate(lines, id).unwrap();
    let had_of = field_span(lines, &member, "of").is_some();
    if let Some(of) = field_span(lines, &member, "of") {
        require(
            of.end == of.start + 1,
            "multiline_date_authoring_unsupported",
        )?;
        let old_date = inline(&lines[of.start]);
        lines[of.start] = format!(
            "{}of: {}",
            " ".repeat(of.indent),
            scalar(&s(stamp), style(old_date))?
        );
    } else {
        let insertion = field.start + 1;
        lines.insert(
            insertion,
            format!("{}of: \"{stamp}\"", " ".repeat(field.indent)),
        );
    }
    if source.is_some() || at.is_some() {
        for (name, value) in [("from", source), ("at", at)] {
            let value = value.ok_or_else(|| error("--source and --at go together"))?;
            let (_, member) = locate(lines, id).unwrap();
            if field_span(lines, &member, name).is_some() {
                replace_field(lines, &member, name, &s(value))?;
            } else {
                let date = field_span(lines, &member, "of")
                    .ok_or_else(|| error("set date field is unavailable"))?;
                let added = field_lines_ordered(name, &Source::Scalar(s(value)), date.indent)?;
                lines.splice(date.end..date.end, added);
            }
        }
    }
    if let Some(why) = why {
        let (_, member) = locate(lines, id).unwrap();
        let insertion = if had_of {
            field_span(lines, &member, &value_field).unwrap().end
        } else {
            field_span(lines, &member, "of").unwrap().end
        };
        lines.insert(
            insertion,
            format!("{}# set {stamp}: {why}", " ".repeat(field.indent)),
        );
    }
    Ok(old)
}

fn bump_updated(lines: &mut Vec<String>, stamp: &str) -> Result<()> {
    let Some(meta) = collections(lines)
        .into_iter()
        .find(|item| item.name == "meta")
    else {
        return Ok(());
    };
    if inline(&lines[meta.start]).starts_with('{') {
        let block = lines[meta.start..meta.end].join("\n");
        let block = match replace_scalar_in_flow(&block, "updated", &s(stamp)) {
            Ok((block, _)) => block,
            Err(_) => {
                let opening = block
                    .find('{')
                    .ok_or_else(|| error("invalid_history_yaml"))?
                    + 1;
                let separator = if block[opening..].trim_start().starts_with('}') {
                    ""
                } else {
                    ", "
                };
                format!(
                    "{}updated: \"{stamp}\"{separator}{}",
                    &block[..opening],
                    &block[opening..]
                )
            }
        };
        lines.splice(meta.start..meta.end, block.split('\n').map(str::to_owned));
        return Ok(());
    }
    let pseudo = Member {
        name: "meta".into(),
        indent: 0,
        start: meta.start,
        end: meta.end,
    };
    if let Some(updated) = field_span(lines, &pseudo, "updated") {
        require(updated.end == updated.start + 1, "invalid_updated_field")?;
        let line = &lines[updated.start];
        let re = Regex::new(r"^(\s+updated:\s*)(\S+)(\s*(?:#.*)?)$").unwrap();
        let found = re
            .captures(line)
            .ok_or_else(|| error("invalid_updated_field"))?;
        lines[updated.start] = format!(
            "{}{}{}",
            &found[1],
            scalar(&s(stamp), style(&found[2]))?,
            &found[3]
        );
    } else {
        let updated_indent = members(lines, &meta)
            .first()
            .map(|member| member.indent)
            .unwrap_or(2);
        lines.insert(
            meta.start + 1,
            format!("{}updated: {stamp}", " ".repeat(updated_indent)),
        );
    }
    Ok(())
}

fn projected_document(source: &crate::history_yaml::OrdinaryValue) -> V {
    source.projected()
}

fn action_stamp(action: &Map) -> String {
    action
        .get("as_of")
        .and_then(|value| text(value).ok())
        .map(str::to_owned)
        .unwrap_or_else(|| chrono::Local::now().date_naive().to_string())
}

fn snapshot_value(reader: &Reader<'_>, dependency: &str, deps_field: &str) -> Result<V> {
    let body = reader.raw().get(dependency).unwrap_or(&V::Null);
    if let Ok(body) = map(body)
        && body.contains_key(deps_field)
    {
        return Ok(body
            .get("verdict")
            .or_else(|| body.get("title"))
            .cloned()
            .unwrap_or_else(|| s(dependency)));
    }
    let value = reader.value(dependency)?;
    if value != V::Null {
        return Ok(value);
    }
    if let Ok(body) = map(body) {
        if let Some(rule) = body.get("rule").or_else(|| body.get("v"))
            && matches!(rule, V::Text(_) | V::Map(_))
        {
            return Ok(rule.clone());
        }
        if let Some(day) = body.get("read").or_else(|| body.get("of")) {
            return Ok(s(&format!("read {}", display(day)?)));
        }
        if let Some(name) = body.get("name").or_else(|| body.get("title")) {
            return Ok(name.clone());
        }
        return Ok(s("present"));
    }
    Ok(body.clone())
}

fn dependency_snapshot(reader: &Reader<'_>, body: &Map) -> Result<Map> {
    let deps_field = text(&reader.fields()["deps"])?;
    let dependencies = body
        .get(deps_field)
        .and_then(|value| match value {
            V::List(values) => Some(values),
            _ => None,
        })
        .ok_or_else(|| error("invalid_dependencies"))?;
    dependencies
        .iter()
        .map(|dependency| {
            let dependency = text(dependency)?;
            Ok((
                dependency.into(),
                snapshot_value(reader, dependency, deps_field)?,
            ))
        })
        .collect()
}

pub(crate) fn preserve_order(value: &V, source: Option<&Source>) -> Source {
    match value {
        V::Map(values) => {
            let mut ordered = Vec::new();
            if let Some(Source::Map(template)) = source {
                for (key, prior) in template {
                    if let Some(value) = values.get(key) {
                        ordered.push((key.clone(), preserve_order(value, Some(prior))));
                    }
                }
            }
            for (key, value) in values {
                if !ordered.iter().any(|(old, _)| old == key) {
                    ordered.push((key.clone(), preserve_order(value, None)));
                }
            }
            Source::Map(ordered)
        }
        V::List(values) => Source::List(
            values
                .iter()
                .enumerate()
                .map(|(i, value)| {
                    let prior = match source {
                        Some(Source::List(a)) => a.get(i),
                        _ => None,
                    };
                    preserve_order(value, prior)
                })
                .collect(),
        ),
        _ => Source::Scalar(value.clone()),
    }
}

fn ordinary_template(value: &crate::history_yaml::OrdinaryValue) -> Option<Source> {
    use crate::history_yaml::OrdinaryValue as O;
    Some(match value {
        O::Scalar(value) => Source::Scalar(value.clone()),
        O::List(values) => Source::List(
            values
                .iter()
                .map(ordinary_template)
                .collect::<Option<_>>()?,
        ),
        O::Map(values) => Source::Map(
            values
                .iter()
                .map(|(key, value)| Some((key.text()?.into(), ordinary_template(value)?)))
                .collect::<Option<_>>()?,
        ),
    })
}

fn ordered_snapshot(
    reader: &Reader<'_>,
    body: &Map,
    seen: &Map,
    source: &crate::history_yaml::OrdinaryValue,
) -> Result<Source> {
    let deps = text(&reader.fields()["deps"])?;
    let V::List(dependencies) = &body[deps] else {
        return Err(error("invalid_dependencies"));
    };
    let mut ordered = Vec::new();
    for dependency in dependencies {
        let id = text(dependency)?;
        let value = field(seen, id)?;
        let prior = if let crate::history_yaml::OrdinaryValue::Map(collections) = source {
            collections
                .iter()
                .find_map(|(_, entries)| entries.get(id))
                .and_then(|body| body.get("v").or_else(|| body.get("quoted")))
                .filter(|prior| authored_equal(&prior.projected(), value))
                .and_then(ordinary_template)
        } else {
            None
        };
        if !ordered.iter().any(|(old, _)| old == id) {
            ordered.push((id.to_owned(), preserve_order(value, prior.as_ref())));
        }
    }
    Ok(Source::Map(ordered))
}

fn display(value: &V) -> Result<String> {
    Ok(match value {
        V::Text(value) => value.clone(),
        V::Date(value) => value.as_str().into(),
        V::DateTime(value) => value.as_str().replacen('T', " ", 1),
        V::Null => "None".into(),
        V::Bool(value) => if *value { "True" } else { "False" }.into(),
        V::Integer(value) => value.as_str().into(),
        V::Float(value) => crate::identity::python_float(value.get()),
        V::List(_) | V::Map(_) => crate::source_text::ordinary_python_str(value),
    })
}

fn authored_equal(left: &V, right: &V) -> bool {
    match (left, right) {
        (V::Date(left), V::Text(right)) | (V::Text(right), V::Date(left)) => left.as_str() == right,
        (V::DateTime(left), V::Text(right)) | (V::Text(right), V::DateTime(left)) => {
            left.as_str() == right
        }
        (V::List(left), V::List(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| authored_equal(left, right))
        }
        (V::Map(left), V::Map(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, left)| {
                    right
                        .get(key)
                        .is_some_and(|right| authored_equal(left, right))
                })
        }
        _ => left == right,
    }
}

fn collection_for(reader: &Reader<'_>, action: &Map) -> Result<String> {
    let candidate = reader.candidate(&V::Map(action.clone()))?;
    let id = text(field(action, "id")?)?;
    crate::reasoning_fields::collections(&candidate)?
        .into_iter()
        .find(|(_, members)| members.contains_key(id))
        .map(|(name, _)| name)
        .ok_or_else(|| error("invalid_candidate"))
}

fn verify_untouched_document(before: &V, after: &V, collection: &str, id: &str) -> Result<()> {
    let mut expected = before.clone();
    let mut actual = after.clone();
    for value in [&mut expected, &mut actual] {
        let doc = map_mut(value)?;
        if let Some(V::Map(entries)) = doc.get_mut(collection) {
            entries.remove(id);
        }
        if let Some(V::Map(meta)) = doc.get_mut("meta") {
            meta.remove("updated");
        }
    }
    if map(&expected)?.get(collection) == Some(&V::Null)
        && map(&actual)?
            .get(collection)
            .is_some_and(|v| matches!(v, V::Map(m) if m.is_empty()))
    {
        map_mut(&mut expected)?.insert(collection.into(), V::Map(Map::new()));
    }
    // An add may open its collection, and every write may create meta.updated.
    for key in [collection, "meta"] {
        if !map(before)?.contains_key(key)
            && map(&actual)?
                .get(key)
                .is_some_and(|v| matches!(v, V::Map(m) if m.is_empty()))
        {
            map_mut(&mut actual)?.remove(key);
        }
    }
    require(
        authored_equal(&expected, &actual),
        "the write broke the record and was undone: unrelated content changed",
    )
}

fn owner_for(document: &source_document::Document, collection: &str, id: &str) -> Option<PathBuf> {
    document
        .origins
        .get(collection)
        .and_then(|m| m.get(id))
        .cloned()
}

fn choose_add_owner(
    document: &source_document::Document,
    inventory: &Inventory,
    collection: &str,
    id: &str,
) -> Result<PathBuf> {
    let head = id.split('.').next().unwrap_or(id);
    let (mut opened, mut own, mut held) = (None, None, None);
    for path in &document.members {
        let bytes = inventory
            .files
            .get(path)
            .ok_or_else(|| error("snapshot_changed"))?;
        let raw = std::str::from_utf8(bytes)
            .map_err(|_| error("nontext_record_authoring_unsupported"))?;
        let lines = raw.split('\n').map(str::to_owned).collect::<Vec<_>>();
        if locate(&lines, id).is_some() {
            return Ok(path.clone());
        }
        let mut theirs = Vec::new();
        for group in collections(&lines) {
            let entries = if group.name == "meta" {
                Vec::new()
            } else {
                members(&lines, &group)
                    .into_iter()
                    .filter(|m| m.name.contains('.'))
                    .collect::<Vec<_>>()
            };
            if group.name == collection {
                if entries
                    .iter()
                    .any(|m| m.name.split('.').next() == Some(head))
                {
                    return Ok(path.clone());
                }
                opened.get_or_insert_with(|| path.clone());
            }
            theirs.extend(entries);
        }
        if !theirs.is_empty() {
            held.get_or_insert_with(|| path.clone());
            if theirs
                .iter()
                .all(|m| m.name.split('.').next() == Some(head))
            {
                own.get_or_insert_with(|| path.clone());
            }
        }
    }
    own.or(opened)
        .or(held)
        .or_else(|| document.members.first().cloned())
        .ok_or_else(|| error("missing_record"))
}

fn add_collection_if_needed(lines: &mut Vec<String>, collection: &str) {
    ensure_collection(lines, collection);
}

fn update_review_candidate(
    document: &V,
    collection: &str,
    id: &str,
    seen_field: &str,
    seen: &Map,
    stamp: Option<&str>,
) -> Result<V> {
    let mut candidate = document.clone();
    let body = map_mut(
        map_mut(&mut candidate)?
            .get_mut(collection)
            .ok_or_else(|| error("invalid_candidate"))?,
    )?
    .get_mut(id)
    .ok_or_else(|| error("invalid_candidate"))?;
    let body = map_mut(body)?;
    body.insert(seen_field.into(), V::Map(seen.clone()));
    if let Some(stamp) = stamp {
        body.insert("reviewed".into(), s(stamp));
    }
    Ok(candidate)
}

fn common_root(entry: &Path, members: &[PathBuf]) -> Result<PathBuf> {
    let mut root = entry
        .parent()
        .ok_or_else(|| error("invalid_path"))?
        .to_path_buf();
    for member in members {
        let parent = member.parent().ok_or_else(|| error("invalid_path"))?;
        while !parent.starts_with(&root) {
            require(root.pop(), "invalid_path")?;
        }
    }
    Ok(root)
}

fn relative(root: &Path, path: &Path) -> Result<String> {
    let path = absolute(path)?;
    // Resolve directory aliases (including macOS /var) without resolving a
    // final file symlink, which publication must still reject.
    let path = crate::project_modes::resolved(
        path.parent().ok_or_else(|| error("invalid_path"))?,
    )?
    .join(path.file_name().ok_or_else(|| error("invalid_path"))?);
    let root = crate::project_modes::resolved(root)?;
    let relative = path
        .strip_prefix(&root)
        .map_err(|_| error("invalid_path"))?;
    let value = name(relative)?.replace('\\', "/");
    history_authority::relative_path(&value)?;
    Ok(value)
}

fn legacy_authority(entry: &Path) -> Result<V> {
    let id = format!(
        "legacy-{}",
        &crate::identity::sha256(name(&absolute(entry)?)?.as_bytes())[..32]
    );
    history_authority::authority(&id, "legacy", &n("0"), Map::new())
}

fn authority(entry: &Path) -> Result<V> {
    let layout = entry_layout(entry)?;
    let path = entry.parent().unwrap().join(layout.authority);
    if let Some(raw) = F::read(&path)? {
        let value = crate::history_yaml::decode_document(&raw)?;
        history_authority::validate_authority(&value)?;
        require(
            string_is(&map(&value)?["authority"], "legacy"),
            "history_direct_writer_unsupported: use the history writer",
        )?;
        Ok(value)
    } else {
        legacy_authority(entry)
    }
}

pub(crate) struct Prepared {
    pub(crate) inventory: Inventory,
    pub(crate) mutation: PreparedMutation,
    pub(crate) output: String,
    pub(crate) root: PathBuf,
    pub(crate) journal: String,
    pub(crate) subject: String,
    pub(crate) diagnostics: Vec<String>,
    pub(crate) page: Option<crate::ordinary_page_capture::PageCapture>,
}

#[allow(clippy::large_enum_variant)]
pub(crate) enum Preparation {
    Draft { output: String, inventory: Inventory },
    Mutation(Prepared),
}

fn nearest_notice(
    reader: &Reader<'_>,
    action: &V,
    document: &source_document::Document,
    inventory: &Inventory,
) -> Result<crate::public_identity::ordinary_sameness::Notice> {
    use crate::public_identity::ordinary_sameness::{Sources, nearest_existing_from_sources};
    let mut hypotheses = std::collections::BTreeMap::new();
    for (name, hypothesis) in map(&document.hypotheses)? {
        if map(hypothesis)?
            .get("error")
            .is_some_and(crate::history_view::truth)
        {
            continue;
        }
        if let Some(path) = map(hypothesis)?.get("path").and_then(|v| text(v).ok())
            && let Some(raw) = inventory.files.get(Path::new(path))
        {
            hypotheses.insert(
                name.clone(),
                crate::history_yaml::decode_ordinary_source_value(raw)?,
            );
        }
    }
    nearest_existing_from_sources(
        reader,
        action,
        &Sources {
            base: Some(&document.source),
            hypotheses: hypotheses.iter().map(|(k, v)| (k.clone(), v)).collect(),
        },
    )
}

pub(crate) fn prepare_with_inventory(
    action: &V,
    route: &WriteRoute,
    source_body: Option<&Source>,
    mut inventory: Inventory,
    supplied_page: Option<crate::ordinary_page_capture::PageCapture>,
) -> Result<Preparation> {
    let a = map(action)?;
    require(
        a.get("hypothesis").is_none_or(|value| *value == V::Null),
        "legacy_named_hypothesis_authoring_requires_history",
    )?;
    require(
        string_is(field(map(route.config())?, "mode")?, "simple"),
        "legacy_authoring_requires_simple_project",
    )?;
    let entry = route
        .paths()
        .first()
        .ok_or_else(|| error("missing_record_path"))?;
    let document = source_document::load(route.paths(), &mut inventory, false)?;
    require(
        document.history.is_none(),
        "history_direct_writer_unsupported: use the history writer",
    )?;
    let source = projected_document(&document.source);
    let runtime = crate::public_workspace::runtime_for_document(&source)?;
    let groups = legacy_named::normalize_groups(&document.hypotheses)?;
    let mut reader =
        Reader::new(&source, runtime.as_ref())?.with_layers(groups.clone(), BTreeSet::new())?;
    reader.for_action(action)?;
    let (normalized, notes) = reader.normalize(action)?;
    let notice = nearest_notice(&reader, &normalized, &document, &inventory)?;
    let mut action = map(&normalized)?.clone();
    let stamp = action_stamp(&action);
    let initial_entries = crate::reasoning_snapshot::entries(&source)?;
    let arrangement_write = text(field(&action, "kind")?)? == "add"
        && map(field(&action, "body")?).is_ok_and(|body| body.contains_key(text(&reader.fields()["deps"]).unwrap_or("")))
        && (crate::reasoning_authoring_guards::arrangement(&reader, field(&action, "body")?)
            || initial_entries.get(text(field(&action, "id")?)?).is_some_and(|(_, old)|
                crate::reasoning_authoring_guards::arrangement(&reader, old)));
    let page = if arrangement_write {
        Some(match supplied_page {
            Some(page) => page,
            None => crate::ordinary_page_capture::PageCapture::capture(
                route.paths(), &route.project().root, Some(s(&stamp)), runtime.as_ref(),
            )?,
        })
    } else { None };
    let page_facts = if let Some(page) = &page {
        let (facts, values) = page.facts_for_document(&source, runtime.as_ref())?;
        for (id, value) in values {
            let body = reader.raw.entry(id.clone()).or_insert_with(|| V::Map(Map::new()));
            map_mut(body)?.insert("v".into(), value);
            reader.ids.insert(id);
        }
        Some(facts)
    } else { None };
    // The clock is captured once for guards and the candidate. Keep the original
    // absence of --as-of for no-op and private-draft behavior.
    let mut dated_action = action.clone();
    if dated_action
        .get("as_of")
        .is_none_or(|value| *value == V::Null)
    {
        dated_action.insert("as_of".into(), s(&stamp));
    }
    let dated_action = V::Map(dated_action);
    let mut refusals = reader.validate(&dated_action)?;
    if let Some(page_facts) = &page_facts {
        let ordinary = crate::reasoning_authoring_guards::validate(&mut reader, &dated_action)?;
        let page_aware = crate::reasoning_authoring_guards::validate_with_page(
            &mut reader, &dated_action, Some(page_facts),
        )?;
        for refusal in ordinary {
            if let Some(index) = refusals.iter().position(|item| *item == refusal) {
                refusals.remove(index);
            }
        }
        refusals.extend(page_aware);
    }
    refusals.extend(notice.refusals);
    require(
        refusals.is_empty(),
        &format!("refused - {}", refusals.join("\n          ")),
    )?;
    let kind = text(field(&action, "kind")?)?.to_owned();
    let id = text(field(&action, "id")?)?.to_owned();
    let entries = crate::reasoning_snapshot::entries(&source)?;
    let mut collection = entries.get(&id).map(|(collection, _)| collection.clone());
    let mut seen = Map::new();
    let mut seen_order = Source::Map(Vec::new());
    let mut supersede = false;
    let mut supersede_ended = None::<String>;
    let mut supersede_old = None::<V>;
    if kind == "add" {
        collection = Some(collection_for(&reader, &action)?);
        if let Ok(body) = map(field(&action, "body")?) {
            let deps = text(&reader.fields()["deps"])?;
            if body.contains_key(deps) {
                seen = dependency_snapshot(&reader, body)?;
                seen_order = ordered_snapshot(&reader, body, &seen, &document.source)?;
                let snapshot = text(&reader.fields()["snapshot"])?;
                map_mut(action.get_mut("body").unwrap())?
                    .insert(snapshot.into(), V::Map(seen.clone()));
            }
        }
        if !entries.contains_key(&id)
            && crate::reasoning_authoring_guards::arrangement(
                &reader,
                field(&action, "body")?,
            )
        {
            map_mut(action.get_mut("body").unwrap())?.insert("born".into(), s(&stamp));
        }
        if let Some((_, old)) = entries.get(&id)
            && let Ok(old_map) = map(old)
            && let Ok(deps) = text(&reader.fields()["deps"])
            && old_map.contains_key(deps)
            && map(field(&action, "body")?).is_ok_and(|body| body.contains_key(deps))
        {
            let old_arrangement = crate::reasoning_authoring_guards::arrangement(&reader, old);
            let new_arrangement = crate::reasoning_authoring_guards::arrangement(
                &reader, field(&action, "body")?,
            );
            let decision = crate::reasoning_authoring_guards::may_supersede(
                &reader, &id, old, field(&action, "body")?, Some(&stamp),
                page_facts.as_ref(), false,
            )?;
            if decision.allowed {
                supersede = true;
                supersede_ended = Some(decision.reason);
                supersede_old = Some(old.clone());
                let mut renewed = if old_arrangement && new_arrangement {
                    let stood = page_facts.as_ref().and_then(|facts| map(facts).ok())
                        .and_then(|facts| facts.get(&id)).and_then(|facts| map(facts).ok())
                        .and_then(|facts| facts.get("stood"));
                    legacy_arrangement::renewal(
                        old, field(&action, "body")?, supersede_ended.as_deref().unwrap(),
                        &stamp, stood,
                    )?
                } else {
                    legacy_replaced::judgment_renewal(
                        old, field(&action, "body")?,
                        &format!("{} on {}", supersede_ended.as_deref().unwrap(), stamp),
                    )?
                };
                if old_arrangement && new_arrangement {
                    let snapshot = text(&reader.fields()["snapshot"])?;
                    map_mut(&mut renewed)?.insert(snapshot.into(), V::Map(seen.clone()));
                }
                action.insert("body".into(), renewed);
            }
        }
    } else if kind == "review" {
        let body = entries
            .get(&id)
            .ok_or_else(|| error(&format!("refused - {id} is not a judgment")))?
            .1
            .clone();
        let body = map(&body)?;
        let deps = text(&reader.fields()["deps"])?;
        require(
            body.contains_key(deps),
            &format!("refused - {id} is not a judgment"),
        )?;
        seen = dependency_snapshot(&reader, body)?;
        seen_order = ordered_snapshot(&reader, body, &seen, &document.source)?;
    }
    let collection =
        collection.ok_or_else(|| error(&format!("refused - no file of the record holds {id}")))?;
    let stamp_review = kind == "review"
        && map(&entries[&id].1).is_ok_and(|body| {
            body.contains_key("reviewed")
                || body.contains_key("replaced")
                    && !crate::reasoning_authoring_guards::arrangement(&reader, &entries[&id].1)
        });
    let candidate = if kind == "review" {
        update_review_candidate(
            &source,
            &collection,
            &id,
            text(&reader.fields()["snapshot"])?,
            &seen,
            stamp_review.then_some(stamp.as_str()),
        )?
    } else {
        let mut candidate_action = action.clone();
        if kind == "set" {
            candidate_action.insert("as_of".into(), s(&stamp));
        }
        reader.candidate(&V::Map(candidate_action))?
    };
    if let Some(draft) =
        Privacy::candidate_draft(route.project(), &V::Map(action.clone()), &candidate)?
    {
        return Ok(Preparation::Draft {
            output: format!("{}\n", crate::public_core_readers::json_value(&draft)?),
            inventory,
        });
    }
    if kind == "set"
        && action.get("as_of").is_none_or(|value| *value == V::Null)
        && action.get("source").is_none_or(|value| *value == V::Null)
        && same_legacy(&reader.value(&id)?, field(&action, "value")?)
    {
        return Ok(Preparation::Draft {
            output: format!(
                "{id} is already {}; nothing written\n",
                scalar(field(&action, "value")?, Style::Bare)?
            ),
            inventory,
        });
    }
    let mut replaced_before = None::<Vec<u8>>;
    let mut replaced_after = None::<Vec<u8>>;
    let mut replaced_version = None::<usize>;
    let mut return_to = None::<legacy_replaced::ReturnTo>;
    if supersede {
        let layout = entry_layout(entry)?;
        let sidecar = entry
            .parent()
            .ok_or_else(|| error("invalid_path"))?
            .join(layout.replaced);
        if inventory.exists(&sidecar)? {
            replaced_before = Some(inventory.read(&sidecar)?);
        }
    }
    let target = if kind == "add" {
        choose_add_owner(&document, &inventory, &collection, &id)?
    } else {
        owner_for(&document, &collection, &id)
            .ok_or_else(|| error(&format!("refused - no file of the record holds {id}")))?
    };
    let before = inventory
        .files
        .get(&target)
        .cloned()
        .ok_or_else(|| error("snapshot_changed"))?;
    let text_source =
        std::str::from_utf8(&before).map_err(|_| error("nontext_record_authoring_unsupported"))?;
    let mut lines = text_source
        .split('\n')
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if supersede {
        let old = supersede_old
            .as_ref()
            .ok_or_else(|| error("invalid_supersede"))?;
        let old_source = legacy_replaced::old_body(&before, &id)?;
        let (sidecar, version, back) = legacy_replaced::keep_replaced(
            replaced_before.as_deref(),
            &id,
            &old_source,
            field(&action, "body")?,
            supersede_ended
                .as_deref()
                .ok_or_else(|| error("invalid_supersede"))?,
            &stamp,
            action.get("drops"),
        )?;
        replaced_after = Some(sidecar);
        replaced_version = Some(version);
        return_to = back;
        let _ = old;
    }
    let mut output = notice.text.lines().map(str::to_owned).collect::<Vec<_>>();
    let diagnostics = notes.clone();
    output.extend(notes);
    match kind.as_str() {
        "add" => {
            let mut body = preserve_order(field(&action, "body")?, source_body);
            if let Source::Map(fields) = &mut body {
                let authored = map(field(&action, "body")?)?;
                for key in ["born", "replaced"] {
                    if let Some(value) = authored.get(key)
                        && !fields.iter().any(|(field, _)| field == key)
                    {
                        fields.push((key.into(), Source::from_typed(value)));
                    }
                }
                let snapshot = text(&reader.fields()["snapshot"])?;
                if !seen.is_empty() {
                    if let Some((_, value)) = fields.iter_mut().find(|(key, _)| key == snapshot) {
                        *value = seen_order.clone();
                    } else {
                        fields.push((snapshot.into(), seen_order.clone()));
                    }
                }
            }
            if supersede {
                let old = supersede_old.as_ref().unwrap();
                if let Source::Map(fields) = &mut body {
                    let snapshot = text(&reader.fields()["snapshot"])?;
                    fields.retain(|(key, _)| key != "replaced" && key != snapshot);
                    fields.push((
                        "replaced".into(),
                        Source::from_typed(&map(field(&action, "body")?)?["replaced"]),
                    ));
                    fields.push((snapshot.into(), seen_order.clone()));
                }
                let old_body = map(old)?;
                let old_verdict = old_body
                    .get("verdict")
                    .or_else(|| old_body.get("title"))
                    .cloned()
                    .unwrap_or_else(|| s(&id));
                let new_body = map(field(&action, "body")?)?;
                let new_verdict = new_body
                    .get("verdict")
                    .or_else(|| new_body.get("title"))
                    .cloned()
                    .unwrap_or_else(|| s(&id));
                let why = supersede_ended.as_deref().unwrap();
                if same_legacy(&old_verdict, &new_verdict) {
                    output.push(format!(
                        "supersede {id}: the same verdict on other grounds - {why}"
                    ));
                } else {
                    output.push(format!(
                        "supersede {id}: {} -> {} - {why}",
                        crate::public_ordinary_readers::short(&old_verdict, 60),
                        crate::public_ordinary_readers::short(&new_verdict, 60)
                    ));
                }
                let layout = entry_layout(entry)?;
                let side = layout.replaced;
                let kept_fields = ["verdict", "because", "request"]
                    .into_iter()
                    .filter(|field| old_body.contains_key(*field))
                    .collect::<Vec<_>>();
                let kept_label = if kept_fields.is_empty() {
                    "body".to_owned()
                } else {
                    kept_fields.join(", ")
                };
                output.push(format!(
                    "  kept: the replaced {kept_label}, in {side} (version {})",
                    replaced_version.ok_or_else(|| error("invalid_supersede"))?
                ));
                for (dependency, why) in legacy_replaced::dropped_dependencies(
                    old,
                    field(&action, "body")?,
                    text(&reader.fields()["deps"])?,
                    action.get("drops"),
                )? {
                    output.push(format!(
                        "  no longer rests on {dependency}{}",
                        why.as_ref()
                            .map(|why| format!(": {}", display(why).unwrap_or_default()))
                            .unwrap_or_default()
                    ));
                }
                if let Some(back) = &return_to {
                    output.push(format!(
                        "  returns to {} {}{} - it stood until {} and fell because {}",
                        if back.exact {
                            "version"
                        } else {
                            "the verdict of version"
                        },
                        back.index,
                        if back.exact { "" } else { ", on other grounds" },
                        back.day,
                        back.ended
                    ));
                }
                replace_entry(&mut lines, &id, &body)?;
            } else {
                add_collection_if_needed(&mut lines, &collection);
                output.push(format!(
                    "add {}",
                    insert_entry(&mut lines, &collection, &id, &body)?
                ));
            }
        }
        "set" => {
            let old = set_entry(
                &mut lines,
                &id,
                field(&action, "value")?,
                &stamp,
                action.get("why").and_then(|value| text(value).ok()),
                action.get("source").and_then(|value| text(value).ok()),
                action.get("at").and_then(|value| text(value).ok()),
            )?;
            output.push(format!(
                "set {id}: {old} -> {} (as of {stamp})",
                scalar(field(&action, "value")?, Style::Bare)?
            ));
            if let Some(source) = action.get("source").and_then(|value| text(value).ok()) {
                output.push(format!(
                    "source: {source}, at {}",
                    text(field(&action, "at")?)?
                ));
            }
        }
        "review" => {
            let (_, member) = locate(&lines, &id)
                .ok_or_else(|| error(&format!("refused - no file of the record holds {id}")))?;
            let snapshot = text(&reader.fields()["snapshot"])?;
            let old_seen = map(map(&entries[&id].1)?
                .get(snapshot)
                .unwrap_or(&V::Map(Map::new())))?
            .clone();
            let changed = old_seen.len() != seen.len()
                || old_seen
                    .iter()
                    .any(|(key, value)| seen.get(key).is_none_or(|now| !same_legacy(value, now)));
            if changed {
                replace_field_ordered(&mut lines, &member, snapshot, &seen_order)?;
            }
            let (_, member) = locate(&lines, &id).unwrap();
            if stamp_review {
                if field_span(&lines, &member, "reviewed").is_some() {
                    replace_date_field(&mut lines, &member, "reviewed", &stamp)?;
                } else {
                    let after =
                        field_span(&lines, &member, "replaced").map_or(member.end, |span| span.end);
                    lines.insert(
                        after,
                        format!("{}reviewed: \"{stamp}\"", " ".repeat(member.indent + 2)),
                    );
                }
            }
            output.push(format!(
                "review {id}: {} ({stamp})",
                if changed {
                    "seen rewritten from what the record holds"
                } else {
                    "what it saw is what the record holds"
                }
            ));
            for dependency in map(&entries[&id].1)?[text(&reader.fields()["deps"])?]
                .clone()
                .into_list()?
            {
                let dependency = text(&dependency)?;
                let old_source = document
                    .source
                    .get(&collection)
                    .and_then(|entries| entries.get(&id))
                    .and_then(|body| body.get(snapshot))
                    .and_then(|snapshot| snapshot.get(dependency))
                    .and_then(ordinary_template);
                let old_text = |value: &V| {
                    crate::source_text::python_str(&preserve_order(value, old_source.as_ref()))
                };
                let new_text = |value: &V| {
                    crate::source_text::python_str(&preserve_order(
                        value,
                        seen_order.get(dependency),
                    ))
                };
                match (old_seen.get(dependency), seen.get(dependency)) {
                    (Some(old), Some(now)) if !same_legacy(old, now) => {
                        let (old, now) = crate::public_ordinary_readers::apart(
                            &s(&old_text(old)),
                            &s(&new_text(now)),
                            40,
                        );
                        output.push(format!("  {dependency}: {old} -> {now}"));
                    }
                    (None, Some(now)) => output.push(format!(
                        "  {dependency}: {} (never checked against it before)",
                        crate::public_ordinary_readers::short(&s(&new_text(now)), 40)
                    )),
                    _ => {}
                }
            }
        }
        _ => return Err(error("unsupported_legacy_authoring_kind")),
    }
    bump_updated(&mut lines, &stamp)?;
    let after = lines.join("\n").into_bytes();
    let parsed_before = crate::history_yaml::decode_ordinary_source_value(&before)?;
    let parsed = crate::history_yaml::decode_ordinary_source_value(&after)?;
    require(
        parsed.get("record").map(|value| value.projected())
            == parsed_before.get("record").map(|value| value.projected())
            && parsed.get("also").map(|value| value.projected())
                == parsed_before.get("also").map(|value| value.projected()),
        "direct_pointer_change: record membership needs a separate migration",
    )?;
    let after_projected = parsed.projected();
    verify_untouched_document(
        &parsed_before.projected(),
        &after_projected,
        &collection,
        &id,
    )?;
    require(
        collections(&lines)
            .iter()
            .flat_map(|group| members(&lines, group))
            .filter(|member| member.name == id)
            .count()
            == 1,
        "the write broke the record and was undone: duplicate entry",
    )?;
    let after_body = crate::reasoning_fields::collections(&after_projected)?
        .get(&collection)
        .and_then(|members| members.get(&id))
        .cloned()
        .ok_or_else(|| {
            error("the write broke the record and was undone: entry missing after write")
        })?;
    let candidate_body =
        crate::reasoning_fields::collections(&candidate)?[&collection][&id].clone();
    require(
        authored_equal(&after_body, &candidate_body),
        "the write broke the record and was undone: entry changed meaning",
    )?;
    let after_projection = crate::public_ordinary_readers::Projection::new(
        &candidate,
        &groups,
        &Map::new(),
        vec![],
        runtime.as_ref(),
    )?;
    let report_sources = if kind == "review" {
        crate::ordinary_write_report::Ancillary::default()
    } else {
        crate::ordinary_write_report::Ancillary::capture(entry, &mut inventory)?
    };
    output.extend(crate::ordinary_write_report::render(
        &kind,
        &id,
        &after_projection.base,
        None,
        &report_sources,
    )?);

    let root = common_root(entry, &document.members)?;
    let entry_relative = relative(&root, entry)?;
    let target_relative = relative(&root, &target)?;
    let record_members = document
        .members
        .iter()
        .map(|path| {
            Ok((
                relative(&root, path)?,
                s(&crate::identity::sha256(
                    inventory
                        .files
                        .get(path)
                        .ok_or_else(|| error("snapshot_changed"))?,
                )),
            ))
        })
        .collect::<Result<Map>>()?;
    let mut input_files = report_sources
        .inputs
        .iter()
        .map(|(path, digest)| {
            Ok((relative(&root, path)?, digest.as_deref().map(s).unwrap_or(V::Null)))
        })
        .collect::<Result<Map>>()?;
    if let Some(page) = &page {
        input_files.insert(
            relative(&root, &page.view_path)?,
            page.view_before.as_deref().map(crate::identity::sha256).map(|v| s(&v)).unwrap_or(V::Null),
        );
    }
    let mut baseline = object([
        ("kind", s("direct/v1")),
        ("transaction_root", s(name(&absolute(&root)?)?)),
        ("record_members", V::Map(record_members)),
        ("policy", route.config().clone()),
        (
            "input_files",
            V::Map(input_files),
        ),
    ]);
    if let Some(page) = &page {
        map_mut(&mut baseline)?.insert("routing".into(), page.routing.clone());
        map_mut(&mut baseline)?.insert(
            "arrangement_inputs".into(),
            legacy_arrangement::recovery_guard(&root, page)?,
        );
    }
    let capabilities = crate::reasoning_fields::capabilities(&source, None)?;
    let receipt = T::semantic_receipt(
        text(&map(&capabilities)?["profile"])?,
        &capabilities,
        &object([(
            "source_sha256",
            s(&crate::identity::sha256(&source.canonical_bytes()?)),
        )]),
        &object([(
            "source_sha256",
            s(&crate::identity::sha256(&candidate.canonical_bytes()?)),
        )]),
    )?;
    let operation = crate::public_history::fresh_id("direct")?;
    let marker = authority(entry)?;
    let role = if target == *entry {
        "record"
    } else {
        "record_member"
    };
    let mut files = vec![FileImage {
        path: target_relative,
        role: role.into(),
        before: Some(before),
        after: Some(after),
    }];
    if supersede {
        let layout = entry_layout(entry)?;
        let side_path = entry
            .parent()
            .ok_or_else(|| error("invalid_path"))?
            .join(layout.replaced);
        files.push(FileImage {
            path: relative(&root, &side_path)?,
            role: "replaced".into(),
            before: replaced_before,
            after: replaced_after,
        });
    }
    let mutation = PreparedMutation::prepare(
        &operation,
        &marker,
        &baseline,
        files,
        &receipt,
        &entry_relative,
        None,
    )?;
    let journal = entry_layout(Path::new(&entry_relative))?.journal;
    Ok(Preparation::Mutation(Prepared {
        inventory,
        mutation,
        output: format!("{}\n", output.join("\n")),
        root,
        journal,
        subject: id,
        diagnostics,
        page,
    }))
}

fn prepare(action: &V, route: &WriteRoute, source_body: Option<&Source>) -> Result<Preparation> {
    prepare_with_inventory(action, route, source_body, Inventory::default(), None)
}

trait IntoList {
    fn into_list(self) -> Result<Vec<V>>;
}
impl IntoList for V {
    fn into_list(self) -> Result<Vec<V>> {
        match self {
            V::List(values) => Ok(values),
            _ => Err(error("invalid_dependencies")),
        }
    }
}

fn verify_prepared(prepared: &Prepared, route: &WriteRoute, inventory: &Inventory) -> Result<()> {
    route.verify()?;
    inventory.verify()?;
    if let Some(page) = &prepared.page {
        page.verify()?;
    }
    let data = prepared.mutation.to_data();
    legacy_named::verify_recovery(
        route,
        &prepared.root,
        &prepared.mutation,
        map(field(map(&data)?, "baseline")?)?,
    )?;
    require(
        self::route(
            &entry_path(&prepared.root, &prepared.mutation)?,
            route.config(),
        )? == AuthorityRoute::Legacy,
        "project_route_changed",
    )
}

fn publish_with_committed(
    prepared: Prepared,
    route: &WriteRoute,
    committed: Option<F::Verify<'_>>,
) -> Result<String> {
    if prepared.journal.is_empty() {
        return Ok(prepared.output);
    }
    let inventory = legacy_named::prepare_directories(&prepared)?;
    let mut verify = |_: &V| verify_prepared(&prepared, route, &inventory);
    F::publish_legacy(
        &prepared.root,
        &prepared.journal,
        &prepared.mutation,
        &mut verify,
        committed,
    )?;
    crate::session_activity::published(
        &prepared.root,
        prepared.mutation.files(),
        Some(&BTreeSet::from([prepared.subject.clone()])),
    );
    Ok(prepared.output)
}

fn publish(prepared: Prepared, route: &WriteRoute) -> Result<String> {
    publish_with_committed(prepared, route, None)
}

pub(crate) fn publish_prepared_with_committed(
    prepared: Prepared,
    route: &WriteRoute,
    committed: F::Verify<'_>,
) -> Result<String> {
    publish_with_committed(prepared, route, Some(committed))
}

fn entry_path(root: &Path, mutation: &PreparedMutation) -> Result<PathBuf> {
    Ok(root.join(text(field(map(&mutation.to_data())?, "entry")?)?))
}

fn verify_recovery(route: &WriteRoute, root: &Path, mutation: &PreparedMutation) -> Result<()> {
    route.verify()?;
    let entry = entry_path(root, mutation)?;
    require(
        self::authority_route(&entry)? == AuthorityRoute::Legacy,
        "project_route_changed",
    )?;
    let data = mutation.to_data();
    let baseline = map(field(map(&data)?, "baseline")?)?;
    let recorded_policy = baseline.get("policy");
    require(
        recorded_policy.is_none_or(|policy| policy == route.config())
            && (!string_is(field(map(route.config())?, "mode")?, "advanced")
                || recorded_policy == Some(route.config())),
        "project_route_changed",
    )?;
    if let Some(expected) = baseline.get("routing") {
        require(
            crate::source_capture::routing_observation(route.paths(), &route.project().root)?
                == *expected,
            "project_route_changed",
        )?;
    }
    legacy_named::verify_recovery(route, root, mutation, baseline)?;
    if let Some(guard) = baseline.get("arrangement_inputs") {
        legacy_arrangement::verify_recovery(root, mutation, guard)?;
    }
    require(
        baseline
            .get("kind")
            .is_some_and(|kind| string_is(kind, "direct/v1")),
        "unsupported public recovery journal",
    )?;
    if let Some(inputs) = baseline.get("input_files") {
        for (path, digest) in map(inputs)? {
            let current = F::read(&F::target(root, path)?)?;
            let matches =
                if let Some(image) = mutation.files().iter().find(|image| image.path == *path) {
                    current == image.before || current == image.after
                } else {
                    current
                        .as_deref()
                        .map(crate::identity::sha256)
                        .map(|value| s(&value))
                        .unwrap_or(V::Null)
                        == *digest
                };
            require(matches, "concurrent_edit: captured report input changed")?;
        }
    }
    let expected = map(field(baseline, "record_members")?)?;
    let mut inventory = Inventory::default();
    let actual = source_document::members(route.paths(), &mut inventory)?;
    let actual_names = actual
        .iter()
        .map(|path| relative(root, path))
        .collect::<Result<BTreeSet<_>>>()?;
    require(
        actual_names == expected.keys().cloned().collect(),
        "concurrent_edit: reader-resolved record membership changed",
    )?;
    for path in actual {
        let relative = relative(root, &path)?;
        let bytes = inventory
            .files
            .get(&path)
            .ok_or_else(|| error("concurrent_edit"))?;
        let digest = s(&crate::identity::sha256(bytes));
        let changed = mutation.files().iter().find(|image| image.path == relative);
        require(
            changed.is_some_and(|image| {
                image.before.as_ref() == Some(bytes) || image.after.as_ref() == Some(bytes)
            }) || changed.is_none() && expected.get(&relative) == Some(&digest),
            "concurrent_edit",
        )?;
    }
    Ok(())
}

/// Resume or cancel a retained ordinary-write transaction. The public recover
/// command can delegate here when this journal's baseline kind is `direct/v1`.
pub fn recovery_pending(original: &[PathBuf], cwd: &Path) -> Result<bool> {
    let route = WriteRoute::capture(original, cwd)?;
    require(route.paths().len() == 1, "choose one logical record entry")?;
    let entry = &route.paths()[0];
    if self::authority_route(entry)? != AuthorityRoute::Legacy {
        return Ok(false);
    }
    let layout = entry_layout(entry)?;
    let Some(raw) = F::read(&entry.parent().unwrap().join(layout.journal))? else {
        return Ok(false);
    };
    let mutation = PreparedMutation::from_bytes(&raw)?;
    let data = mutation.to_data();
    let baseline = map(field(map(&data)?, "baseline")?)?;
    Ok(baseline
        .get("kind")
        .is_some_and(|kind| string_is(kind, "direct/v1")))
}

pub fn recover(original: &[PathBuf], cwd: &Path, before: bool) -> Result<V> {
    let route = WriteRoute::capture(original, cwd)?;
    require(route.paths().len() == 1, "choose one logical record entry")?;
    let entry = &route.paths()[0];
    require(
        self::authority_route(entry)? == AuthorityRoute::Legacy,
        "history_direct_writer_unsupported: use the history writer",
    )?;
    let local_layout = entry_layout(entry)?;
    let local_journal = entry.parent().unwrap().join(&local_layout.journal);
    let raw = F::read(&local_journal)?.ok_or_else(|| error("no_recovery_pending"))?;
    let mutation = PreparedMutation::from_bytes(&raw)?;
    let data = mutation.to_data();
    let baseline = map(field(map(&data)?, "baseline")?)?;
    require(
        baseline
            .get("kind")
            .is_some_and(|kind| string_is(kind, "direct/v1")),
        "unsupported public recovery journal",
    )?;
    let root = PathBuf::from(text(field(baseline, "transaction_root")?)?);
    require(root.is_absolute(), "transaction_root_mismatch")?;
    let journal = relative(&root, &local_journal)?;
    let direction = if before {
        F::Direction::Before
    } else {
        F::Direction::After
    };
    let mut verify = |_: &V| verify_recovery(&route, &root, &mutation);
    let recovered = F::recover_legacy(&root, &journal, direction, &mut verify, None, None)?;
    let data = recovered.to_data();
    Ok(object([
        ("state", s(if before { "restored" } else { "recovered" })),
        ("operation", field(map(&data)?, "operation")?.clone()),
        ("mutation_digest", field(map(&data)?, "digest")?.clone()),
    ]))
}

pub(crate) fn write(
    action: &V,
    route: &WriteRoute,
    source_body: Option<&Source>,
) -> Result<String> {
    let prepared = if map(action)?
        .get("hypothesis")
        .is_some_and(|v| *v != V::Null)
    {
        legacy_named::prepare(action, route, source_body)?
    } else {
        prepare(action, route, source_body)?
    };
    match prepared {
        Preparation::Draft { output, .. } => Ok(output),
        Preparation::Mutation(prepared) => publish(prepared, route),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn replacement(existing_sidecar: bool) -> (tempfile::TempDir, PathBuf, WriteRoute, Prepared) {
        let temp = tempfile::tempdir().unwrap();
        let entry = temp.path().join("GROUNDING.yaml");
        fs::write(
            &entry,
            include_str!("../tests/fixtures/legacy-review/supersede-before.yaml"),
        )
        .unwrap();
        if existing_sidecar {
            fs::create_dir(temp.path().join(".kpopper")).unwrap();
            fs::write(
                temp.path().join(".kpopper/replaced.yaml"),
                "d.other:\n- verdict: retained\n  ended: earlier evidence\n  day: '2026-09-01'\n",
            )
            .unwrap();
        }
        let route = WriteRoute::capture(std::slice::from_ref(&entry), temp.path()).unwrap();
        let action = object([
            ("kind", s("add")),
            ("id", s("d.keep")),
            (
                "body",
                object([
                    ("verdict", s("stop")),
                    ("rests_on", V::List(vec![s("p.alpha"), s("p.beta")])),
                    ("wrong_if", s("p.alpha > 9")),
                    ("because", s("new evidence")),
                ]),
            ),
            ("as_of", s("2026-09-19")),
            ("why", V::Null),
            ("into", V::Null),
            ("hypothesis", V::Null),
            ("source", V::Null),
            ("at", V::Null),
        ]);
        let Preparation::Mutation(prepared) = prepare(&action, &route, None).unwrap() else {
            panic!("expected replacement")
        };
        assert_eq!(prepared.mutation.files().len(), 2);
        (temp, entry, route, prepared)
    }

    fn page_arrangement_replacement() -> (tempfile::TempDir, PathBuf, WriteRoute, Prepared) {
        let temp = tempfile::tempdir().unwrap();
        let cases: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../tests/fixtures/ordinary-page-facts.json"
        )).unwrap();
        let case = cases.as_array().unwrap().iter()
            .find(|case| case["name"] == "page-arrangement-fired-dry").unwrap();
        for (relative, raw) in case["files"].as_object().unwrap() {
            if relative.contains("hypotheses/") { continue }
            let path = temp.path().join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, raw.as_str().unwrap()).unwrap();
        }
        let entry = temp.path().join("GROUNDING.yaml");
        let route = WriteRoute::capture(std::slice::from_ref(&entry), temp.path()).unwrap();
        let action = object([
            ("kind", s("add")), ("id", s("v.layout")),
            ("body", object([
                ("verdict", s("re-decided")),
                ("rests_on", V::List(vec![s("s.now"), s("page.unserved")])),
                ("wrong_if", s("page.unserved > 0")),
            ])),
            ("as_of", s("2026-09-20")), ("why", V::Null), ("into", V::Null),
            ("hypothesis", V::Null), ("source", V::Null), ("at", V::Null),
        ]);
        let Preparation::Mutation(prepared) = prepare(&action, &route, None).unwrap() else {
            panic!("expected page arrangement replacement")
        };
        let data = prepared.mutation.to_data();
        let baseline = map(&map(&data).unwrap()["baseline"]).unwrap();
        assert!(baseline.contains_key("routing"));
        assert!(baseline.contains_key("arrangement_inputs"));
        assert!(prepared.page.is_some());
        (temp, entry, route, prepared)
    }

    #[test]
    fn supersede_recovers_or_rolls_back_both_partial_images() {
        for existing in [false, true] {
            for before in [false, true] {
                for partial_role in ["record", "replaced"] {
                    let (temp, entry, route, prepared) = replacement(existing);
                    let images = prepared.mutation.files().to_vec();
                    let mut verify = |_: &V| {
                        route.verify()?;
                        prepared.inventory.verify()
                    };
                    let mut stop = |_: &V| Err(error("retained_after_write"));
                    assert_eq!(
                        F::publish_legacy(
                            &prepared.root,
                            &prepared.journal,
                            &prepared.mutation,
                            &mut verify,
                            Some(&mut stop)
                        )
                        .unwrap_err()
                        .0,
                        "retained_after_write"
                    );
                    // One participant still has its preimage after an interrupted write.
                    let image = images
                        .iter()
                        .find(|image| image.role == partial_role)
                        .unwrap();
                    let path = prepared.root.join(&image.path);
                    if let Some(bytes) = &image.before {
                        fs::write(&path, bytes).unwrap();
                    } else {
                        fs::remove_file(&path).unwrap();
                    }
                    drop(route);
                    recover(std::slice::from_ref(&entry), temp.path(), before).unwrap();
                    for image in &images {
                        let expected = if before { &image.before } else { &image.after };
                        assert_eq!(
                            F::read(&prepared.root.join(&image.path)).unwrap(),
                            *expected,
                            "existing={existing} rollback={before} partial={partial_role} image={}",
                            image.path
                        );
                    }
                    assert!(!prepared.root.join(&prepared.journal).exists());
                }
            }
        }
    }

    #[test]
    fn page_arrangement_rechecks_view_and_recovers_both_directions() {
        let (temp, entry, route, prepared) = page_arrangement_replacement();
        let before = fs::read(&entry).unwrap();
        fs::write(temp.path().join(".kpopper/view.yaml"), "tabs: []\n").unwrap();
        assert_eq!(publish(prepared, &route).unwrap_err().0, "snapshot_changed");
        assert_eq!(fs::read(&entry).unwrap(), before);

        for rollback in [false, true] {
            let (temp, entry, route, prepared) = page_arrangement_replacement();
            let images = prepared.mutation.files().to_vec();
            let mut verify = |_: &V| verify_prepared(&prepared, &route, &prepared.inventory);
            let mut stop = |_: &V| Err(error("retained_after_write"));
            assert_eq!(F::publish_legacy(&prepared.root, &prepared.journal,
                &prepared.mutation, &mut verify, Some(&mut stop)).unwrap_err().0,
                "retained_after_write");
            let partial = images.iter().find(|image| image.role == "replaced").unwrap();
            let path = prepared.root.join(&partial.path);
            match &partial.before {
                Some(raw) => fs::write(&path, raw).unwrap(),
                None => fs::remove_file(&path).unwrap(),
            }
            drop(route);
            recover(std::slice::from_ref(&entry), temp.path(), rollback).unwrap();
            for image in images {
                assert_eq!(F::read(&prepared.root.join(&image.path)).unwrap(),
                    if rollback { image.before } else { image.after });
            }
        }
    }

    #[test]
    fn supersede_never_overwrites_a_sidecar_changed_after_preparation() {
        for existing in [false, true] {
            let (temp, entry, route, prepared) = replacement(existing);
            let record_before = fs::read(&entry).unwrap();
            let sidecar = temp.path().join(".kpopper/replaced.yaml");
            fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
            fs::write(&sidecar, "d.user: [{verdict: preserve}]\n").unwrap();
            assert!(publish(prepared, &route).is_err());
            assert_eq!(fs::read(&entry).unwrap(), record_before);
            assert_eq!(
                fs::read_to_string(sidecar).unwrap(),
                "d.user: [{verdict: preserve}]\n"
            );
        }
    }

    #[test]
    fn advanced_mode_recovers_policy_bound_direct_journals_without_granting_writes() {
        fn configured(mode: &str) -> (tempfile::TempDir, PathBuf, WriteRoute, Prepared) {
            let temp = tempfile::tempdir().unwrap();
            assert!(std::process::Command::new("git").args(["init", "-q"])
                .current_dir(temp.path()).status().unwrap().success());
            let entry = temp.path().join("GROUNDING.yaml");
            fs::write(&entry, "known:\n  p.a: {v: 1}\n").unwrap();
            let project = crate::project_modes::Project::open(temp.path()).unwrap();
            fs::create_dir_all(project.config_path.parent().unwrap()).unwrap();
            fs::write(&project.config_path,
                format!(r#"{{"version":1,"mode":"{mode}","generation":1,"record":"GROUNDING.yaml"}}"#)).unwrap();
            let route = WriteRoute::capture(std::slice::from_ref(&entry), temp.path()).unwrap();
            let action = object([
                ("kind", s("set")), ("id", s("p.a")), ("value", n("2")),
                ("as_of", s("2026-09-20")), ("why", V::Null), ("source", V::Null), ("at", V::Null),
            ]);
            let Preparation::Mutation(prepared) = prepare(
                &action, &route, None).unwrap() else { panic!() };
            (temp, entry, route, prepared)
        }

        // Construct the same report journal a previously authorized advanced
        // adapter would retain; ordinary direct writes remain disallowed there.
        let (temp, entry, simple, prepared) = configured("simple");
        let project = crate::project_modes::Project::open(temp.path()).unwrap();
        drop(simple);
        fs::write(&project.config_path,
            r#"{"version":1,"mode":"advanced","generation":2,"record":"GROUNDING.yaml"}"#).unwrap();
        let advanced = WriteRoute::capture(std::slice::from_ref(&entry), temp.path()).unwrap();
        assert_eq!(route(&entry, advanced.config()).unwrap_err().0,
            "legacy_authoring_requires_simple_project");
        let data = prepared.mutation.to_data();
        let mut baseline = map(&map(&data).unwrap()["baseline"]).unwrap().clone();
        baseline.insert("policy".into(), advanced.config().clone());
        let mutation = PreparedMutation::prepare(
            text(&map(&data).unwrap()["operation"]).unwrap(),
            &map(&data).unwrap()["authority"], &V::Map(baseline),
            prepared.mutation.files().to_vec(), &map(&data).unwrap()["receipt"],
            text(&map(&data).unwrap()["entry"]).unwrap(), None).unwrap();
        let journal = prepared.root.join(&prepared.journal);
        fs::create_dir_all(journal.parent().unwrap()).unwrap();
        fs::write(&journal, mutation.to_bytes().unwrap()).unwrap();
        let image = mutation.files().iter().find(|image| image.role == "record").unwrap();
        fs::write(&entry, image.after.as_ref().unwrap()).unwrap();
        drop(advanced);
        assert!(recovery_pending(std::slice::from_ref(&entry), temp.path()).unwrap());
        recover(std::slice::from_ref(&entry), temp.path(), false).unwrap();
        assert!(fs::read_to_string(&entry).unwrap().contains("v: 2"));

        // A journal captured under Simple cannot bypass a later Advanced policy.
        let (temp, entry, route, prepared) = configured("simple");
        let journal = prepared.root.join(&prepared.journal);
        fs::create_dir_all(journal.parent().unwrap()).unwrap();
        fs::write(&journal, prepared.mutation.to_bytes().unwrap()).unwrap();
        let image = prepared.mutation.files().iter().find(|image| image.role == "record").unwrap();
        fs::write(&entry, image.after.as_ref().unwrap()).unwrap();
        let project = crate::project_modes::Project::open(temp.path()).unwrap();
        fs::write(&project.config_path,
            r#"{"version":1,"mode":"advanced","generation":2,"record":"GROUNDING.yaml"}"#).unwrap();
        drop(route);
        assert!(recovery_pending(std::slice::from_ref(&entry), temp.path()).unwrap());
        assert_eq!(recover(std::slice::from_ref(&entry), temp.path(), false).unwrap_err().0,
            "project_route_changed");
    }

    #[test]
    fn write_guard_checks_the_rest_of_the_record() {
        let before = object([
            ("known", object([("p.a", n("1")), ("p.b", n("2"))])),
            ("meta", object([("updated", s("2026-09-01"))])),
        ]);
        let allowed = object([
            ("known", object([("p.a", n("3")), ("p.b", n("2"))])),
            ("meta", object([("updated", s("2026-09-19"))])),
        ]);
        verify_untouched_document(&before, &allowed, "known", "p.a").unwrap();
        let lost_other = object([
            ("known", object([("p.a", n("3"))])),
            ("meta", object([("updated", s("2026-09-19"))])),
        ]);
        assert!(verify_untouched_document(&before, &lost_other, "known", "p.a").is_err());
    }

    #[test]
    fn captured_source_change_refuses_before_publication() {
        let temp = tempfile::tempdir().unwrap();
        let entry = temp.path().join("GROUNDING.yaml");
        fs::write(&entry, "known:\n  p.a: {v: 1}\n").unwrap();
        let route = WriteRoute::capture(std::slice::from_ref(&entry), temp.path()).unwrap();
        let action = object([
            ("kind", s("set")),
            ("id", s("p.a")),
            ("value", n("2")),
            ("as_of", s("2026-09-19")),
            ("why", V::Null),
            ("into", V::Null),
            ("hypothesis", V::Null),
            ("source", V::Null),
            ("at", V::Null),
        ]);
        let Preparation::Mutation(prepared) = prepare(&action, &route, None).unwrap() else {
            panic!("expected mutation")
        };
        fs::write(&entry, "known:\n  p.a: {v: 7}\n").unwrap();
        assert_eq!(
            publish(prepared, &route).unwrap_err().0,
            "concurrent_edit"
        );
        assert_eq!(
            fs::read_to_string(entry).unwrap(),
            "known:\n  p.a: {v: 7}\n"
        );
    }

    #[test]
    fn report_inputs_are_bound_before_publication_and_during_recovery() {
        for existing in [false, true] {
            for before_publication in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let entry = temp.path().join("GROUNDING.yaml");
                let brief = temp.path().join(".kpopper/view.yaml");
                fs::write(&entry, "known:\n  p.a: {v: 1}\n").unwrap();
                if existing {
                    fs::create_dir_all(brief.parent().unwrap()).unwrap();
                    fs::write(&brief, "sections: []\n").unwrap();
                }
                let route = WriteRoute::capture(std::slice::from_ref(&entry), temp.path()).unwrap();
                let action = object([
                    ("kind", s("set")),
                    ("id", s("p.a")),
                    ("value", n("2")),
                    ("as_of", s("2026-09-19")),
                    ("why", V::Null),
                    ("into", V::Null),
                    ("hypothesis", V::Null),
                    ("source", V::Null),
                    ("at", V::Null),
                ]);
                let Preparation::Mutation(prepared) = prepare(&action, &route, None).unwrap()
                else {
                    panic!("expected mutation")
                };
                let record_before = fs::read(&entry).unwrap();
                if !before_publication {
                    let inventory = legacy_named::prepare_directories(&prepared).unwrap();
                    let mut verify = |_: &V| verify_prepared(&prepared, &route, &inventory);
                    let mut stop = |_: &V| Err(error("retained_after_write"));
                    assert_eq!(
                        F::publish_legacy(
                            &prepared.root,
                            &prepared.journal,
                            &prepared.mutation,
                            &mut verify,
                            Some(&mut stop)
                        )
                        .unwrap_err()
                        .0,
                        "retained_after_write"
                    );
                }
                fs::create_dir_all(brief.parent().unwrap()).unwrap();
                fs::write(&brief, "sections: [{title: changed, text: preserve}]\n").unwrap();
                if before_publication {
                    assert!(publish(prepared, &route).is_err());
                    assert_eq!(fs::read(&entry).unwrap(), record_before);
                } else {
                    let journal = prepared.root.join(&prepared.journal);
                    let retained = fs::read(&journal).unwrap();
                    let record_after = fs::read(&entry).unwrap();
                    drop(route);
                    assert!(recover(std::slice::from_ref(&entry), temp.path(), false).is_err());
                    assert_eq!(fs::read(&entry).unwrap(), record_after);
                    assert_eq!(fs::read(journal).unwrap(), retained);
                }
                assert_eq!(
                    fs::read_to_string(brief).unwrap(),
                    "sections: [{title: changed, text: preserve}]\n"
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn recovery_accepts_directory_aliases_without_following_file_symlinks() {
        for rollback in [false, true] {
            let (temp, entry, route, prepared) = replacement(false);
            let inventory = legacy_named::prepare_directories(&prepared).unwrap();
            let mut verify = |_: &V| verify_prepared(&prepared, &route, &inventory);
            let mut stop = |_: &V| Err(error("retained_after_write"));
            assert_eq!(F::publish_legacy(&prepared.root, &prepared.journal, &prepared.mutation,
                &mut verify, Some(&mut stop)).unwrap_err().0, "retained_after_write");
            drop(route);
            let aliases = tempfile::tempdir().unwrap();
            let directory = aliases.path().join("project-alias");
            std::os::unix::fs::symlink(temp.path(), &directory).unwrap();
            let alias_entry = directory.join(entry.file_name().unwrap());
            recover(&[alias_entry], &directory, rollback).unwrap();
            for image in prepared.mutation.files() {
                assert_eq!(F::read(&prepared.root.join(&image.path)).unwrap(),
                    if rollback { image.before.clone() } else { image.after.clone() });
            }
            let file_alias = temp.path().join("alias.yaml");
            std::os::unix::fs::symlink(&entry, &file_alias).unwrap();
            assert_eq!(relative(temp.path(), &file_alias).unwrap(), "alias.yaml");
            assert!(F::target(temp.path(), "alias.yaml").is_err());
        }
    }

    #[test]
    fn retained_publication_can_be_completed_by_legacy_recovery() {
        for before in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let entry = temp.path().join("GROUNDING.yaml");
            fs::write(&entry, "known:\n  p.a: {v: 1}\n").unwrap();
            let route = WriteRoute::capture(std::slice::from_ref(&entry), temp.path()).unwrap();
            let action = object([
                ("kind", s("set")),
                ("id", s("p.a")),
                ("value", n("2")),
                ("as_of", s("2026-09-19")),
                ("why", V::Null),
                ("into", V::Null),
                ("hypothesis", V::Null),
                ("source", V::Null),
                ("at", V::Null),
            ]);
            let Preparation::Mutation(prepared) = prepare(&action, &route, None).unwrap() else {
                panic!("expected mutation")
            };
            let mut verify = |_: &V| {
                route.verify()?;
                prepared.inventory.verify()
            };
            let mut stop = |_: &V| Err(error("injected_after_publication"));
            assert_eq!(
                F::publish_legacy(
                    &prepared.root,
                    &prepared.journal,
                    &prepared.mutation,
                    &mut verify,
                    Some(&mut stop),
                )
                .unwrap_err()
                .0,
                "injected_after_publication"
            );
            assert!(entry.parent().unwrap().join(&prepared.journal).is_file());
            drop(route);
            let result = recover(std::slice::from_ref(&entry), temp.path(), before).unwrap();
            assert!(string_is(
                &map(&result).unwrap()["state"],
                if before { "restored" } else { "recovered" }
            ));
            assert_eq!(
                fs::read_to_string(&entry).unwrap(),
                if before {
                    "known:\n  p.a: {v: 1}\n"
                } else {
                    "known:\n  p.a: {v: 2, of: \"2026-09-19\"}\n"
                }
            );
            assert!(!entry.parent().unwrap().join(prepared.journal).exists());
        }
    }
}
