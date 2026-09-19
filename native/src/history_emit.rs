//! Sorted block YAML compatible with the retained SafeDumper byte contract.
//! Scalar writing is adapted from PyYAML (see third_party/PyYAML-LICENSE).
use crate::{Result, history_yaml as Y, require, value::TypedValue as V};

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
    column: usize,
    whitespace: bool,
    indention: bool,
}
impl Writer {
    fn new() -> Self {
        Self {
            out: String::new(),
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
                    if start + 1 == end && self.column > 80 && split {
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
                    if start + 1 == end && self.column > 80 && split && start != 0 && end != t.len()
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
                && self.column as isize + end as isize - start as isize > 80
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
    fn node(&mut self, v: &V, parent: Option<usize>, mapping: bool, key: bool) -> Result<()> {
        match v {
            V::Map(m) if !m.is_empty() => {
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
                    self.node(&V::Text(k.clone()), Some(indent), true, simple)?;
                    if !simple {
                        self.indent(indent);
                    }
                    self.indicator(":", !simple, !simple);
                    self.node(v, Some(indent), true, false)?;
                }
            }
            V::List(a) if !a.is_empty() => {
                let indent =
                    parent.map_or(0, |i| if mapping && !self.indention { i } else { i + 2 });
                for v in a {
                    self.indent(indent);
                    self.indicator("-", true, true);
                    self.node(v, Some(indent), false, false)?;
                }
            }
            V::Map(_) => {
                self.indicator("{", true, false);
                self.indicator("}", false, false);
            }
            V::List(_) => {
                self.indicator("[", true, false);
                self.indicator("]", false, false);
            }
            _ => {
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
pub fn encode_document(value: &V) -> Result<Vec<u8>> {
    Y::validate_value(value, Y::MAX_DOCUMENT_BYTES)?;
    require(matches!(value, V::Map(_)), "invalid_schema")?;
    let mut w = Writer::new();
    w.node(value, None, false, false)?;
    w.indent(0);
    let bytes = w.out.into_bytes();
    require(
        Y::decode_document(&bytes)?.digest()? == value.digest()?,
        "serialization_changed",
    )?;
    Ok(bytes)
}
