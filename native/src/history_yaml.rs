//! Strict history YAML values. Resolution follows the retained PyYAML 1.1 contract.
//! The syntax parser does not choose scalar types or collapse mapping entries.
use crate::{
    Error, Result, require,
    value::{Date, DateTime, FiniteFloat, Integer, MAX_DEPTH, MAX_VALUES, TypedValue},
};
use libyaml_safer::{EventData as Event, Parser, ScalarStyle, Scanner, TokenData};
use num_bigint::BigInt;
use regex::Regex;
use std::{collections::BTreeSet, sync::LazyLock};

pub const MAX_DOCUMENT_BYTES: usize = 16 * 1024 * 1024;
static INT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\A(?:[-+]?0b[0-1_]+|[-+]?0[0-7_]+|[-+]?(?:0|[1-9][0-9_]*)|[-+]?0x[0-9a-fA-F_]+|[-+]?[1-9][0-9_]*(?::[0-5]?[0-9])+)\z").unwrap()
});
static FLOAT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\A(?:[-+]?(?:[0-9][0-9_]*)\.[0-9_]*(?:[eE][-+][0-9]+)?|\.[0-9][0-9_]*(?:[eE][-+][0-9]+)?|[-+]?[0-9][0-9_]*(?::[0-5]?[0-9])+\.[0-9_]*|[-+]?\.(?:inf|Inf|INF)|\.(?:nan|NaN|NAN))\z").unwrap()
});
static STAMP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\A[0-9]{4}-[0-9]{1,2}-[0-9]{1,2}(?:[Tt]|[ \t]+)[0-9]{1,2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]*)?(?:[ \t]*(?:Z|[-+][0-9]{1,2}(?::[0-9]{2})?))?\z").unwrap()
});
static TIMESTAMP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\A(?P<y>[0-9]{4})-(?P<m>[0-9]{1,2})-(?P<d>[0-9]{1,2})(?:(?:[Tt]|[ \t]+)(?P<h>[0-9]{1,2}):(?P<n>[0-9]{2}):(?P<s>[0-9]{2})(?:\.(?P<f>[0-9]*))?(?:[ \t]*(?P<z>Z|(?P<sign>[-+])(?P<zh>[0-9]{1,2})(?::(?P<zm>[0-9]{2}))?))?)?\z").unwrap()
});
fn invalid() -> Error {
    Error("invalid_history_yaml".into())
}
// Merge/value tags are meaningful only as mapping keys, before flattening.
#[derive(Debug)]
enum Node {
    Scalar(TypedValue),
    List(Vec<Node>),
    Map(Vec<(Node, Node)>),
    Merge,
    ValueKey(String),
}
/// Source mapping order is distinct from canonical identity ordering. Some retained
/// readers use the source's printed representation as a clock-group key.
#[derive(Clone, Debug)]
pub enum SourceValue {
    Scalar(TypedValue),
    List(Vec<SourceValue>),
    Map(Vec<(String, SourceValue)>),
}
impl SourceValue {
    pub fn from_typed(value: &TypedValue) -> Self {
        match value {
            TypedValue::List(a) => Self::List(a.iter().map(Self::from_typed).collect()),
            TypedValue::Map(m) => Self::Map(
                m.iter()
                    .map(|(k, v)| (k.clone(), Self::from_typed(v)))
                    .collect(),
            ),
            _ => Self::Scalar(value.clone()),
        }
    }
    pub fn typed(&self) -> TypedValue {
        match self {
            Self::Scalar(v) => v.clone(),
            Self::List(a) => TypedValue::List(a.iter().map(Self::typed).collect()),
            Self::Map(a) => {
                TypedValue::Map(a.iter().map(|(k, v)| (k.clone(), v.typed())).collect())
            }
        }
    }
    pub fn get(&self, key: &str) -> Option<&Self> {
        let Self::Map(a) = self else {
            return None;
        };
        a.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
}
struct Reader<'a> {
    parser: Parser<&'a [u8]>,
    nodes: usize,
    anchors: BTreeSet<String>,
}
impl<'a> Reader<'a> {
    fn next(&mut self) -> Result<Event> {
        let event = self.parser.parse().map_err(|_| invalid())?.data;
        let anchor = match &event {
            Event::Scalar { anchor, .. }
            | Event::SequenceStart { anchor, .. }
            | Event::MappingStart { anchor, .. } => anchor,
            Event::Alias { .. } => return Err(invalid()),
            _ => return Ok(event),
        };
        if let Some(anchor) = anchor {
            require(self.anchors.insert(anchor.clone()), "invalid_history_yaml")?;
        }
        Ok(event)
    }
    fn node(&mut self, event: Event, depth: usize) -> Result<Node> {
        self.nodes += 1;
        // Source keys and merge syntax count here; detached values have their own exact budget.
        require(
            depth <= MAX_DEPTH + 1 && self.nodes <= MAX_VALUES * 2,
            "history_limit",
        )?;
        match event {
            Event::Scalar {
                value, style, tag, ..
            } => scalar(&value, style, tag.as_deref()),
            Event::SequenceStart { tag, .. } => {
                collection_tag(tag.as_deref(), "seq")?;
                let mut values = Vec::new();
                loop {
                    let event = self.next()?;
                    if matches!(event, Event::SequenceEnd) {
                        break;
                    }
                    values.push(self.node(event, depth + 1)?);
                }
                Ok(Node::List(values))
            }
            Event::MappingStart { tag, .. } => {
                collection_tag(tag.as_deref(), "map")?;
                let mut pairs = Vec::new();
                loop {
                    let event = self.next()?;
                    if matches!(event, Event::MappingEnd) {
                        break;
                    }
                    let key = self.node(event, depth + 1)?;
                    let event = self.next()?;
                    pairs.push((key, self.node(event, depth + 1)?));
                }
                Ok(Node::Map(pairs))
            }
            _ => Err(invalid()),
        }
    }
}
fn tag_name(tag: &str) -> Result<String> {
    if tag == "!" {
        return Ok(String::new());
    }
    tag.strip_prefix("tag:yaml.org,2002:")
        .map(str::to_owned)
        .ok_or_else(invalid)
}
fn collection_tag(tag: Option<&str>, expected: &str) -> Result<()> {
    if let Some(tag) = tag {
        let name = tag_name(tag)?;
        require(name.is_empty() || name == expected, "invalid_history_yaml")?;
    }
    Ok(())
}
fn scalar(text: &str, style: ScalarStyle, tag: Option<&str>) -> Result<Node> {
    let explicit = tag.map(tag_name).transpose()?;
    let kind = if let Some(ref name) = explicit {
        if name.is_empty() { resolve(text) } else { name }
    } else if style == ScalarStyle::Plain {
        resolve(text)
    } else {
        "str"
    };
    Ok(Node::Scalar(match kind {
        "str" => TypedValue::Text(text.into()),
        "null" => TypedValue::Null,
        "bool" => TypedValue::Bool(match text.to_ascii_lowercase().as_str() {
            "yes" | "true" | "on" => true,
            "no" | "false" | "off" => false,
            _ => return Err(invalid()),
        }),
        "int" => TypedValue::Integer(integer(text)?),
        "float" => TypedValue::Float(float(text)?),
        "timestamp" => timestamp(text)?,
        "merge" => return Ok(Node::Merge),
        "value" => return Ok(Node::ValueKey(text.into())),
        _ => return Err(invalid()),
    }))
}
pub(crate) fn resolve(text: &str) -> &str {
    match text {
        "" | "~" | "null" | "Null" | "NULL" => "null",
        "yes" | "Yes" | "YES" | "no" | "No" | "NO" | "true" | "True" | "TRUE" | "false"
        | "False" | "FALSE" | "on" | "On" | "ON" | "off" | "Off" | "OFF" => "bool",
        "<<" => "merge",
        "=" => "value",
        _ if INT.is_match(text) => "int",
        _ if FLOAT.is_match(text) => "float",
        _ if (text.len() == 10
            && text.as_bytes()[4] == b'-'
            && text.as_bytes()[7] == b'-'
            && text
                .bytes()
                .enumerate()
                .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit()))
            || STAMP.is_match(text) =>
        {
            "timestamp"
        }
        _ => "str",
    }
}
// Decimal digits accepted by the pinned Python Unicode 16 database. Implicit YAML
// resolution remains ASCII-only; these are used only by numeric constructors.
pub(crate) fn numeric_text(text: &str) -> String {
    const ZEROES: &[u32] = &[
        0x30, 0x660, 0x6f0, 0x7c0, 0x966, 0x9e6, 0xa66, 0xae6, 0xb66, 0xbe6, 0xc66, 0xce6, 0xd66,
        0xde6, 0xe50, 0xed0, 0xf20, 0x1040, 0x1090, 0x17e0, 0x1810, 0x1946, 0x19d0, 0x1a80, 0x1a90,
        0x1b50, 0x1bb0, 0x1c40, 0x1c50, 0xa620, 0xa8d0, 0xa900, 0xa9d0, 0xa9f0, 0xaa50, 0xabf0,
        0xff10, 0x104a0, 0x10d30, 0x10d40, 0x11066, 0x110f0, 0x11136, 0x111d0, 0x112f0, 0x11450,
        0x114d0, 0x11650, 0x116c0, 0x116d0, 0x116da, 0x11730, 0x118e0, 0x11950, 0x11bf0, 0x11c50,
        0x11d50, 0x11da0, 0x11f50, 0x16130, 0x16a60, 0x16ac0, 0x16b50, 0x16d70, 0x1ccf0, 0x1d7ce,
        0x1d7d8, 0x1d7e2, 0x1d7ec, 0x1d7f6, 0x1e140, 0x1e2f0, 0x1e4f0, 0x1e5f1, 0x1e950, 0x1fbf0,
    ];
    text.chars()
        .map(|c| {
            if c.is_ascii() {
                return c;
            }
            ZEROES
                .iter()
                .find_map(|z| {
                    (c as u32)
                        .checked_sub(*z)
                        .filter(|d| *d < 10)
                        .map(|d| char::from(b'0' + d as u8))
                })
                .unwrap_or(c)
        })
        .collect()
}
fn integer(text: &str) -> Result<Integer> {
    let clean = text.replace('_', "");
    let (negative, value) = if let Some(s) = clean.strip_prefix('-') {
        (true, s)
    } else {
        (false, clean.strip_prefix('+').unwrap_or(&clean))
    };
    let parse = |s: &str, base| -> Result<BigInt> {
        let normalized = numeric_text(s);
        let digits = normalized.trim();
        let significant = digits
            .trim_start_matches(['+', '-'])
            .trim_start_matches('0');
        let limit = match base {
            2 => 14285,
            8 => 4762,
            16 => 3572,
            _ => 4300,
        };
        require(significant.len() <= limit, "history_limit")?;
        BigInt::parse_bytes(digits.as_bytes(), base).ok_or_else(invalid)
    };
    let mut number = if let Some(s) = value.strip_prefix("0b") {
        parse(s, 2)?
    } else if let Some(s) = value.strip_prefix("0x") {
        parse(s, 16)?
    } else if value.starts_with('0') {
        parse(value, 8)?
    } else if value.contains(':') {
        let mut n = BigInt::from(0);
        for part in value.split(':') {
            n = n * 60 + parse(part, 10)?;
            require(n.bits() <= 14285, "history_limit")?;
        }
        n
    } else {
        parse(value, 10)?
    };
    if negative {
        number = -number;
    }
    let decimal = number.to_string();
    require(
        decimal.trim_start_matches('-').len() <= 4300,
        "history_limit",
    )?;
    Integer::new(&decimal)
}
fn float(text: &str) -> Result<FiniteFloat> {
    let clean = text.replace('_', "").to_ascii_lowercase();
    let (sign, value) = if let Some(s) = clean.strip_prefix('-') {
        (-1., s)
    } else {
        (1., clean.strip_prefix('+').unwrap_or(&clean))
    };
    let parse = |s: &str| numeric_text(s).trim().parse::<f64>().map_err(|_| invalid());
    let number = if value.contains(':') {
        // Python adds from the least significant sexagesimal component.
        let mut result = 0.;
        let mut base = 1.;
        for part in value.rsplit(':') {
            result += parse(part)? * base;
            base *= 60.;
        }
        result
    } else {
        parse(value)?
    };
    FiniteFloat::new(sign * number)
}
fn timestamp(text: &str) -> Result<TypedValue> {
    let c = TIMESTAMP.captures(text).ok_or_else(invalid)?;
    let num = |name: &str| {
        c.name(name)
            .map_or(Ok(0), |v| v.as_str().parse::<u32>().map_err(|_| invalid()))
    };
    let date = format!("{:04}-{:02}-{:02}", num("y")?, num("m")?, num("d")?);
    if c.name("h").is_none() {
        return Ok(TypedValue::Date(Date::new(&date)?));
    }
    let mut value = format!("{date}T{:02}:{:02}:{:02}", num("h")?, num("n")?, num("s")?);
    let fraction = c.name("f").map_or("", |v| v.as_str());
    let fraction = format!("{:0<6}", &fraction[..fraction.len().min(6)]);
    if fraction != "000000" {
        value.push('.');
        value.push_str(&fraction);
    }
    if c.name("z").is_some() {
        let minutes = num("zh")? * 60 + num("zm")?;
        require(minutes < 24 * 60, "invalid_history_yaml")?;
        let sign = if minutes != 0 && c.name("sign").is_some_and(|s| s.as_str() == "-") {
            '-'
        } else {
            '+'
        };
        value.push_str(&format!("{sign}{:02}:{:02}", minutes / 60, minutes % 60));
    }
    Ok(TypedValue::DateTime(DateTime::new(&value)?))
}
fn flatten(pairs: Vec<(Node, Node)>) -> Result<Vec<(Node, Node)>> {
    let mut merged = Vec::new();
    let mut own = Vec::new();
    for (key, value) in pairs {
        match key {
            Node::Merge => match value {
                Node::Map(pairs) => merged.extend(flatten(pairs)?),
                Node::List(values) => {
                    for value in values.into_iter().rev() {
                        if let Node::Map(pairs) = value {
                            merged.extend(flatten(pairs)?);
                        } else {
                            return Err(invalid());
                        }
                    }
                }
                _ => return Err(invalid()),
            },
            Node::ValueKey(text) => own.push((Node::Scalar(TypedValue::Text(text)), value)),
            _ => own.push((key, value)),
        }
    }
    merged.extend(own);
    Ok(merged)
}
fn construct(node: Node) -> Result<SourceValue> {
    match node {
        Node::Scalar(v) => Ok(SourceValue::Scalar(v)),
        Node::List(values) => Ok(SourceValue::List(
            values.into_iter().map(construct).collect::<Result<_>>()?,
        )),
        Node::Map(pairs) => {
            let mut values = Vec::new();
            let mut keys = BTreeSet::new();
            for (key, value) in flatten(pairs)? {
                let Node::Scalar(TypedValue::Text(key)) = key else {
                    return Err(Error("invalid_yaml_key".into()));
                };
                require(keys.insert(key.clone()), "invalid_yaml_key")?;
                values.push((key, construct(value)?));
            }
            Ok(SourceValue::Map(values))
        }
        _ => Err(invalid()),
    }
}

// Pure PyYAML differs from LibYAML in two lexical policies. Tabs may occur in
// quoted/block content or comments, never token separators or plain scalars.
// Version directives accept any 1.x; they do not change the 1.1 scalar resolver.
fn source_for_parser(raw: &[u8]) -> Result<Vec<u8>> {
    if !raw.contains(&b'\t') && !raw.windows(5).any(|w| w == b"%YAML") {
        return Ok(raw.to_vec());
    }
    fn gap(raw: &[u8]) -> Result<()> {
        let mut comment = false;
        for c in std::str::from_utf8(raw).map_err(|_| invalid())?.chars() {
            match c {
                '#' => comment = true,
                '\n' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}' => comment = false,
                '\t' if !comment => return Err(invalid()),
                _ => {}
            }
        }
        Ok(())
    }
    let mut scanner = Scanner::new();
    scanner.set_input(raw);
    let mut end = 0;
    let mut replacements = Vec::new();
    let mut tokens = 0;
    loop {
        tokens += 1;
        require(tokens <= MAX_VALUES * 8, "history_limit")?;
        let token = Scanner::scan(&mut scanner).map_err(|_| invalid())?;
        let start = token.start_mark.index as usize;
        let stop = token.end_mark.index as usize;
        require(start <= stop && stop <= raw.len(), "invalid_history_yaml")?;
        if start > end {
            gap(&raw[end..start])?;
        }
        let source = &raw[start..stop];
        match token.data {
            TokenData::Scalar {
                style: ScalarStyle::SingleQuoted | ScalarStyle::DoubleQuoted,
                ..
            } => {}
            TokenData::Scalar {
                style: ScalarStyle::Literal | ScalarStyle::Folded,
                ..
            } => {
                gap(source
                    .split(|c| *c == b'\n' || *c == b'\r')
                    .next()
                    .unwrap_or_default())?;
            }
            TokenData::VersionDirective { major: 1, minor } => {
                require(!source.contains(&b'\t'), "invalid_history_yaml")?;
                if minor != 1 && minor != 2 {
                    replacements.push((start, stop));
                }
            }
            TokenData::StreamEnd => break,
            _ => require(!source.contains(&b'\t'), "invalid_history_yaml")?,
        }
        end = end.max(stop);
    }
    let mut result = raw.to_vec();
    for (start, stop) in replacements.into_iter().rev() {
        result.splice(start..stop, b"%YAML 1.1".iter().copied());
    }
    Ok(result)
}

pub fn decode_document(raw: &[u8]) -> Result<TypedValue> {
    Ok(decode_source_document(raw)?.typed())
}

pub fn decode_source_document(raw: &[u8]) -> Result<SourceValue> {
    require(raw.len() <= MAX_DOCUMENT_BYTES, "history_limit")?;
    std::str::from_utf8(raw).map_err(|_| invalid())?;
    let source = source_for_parser(raw)?;
    let mut parser = Parser::new();
    parser.set_input(source.as_slice());
    let mut reader = Reader {
        parser,
        nodes: 0,
        anchors: BTreeSet::new(),
    };
    require(
        matches!(reader.next()?, Event::StreamStart { .. }),
        "invalid_history_yaml",
    )?;
    require(
        matches!(reader.next()?, Event::DocumentStart { .. }),
        "invalid_schema",
    )?;
    let event = reader.next()?;
    let node = reader.node(event, 0)?;
    require(
        matches!(reader.next()?, Event::DocumentEnd { .. })
            && matches!(reader.next()?, Event::StreamEnd),
        "invalid_history_yaml",
    )?;
    let value = construct(node)?;
    require(matches!(value, SourceValue::Map(_)), "invalid_schema")?;
    validate_value(&value.typed(), MAX_DOCUMENT_BYTES)?;
    Ok(value)
}

/// Match history's compact ASCII JSON accounting, including escaped keys and dates.
pub(crate) fn validate_value(value: &TypedValue, maximum: usize) -> Result<()> {
    value.validate()?;
    let mut pending = vec![value];
    while let Some(item) = pending.pop() {
        match item {
            TypedValue::Integer(v) => require(
                v.as_str().trim_start_matches('-').len() <= 4300,
                "history_limit",
            )?,
            TypedValue::Map(v) => pending.extend(v.values()),
            TypedValue::List(v) => pending.extend(v.iter()),
            _ => {}
        }
    }
    require(compact_json_size(value) <= maximum, "history_limit")
}
pub(crate) fn compact_json_size(v: &TypedValue) -> usize {
    fn string(s: &str) -> usize {
        2 + s
            .chars()
            .map(|c| match c {
                '"' | '\\' | '\u{8}' | '\u{c}' | '\n' | '\r' | '\t' => 2,
                c if !('\u{20}'..'\u{7f}').contains(&c) => {
                    if (c as u32) > 0xffff {
                        12
                    } else {
                        6
                    }
                }
                _ => 1,
            })
            .sum::<usize>()
    }
    match v {
        TypedValue::Null => 4,
        TypedValue::Bool(true) => 4,
        TypedValue::Bool(false) => 5,
        TypedValue::Integer(v) => v.as_str().len(),
        TypedValue::Float(v) => crate::identity::python_float(v.get()).len(),
        TypedValue::Text(v) => string(v),
        TypedValue::Date(v) => string(v.as_str()),
        TypedValue::DateTime(v) => string(v.as_str()),
        TypedValue::List(v) => {
            2 + v.len().saturating_sub(1) + v.iter().map(compact_json_size).sum::<usize>()
        }
        TypedValue::Map(v) => {
            2 + v.len().saturating_sub(1)
                + v.iter()
                    .map(|(k, v)| string(k) + 1 + compact_json_size(v))
                    .sum::<usize>()
        }
    }
}

fn quote(text: &str) -> String {
    // JSON escapes are valid in YAML; YAML line separators must additionally be escaped.
    let json = serde_json::to_string(text).unwrap();
    let mut out = String::new();
    for c in json.chars() {
        if ('\u{7f}'..='\u{9f}').contains(&c)
            || matches!(
                c,
                '\u{2028}' | '\u{2029}' | '\u{feff}' | '\u{fffe}' | '\u{ffff}'
            )
        {
            out.push_str(&format!("\\u{:04x}", c as u32));
        } else {
            out.push(c);
        }
    }
    out
}
pub fn encode_document(value: &TypedValue) -> Result<Vec<u8>> {
    validate_value(value, MAX_DOCUMENT_BYTES)?;
    require(matches!(value, TypedValue::Map(_)), "invalid_schema")?;
    fn write(v: &TypedValue, out: &mut String) {
        match v {
            TypedValue::Null => out.push_str("null"),
            TypedValue::Bool(v) => out.push_str(if *v { "true" } else { "false" }),
            TypedValue::Integer(v) => out.push_str(&format!("!!int {}", quote(v.as_str()))),
            TypedValue::Float(v) => out.push_str(&format!(
                "!!float {}",
                quote(&crate::identity::python_float(v.get()))
            )),
            TypedValue::Text(v) => out.push_str(&quote(v)),
            TypedValue::Date(v) => out.push_str(&format!("!!timestamp {}", quote(v.as_str()))),
            TypedValue::DateTime(v) => out.push_str(&format!("!!timestamp {}", quote(v.as_str()))),
            TypedValue::List(v) => {
                out.push('[');
                for (i, v) in v.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    write(v, out);
                }
                out.push(']');
            }
            TypedValue::Map(v) => {
                out.push('{');
                for (i, (k, v)) in v.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str("? ");
                    out.push_str(&quote(k));
                    out.push_str(" : ");
                    write(v, out);
                }
                out.push('}');
            }
        }
    }
    let mut out = String::new();
    write(value, &mut out);
    out.push('\n');
    let raw = out.into_bytes();
    require(decode_document(&raw)? == *value, "serialization_changed")?;
    Ok(raw)
}
