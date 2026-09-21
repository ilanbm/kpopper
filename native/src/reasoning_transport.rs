//! Strict KP2/KP3 framing. This module never evaluates an expression or falls back.
use crate::{Error, Result, require};
use num_bigint::BigInt;
use serde_json::{Map, Value as J, json};
use std::{collections::BTreeSet, sync::LazyLock};
const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
const UNAVAILABLE: &[&str] = &[
    "missing_reference",
    "missing_input",
    "contested",
    "unavailable_input",
];
const DIAGNOSTICS: &[&str] = &[
    "missing_reference",
    "missing_input",
    "contested",
    "unavailable_input",
    "invalid_expression",
    "division_by_zero",
    "cyclic_reference",
    "type_error",
    "undeclared_dependency",
    "invalid_transport",
    "invalid_limits",
    "unsupported_capability",
    "step_limit",
    "depth_limit",
    "parser_depth_limit",
    "number_limit",
    "edge_limit",
    "expression_limit",
    "transport_limit",
    "node_limit",
];
static NUMBER: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\A-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?\z").unwrap()
});
static OP: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\A[a-z][a-z0-9_./-]*\z").unwrap());
fn err(s: &str) -> Error {
    Error(s.into())
}
fn map<'a>(j: &'a J, message: &str) -> Result<&'a Map<String, J>> {
    j.as_object().ok_or_else(|| err(message))
}
fn text<'a>(j: &'a J, message: &str) -> Result<&'a str> {
    j.as_str().ok_or_else(|| err(message))
}
fn hex(s: &str) -> String {
    s.as_bytes().iter().map(|b| format!("{b:02x}")).collect()
}
fn identifier(s: &str) -> bool {
    (1..=500).contains(&s.chars().count())
}
struct Tokens {
    items: Vec<String>,
    used: usize,
    composed: bool,
}
impl Tokens {
    fn put(&mut self, s: impl Into<String>) -> Result<()> {
        let s = s.into();
        self.used += s.len() + usize::from(!self.items.is_empty());
        if self.composed {
            require(self.used <= MAX_BYTES, "transport limit")?;
        }
        self.items.push(s);
        Ok(())
    }
    fn text(&mut self, j: &J) -> Result<()> {
        let s = text(
            j,
            if self.composed {
                "invalid or oversized text"
            } else {
                "transport text must be a string"
            },
        )?;
        if self.composed {
            require(
                s.chars().count() <= MAX_BYTES / 2,
                "invalid or oversized text",
            )?;
            require(
                self.used + 2 * s.len() + usize::from(!self.items.is_empty()) <= MAX_BYTES,
                "transport limit",
            )?;
        }
        self.put(hex(s))
    }
    fn expression(&mut self, expr: &J, count: &mut usize) -> Result<()> {
        enum Item<'a> {
            Expr(&'a J, usize),
            Text(&'a str),
        }
        let mut pending = vec![Item::Expr(expr, 0)];
        while let Some(item) = pending.pop() {
            let (current, depth) = match item {
                Item::Text(s) => {
                    self.text(&J::String(s.into()))?;
                    continue;
                }
                Item::Expr(v, d) => (v, d),
            };
            *count += 1;
            require(*count <= 1_000_000 && depth <= 128, "expression limit")?;
            let m = map(current, "invalid or cyclic expression")?;
            if m.len() == 1 {
                if let Some(v) = m.get("num") {
                    let s = text(v, "invalid number lexeme")?;
                    require(
                        s.len() <= 8200 && NUMBER.is_match(s),
                        "invalid number lexeme",
                    )?;
                    self.put("n")?;
                    self.put(s)?;
                    continue;
                }
                if let Some(v) = m.get("bool").and_then(J::as_bool) {
                    self.put("b")?;
                    self.put(if v { "1" } else { "0" })?;
                    continue;
                }
                if let Some(v) = m.get("text") {
                    self.put("s")?;
                    self.text(v)?;
                    continue;
                }
                if m.get("null") == Some(&J::Bool(true)) {
                    self.put("z")?;
                    continue;
                }
                if let Some(v) = m.get("ref")
                    && (!self.composed || v.as_str().is_some_and(identifier))
                {
                    self.put("r")?;
                    self.text(v)?;
                    continue;
                }
                if let Some(v) = m.get("unavailable").and_then(J::as_str)
                    && UNAVAILABLE.contains(&v)
                {
                    self.put("u")?;
                    self.put(v)?;
                    continue;
                }
                if self.composed {
                    if let Some(a) = m
                        .get("list")
                        .and_then(J::as_array)
                        .filter(|a| a.len() <= 10000)
                    {
                        self.put("l")?;
                        self.put(a.len().to_string())?;
                        for c in a.iter().rev() {
                            pending.push(Item::Expr(c, depth + 1));
                        }
                        continue;
                    }
                    if let Some(fields) = m
                        .get("record")
                        .and_then(J::as_object)
                        .filter(|m| m.len() <= 10000)
                    {
                        require(
                            fields.keys().all(|s| s.chars().count() <= 500),
                            "invalid record key",
                        )?;
                        self.put("m")?;
                        self.put(fields.len().to_string())?;
                        for (key, v) in fields.iter().rev() {
                            pending.push(Item::Expr(v, depth + 1));
                            pending.push(Item::Text(key));
                        }
                        continue;
                    }
                }
            }
            if m.len() == 2 && m.contains_key("op") && m.contains_key("args") {
                let name = text(&m["op"], "invalid operation")?;
                let args = m["args"]
                    .as_array()
                    .ok_or_else(|| err("invalid operation"))?;
                require(
                    OP.is_match(name)
                        && args.len() == if self.composed && name == "not" { 1 } else { 2 },
                    "invalid operation",
                )?;
                self.put("o")?;
                self.put(name)?;
                self.put(args.len().to_string())?;
                for child in args.iter().rev() {
                    pending.push(Item::Expr(child, depth + 1));
                }
                continue;
            }
            if self.composed
                && m.len() == 3
                && ["if", "then", "else"].iter().all(|k| m.contains_key(*k))
            {
                self.put("i")?;
                for key in ["else", "then", "if"] {
                    pending.push(Item::Expr(&m[key], depth + 1));
                }
                continue;
            }
            if self.composed
                && m.len() == 2
                && m.contains_key("field")
                && m.get("key")
                    .and_then(J::as_str)
                    .is_some_and(|s| s.chars().count() <= 500)
            {
                self.put("f")?;
                self.text(&m["key"])?;
                pending.push(Item::Expr(&m["field"], depth + 1));
                continue;
            }
            return Err(err("invalid expression"));
        }
        Ok(())
    }
}
pub fn encode_request(request: &J) -> Result<String> {
    let m = map(request, "invalid request fields")?;
    if m.get("version").and_then(J::as_f64) == Some(4.0) {
        return crate::reasoning_query::encode_request(request)
            .map(|b| String::from_utf8(b).expect("canonical UTF-8"));
    }
    let composed = m.get("protocol").and_then(J::as_str) == Some("KP3");
    require(
        ["nodes", "expression", "declared"]
            .iter()
            .all(|k| m.contains_key(*k))
            && m.keys().all(|k| {
                ["nodes", "expression", "declared", "limits"].contains(&k.as_str())
                    || composed && k == "protocol"
            }),
        "invalid request fields",
    )?;
    let empty = Map::new();
    let limits = match m.get("limits") {
        None | Some(J::Null) => &empty,
        Some(v) => map(v, "invalid limits")?,
    };
    let mut bounds = vec![
        ("steps", 1_000_000, 10_000_000),
        ("depth", 128, 4096),
        ("digits", 256, 4096),
    ];
    if composed {
        bounds.extend([
            ("value_nodes", 10000, 10000),
            ("value_depth", 128, 128),
            ("value_bytes", 16777216, 16777216),
        ]);
    }
    require(
        limits.keys().all(|k| bounds.iter().any(|(n, _, _)| n == k)),
        "invalid limits",
    )?;
    let bounds = bounds
        .iter()
        .map(|(key, default, hard)| {
            let v = limits
                .get(*key)
                .map_or(Some(*default), J::as_u64)
                .ok_or_else(|| err("invalid limits"))?;
            require(v > 0 && v <= *hard, "invalid limits")?;
            Ok(v)
        })
        .collect::<Result<Vec<_>>>()?;
    let declared = m["declared"]
        .as_array()
        .ok_or_else(|| err("invalid declared IDs"))?;
    require(declared.len() <= 100000, "invalid declared IDs")?;
    let declared = declared
        .iter()
        .map(|v| {
            let s = text(v, "invalid declared IDs")?;
            require(!composed || identifier(s), "invalid declared IDs")?;
            Ok(s)
        })
        .collect::<Result<Vec<_>>>()?;
    let sorted = declared.iter().copied().collect::<BTreeSet<_>>();
    require(sorted.len() == declared.len(), "invalid declared IDs")?;
    let nodes = map(&m["nodes"], "invalid nodes")?;
    require(
        nodes.len() <= 20000 && (!composed || nodes.keys().all(|s| identifier(s))),
        "invalid nodes",
    )?;
    let mut tokens = Tokens {
        items: Vec::new(),
        used: 0,
        composed,
    };
    tokens.put(if composed { "KP3" } else { "KP2" })?;
    for bound in bounds {
        tokens.put(bound.to_string())?;
    }
    tokens.put(sorted.len().to_string())?;
    for id in sorted {
        tokens.text(&J::String(id.into()))?;
    }
    tokens.put(nodes.len().to_string())?;
    let mut count = 0;
    for (id, node) in nodes {
        tokens.text(&J::String(id.clone()))?;
        tokens.expression(node, &mut count)?;
    }
    tokens.expression(&m["expression"], &mut count)?;
    require(tokens.used <= MAX_BYTES, "transport limit")?;
    Ok(tokens.items.join("\t"))
}
struct Reader<'a> {
    line: &'a str,
    position: usize,
    value_start: Option<usize>,
    value_nodes: usize,
    composed: bool,
}
fn natural_lexeme(s: &str) -> bool {
    !s.is_empty() && (s == "0" || !s.starts_with('0')) && s.bytes().all(|b| b.is_ascii_digit())
}
fn integer_lexeme(s: &str) -> bool {
    if let Some(s) = s.strip_prefix('-') {
        s != "0" && natural_lexeme(s)
    } else {
        natural_lexeme(s)
    }
}
impl<'a> Reader<'a> {
    fn take(&mut self) -> Result<&'a str> {
        require(self.position <= self.line.len(), "truncated response")?;
        let end = self.line[self.position..]
            .find('\t')
            .map_or(self.line.len(), |n| n + self.position);
        if let Some(start) = self.value_start {
            require(end - start <= MAX_BYTES, "value byte limit")?;
        }
        let s = &self.line[self.position..end];
        self.position = end + 1;
        Ok(s)
    }
    fn natural(&mut self, bound: usize) -> Result<usize> {
        let s = self.take()?;
        require(s.len() <= 8 && natural_lexeme(s), "invalid count")?;
        let n = s.parse::<usize>().map_err(|_| err("invalid count"))?;
        require(
            n <= bound,
            if self.composed {
                "invalid count"
            } else {
                "count limit"
            },
        )?;
        Ok(n)
    }
    fn text(&mut self) -> Result<String> {
        let s = self.take()?;
        require(
            s.len() % 2 == 0
                && s.bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
            "invalid hex",
        )?;
        let raw = s
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
            .collect::<Vec<_>>();
        String::from_utf8(raw).map_err(|_| err("invalid UTF-8"))
    }
    fn value(&mut self, depth: usize) -> Result<J> {
        self.value_nodes += 1;
        if self.composed {
            require(
                self.value_nodes <= 10000 && depth <= 128,
                "value collection limit",
            )?;
        }
        match self.take()? {
            "u" if depth == 0 => Ok(J::Null),
            "n" => {
                let num = self.take()?;
                let den = self.take()?;
                require(
                    num.trim_start_matches('-').len() <= 4096
                        && den.len() <= 4096
                        && integer_lexeme(num)
                        && natural_lexeme(den)
                        && den != "0",
                    "invalid rational value",
                )?;
                let mut a = num
                    .parse::<BigInt>()
                    .map_err(|_| err("invalid rational value"))?;
                let mut b = den
                    .parse::<BigInt>()
                    .map_err(|_| err("invalid rational value"))?;
                while b != BigInt::from(0) {
                    let r = &a % &b;
                    a = b;
                    b = r;
                }
                require(
                    a == BigInt::from(1) || a == BigInt::from(-1),
                    if self.composed {
                        "invalid rational value"
                    } else {
                        "noncanonical rational value"
                    },
                )?;
                Ok(json!({"type":"number","numerator":num,"denominator":den}))
            }
            "b" => {
                let b = self.take()?;
                require(b == "0" || b == "1", "invalid boolean")?;
                Ok(json!({"type":"boolean","value":b=="1"}))
            }
            "s" => Ok(json!({"type":"text","value":self.text()?})),
            "z" => Ok(json!({"type":"null"})),
            "l" if self.composed => {
                let n = self.natural(10000)?;
                let mut a = Vec::new();
                for _ in 0..n {
                    a.push(self.value(depth + 1)?);
                }
                Ok(json!({"type":"list","items":a}))
            }
            "m" if self.composed => {
                let n = self.natural(10000)?;
                let mut fields = Map::new();
                let mut previous: Option<String> = None;
                for _ in 0..n {
                    let key = self.text()?;
                    require(
                        key.chars().count() <= 500 && previous.as_ref().is_none_or(|p| p < &key),
                        "noncanonical or duplicate record key",
                    )?;
                    previous = Some(key.clone());
                    fields.insert(key, self.value(depth + 1)?);
                }
                Ok(json!({"type":"record","fields":fields}))
            }
            _ => Err(err("invalid value tag")),
        }
    }
    fn ordered_texts(&mut self) -> Result<Vec<String>> {
        let n = self.natural(100000)?;
        let mut result = Vec::new();
        for _ in 0..n {
            result.push(self.text()?);
        }
        ordered(&result)?;
        Ok(result)
    }
}
fn ordered(values: &[String]) -> Result<()> {
    require(
        values.windows(2).all(|w| w[0] < w[1]),
        "noncanonical or duplicate response fields",
    )
}
pub fn decode_response(raw: &[u8]) -> Result<J> {
    if raw.starts_with(b"KR4 ") {
        return crate::reasoning_query::decode_frame(raw, "KR4");
    }
    require(raw.is_ascii(), "invalid response framing")?;
    let line = std::str::from_utf8(raw).unwrap();
    require(line.len() <= MAX_RESPONSE_BYTES, "invalid response")?;
    let line = line.strip_suffix('\n').unwrap_or(line);
    require(!line.contains(['\n', '\r']), "invalid response framing")?;
    let composed = line.starts_with("KR3\t");
    let mut r = Reader {
        line,
        position: 0,
        value_start: None,
        value_nodes: 0,
        composed,
    };
    require(
        r.take()? == if composed { "KR3" } else { "KR2" },
        "unsupported response protocol",
    )?;
    let status = r.take()?;
    require(
        ["ok", "unknown", "error", "limit", "unsupported_capability"].contains(&status),
        "invalid status",
    )?;
    if composed {
        r.value_start = Some(r.position);
    }
    let value = r.value(0)?;
    r.value_start = None;
    require((status == "ok") != value.is_null(), "status/value mismatch")?;
    let n = r.natural(DIAGNOSTICS.len() + if composed { 2 } else { 0 })?;
    let mut diagnostics = Vec::new();
    for _ in 0..n {
        diagnostics.push(r.take()?.to_owned());
    }
    ordered(&diagnostics)?;
    require(
        diagnostics.iter().all(|s| {
            DIAGNOSTICS.contains(&s.as_str())
                || composed && ["missing_field", "collection_limit"].contains(&s.as_str())
        }),
        "invalid diagnostic",
    )?;
    let potential = r.ordered_texts()?;
    let executed = r.ordered_texts()?;
    require(
        executed.iter().all(|s| potential.binary_search(s).is_ok()),
        "executed reads outside potential closure",
    )?;
    let steps = r.natural(10000000)?;
    let preflight = r.natural(10000000)?;
    let n = r.natural(20000)?;
    let mut counts = Vec::new();
    for _ in 0..n {
        counts.push((r.text()?, r.natural(1)?));
    }
    ordered(&counts.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>())?;
    require(
        counts
            .iter()
            .all(|(name, count)| *count == 1 && executed.binary_search(name).is_ok()),
        "invalid node evaluation count",
    )?;
    require(r.position > line.len(), "trailing response data")?;
    Ok(
        json!({"status":status,"value":value,"diagnostics":diagnostics,"potential_reads":potential,"executed_reads":executed,"steps":steps,"preflight_steps":preflight,"node_evaluations":counts.into_iter().map(|(k,v)|(k,json!(v))).collect::<Map<String,J>>()}),
    )
}
