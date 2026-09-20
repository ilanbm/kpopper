//! Ordinary identity edits operate on captured text and preserve source layout.
use crate::{
    Result,
    history_authoring::{obj, s},
    history_contract::{error, map, text},
    history_identity_rewrite::{expression, tokens},
    history_yaml::SourceValue as Source,
    require,
    value::TypedValue as V,
};
use libyaml_safer::{Event, EventData as E, Parser, ScalarStyle};
use regex::Regex;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::LazyLock,
};

static TOP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Za-z_][A-Za-z0-9_]*):(?: |$)").unwrap());
static MEMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^( +)([A-Za-z_][A-Za-z0-9_.]*):(?: |$)").unwrap());
static FLOW: &str = r#"("(?:[^"\\]|\\.)*"|'(?:[^']|'')*'|[^,}\n]+)"#;

struct Node {
    start: usize,
    end: usize,
    kind: Kind,
}
enum Kind {
    Pending,
    Scalar(String, ScalarStyle),
    List(Vec<usize>),
    Map(Vec<(usize, usize)>),
}
struct Graph<'a> {
    parser: Parser<&'a [u8]>,
    nodes: Vec<Node>,
    anchors: BTreeMap<String, usize>,
    source_len: usize,
}
impl Graph<'_> {
    fn next(&mut self) -> Result<Event> {
        self.parser
            .parse()
            .map_err(|_| error("invalid_identity_yaml"))
    }
    fn offset(&self, index: u64) -> Result<usize> {
        require(index as usize <= self.source_len, "invalid_identity_yaml")?;
        Ok(index as usize)
    }
    fn node(&mut self, event: Event, depth: usize) -> Result<usize> {
        require(depth <= 128 && self.nodes.len() < 200_000, "history_limit")?;
        if let E::Alias { anchor } = &event.data {
            return self
                .anchors
                .get(anchor)
                .copied()
                .ok_or_else(|| error("invalid_identity_yaml"));
        }
        let id = self.nodes.len();
        let start = self.offset(event.start_mark.index)?;
        let mut end = self.offset(event.end_mark.index)?;
        self.nodes.push(Node {
            start,
            end,
            kind: Kind::Pending,
        });
        let anchor = match &event.data {
            E::Scalar { anchor, .. }
            | E::SequenceStart { anchor, .. }
            | E::MappingStart { anchor, .. } => anchor.clone(),
            _ => None,
        };
        if let Some(anchor) = anchor {
            require(
                self.anchors.insert(anchor, id).is_none(),
                "invalid_identity_yaml",
            )?;
        }
        let kind = match event.data {
            E::Scalar { value, style, .. } => Kind::Scalar(value, style),
            E::SequenceStart { .. } => {
                let mut values = vec![];
                loop {
                    let event = self.next()?;
                    if matches!(event.data, E::SequenceEnd) {
                        end = self.offset(event.end_mark.index)?;
                        break;
                    }
                    values.push(self.node(event, depth + 1)?);
                }
                Kind::List(values)
            }
            E::MappingStart { .. } => {
                let mut pairs = vec![];
                loop {
                    let event = self.next()?;
                    if matches!(event.data, E::MappingEnd) {
                        end = self.offset(event.end_mark.index)?;
                        break;
                    }
                    let key = self.node(event, depth + 1)?;
                    let event = self.next()?;
                    pairs.push((key, self.node(event, depth + 1)?));
                }
                Kind::Map(pairs)
            }
            _ => return Err(error("invalid_identity_yaml")),
        };
        self.nodes[id] = Node { start, end, kind };
        Ok(id)
    }
    fn scalar(&self, id: usize) -> Option<&str> {
        if let Kind::Scalar(value, _) = &self.nodes[id].kind {
            Some(value)
        } else {
            None
        }
    }
}
fn parse(value: &str) -> Result<(Graph<'_>, usize)> {
    require(value.len() <= 16 * 1024 * 1024, "history_limit")?;
    let mut parser = Parser::new();
    parser.set_input(value.as_bytes());
    let mut graph = Graph {
        parser,
        nodes: vec![],
        anchors: BTreeMap::new(),
        source_len: value.len(),
    };
    require(
        matches!(graph.next()?.data, E::StreamStart { .. }),
        "invalid_identity_yaml",
    )?;
    require(
        matches!(graph.next()?.data, E::DocumentStart { .. }),
        "invalid_identity_yaml",
    )?;
    let event = graph.next()?;
    let root = graph.node(event, 0)?;
    require(
        matches!(graph.next()?.data, E::DocumentEnd { .. })
            && matches!(graph.next()?.data, E::StreamEnd),
        "invalid_identity_yaml",
    )?;
    Ok((graph, root))
}
fn token_count(value: &str, old: &str, new: &str) -> (String, usize) {
    let count = (value.len() - tokens(value, old, "").len()) / old.len();
    (tokens(value, old, new), count)
}
struct Visitor<'a, 'b> {
    graph: &'a Graph<'b>,
    source: &'a str,
    old: &'a str,
    new: &'a str,
    predicate: &'a str,
    snapshot: &'a str,
    seen: BTreeSet<usize>,
    spans: Vec<(usize, usize, String, usize)>,
}
impl Visitor<'_, '_> {
    fn original(&self, id: usize) -> &str {
        let n = &self.graph.nodes[id];
        &self.source[n.start..n.end]
    }
    fn span(&mut self, id: usize, value: String, count: usize) {
        let n = &self.graph.nodes[id];
        self.spans.push((n.start, n.end, value, count));
    }
    fn protect(&mut self, id: usize) {
        self.span(id, self.original(id).into(), 0);
    }
    fn visit(&mut self, id: usize, executable: bool, snapshot: bool) -> Result<()> {
        if !self.seen.insert(id) {
            return Ok(());
        }
        match &self.graph.nodes[id].kind {
            Kind::Map(pairs)
                if executable
                    && pairs.len() == 1
                    && self.graph.scalar(pairs[0].0) == Some("expr") =>
            {
                let value = pairs[0].1;
                let original = self.graph.scalar(value).map(|v| obj([("expr", s(v))]));
                if let Some(original) = original.filter(|v| {
                    crate::reasoning_language::legacy_references(v)
                        .iter()
                        .any(|id| id == self.old)
                }) {
                    let changed = expression(&original, self.old, self.new)?;
                    let mut replacement = serde_json::to_string(text(&map(&changed)?["expr"])?)?;
                    if matches!(
                        self.graph.nodes[value].kind,
                        Kind::Scalar(_, ScalarStyle::Literal | ScalarStyle::Folded)
                    ) && self.original(value).ends_with('\n')
                    {
                        replacement.push('\n');
                    }
                    self.span(value, replacement, 1);
                } else {
                    self.protect(value);
                }
            }
            Kind::Map(pairs)
                if executable
                    && pairs.len() == 1
                    && self
                        .graph
                        .scalar(pairs[0].0)
                        .is_some_and(|v| ["text", "num", "bool"].contains(&v)) =>
            {
                self.protect(pairs[0].1)
            }
            Kind::Map(pairs) => {
                for &(key, value) in pairs {
                    if snapshot
                        && self.graph.scalar(key) == Some("computed")
                        && matches!(self.graph.nodes[value].kind, Kind::Map(_))
                    {
                        if let Kind::Map(parts) = &self.graph.nodes[value].kind {
                            for &(part, payload) in parts {
                                match self.graph.scalar(part) {
                                    Some("value") => self.protect(payload),
                                    Some("rule") => self.visit(payload, true, false)?,
                                    _ => {}
                                }
                            }
                        }
                    } else {
                        let key = self.graph.scalar(key);
                        self.visit(
                            value,
                            executable || key == Some("rule") || key == Some(self.predicate),
                            snapshot || key == Some(self.snapshot),
                        )?;
                    }
                }
            }
            Kind::List(values) => {
                for &value in values {
                    self.visit(value, executable, snapshot)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}
/// Exact whole-token edits, including the reference's count of changed expression scalars.
pub(crate) fn rewrite_text(
    source: &str,
    old: &str,
    new: &str,
    predicate: &str,
    snapshot: &str,
) -> Result<(String, usize)> {
    require(!old.is_empty(), "invalid_identity_subjects")?;
    let (graph, root) = match parse(source) {
        Ok(v) => v,
        Err(e) if e.0 == "history_limit" => return Err(e),
        Err(_) => return Ok(token_count(source, old, new)),
    };
    let mut visitor = Visitor {
        graph: &graph,
        source,
        old,
        new,
        predicate,
        snapshot,
        seen: BTreeSet::new(),
        spans: vec![],
    };
    visitor.visit(root, false, false)?;
    visitor.spans.sort();
    let (mut out, mut count, mut start) = (String::new(), 0, 0);
    for (left, right, replacement, edits) in visitor.spans {
        require(
            start <= left && left <= right && right <= source.len(),
            "invalid_identity_yaml",
        )?;
        let (changed, n) = token_count(&source[start..left], old, new);
        out.push_str(&changed);
        out.push_str(&replacement);
        count += n + edits;
        start = right;
    }
    let (changed, n) = token_count(&source[start..], old, new);
    out.push_str(&changed);
    Ok((out, count + n))
}
fn scalar_spans(
    graph: &Graph<'_>,
    id: usize,
    seen: &mut BTreeSet<usize>,
    out: &mut Vec<(usize, usize)>,
) {
    if !seen.insert(id) {
        return;
    }
    let node = &graph.nodes[id];
    match &node.kind {
        Kind::Scalar(_, _) => out.push((node.start, node.end)),
        Kind::List(values) => {
            for &v in values {
                scalar_spans(graph, v, seen, out);
            }
        }
        Kind::Map(pairs) => {
            for &(k, v) in pairs {
                scalar_spans(graph, k, seen, out);
                scalar_spans(graph, v, seen, out);
            }
        }
        Kind::Pending => {}
    }
}
fn whitespace(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}
fn distinct_ids(value: &str) -> Vec<&str> {
    value
        .split(|c| c == ',' || whitespace(c))
        .filter(|v| !v.is_empty())
        .collect()
}
fn string_scalar(value: &str, like: &str) -> String {
    if like.starts_with('\'') && !value.contains('\n') {
        return format!("'{}'", value.replace('\'', "''"));
    }
    let date = Regex::new(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}$").unwrap();
    if date.is_match(like) && date.is_match(value) {
        return value.into();
    }
    let bare = Regex::new(r"^[A-Za-z_][A-Za-z0-9_.-]*$").unwrap();
    if !like.starts_with(['\'', '"'])
        && bare.is_match(value)
        && crate::history_yaml::decode_ordinary_source_value(value.as_bytes())
            .is_ok_and(|v| v.projected() == s(value))
    {
        return value.into();
    }
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    )
}
pub(crate) fn dedupe_text(source: &str, survivor: &str) -> String {
    let flow = if let Ok((graph, root)) = parse(source) {
        let mut protected = vec![];
        scalar_spans(&graph, root, &mut BTreeSet::new(), &mut protected);
        Regex::new(r"\[([^\[\]]*)\]")
            .unwrap()
            .replace_all(source, |m: &regex::Captures<'_>| {
                let at = m.get(0).unwrap().start();
                if protected.iter().any(|(a, b)| *a <= at && at < *b)
                    || token_count(&m[1], survivor, "").1 < 2
                {
                    return m[0].to_owned();
                }
                let mut had = false;
                let inner = m[1].replace('\n', " ");
                let mut out = vec![];
                for v in inner.split(',').map(str::trim).filter(|v| !v.is_empty()) {
                    if v == survivor {
                        if had {
                            continue;
                        }
                        had = true;
                    }
                    out.push(v);
                }
                format!("[{}]", out.join(", "))
            })
            .into_owned()
    } else {
        source.into()
    };
    Regex::new(r#"(?m)^(\s*distinct_from:\s*)(["']?)([A-Za-z0-9_.,\s]+?)(["']?)\s*$"#)
        .unwrap()
        .replace_all(&flow, |m: &regex::Captures<'_>| {
            if m[2] != m[4] {
                return m[0].into();
            }
            let mut ids = vec![];
            for id in distinct_ids(&m[3]) {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            format!("{}{}", &m[1], string_scalar(&ids.join(", "), ""))
        })
        .into_owned()
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}
fn members(lines: &[String], start: usize, end: usize) -> (Option<usize>, Vec<(String, usize)>) {
    let mut ind = None;
    let mut out = vec![];
    for (i, line) in lines.iter().enumerate().take(end).skip(start + 1) {
        if line.trim().is_empty() || line.trim().starts_with('#') {
            continue;
        }
        let expected = *ind.get_or_insert(indent(line));
        if let Some(m) = MEMBER.captures(line) {
            if m[1].len() == expected {
                out.push((m[2].into(), i));
            }
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
fn collections(lines: &[String]) -> Vec<(String, usize, usize)> {
    let starts = lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| TOP.captures(l).map(|m| (m[1].to_owned(), i)))
        .collect::<Vec<_>>();
    starts
        .iter()
        .enumerate()
        .map(|(i, (n, s))| {
            (
                n.clone(),
                *s,
                starts.get(i + 1).map_or(lines.len(), |(_, s)| *s),
            )
        })
        .collect()
}
fn locate(lines: &[String], id: &str) -> Option<(String, usize, usize, usize)> {
    for (name, start, end) in collections(lines) {
        let (ind, entries) = members(lines, start, end);
        for (entry, i) in entries {
            if entry == id {
                return Some((
                    name,
                    ind.unwrap(),
                    i,
                    block_end(lines, i, ind.unwrap(), end),
                ));
            }
        }
    }
    None
}
fn inline(line: &str) -> &str {
    line.split_once(':').map_or("", |(_, v)| v.trim())
}
fn field_span(
    lines: &[String],
    start: usize,
    end: usize,
    field: &str,
) -> Option<(usize, usize, usize)> {
    let (ind, fields) = members(lines, start, end);
    fields
        .into_iter()
        .find(|(name, _)| name == field)
        .map(|(_, i)| (ind.unwrap(), i, block_end(lines, i, ind.unwrap(), end)))
}
fn replace_field(
    lines: &mut Vec<String>,
    start: usize,
    end: usize,
    field: &str,
    new: Vec<String>,
) -> usize {
    let (_, a, b) = field_span(lines, start, end, field).unwrap_or((0, end, end));
    let next = end + new.len() - (b - a);
    lines.splice(a..b, new);
    next
}
fn field_lines(field: &str, value: &Source, ind: usize) -> Result<Vec<String>> {
    let lines = crate::legacy_authoring::entry_lines_ordered(
        "entry",
        &Source::Map(vec![(field.into(), value.clone())]),
        ind.saturating_sub(2),
        ind,
        false,
    )?;
    Ok(lines.into_iter().skip(1).collect())
}
fn python_equal(left: &V, right: &V) -> bool {
    match (left, right) {
        (V::List(a), V::List(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| python_equal(a, b))
        }
        (V::Map(a), V::Map(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(k, a)| b.get(k).is_some_and(|b| python_equal(a, b)))
        }
        (V::Map(_) | V::List(_), _) | (_, V::Map(_) | V::List(_)) => false,
        _ => crate::history_yaml::ordinary_key(left.clone())
            .ok()
            .zip(crate::history_yaml::ordinary_key(right.clone()).ok())
            .is_some_and(|(a, b)| a.python_eq(&b)),
    }
}
fn set_fields(lines: &mut Vec<String>, id: &str, body: &Source, was: &Source) -> Result<()> {
    let (_, ind, start, mut end) =
        locate(lines, id).ok_or_else(|| error("identity_target_not_found"))?;
    let find = members(lines, start, end).0.unwrap_or(ind + 2);
    if inline(&lines[start]).starts_with('{') {
        let comments = lines[start + 1..end]
            .iter()
            .filter(|l| l.trim().starts_with('#'))
            .cloned()
            .collect::<Vec<_>>();
        let mut new = crate::legacy_authoring::entry_lines_ordered(id, body, ind, find, true)?;
        new.extend(comments);
        lines.splice(start..end, new);
        return Ok(());
    }
    let (Source::Map(body), Source::Map(was)) = (body, was) else {
        return Err(error("identity_invalid_body"));
    };
    for (field, _) in was {
        if field != "also" && !body.iter().any(|(k, _)| k == field) {
            if let Some((_, a, b)) = field_span(lines, start, end, field) {
                lines.drain(a..b);
                end -= b - a;
            }
        }
    }
    for (field, value) in body {
        if was
            .iter()
            .any(|(k, v)| k == field && python_equal(&v.typed(), &value.typed()))
        {
            continue;
        }
        end = replace_field(lines, start, end, field, field_lines(field, value, find)?);
    }
    Ok(())
}
fn drop_key(
    lines: &mut Vec<String>,
    start: usize,
    end: usize,
    field: &str,
    key: &str,
) -> Result<usize> {
    let Some((_, a, b)) = field_span(lines, start, end, field) else {
        return Ok(end);
    };
    if inline(&lines[a]).starts_with('{') {
        let block = lines[a..b].join("\n");
        let re = Regex::new(&format!(r"{}\s*:\s*{FLOW}\s*,?\s*", regex::escape(key))).unwrap();
        let (mut output, mut last) = (String::new(), 0);
        for m in re.find_iter(&block) {
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
        let new = output.split('\n').map(str::to_owned).collect::<Vec<_>>();
        let next = end + new.len() - (b - a);
        lines.splice(a..b, new);
        Ok(next)
    } else if let Some((_, a, b)) = field_span(lines, a, b, key) {
        lines.drain(a..b);
        Ok(end - (b - a))
    } else {
        Ok(end)
    }
}
fn add_in(lines: &mut Vec<String>, id: &str, body: &Source, collection: &str) -> Result<()> {
    let (_, start, end) = collections(lines)
        .into_iter()
        .find(|(name, _, _)| name == collection)
        .ok_or_else(|| error(&format!("no collection {collection} in this file")))?;
    let (ind, entries) = members(lines, start, end);
    let ind = ind.unwrap_or(2);
    if entries.is_empty() {
        let new = crate::legacy_authoring::entry_lines_ordered(id, body, ind, ind + 2, false)?;
        lines.splice(start + 1..start + 1, new);
        return Ok(());
    }
    let common = |a: &str| {
        a.split('.')
            .zip(id.split('.'))
            .take_while(|(a, b)| a == b)
            .count()
    };
    let best = entries.iter().map(|(n, _)| common(n)).max().unwrap();
    let siblings = entries
        .iter()
        .filter(|(n, _)| best == 0 || common(n) == best)
        .collect::<Vec<_>>();
    let (anchor, before) = siblings
        .iter()
        .find(|(n, _)| n.as_str() > id)
        .map_or((*siblings.last().unwrap(), false), |v| (*v, true));
    let (_, at) = anchor;
    let ae = block_end(lines, *at, ind, end);
    let find = members(lines, *at, ae).0.unwrap_or(ind + 2);
    let mut new = crate::legacy_authoring::entry_lines_ordered(
        id,
        body,
        ind,
        find,
        inline(&lines[*at]).starts_with('{'),
    )?;
    let mut pos = if before { *at } else { ae };
    if before {
        if pos > 0 && lines[pos - 1].trim().is_empty() && pos - 1 > start {
            new.push(String::new());
        }
    } else if pos < end && lines[pos].trim().is_empty() {
        pos += 1;
        new.push(String::new());
    }
    lines.splice(pos..pos, new);
    Ok(())
}
/// Edit one physical member of a world before the whole-token rename.
pub(crate) fn same_file(
    source: &str,
    survivor: &str,
    retired: &str,
    raw: &crate::history_contract::Map,
    merged: Option<&Source>,
    deps: &str,
    snapshot: &str,
) -> Result<String> {
    let mut lines = source.split('\n').map(str::to_owned).collect::<Vec<_>>();
    for (id, body) in raw {
        if id == survivor || id == retired || locate(&lines, id).is_none() {
            continue;
        }
        let Ok(body) = map(body) else {
            continue;
        };
        let both_deps = body
            .get(deps)
            .and_then(|v| if let V::List(v) = v { Some(v) } else { None })
            .filter(|v| v.contains(&s(survivor)) && v.contains(&s(retired)));
        let both_seen = body
            .get(snapshot)
            .and_then(|v| map(v).ok())
            .is_some_and(|m| m.contains_key(survivor) && m.contains_key(retired));
        if both_deps.is_none() && !both_seen {
            continue;
        }
        let (_, ind, start, mut end) = locate(&lines, id).unwrap();
        let find = members(&lines, start, end).0.unwrap_or(ind + 2);
        if let Some(values) = both_deps {
            let value = Source::from_typed(&V::List(
                values
                    .iter()
                    .filter(|v| **v != s(retired))
                    .cloned()
                    .collect(),
            ));
            end = replace_field(
                &mut lines,
                start,
                end,
                deps,
                field_lines(deps, &value, find)?,
            );
        }
        if both_seen {
            drop_key(&mut lines, start, end, snapshot, retired)?;
        }
    }
    let mut removed = None;
    if raw.contains_key(retired) {
        if let Some((collection, _, start, end)) = locate(&lines, retired) {
            let block = lines[start..end].to_vec();
            let after = if end < lines.len()
                && lines[end].trim().is_empty()
                && start > 0
                && lines[start - 1].trim().is_empty()
            {
                end + 1
            } else {
                end
            };
            lines.drain(start..after);
            if let Some((_, start, end)) = collections(&lines)
                .into_iter()
                .find(|(name, _, _)| *name == collection)
            {
                if members(&lines, start, end).1.is_empty() {
                    lines.drain(start..end);
                }
            }
            removed = Some((collection, block));
        }
    }
    if let Some(merged) = merged {
        if locate(&lines, survivor).is_some() {
            set_fields(
                &mut lines,
                survivor,
                merged,
                &Source::from_typed(&raw[survivor]),
            )?;
        } else if !raw.contains_key(survivor) {
            if let Some((collection, _)) = &removed {
                add_in(&mut lines, survivor, merged, collection)?;
            }
        }
    }
    if let Some((_, block)) = removed {
        if let Some((_, ind, start, end)) = locate(&lines, survivor) {
            let find = members(&lines, start, end).0.unwrap_or(ind + 2);
            let comments = block
                .iter()
                .filter(|l| l.trim().starts_with('#'))
                .map(|l| format!("{}{}", " ".repeat(find), l.trim()))
                .collect::<Vec<_>>();
            lines.splice(end..end, comments);
        }
    }
    Ok(lines.join("\n"))
}
pub(crate) fn append_also(source: &str, survivor: &str, also: &[String]) -> Result<String> {
    let mut lines = source.split('\n').map(str::to_owned).collect::<Vec<_>>();
    let Some((_, ind, start, mut end)) = locate(&lines, survivor) else {
        return Ok(source.into());
    };
    if inline(&lines[start]).starts_with('{') {
        let block = lines[start..end].join("\n");
        let value = format!("also: [{}]", also.join(", "));
        let changed = if Regex::new(r"\balso:").unwrap().is_match(&block) {
            Regex::new(&format!(r"\balso:\s*(\[[^\]]*\]|{FLOW})"))
                .unwrap()
                .replace_all(&block, regex::NoExpand(&value))
                .into_owned()
        } else {
            let at = block
                .rfind('}')
                .ok_or_else(|| error("invalid_identity_yaml"))?;
            format!("{}, {value}{}", block[..at].trim_end(), &block[at..])
        };
        lines.splice(start..end, changed.split('\n').map(str::to_owned));
    } else {
        let find = members(&lines, start, end).0.unwrap_or(ind + 2);
        let new = field_lines(
            "also",
            &Source::List(also.iter().map(|v| Source::Scalar(s(v))).collect()),
            find,
        )?;
        if field_span(&lines, start, end, "also").is_some() {
            replace_field(&mut lines, start, end, "also", new);
        } else {
            while end > start + 1 && lines[end - 1].trim().starts_with('#') {
                end -= 1;
            }
            lines.splice(end..end, new);
        }
    }
    Ok(lines.join("\n"))
}
fn sections(brief: &V) -> Vec<&V> {
    let mut sections = match get(brief, "sections") {
        V::List(a) => a.iter().filter(|v| matches!(v, V::Map(_))).collect(),
        _ => vec![],
    };
    if let V::List(tabs) = get(brief, "tabs") {
        for tab in tabs {
            if let V::List(a) = get(tab, "sections") {
                sections.extend(a.iter().filter(|v| matches!(v, V::Map(_))));
            }
        }
    }
    sections
}
fn section_span(lines: &[String], title: &str) -> Option<(usize, usize)> {
    let item = Regex::new(r"^( *)- +title:\s*(.+?)\s*$").unwrap();
    for (i, line) in lines.iter().enumerate() {
        let Some(m) = item.captures(line) else {
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
pub(crate) fn brief_file(source: &str, survivor: &str, retired: &str) -> Result<String> {
    let brief = crate::history_yaml::decode_ordinary_source_value(source.as_bytes())?.projected();
    let mut lines = source.split('\n').map(str::to_owned).collect::<Vec<_>>();
    for section in sections(&brief) {
        if map(get(section, "seen"))
            .is_ok_and(|m| m.contains_key(survivor) && m.contains_key(retired))
            && crate::history_view::truth(get(section, "text"))
        {
            if let Some((start, end)) = section_span(
                &lines,
                &crate::source_text::ordinary_python_str(get(section, "title")),
            ) {
                drop_key(&mut lines, start, end, "seen", retired)?;
            }
        }
    }
    if map(get(&brief, "labels")).is_ok_and(|m| m.contains_key(survivor) && m.contains_key(retired))
    {
        if let Some((_, start, end)) = collections(&lines)
            .into_iter()
            .find(|(name, _, _)| name == "labels")
        {
            let (ind, labels) = members(&lines, start, end);
            if let Some((_, at)) = labels.into_iter().find(|(name, _)| name == retired) {
                let end = block_end(&lines, at, ind.unwrap(), end);
                lines.drain(at..end);
            }
        }
    }
    Ok(lines.join("\n"))
}
fn template_refs(value: &V, id: &str) -> bool {
    let V::Text(value) = value else {
        return false;
    };
    Regex::new(r"\{\{\s*([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+)\s*\}\}")
        .unwrap()
        .captures_iter(value)
        .any(|m| &m[1] == id)
}
fn iterable_text(value: &V) -> Vec<String> {
    match value {
        V::List(a) => a
            .iter()
            .map(crate::source_text::ordinary_python_str)
            .collect(),
        V::Map(m) => m.keys().cloned().collect(),
        V::Text(s) => s.chars().map(|c| c.to_string()).collect(),
        _ => vec![],
    }
}
pub(crate) fn mentions(
    retired: &str,
    worlds: &[(Option<String>, crate::history_contract::Map)],
    fields: &crate::history_contract::Map,
    brief: Option<&V>,
    sources: &super::ordinary_sameness::Sources<'_>,
) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = vec![];
    let mut hit = |kind: &str, at: String| {
        if let Some((_, places)) = out.iter_mut().find(|(k, _)| k == kind) {
            places.push(at);
        } else {
            out.push((kind.into(), vec![at]));
        }
    };
    let field = |name: &str| {
        fields
            .get(name)
            .and_then(|v| text(v).ok())
            .unwrap_or("")
            .to_owned()
    };
    let (deps, snapshot, predicate) = (field("deps"), field("snapshot"), field("predicate"));
    for (name, raw) in worlds {
        let tag = name
            .as_ref()
            .map_or(String::new(), |name| format!(" (in hypothesis {name})"));
        for (id, body) in raw {
            if id == retired || !matches!(body, V::Map(_)) {
                continue;
            }
            if matches!(get(body, &deps), V::List(a) if a.contains(&s(retired))) {
                hit(&deps, format!("{id}{tag}"));
            }
            if map(get(body, &snapshot)).is_ok_and(|m| m.contains_key(retired)) {
                hit(&snapshot, format!("{id}{tag}"));
            }
            let references = |v: &V| {
                matches!(v, V::Map(_) if crate::reasoning_language::legacy_references(v).iter().any(|v| v == retired))
                    || matches!(v, V::Text(v) if token_count(v, retired, "").1 > 0)
            };
            if references(get(body, &predicate)) {
                hit(&predicate, format!("{id}{tag}"));
            }
            if references(&rule_of(body)) {
                hit("rule", format!("{id}{tag}"));
            }
            let source = name
                .as_ref()
                .and_then(|n| sources.hypotheses.get(n).copied())
                .or_else(|| if name.is_none() { sources.base } else { None });
            let ordered = source.and_then(|source| {
                if let crate::history_yaml::OrdinaryValue::Map(collections) = source {
                    collections.iter().find_map(|(_, v)| v.get(id))
                } else {
                    None
                }
            });
            let body_map = map(body).unwrap();
            let fields = if let Some(crate::history_yaml::OrdinaryValue::Map(fields)) = ordered {
                fields
                    .iter()
                    .filter_map(|(k, _)| k.text())
                    .collect::<Vec<_>>()
            } else {
                body_map.keys().map(String::as_str).collect()
            };
            for field in fields {
                if body_map
                    .get(field)
                    .is_some_and(|v| template_refs(v, retired))
                {
                    hit("text", format!("{id} ({field}){tag}"));
                }
            }
            if matches!(get(body, "from"), V::Text(v) if v.split(whitespace).filter(|v| !v.is_empty()).collect::<Vec<_>>().join(" ") == retired)
            {
                hit("from", format!("{id}{tag}"));
            }
            if ids_value(get(body, "distinct_from")).contains(&retired.to_owned()) {
                hit("distinct_from", format!("{id}{tag}"));
            }
        }
    }
    if let Some(brief) = brief {
        let title = |value: &V| {
            let v = get(value, "title");
            let title = if crate::history_view::truth(v) {
                crate::source_text::ordinary_python_str(v)
            } else {
                "?".into()
            };
            crate::source_text::ordinary_python_repr(&s(&title))
        };
        if let V::List(tabs) = get(brief, "tabs") {
            for tab in tabs {
                if iterable_text(get(tab, "serves")).contains(&retired.to_owned()) {
                    hit("the brief", format!("tab {} (serves)", title(tab)));
                }
            }
        }
        for section in sections(brief) {
            let mut what = vec![];
            if template_refs(get(section, "text"), retired) {
                what.push("text");
            }
            if map(get(section, "seen")).is_ok_and(|m| m.contains_key(retired)) {
                what.push("seen");
            }
            let pick = get(section, "pick");
            let picks = if let V::Text(v) = pick {
                vec![v.clone()]
            } else {
                iterable_text(pick)
            };
            if picks.contains(&retired.to_owned()) {
                what.push("pick");
            }
            if !what.is_empty() {
                hit(
                    "the brief",
                    format!("section {} ({})", title(section), what.join(", ")),
                );
            }
        }
        let groups = get(brief, "groups");
        let groups = crate::history_emit::encode_source(&Source::from_typed(groups), 80)
            .ok()
            .and_then(|v| String::from_utf8(v).ok())
            .unwrap_or_default();
        if token_count(&groups, retired, "").1 > 0
            || map(get(brief, "labels")).is_ok_and(|m| m.contains_key(retired))
        {
            hit("the brief", "groups and labels".into());
        }
    }
    out
}
const READING: &[&str] = &[
    "v", "quoted", "rule", "unit", "from", "at", "of", "read", "url", "file",
];
fn get<'a>(value: &'a V, key: &str) -> &'a V {
    map(value).ok().and_then(|m| m.get(key)).unwrap_or(&V::Null)
}
fn shaped(value: &V, deps: &str) -> bool {
    matches!(get(value, deps), V::List(a) if !a.is_empty() && a.iter().all(|v| matches!(v, V::Text(_))))
}
fn rule_of(value: &V) -> V {
    match get(value, "rule") {
        v @ V::Map(_) => v.clone(),
        V::Text(v) if !v.trim().is_empty() => s(v),
        _ => match get(value, "v") {
            V::Text(v)
                if crate::ordinary_reader::EXPR.is_match(v)
                    && crate::ordinary_reader::ID.is_match(v) =>
            {
                s(v)
            }
            _ => V::Null,
        },
    }
}
fn claim(value: &V) -> V {
    if !matches!(value, V::Map(_)) {
        return value.clone();
    }
    let v = if get(value, "v") != &V::Null {
        get(value, "v")
    } else {
        get(value, "quoted")
    };
    if v != &V::Null
        && !matches!(v, V::Text(v) if crate::ordinary_reader::EXPR.is_match(v) && crate::ordinary_reader::ID.is_match(v))
    {
        v.clone()
    } else {
        rule_of(value)
    }
}
fn verdict(value: &V) -> &V {
    if get(value, "verdict") == &V::Null {
        get(value, "title")
    } else {
        get(value, "verdict")
    }
}
fn short(value: &V) -> String {
    crate::public_ordinary_readers::short(value, 40)
}
fn add_ordered(body: &mut Vec<(String, Source)>, key: &str, value: Source) {
    if let Some((_, old)) = body.iter_mut().find(|(k, _)| k == key) {
        *old = value;
    } else {
        body.push((key.into(), value));
    }
}
fn ids_value(value: &V) -> Vec<String> {
    let value = if crate::history_view::truth(value) {
        crate::source_text::ordinary_python_str(value)
    } else {
        String::new()
    };
    distinct_ids(&value)
        .into_iter()
        .map(str::to_owned)
        .collect()
}
pub(crate) struct Merge<'a, 'r> {
    pub survivor: &'a str,
    pub retired: &'a str,
    pub reader: &'a crate::ordinary_reader::Reader<'r>,
    pub as_of: Option<&'a str>,
}
pub(crate) type Supersede<'a> =
    dyn FnMut(&str, &V, &V, Option<&str>) -> Result<(bool, String)> + 'a;
impl Merge<'_, '_> {
    /// Preserve reading provenance and source field order; admission is supplied by the shared writer.
    pub(crate) fn bodies(
        &self,
        sb: &Source,
        rb: &Source,
        supersede: &mut Supersede<'_>,
    ) -> Result<(Source, Vec<String>)> {
        let (Source::Map(sm), Source::Map(rm)) = (sb, rb) else {
            return Err(error("identity_invalid_body"));
        };
        let (sv, rv) = (sb.typed(), rb.typed());
        let (survivor, retired) = (self.survivor, self.retired);
        let mut notes = vec![];
        let mut body;
        if shaped(&sv, text(&self.reader.fields()["deps"])?) {
            let (vs, vr) = (verdict(&sv), verdict(&rv));
            if vs != &V::Null && vr != &V::Null && !crate::ordinary_reader::same_legacy(vs, vr) {
                let (ok, why) = supersede(survivor, &sv, &rv, self.as_of)?;
                if !ok {
                    let vs = crate::source_text::ordinary_python_repr(&s(
                        &crate::public_ordinary_readers::short(vs, 60),
                    ));
                    let vr = crate::source_text::ordinary_python_repr(&s(
                        &crate::public_ordinary_readers::short(vr, 60),
                    ));
                    return Err(error(&format!(
                        "refused - {survivor} concludes {vs} and {retired} {vr} - {why}: two verdicts on one subject are a contradiction, not one subject twice; keep the one that holds, or write the other through add --hypothesis, for a person to fold"
                    )));
                }
                body = rm.clone();
                notes.push(format!("{survivor} takes {retired}'s verdict - {why}"));
            } else {
                body = sm.clone();
            }
        } else {
            let (cs, cr) = (claim(&sv), claim(&rv));
            let rules = [&sv, &rv].iter().all(|b| {
                get(b, "rule") != &V::Null
                    && get(b, "v") == &V::Null
                    && get(b, "quoted") == &V::Null
            });
            let equal = if rules && matches!(cs, V::Map(_)) && matches!(cr, V::Map(_)) {
                python_equal(&cs, &cr) || {
                    let lower = |v: &V| {
                        crate::reasoning_language::legacy_expression(v, false)
                            .or_else(|_| crate::reasoning_language::legacy_expression(v, true))
                    };
                    lower(&cs)
                        .ok()
                        .zip(lower(&cr).ok())
                        .is_some_and(|(a, b)| a == b)
                }
            } else {
                crate::ordinary_reader::same_legacy(&cs, &cr)
            };
            let ds = crate::reasoning_authoring_guards::read_on(&sv, self.reader);
            let dr = crate::reasoning_authoring_guards::read_on(&rv, self.reader);
            let mut other = sm.is_empty();
            if !sm.is_empty() && cr != V::Null && (cs == V::Null || !equal) {
                if dr.is_some() && (ds.is_none() || dr >= ds) {
                    let (ok, why) = supersede(survivor, &sv, &cr, dr.as_deref())?;
                    if !ok {
                        return Err(error(&format!(
                            "refused - {survivor} holds {} as of {} and {retired} holds {} as of {} - {why}: two readings that disagree are a contradiction, not one subject twice; set the one that is right, or open a hypothesis, then same",
                            short(&cs),
                            ds.as_deref().unwrap_or("None"),
                            short(&cr),
                            dr.as_deref().unwrap()
                        )));
                    }
                    other = true;
                    notes.push(format!(
                        "{survivor} takes {retired}'s reading: {} -> {} - {why}",
                        short(&cs),
                        short(&cr)
                    ));
                } else if ds.is_none() && dr.is_none() {
                    return Err(error(&format!(
                        "refused - {survivor} holds {} and {retired} holds {}, and nothing dates either reading: two readings that disagree are a contradiction, not one subject twice; set the one that is right, or open a hypothesis, then same",
                        short(&cs),
                        short(&cr)
                    )));
                } else {
                    notes.push(format!(
                        "{survivor} keeps its reading {} as of {}; {retired}'s {}{} is older",
                        short(&cs),
                        ds.as_deref().unwrap_or("None"),
                        short(&cr),
                        dr.as_ref()
                            .map_or(", undated".into(), |d| format!(" as of {d}"))
                    ));
                }
            } else if !sm.is_empty()
                && cs == V::Null
                && cr == V::Null
                && dr.is_some()
                && (ds.is_none() || dr > ds)
            {
                other = true;
                notes.push(format!(
                    "{survivor} takes {retired}'s reading of {}",
                    dr.as_deref().unwrap()
                ));
            }
            if sm.is_empty() {
                body = rm.clone();
            } else if other {
                body = vec![];
                for (field, value) in sm {
                    if READING.contains(&field.as_str()) && field != "unit" {
                        if let Some((_, value)) = rm.iter().find(|(k, _)| k == field) {
                            body.push((field.clone(), value.clone()));
                        }
                    } else {
                        body.push((field.clone(), value.clone()));
                    }
                }
                for field in READING {
                    if !body.iter().any(|(k, _)| k == field) {
                        if let Some((_, value)) = rm.iter().find(|(k, _)| k == field) {
                            body.push(((*field).into(), value.clone()));
                        }
                    }
                }
                for (field, value) in rm {
                    if !body.iter().any(|(k, _)| k == field)
                        && !READING.contains(&field.as_str())
                        && !["also", "distinct_from"].contains(&field.as_str())
                    {
                        body.push((field.clone(), value.clone()));
                    }
                }
            } else {
                body = sm.clone();
                for (field, value) in rm {
                    if !body.iter().any(|(k, _)| k == field)
                        && !READING.contains(&field.as_str())
                        && !["also", "distinct_from"].contains(&field.as_str())
                    {
                        body.push((field.clone(), value.clone()));
                    }
                }
            }
        }
        if !crate::history_view::truth(get(&Source::Map(body.clone()).typed(), "name")) {
            let name = crate::reasoning_authoring::named(&rv);
            if !name.is_empty() {
                add_ordered(&mut body, "name", Source::Scalar(s(&name)));
            }
        }
        body.retain(|(k, _)| !["also", "distinct_from"].contains(&k.as_str()));
        let mut also = vec![];
        let mut distinct = vec![];
        for src in [&sv, &rv] {
            let aliases = match get(src, "also") {
                V::Text(v) => vec![s(v)],
                V::List(a) => a.clone(),
                _ => vec![],
            };
            for alias in aliases {
                if alias != s(survivor) && !also.iter().any(|a| python_equal(a, &alias)) {
                    also.push(alias);
                }
            }
            for id in ids_value(get(src, "distinct_from")) {
                if id != survivor && id != retired && !distinct.contains(&id) {
                    distinct.push(id);
                }
            }
        }
        if !also.iter().any(|v| *v == s(retired)) {
            also.push(s(retired));
        }
        body.push(("also".into(), Source::from_typed(&V::List(also))));
        if !distinct.is_empty() {
            body.push((
                "distinct_from".into(),
                Source::Scalar(s(&distinct.join(", "))),
            ));
        }
        Ok((Source::Map(body), notes))
    }
}
pub(crate) fn bump_updated(lines: &mut Vec<String>, stamp: &str) -> Result<bool> {
    for (name, start, end) in collections(lines) {
        if name != "meta" {
            continue;
        }
        if inline(&lines[start]).starts_with('{') {
            let block = lines[start..end].join("\n");
            let (graph, root) = parse(&block)?;
            let Kind::Map(root) = &graph.nodes[root].kind else {
                return Err(error("invalid_identity_yaml"));
            };
            let metadata = root[0].1;
            let Kind::Map(pairs) = &graph.nodes[metadata].kind else {
                return Err(error("invalid_identity_yaml"));
            };
            let changed = if let Some((_, id)) = pairs
                .iter()
                .find(|(k, _)| graph.scalar(*k) == Some("updated"))
            {
                let n = &graph.nodes[*id];
                format!(
                    "{}{}{}",
                    &block[..n.start],
                    string_scalar(stamp, &block[n.start..n.end]),
                    &block[n.end..]
                )
            } else {
                let at = graph.nodes[metadata].start + 1;
                format!(
                    "{}updated: {}{}{}",
                    &block[..at],
                    string_scalar(stamp, ""),
                    if pairs.is_empty() { "" } else { ", " },
                    &block[at..]
                )
            };
            lines.splice(start..end, changed.split('\n').map(str::to_owned));
            return Ok(true);
        }
        let re = Regex::new(r"^(\s+updated:\s*)(\S+)(\s*(?:#.*)?)$").unwrap();
        for line in lines.iter_mut().take(end).skip(start + 1) {
            if let Some(m) = re.captures(line) {
                *line = format!("{}{}{}", &m[1], string_scalar(stamp, &m[2]), &m[3]);
                return Ok(true);
            }
        }
        let ind = members(lines, start, end).0.unwrap_or(2);
        lines.insert(start + 1, format!("{}updated: {stamp}", " ".repeat(ind)));
        return Ok(true);
    }
    Ok(false)
}
/// The ordinary `distinct` line edit. Validation and publication belong to the caller.
pub(crate) fn distinct_text(
    source: &str,
    id: &str,
    ids: &str,
    why: &str,
    stamp: &str,
    in_hypothesis: bool,
) -> Result<String> {
    let mut lines = source.split('\n').map(str::to_owned).collect::<Vec<_>>();
    let (_, ind, start, end) =
        locate(&lines, id).ok_or_else(|| error("identity_target_not_found"))?;
    let written = string_scalar(ids, "");
    let comment = format!("# distinct {stamp}: {why}");
    if inline(&lines[start]).starts_with('{') {
        let block = lines[start..end].join("\n");
        let re = Regex::new(r"\bdistinct_from:").unwrap();
        let changed = if re.is_match(&block) {
            Regex::new(&format!(r"\bdistinct_from:\s*{FLOW}"))
                .unwrap()
                .replace_all(
                    &block,
                    regex::NoExpand(&format!("distinct_from: {written}")),
                )
                .into_owned()
        } else {
            let at = block
                .rfind('}')
                .ok_or_else(|| error("invalid_identity_yaml"))?;
            format!(
                "{}, distinct_from: {}{}",
                block[..at].trim_end(),
                written,
                &block[at..]
            )
        };
        let mut new = changed.split('\n').map(str::to_owned).collect::<Vec<_>>();
        new.push(format!("{}{comment}", " ".repeat(ind + 2)));
        lines.splice(start..end, new);
    } else {
        let find = members(&lines, start, end).0.unwrap_or(ind + 2);
        let end = replace_field(
            &mut lines,
            start,
            end,
            "distinct_from",
            vec![format!("{}distinct_from: {written}", " ".repeat(find))],
        );
        lines.insert(end, format!("{}{comment}", " ".repeat(find)));
    }
    if !in_hypothesis {
        bump_updated(&mut lines, stamp)?;
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn captured_text_edits_match_python() {
        let cases: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/ordinary-identity-text.json"
        ))
        .unwrap();
        for case in cases.as_array().unwrap() {
            let source = case["source"].as_str().unwrap();
            let actual = match case["kind"].as_str().unwrap() {
                "rewrite" => {
                    let (out, count) = rewrite_text(
                        source,
                        "p.old",
                        "p.keep",
                        case["predicate"].as_str().unwrap_or("wrong_if"),
                        case["snapshot"].as_str().unwrap_or("seen"),
                    )
                    .unwrap();
                    serde_json::json!([out, count])
                }
                "dedupe" => serde_json::json!(dedupe_text(source, "p.keep")),
                "distinct" => serde_json::json!(
                    distinct_text(
                        source,
                        "p.keep",
                        case["ids"].as_str().unwrap(),
                        case["why"].as_str().unwrap(),
                        "2026-09-20",
                        case["hypothesis"].as_bool().unwrap()
                    )
                    .unwrap()
                ),
                "same-file" => {
                    let doc = crate::history_yaml::decode_ordinary_source_value(source.as_bytes())
                        .unwrap()
                        .projected();
                    let raw = crate::reasoning_fields::collections(&doc)
                        .unwrap()
                        .into_values()
                        .flatten()
                        .collect();
                    let merged = crate::history_yaml::decode_source_value(
                        case["merged"].as_str().unwrap().as_bytes(),
                    )
                    .unwrap();
                    let changed = same_file(
                        source,
                        "p.keep",
                        "p.old",
                        &raw,
                        Some(&merged),
                        case["deps"].as_str().unwrap(),
                        case["snapshot"].as_str().unwrap(),
                    );
                    if let Some(failure) = case["error"].as_str() {
                        assert_eq!(changed.unwrap_err().0, failure, "{}", case["name"]);
                        continue;
                    }
                    let (changed, _) = rewrite_text(
                        &changed.unwrap(),
                        "p.old",
                        "p.keep",
                        "wrong_if",
                        case["snapshot"].as_str().unwrap(),
                    )
                    .unwrap();
                    let changed = dedupe_text(&changed, "p.keep");
                    let also = case["also"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| v.as_str().unwrap().to_owned())
                        .collect::<Vec<_>>();
                    let changed = append_also(&changed, "p.keep", &also).unwrap();
                    let mut lines = changed.split('\n').map(str::to_owned).collect::<Vec<_>>();
                    bump_updated(&mut lines, "2026-09-20").unwrap();
                    serde_json::json!(lines.join("\n"))
                }
                "merge" => {
                    let doc = crate::history_yaml::decode_ordinary_source_value(source.as_bytes())
                        .unwrap()
                        .projected();
                    let reader = crate::ordinary_reader::Reader::new(&doc, None).unwrap();
                    let left = crate::history_yaml::decode_source_value(
                        case["left"].as_str().unwrap().as_bytes(),
                    )
                    .unwrap();
                    let right = crate::history_yaml::decode_source_value(
                        case["right"].as_str().unwrap().as_bytes(),
                    )
                    .unwrap();
                    let mut calls = vec![];
                    let mut admission = |id: &str, old: &V, new: &V, stamp: Option<&str>| {
                        calls.push(serde_json::json!([
                            id,
                            crate::ordinary_reader::json_value(old, 0).unwrap(),
                            crate::ordinary_reader::json_value(new, 0).unwrap(),
                            stamp
                        ]));
                        Ok((
                            case["allow"].as_bool().unwrap(),
                            "oracle admission".to_owned(),
                        ))
                    };
                    let result = Merge {
                        survivor: "p.keep",
                        retired: "p.old",
                        reader: &reader,
                        as_of: Some("2026-09-20"),
                    }
                    .bodies(&left, &right, &mut admission);
                    assert_eq!(serde_json::json!(calls), case["calls"], "{}", case["name"]);
                    if let Some(failure) = case["expected"]["error"].as_str() {
                        assert_eq!(result.unwrap_err().0, failure, "{}", case["name"]);
                    } else {
                        let (body, notes) = result.unwrap();
                        let expected = crate::history_yaml::decode_source_value(
                            case["expected"]["body"].as_str().unwrap().as_bytes(),
                        )
                        .unwrap();
                        assert_eq!(
                            crate::history_emit::encode_source(&body, 100).unwrap(),
                            crate::history_emit::encode_source(&expected, 100).unwrap(),
                            "{}",
                            case["name"]
                        );
                        assert_eq!(
                            serde_json::json!(notes),
                            case["expected"]["notes"],
                            "{}",
                            case["name"]
                        );
                    }
                    continue;
                }
                "brief-file" => {
                    let record = crate::history_yaml::decode_ordinary_source_value(
                        case["record"].as_str().unwrap().as_bytes(),
                    )
                    .unwrap();
                    let doc = record.projected();
                    let reader = crate::ordinary_reader::Reader::new(&doc, None).unwrap();
                    let raw = crate::reasoning_fields::collections(&doc)
                        .unwrap()
                        .into_values()
                        .flatten()
                        .collect();
                    let brief =
                        crate::history_yaml::decode_ordinary_source_value(source.as_bytes())
                            .unwrap()
                            .projected();
                    let got = mentions(
                        "p.old",
                        &[(None, raw)],
                        reader.fields(),
                        Some(&brief),
                        &super::super::ordinary_sameness::Sources {
                            base: Some(&record),
                            hypotheses: BTreeMap::new(),
                        },
                    );
                    assert_eq!(serde_json::json!(got), case["mentions"], "{}", case["name"]);
                    let changed = brief_file(source, "p.keep", "p.old").unwrap();
                    let (changed, _) =
                        rewrite_text(&changed, "p.old", "p.keep", "wrong_if", "seen").unwrap();
                    serde_json::json!(dedupe_text(&changed, "p.keep"))
                }
                _ => panic!("unknown text fixture"),
            };
            assert_eq!(actual, case["expected"], "{}", case["name"]);
        }
    }
}
