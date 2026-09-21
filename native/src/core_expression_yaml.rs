//! Marked YAML edits used by explicit expression migration. All offsets come from
//! the parser's Unicode marks; aliases share the original node and source span.
use crate::{
    Error, Result, history_emit,
    history_yaml::{self as Y, SourceValue as S},
    require,
    value::TypedValue as V,
};
use libyaml_safer::{Event, EventData as E, MappingStyle, Mark, Parser};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
#[derive(Clone)]
struct Node {
    start: Mark,
    end: Mark,
    kind: Kind,
}
#[derive(Clone)]
enum Kind {
    Scalar(String),
    Sequence,
    Mapping(Vec<(Link, Link)>, bool),
}
type Link = Rc<RefCell<Node>>;
struct Composer<'a> {
    parser: Parser<&'a [u8]>,
    anchors: BTreeMap<String, Link>,
    count: usize,
    // Only non-ASCII boundaries need an offset correction.
    indices: Vec<(u64, u64)>,
    source: &'a str,
}
impl Composer<'_> {
    fn next(&mut self) -> Result<Event> {
        let mut event = self
            .parser
            .parse()
            .map_err(|e| Error(format!("invalid migration record: {e}")))?;
        for mark in [&mut event.start_mark, &mut event.end_mark] {
            require(
                self.source.is_char_boundary(mark.index as usize),
                "invalid migration Unicode span",
            )?;
            let count = self.indices.partition_point(|(end, _)| *end <= mark.index);
            if count > 0 {
                mark.index -= self.indices[count - 1].1;
            }
        }
        Ok(event)
    }
    fn node(&mut self, event: Event, depth: usize) -> Result<Link> {
        require(depth < 128 && self.count < 200_000, "migration YAML limit")?;
        self.count += 1;
        if let E::Alias { anchor } = &event.data {
            return self.anchors.get(anchor).cloned().ok_or_else(|| {
                let line=self.source.lines().nth(event.start_mark.line as usize).unwrap_or("");
                let column=event.start_mark.column as usize;
                Error(format!("invalid migration record: found undefined alias '{anchor}'\n  in \"<unicode string>\", line {}, column {}:\n    {line}\n    {}^",event.start_mark.line+1,column+1," ".repeat(column)))
            });
        }
        let anchor = match &event.data {
            E::Scalar { anchor, .. }
            | E::SequenceStart { anchor, .. }
            | E::MappingStart { anchor, .. } => anchor.clone(),
            _ => None,
        };
        let node = Rc::new(RefCell::new(Node {
            start: event.start_mark,
            end: event.end_mark,
            kind: Kind::Sequence,
        }));
        if let Some(anchor) = anchor {
            require(
                self.anchors.insert(anchor, node.clone()).is_none(),
                "duplicate migration alias",
            )?;
        }
        let kind = match event.data {
            E::Scalar { value, .. } => Kind::Scalar(value),
            E::SequenceStart { .. } => {
                loop {
                    let next = self.next()?;
                    if matches!(next.data, E::SequenceEnd) {
                        node.borrow_mut().end = next.end_mark;
                        break;
                    }
                    self.node(next, depth + 1)?;
                }
                Kind::Sequence
            }
            E::MappingStart { style, .. } => {
                let mut pairs = Vec::new();
                loop {
                    let next = self.next()?;
                    if matches!(next.data, E::MappingEnd) {
                        node.borrow_mut().end = next.end_mark;
                        break;
                    }
                    let key = self.node(next, depth + 1)?;
                    let next = self.next()?;
                    let value = self.node(next, depth + 1)?;
                    pairs.push((key, value));
                }
                Kind::Mapping(pairs, style == MappingStyle::Flow)
            }
            _ => return Err(Error("invalid migration record".into())),
        };
        node.borrow_mut().kind = kind;
        Ok(node)
    }
}
fn compose(raw: &[u8]) -> Result<Option<Link>> {
    let mut parser = Parser::new();
    parser.set_input(raw);
    let text = std::str::from_utf8(raw).map_err(|_| Error("invalid migration UTF-8".into()))?;
    let mut excess = 0u64;
    let indices = text
        .char_indices()
        .filter_map(|(byte, c)| {
            if c.len_utf8() == 1 {
                return None;
            }
            excess += c.len_utf8() as u64 - 1;
            Some(((byte + c.len_utf8()) as u64, excess))
        })
        .collect();
    let mut c = Composer {
        parser,
        anchors: BTreeMap::new(),
        count: 0,
        indices,
        source: text,
    };
    require(
        matches!(c.next()?.data, E::StreamStart { .. }),
        "invalid migration record",
    )?;
    let event = c.next()?;
    if matches!(event.data, E::StreamEnd) {
        return Ok(None);
    }
    require(
        matches!(event.data, E::DocumentStart { .. }),
        "invalid migration record",
    )?;
    let e = c.next()?;
    let node = c.node(e, 0)?;
    require(
        matches!(c.next()?.data, E::DocumentEnd { .. }) && matches!(c.next()?.data, E::StreamEnd),
        "invalid migration record",
    )?;
    Ok(Some(node))
}
/// SafeDumper's flow representation, including sorted mapping keys and Unicode.
pub(super) fn flow(value: &V) -> Result<String> {
    use libyaml_safer::{Emitter, Encoding, ScalarStyle, SequenceStyle};
    fn emit(v: &V, e: &mut Emitter<'_>) -> Result<()> {
        let send = |e: &mut Emitter<'_>, event| {
            e.emit(event).map_err(|x| Error(format!("yaml_emit: {x}")))
        };
        match v {
            V::Map(m) => {
                send(
                    e,
                    Event::mapping_start(
                        None,
                        Some("tag:yaml.org,2002:map"),
                        true,
                        MappingStyle::Flow,
                    ),
                )?;
                for (k, v) in m {
                    emit(&V::Text(k.clone()), e)?;
                    emit(v, e)?;
                }
                send(e, Event::mapping_end())?;
            }
            V::List(a) => {
                send(
                    e,
                    Event::sequence_start(
                        None,
                        Some("tag:yaml.org,2002:seq"),
                        true,
                        SequenceStyle::Flow,
                    ),
                )?;
                for v in a {
                    emit(v, e)?;
                }
                send(e, Event::sequence_end())?;
            }
            _ => {
                let (tag, raw) = match v {
                    V::Text(s) => ("str", s.clone()),
                    V::Null => ("null", "null".into()),
                    V::Bool(b) => ("bool", b.to_string()),
                    V::Integer(i) => ("int", i.as_str().into()),
                    V::Float(f) => {
                        let mut s = crate::identity::python_float(f.get());
                        if !s.contains('.')
                            && let Some(at) = s.find('e')
                        {
                            s.insert_str(at, ".0");
                        }
                        ("float", s)
                    }
                    V::Date(d) => ("timestamp", d.as_str().into()),
                    V::DateTime(d) => ("timestamp", d.as_str().replacen('T', " ", 1)),
                    _ => return Err(Error("invalid migration scalar".into())),
                };
                let style = if tag == "str" && Y::resolve(&raw) != "str" {
                    ScalarStyle::SingleQuoted
                } else {
                    ScalarStyle::Any
                };
                send(
                    e,
                    Event::scalar(
                        None,
                        Some(&format!("tag:yaml.org,2002:{tag}")),
                        &raw,
                        true,
                        true,
                        style,
                    ),
                )?;
            }
        }
        Ok(())
    }
    let mut bytes = vec![];
    {
        let mut e = Emitter::new();
        e.set_output_string(&mut bytes);
        e.set_unicode(true);
        e.set_width(1_000_000);
        for event in [
            Event::stream_start(Encoding::Utf8),
            Event::document_start(None, &[], true),
        ] {
            e.emit(event).map_err(|x| Error(x.to_string()))?;
        }
        emit(value, &mut e)?;
        e.emit(Event::document_end(true))
            .map_err(|x| Error(x.to_string()))?;
        e.emit(Event::stream_end())
            .map_err(|x| Error(x.to_string()))?;
    }
    let text = String::from_utf8(bytes).map_err(|_| Error("invalid migration text".into()))?;
    Ok(text.trim().trim_end_matches("\n...").to_owned())
}
pub(super) fn parse(raw: &[u8]) -> Result<V> {
    let value = Y::decode_ordinary_source_value(raw)?.strict_typed()?;
    let value = if crate::history_view::truth(&value) {
        value
    } else {
        V::Map(BTreeMap::new())
    };
    require(
        matches!(value, V::Map(_)),
        "migration requires a mapping of collections",
    )?;
    Ok(value)
}
fn metadata_order(v: &V) -> S {
    match v {
        V::Map(m) => {
            let mut keys = m.keys().collect::<Vec<_>>();
            if m.get("profile") == Some(&V::Text("core/v1".into()))
                && m.contains_key("version")
                && m.contains_key("requires")
            {
                keys.sort_by_key(|k| match k.as_str() {
                    "version" => 0,
                    "profile" => 1,
                    "requires" => 2,
                    _ => 3,
                });
            }
            S::Map(
                keys.into_iter()
                    .map(|k| (k.clone(), metadata_order(&m[k])))
                    .collect(),
            )
        }
        V::List(a) => S::List(a.iter().map(metadata_order).collect()),
        _ => S::Scalar(v.clone()),
    }
}
pub(super) fn patch(raw: &[u8], before: &V, after: &V) -> Result<Vec<u8>> {
    let text = std::str::from_utf8(raw).map_err(|_| Error("invalid migration UTF-8".into()))?;
    let Some(root) = compose(raw)? else {
        return history_emit::encode_document(after);
    };
    let chars: Vec<char> = text.chars().collect();
    let mut edits: Vec<(usize, usize, String)> = vec![];
    fn visit(
        node: &Link,
        old: &V,
        new: &V,
        chars: &[char],
        edits: &mut Vec<(usize, usize, String)>,
        depth: usize,
    ) -> Result<()> {
        if old == new {
            return Ok(());
        }
        require(depth < 128, "migration YAML limit")?;
        let node = node.borrow();
        if let (V::Map(old), V::Map(new), Kind::Mapping(pairs, flow_style)) = (old, new, &node.kind)
        {
            let mut lookup = BTreeMap::new();
            for (k, v) in pairs {
                if let Kind::Scalar(key) = &k.borrow().kind {
                    lookup.insert(key.clone(), (k.clone(), v.clone()));
                }
            }
            let mut removed: Vec<_> = old.keys().filter(|k| !new.contains_key(*k)).collect();
            let mut added: Vec<_> = new.keys().filter(|k| !old.contains_key(*k)).collect();
            if removed == ["v"] && added == ["rule"] {
                let (k, v) = lookup
                    .get("v")
                    .ok_or_else(|| Error("missing migration field".into()))?;
                edits.push((
                    k.borrow().start.index as usize,
                    k.borrow().end.index as usize,
                    "rule".into(),
                ));
                edits.push((
                    v.borrow().start.index as usize,
                    v.borrow().end.index as usize,
                    flow(&new["rule"])?,
                ));
                removed.clear();
                added.clear();
            }
            if removed.is_empty() {
                if !added.is_empty() {
                    let extra = V::Map(
                        added
                            .into_iter()
                            .map(|k| (k.clone(), new[k].clone()))
                            .collect(),
                    );
                    if *flow_style {
                        let at = node.start.index as usize;
                        require(
                            chars.get(at) == Some(&'{'),
                            "migration cannot add metadata to an aliased mapping",
                        )?;
                        let extra = flow(&extra)?;
                        edits.push((
                            at + 1,
                            at + 1,
                            extra[1..extra.len() - 1].to_owned()
                                + if old.is_empty() { "" } else { ", " },
                        ));
                    } else {
                        let first = pairs
                            .first()
                            .ok_or_else(|| Error("empty block migration mapping".into()))?
                            .0
                            .borrow();
                        let indent = first.start.column as usize;
                        let at = first.start.index as usize - indent;
                        let dumped = String::from_utf8(history_emit::encode_source(
                            &metadata_order(&extra),
                            80,
                        )?)
                        .map_err(|_| Error("invalid migration text".into()))?;
                        let insert = dumped
                            .lines()
                            .map(|line| " ".repeat(indent) + line + "\n")
                            .collect::<String>();
                        edits.push((at, at, insert));
                    }
                }
                for (k, old) in old {
                    if let Some(new) = new.get(k) {
                        let value = &lookup
                            .get(k)
                            .ok_or_else(|| Error("missing migration field".into()))?
                            .1;
                        visit(value, old, new, chars, edits, depth + 1)?;
                    }
                }
                return Ok(());
            }
        }
        let start = node.start.index as usize;
        let mut end = node.end.index as usize;
        while end > start && matches!(chars[end - 1], '\r' | '\n') {
            end -= 1;
        }
        edits.push((start, end, flow(new)?));
        Ok(())
    }
    visit(&root, before, after, &chars, &mut edits, 0)?;
    edits.sort();
    let mut output = chars;
    for (start, end, value) in edits.into_iter().rev() {
        require(
            start <= end && end <= output.len(),
            "invalid migration span",
        )?;
        output.splice(start..end, value.chars());
    }
    let result = output.into_iter().collect::<String>().into_bytes();
    let parsed = match parse(&result) {
        Ok(value) => value,
        Err(error) => {
            compose(&result)?;
            return Err(error);
        }
    };
    require(
        parsed == *after,
        "migration byte patch did not preserve the document",
    )?;
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_comments_and_history_are_preserved() {
        let raw="# עברית\nknown:\n  p.a: {v: 2}\n  p.b:\n    v: p.a + 1 # keep\n    seen: {p.a: {rule: old text}}\n".as_bytes();
        let before = parse(raw).unwrap();
        let mut after = before.clone();
        let V::Map(doc) = &mut after else { panic!() };
        let V::Map(known) = doc.get_mut("known").unwrap() else {
            panic!()
        };
        let V::Map(body) = known.get_mut("p.b").unwrap() else {
            panic!()
        };
        body.remove("v");
        body.insert(
            "rule".into(),
            V::Map(BTreeMap::from([("expr".into(), V::Text("p.a + 1".into()))])),
        );
        assert_eq!(
            String::from_utf8(patch(raw, &before, &after).unwrap()).unwrap(),
            "# עברית\nknown:\n  p.a: {v: 2}\n  p.b:\n    rule: {expr: p.a + 1} # keep\n    seen: {p.a: {rule: old text}}\n"
        );
    }
}
