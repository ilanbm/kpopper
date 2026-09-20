//! Sorted block YAML compatible with the retained SafeDumper byte contract.
//! Scalar writing is adapted from PyYAML (see third_party/PyYAML-LICENSE).
use crate::{Result, history_yaml as Y, require, value::TypedValue as V};
use crate::{ordinary_source::Source as O, ordinary_value::Scalar as Atom};
use Y::SourceValue as S;

fn linebreak(c: char) -> bool {
    matches!(c, '\n' | '\u{85}' | '\u{2028}' | '\u{2029}')
}
fn whitespace(c: char) -> bool {
    c == '\0' || c == ' ' || c == '\t' || c == '\r' || linebreak(c)
}
struct Analysis {
    plain: bool,
    single: bool,
    multiline: bool,
}
fn analyze(text: &[char]) -> Analysis {
    let mut plain = true;
    let mut single = true;
    let mut multiline = false;
    for (i, &c) in text.iter().enumerate() {
        let before = i == 0 || whitespace(text[i - 1]);
        let after = i + 1 == text.len() || whitespace(text[i + 1]);
        if (i == 0 && ("#,[]{}&*!|>'\"%@`".contains(c) || "?:-".contains(c) && after))
            || (c == ':' && after)
            || (c == '#' && before)
        {
            plain = false;
        }
        if linebreak(c) {
            multiline = true;
            plain = false;
        }
        if (i == 0 || i + 1 == text.len()) && (c == ' ' || linebreak(c)) {
            plain = false;
        }
        if i > 0 && (c == ' ' && linebreak(text[i - 1]) || linebreak(c) && text[i - 1] == ' ') {
            plain = false;
            single = false;
        }
        if !(c == '\n'
            || (' '..='~').contains(&c)
            || c == '\u{85}'
            || ('\u{a0}'..='\u{d7ff}').contains(&c)
            || ('\u{e000}'..='\u{fffd}').contains(&c)
            || ('\u{10000}'..'\u{10ffff}').contains(&c))
            || c == '\u{feff}'
        {
            plain = false;
            single = false;
        }
    }
    if text.starts_with(&['-', '-', '-']) || text.starts_with(&['.', '.', '.']) {
        plain = false;
    }
    Analysis {
        plain,
        single,
        multiline,
    }
}
struct Writer {
    out: String,
    width: usize,
    column: usize,
    whitespace: bool,
    indention: bool,
}
impl Writer {
    fn new(width: usize) -> Self {
        Self {
            out: String::new(),
            width: if width > 4 { width } else { 80 },
            column: 0,
            whitespace: true,
            indention: true,
        }
    }
    fn raw(&mut self, s: &str) {
        self.out.push_str(s);
        self.column += s.chars().count();
    }
    fn chars(&mut self, s: &[char]) {
        for c in s {
            self.out.push(*c);
        }
        self.column += s.len();
    }
    fn line(&mut self, c: char) {
        self.out.push(c);
        self.column = 0;
        self.whitespace = true;
        self.indention = true;
    }
    fn indent(&mut self, n: usize) {
        if !self.indention || self.column > n || self.column == n && !self.whitespace {
            self.line('\n');
        }
        if self.column < n {
            self.whitespace = true;
            self.raw(&" ".repeat(n - self.column));
        }
    }
    fn indicator(&mut self, s: &str, need_space: bool, indention: bool) {
        if !self.whitespace && need_space {
            self.raw(" ");
        }
        self.whitespace = false;
        self.indention &= indention;
        self.raw(s);
    }
    fn quoted_or_plain(&mut self, text: &str, tag: &str, indent: usize, key: bool) {
        let chars: Vec<char> = text.chars().collect();
        let a = analyze(&chars);
        if Y::resolve(text) == tag && a.plain && !(key && (chars.is_empty() || a.multiline)) {
            self.scalar_plain(&chars, indent, !key);
        } else if a.single && !(key && a.multiline) {
            self.scalar_single(&chars, indent, !key);
        } else {
            self.scalar_double(&chars, indent, !key);
        }
    }
    fn scalar_plain(&mut self, t: &[char], indent: usize, split: bool) {
        if t.is_empty() {
            return;
        }
        if !self.whitespace {
            self.raw(" ");
        }
        self.whitespace = false;
        self.indention = false;
        let mut start = 0;
        let mut spaces = false;
        for end in 0..=t.len() {
            let ch = t.get(end).copied();
            if spaces {
                if ch != Some(' ') {
                    if start + 1 == end && self.column > self.width && split {
                        self.indent(indent);
                        self.whitespace = false;
                        self.indention = false;
                    } else {
                        self.chars(&t[start..end]);
                    }
                    start = end;
                }
            } else if ch.is_none() || ch == Some(' ') {
                self.chars(&t[start..end]);
                start = end;
            }
            if let Some(c) = ch {
                spaces = c == ' ';
            }
        }
    }
    fn scalar_single(&mut self, t: &[char], indent: usize, split: bool) {
        self.indicator("'", true, false);
        let (mut start, mut spaces, mut breaks) = (0, false, false);
        for end in 0..=t.len() {
            let ch = t.get(end).copied();
            if spaces {
                if ch != Some(' ') {
                    if start + 1 == end
                        && self.column > self.width
                        && split
                        && start != 0
                        && end != t.len()
                    {
                        self.indent(indent);
                    } else {
                        self.chars(&t[start..end]);
                    }
                    start = end;
                }
            } else if breaks {
                if !ch.is_some_and(linebreak) {
                    if t[start] == '\n' {
                        self.line('\n');
                    }
                    for &c in &t[start..end] {
                        self.line(c);
                    }
                    self.indent(indent);
                    start = end;
                }
            } else if (ch.is_none() || ch.is_some_and(|c| c == ' ' || c == '\'' || linebreak(c)))
                && start < end
            {
                self.chars(&t[start..end]);
                start = end;
            }
            if ch == Some('\'') {
                self.raw("''");
                start = end + 1;
            }
            if let Some(c) = ch {
                spaces = c == ' ';
                breaks = linebreak(c);
            }
        }
        self.indicator("'", false, false);
    }
    fn scalar_double(&mut self, t: &[char], indent: usize, split: bool) {
        self.indicator("\"", true, false);
        let mut start = 0;
        for end in 0..=t.len() {
            let ch = t.get(end).copied();
            if ch.is_none_or(|c| {
                "\"\\\u{85}\u{2028}\u{2029}\u{feff}".contains(c)
                    || !((' '..='~').contains(&c)
                        || ('\u{a0}'..='\u{d7ff}').contains(&c)
                        || ('\u{e000}'..='\u{fffd}').contains(&c))
            }) {
                if start < end {
                    self.chars(&t[start..end]);
                    start = end;
                }
                if let Some(c) = ch {
                    let escape = match c {
                        '\0' => Some('0'),
                        '\u{7}' => Some('a'),
                        '\u{8}' => Some('b'),
                        '\t' => Some('t'),
                        '\n' => Some('n'),
                        '\u{b}' => Some('v'),
                        '\u{c}' => Some('f'),
                        '\r' => Some('r'),
                        '\u{1b}' => Some('e'),
                        '"' => Some('"'),
                        '\\' => Some('\\'),
                        '\u{85}' => Some('N'),
                        '\u{a0}' => Some('_'),
                        '\u{2028}' => Some('L'),
                        '\u{2029}' => Some('P'),
                        _ => None,
                    };
                    self.raw(&if let Some(c) = escape {
                        format!("\\{c}")
                    } else if c <= '\u{ff}' {
                        format!("\\x{:02X}", c as u32)
                    } else if c <= '\u{ffff}' {
                        format!("\\u{:04X}", c as u32)
                    } else {
                        format!("\\U{:08X}", c as u32)
                    });
                    start = end + 1;
                }
            }
            if end > 0
                && end + 1 < t.len()
                && (ch == Some(' ') || start >= end)
                && self.column as isize + end as isize - start as isize > self.width as isize
                && split
            {
                if start < end {
                    self.chars(&t[start..end]);
                    start = end;
                }
                self.raw("\\");
                self.indent(indent);
                self.whitespace = false;
                self.indention = false;
                if t[start] == ' ' {
                    self.raw("\\");
                }
            }
        }
        self.indicator("\"", false, false);
    }
    fn node(&mut self, v: &S, parent: Option<usize>, mapping: bool, key: bool) -> Result<()> {
        match v {
            S::Map(m) if !m.is_empty() => {
                let indent = parent.map_or(0, |i| i + 2);
                for (k, v) in m {
                    self.indent(indent);
                    let chars: Vec<_> = k.chars().collect();
                    // SafeDumper counts the prepared !!str tag even when implicit.
                    let simple =
                        chars.len() + 5 < 128 && !chars.is_empty() && !analyze(&chars).multiline;
                    if !simple {
                        self.indicator("?", true, true);
                    }
                    self.node(&S::Scalar(V::Text(k.clone())), Some(indent), true, simple)?;
                    if !simple {
                        self.indent(indent);
                    }
                    self.indicator(":", !simple, !simple);
                    self.node(v, Some(indent), true, false)?;
                }
            }
            S::List(a) if !a.is_empty() => {
                let indent =
                    parent.map_or(0, |i| if mapping && !self.indention { i } else { i + 2 });
                for v in a {
                    self.indent(indent);
                    self.indicator("-", true, true);
                    self.node(v, Some(indent), false, false)?;
                }
            }
            S::Map(_) => {
                self.indicator("{", true, false);
                self.indicator("}", false, false);
            }
            S::List(_) => {
                self.indicator("[", true, false);
                self.indicator("]", false, false);
            }
            S::Scalar(v) => {
                let (tag, raw) = match v {
                    V::Text(v) => ("str", v.clone()),
                    V::Null => ("null", "null".into()),
                    V::Bool(v) => ("bool", if *v { "true" } else { "false" }.into()),
                    V::Integer(v) => ("int", v.as_str().into()),
                    V::Float(v) => {
                        let mut raw = crate::identity::python_float(v.get());
                        if !raw.contains('.')
                            && let Some(i) = raw.find('e')
                        {
                            raw.insert_str(i, ".0");
                        }
                        ("float", raw)
                    }
                    V::Date(v) => ("timestamp", v.as_str().into()),
                    V::DateTime(v) => ("timestamp", v.as_str().replacen('T', " ", 1)),
                    _ => unreachable!(),
                };
                self.quoted_or_plain(&raw, tag, parent.map_or(2, |i| i + 2), key);
            }
        }
        Ok(())
    }
}
/// Identity from a separately verified source graph, never inferred by equality.
#[derive(Clone, Debug)]
pub(crate) struct OrdinaryIdentity {
    pub id: Option<usize>,
    pub children: Vec<OrdinaryIdentity>,
}
#[derive(Default)]
struct Aliases {
    anchors: std::collections::BTreeMap<usize, String>,
    emitted: std::collections::BTreeSet<usize>,
}
fn identity_plan(value: &O, identity: &OrdinaryIdentity) -> Result<Aliases> {
    fn validate(value: &O, id: &OrdinaryIdentity, depth: usize) -> Result<()> {
        require(depth <= crate::value::MAX_DEPTH + 1, "history_limit")?;
        let children = match value {
            O::Map(m) => m.len() * 2,
            O::List(a) => a.len(),
            _ => 0,
        };
        require(id.children.len() == children, "invalid_source_identity")?;
        if id.id.is_some() {
            require(
                matches!(
                    value,
                    O::Map(_) | O::List(_) | O::Scalar(Atom::Finite(V::Date(_) | V::DateTime(_)))
                ),
                "invalid_source_identity",
            )?;
        }
        match value {
            O::Map(m) => {
                for (i, (key, value)) in m.iter().enumerate() {
                    validate(
                        &O::Scalar(key.scalar().clone()),
                        &id.children[i * 2],
                        depth + 1,
                    )?;
                    validate(value, &id.children[i * 2 + 1], depth + 1)?;
                }
            }
            O::List(a) => {
                for (value, id) in a.iter().zip(&id.children) {
                    validate(value, id, depth + 1)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn plan(
        id: &OrdinaryIdentity,
        seen: &mut std::collections::BTreeSet<usize>,
        anchors: &mut std::collections::BTreeMap<usize, String>,
    ) {
        if let Some(key) = id.id
            && !seen.insert(key)
        {
            if !anchors.contains_key(&key) {
                anchors.insert(key, format!("id{:03}", anchors.len() + 1));
            }
            return;
        }
        for child in &id.children {
            plan(child, seen, anchors);
        }
    }
    validate(value, identity, 0)?;
    let mut aliases = Aliases::default();
    plan(identity, &mut Default::default(), &mut aliases.anchors);
    Ok(aliases)
}
impl Writer {
    fn ordinary_node(
        &mut self,
        value: &O,
        parent: Option<usize>,
        mapping: bool,
        key: bool,
        identity: Option<&OrdinaryIdentity>,
        aliases: &mut Aliases,
    ) -> Result<()> {
        if let Some(id) = identity.and_then(|v| v.id)
            && let Some(anchor) = aliases.anchors.get(&id)
        {
            if !aliases.emitted.insert(id) {
                self.indicator(&format!("*{anchor}"), true, false);
                return Ok(());
            }
            self.indicator(&format!("&{anchor}"), true, false);
        }
        let child = |i| identity.and_then(|v| v.children.get(i));
        match value {
            O::Map(fields) if !fields.is_empty() => {
                let indent = parent.map_or(0, |i| i + 2);
                for (index, (name, value)) in fields.iter().enumerate() {
                    self.indent(indent);
                    let (raw, tag) = match name.scalar() {
                        Atom::NonFinite(v) => (v.yaml().into(), 7),
                        Atom::Finite(value) => match value {
                            V::Text(v) => (v.clone(), 5),
                            V::Bool(v) => (v.to_string(), 6),
                            V::Null => ("null".into(), 6),
                            V::Integer(v) => (v.as_str().into(), 5),
                            V::Float(v) => (crate::identity::python_float(v.get()), 7),
                            V::Date(v) => (v.as_str().into(), 11),
                            V::DateTime(v) => (v.as_str().replacen('T', " ", 1), 11),
                            _ => return Err(crate::Error("invalid_yaml_key".into())),
                        },
                    };
                    let chars = raw.chars().collect::<Vec<_>>();
                    let simple =
                        chars.len() + tag < 128 && !chars.is_empty() && !analyze(&chars).multiline;
                    if !simple {
                        self.indicator("?", true, true);
                    }
                    self.ordinary_node(
                        &O::Scalar(name.scalar().clone()),
                        Some(indent),
                        true,
                        simple,
                        child(index * 2),
                        aliases,
                    )?;
                    if !simple {
                        self.indent(indent);
                    }
                    self.indicator(":", !simple, !simple);
                    self.ordinary_node(
                        value,
                        Some(indent),
                        true,
                        false,
                        child(index * 2 + 1),
                        aliases,
                    )?;
                }
            }
            O::List(items) if !items.is_empty() => {
                let indent =
                    parent.map_or(0, |i| if mapping && !self.indention { i } else { i + 2 });
                for (index, value) in items.iter().enumerate() {
                    self.indent(indent);
                    self.indicator("-", true, true);
                    self.ordinary_node(value, Some(indent), false, false, child(index), aliases)?;
                }
            }
            O::Map(_) => self.node(&S::Map(vec![]), parent, mapping, key)?,
            O::List(_) => self.node(&S::List(vec![]), parent, mapping, key)?,
            O::Scalar(Atom::Finite(v)) => self.node(&S::Scalar(v.clone()), parent, mapping, key)?,
            O::Scalar(Atom::NonFinite(v)) => {
                self.quoted_or_plain(v.yaml(), "float", parent.map_or(2, |i| i + 2), key)
            }
        }
        Ok(())
    }
}

/// Ordinary scalar keys retain their types and source order. Strict history
/// encoders remain restricted to their original finite, text-keyed algebra.
pub(crate) fn encode_ordinary_source(value: &Y::OrdinaryValue, width: usize) -> Result<Vec<u8>> {
    encode_ordinary_source_identity(value, width, None)
}
pub(crate) fn encode_ordinary_source_identity(
    value: &Y::OrdinaryValue,
    width: usize,
    identity: Option<&OrdinaryIdentity>,
) -> Result<Vec<u8>> {
    let projected = value.projected();
    Y::validate_value(&projected, Y::MAX_DOCUMENT_BYTES)?;
    let bytes = encode_ordinary_display(&O::from_finite(value), width, identity)?;
    require(
        Y::decode_ordinary_source_value(&bytes)?.projected() == projected,
        "serialization_changed",
    )?;
    Ok(bytes)
}

/// SafeDumper-compatible display bytes over the complete ordinary source algebra.
/// This is not a mutation encoder: SafeDumper itself does not preserve distinct
/// NaN key constructors when its display is parsed again. Finite mutation callers
/// retain their separate semantic round-trip check above.
pub(crate) fn encode_ordinary_display(
    value: &O,
    width: usize,
    identity: Option<&OrdinaryIdentity>,
) -> Result<Vec<u8>> {
    value.projected().validate()?;
    let mut aliases = identity
        .map(|id| identity_plan(value, id))
        .transpose()?
        .unwrap_or_default();
    let mut writer = Writer::new(width);
    writer.ordinary_node(value, None, false, false, identity, &mut aliases)?;
    writer.indent(0);
    let mut bytes = writer.out.into_bytes();
    if matches!(value, O::Scalar(_))
        && !matches!(bytes.first(), Some(b'\'' | b'"' | b'|' | b'>'))
        && !bytes.ends_with(b"...\n")
    {
        bytes.extend_from_slice(b"...\n");
    }
    require(bytes.len() <= Y::MAX_DOCUMENT_BYTES, "history_limit")?;
    Ok(bytes)
}

pub fn encode_document(value: &V) -> Result<Vec<u8>> {
    encode_source(&S::from_typed(value), 80)
}

/// Preserve source mapping order while retaining the SafeDumper representation.
pub fn encode_source(value: &S, width: usize) -> Result<Vec<u8>> {
    require(matches!(value, S::Map(_)), "invalid_schema")?;
    encode_source_value(value, width)
}

/// The same SafeDumper preimage for scalar/list roots and ordered mapping data.
pub(crate) fn encode_source_value(value: &S, width: usize) -> Result<Vec<u8>> {
    let typed = value.typed();
    Y::validate_value(&typed, Y::MAX_DOCUMENT_BYTES)?;
    let mut w = Writer::new(width);
    w.node(value, None, false, false)?;
    w.indent(0);
    let mut bytes = w.out.into_bytes();
    if matches!(value, S::Scalar(_))
        && !matches!(bytes.first(), Some(b'\'' | b'"' | b'|' | b'>'))
        && !bytes.ends_with(b"...\n")
    {
        bytes.extend_from_slice(b"...\n");
    }
    require(
        Y::decode_ordinary_source_value(&bytes)?
            .strict_typed()?
            .digest()?
            == typed.digest()?,
        "serialization_changed",
    )?;
    Ok(bytes)
}

/// Sorted SafeDumper bytes for watch and retained-source identity. Unlike the
/// record encoder this accepts scalar/list roots and preserves PyYAML endings.
pub fn encode_value(value: &V) -> Result<Vec<u8>> {
    encode_source_value(&S::from_typed(value), 80)
}

#[cfg(test)]
mod watch_preimage_tests {
    #[test]
    fn sorted_arbitrary_root_preimages_match_python() {
        let cases: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/watch-encoding.json")).unwrap();
        for case in cases.as_array().unwrap() {
            let value = crate::history_yaml::decode_ordinary_source_value(
                case["input"].as_str().unwrap().as_bytes(),
            )
            .unwrap()
            .strict_typed()
            .unwrap();
            let raw = super::encode_value(&value).unwrap();
            assert_eq!(
                String::from_utf8(raw.clone()).unwrap(),
                case["output"].as_str().unwrap(),
                "{}",
                case["input"]
            );
            assert_eq!(
                crate::identity::sha256(&raw),
                case["sha256"].as_str().unwrap()
            );
        }
    }
}
