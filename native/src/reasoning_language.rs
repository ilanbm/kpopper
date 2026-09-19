//! Lower a restricted authored grammar; AST parsing never executes Python code.
use crate::{
    Error, Result,
    history_contract::{Map, map},
    identity::python_float,
    require,
    value::TypedValue as V,
};
use rustpython_parser::{
    Parse,
    ast::{self, Ranged},
};
use serde_json::{Map as JsonMap, Value as J, json};
use std::{collections::BTreeSet, sync::LazyLock};
use unicode_normalization::UnicodeNormalization;
static NUMBER: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\A-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?\z").unwrap()
});
const OPS: &[&str] = &[
    "add", "sub", "mul", "div", "eq", "ne", "lt", "le", "gt", "ge", "and", "or", "not",
];
fn err(s: &str) -> Error {
    Error(s.into())
}
fn trim(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
}
fn keys(m: &Map, wanted: &[&str]) -> bool {
    m.len() == wanted.len() && wanted.iter().all(|k| m.contains_key(*k))
}
fn text(v: &V) -> Option<&str> {
    if let V::Text(s) = v { Some(s) } else { None }
}
fn structured(value: &V, depth: usize) -> Result<J> {
    require(
        depth <= 128 && matches!(value, V::Map(_)),
        "expression nesting exceeds core/v1 bounds or contains a cycle",
    )?;
    let m = map(value)?;
    if keys(m, &["op", "args"]) {
        let op = text(&m["op"]).unwrap_or("");
        require(OPS.contains(&op), "unsupported_capability")?;
        let V::List(args) = &m["args"] else {
            return Err(err("invalid operator arity"));
        };
        require(
            args.len() == if op == "not" { 1 } else { 2 },
            "invalid operator arity",
        )?;
        return Ok(
            json!({"op":op,"args":args.iter().map(|a|structured(a,depth+1)).collect::<Result<Vec<_>>>()?}),
        );
    }
    if keys(m, &["if", "then", "else"]) {
        return Ok(
            json!({"if":structured(&m["if"],depth+1)?,"then":structured(&m["then"],depth+1)?,"else":structured(&m["else"],depth+1)?}),
        );
    }
    if keys(m, &["list"]) {
        if let V::List(a) = &m["list"] {
            return Ok(
                json!({"list":a.iter().map(|v|structured(v,depth+1)).collect::<Result<Vec<_>>>()?}),
            );
        }
        return Err(err("invalid composition expression fields"));
    }
    if keys(m, &["record"]) {
        if let V::Map(a) = &m["record"] {
            return Ok(
                json!({"record":a.iter().map(|(k,v)|structured(v,depth+1).map(|v|(k.clone(),v))).collect::<Result<JsonMap<_,_>>>()?}),
            );
        }
        return Err(err("invalid composition expression fields"));
    }
    if keys(m, &["field", "key"]) {
        if let Some(key) = text(&m["key"]) {
            return Ok(json!({"field":structured(&m["field"],depth+1)?,"key":key}));
        }
        return Err(err("invalid composition expression fields"));
    }
    if keys(m, &["num"]) && text(&m["num"]).is_some_and(|s| s.len() <= 8200 && NUMBER.is_match(s)) {
        return Ok(json!({"num":text(&m["num"]).unwrap()}));
    }
    if keys(m, &["bool"])
        && let V::Bool(v) = &m["bool"]
    {
        return Ok(json!({"bool":v}));
    }
    if keys(m, &["text"])
        && let Some(v) = text(&m["text"])
    {
        return Ok(json!({"text":v}));
    }
    if keys(m, &["null"]) && m["null"] == V::Bool(true) {
        return Ok(json!({"null":true}));
    }
    if keys(m, &["ref"])
        && text(&m["ref"]).is_some_and(|s| !s.is_empty() && s.chars().count() <= 500)
    {
        return Ok(json!({"ref":text(&m["ref"]).unwrap()}));
    }
    Err(err("invalid expression fields"))
}
fn segment<'a>(source: &'a str, node: &ast::Expr) -> Result<&'a str> {
    let range = node.range();
    source
        .get(u32::from(range.start()) as usize..u32::from(range.end()) as usize)
        .ok_or_else(|| err("invalid expression syntax"))
}
fn name_part(part: &str) -> bool {
    crate::python_identifiers::identifier(part)
}
fn string_literal(node: &ast::Expr) -> Option<&str> {
    if let ast::Expr::Constant(c) = node
        && let ast::Constant::Str(s) = &c.value
    {
        Some(s)
    } else {
        None
    }
}
fn op(name: &str, args: Vec<J>) -> J {
    json!({"op":name,"args":args})
}
fn visit(source: &str, node: &ast::Expr, depth: usize) -> Result<J> {
    require(depth <= 128, "expression nesting exceeds 128")?;
    match node {
        ast::Expr::Name(_) | ast::Expr::Attribute(_) => {
            let spelling = segment(source, node)?;
            let parts = spelling.split('.').map(trim).collect::<Vec<_>>();
            require(parts.iter().all(|p| name_part(p)), "invalid reference")?;
            let name = parts.join(".");
            Ok(match name.as_str() {
                "true" | "True" => json!({"bool":true}),
                "false" | "False" => json!({"bool":false}),
                "null" => json!({"null":true}),
                _ => json!({"ref":name}),
            })
        }
        ast::Expr::Constant(c) => match &c.value {
            ast::Constant::None => Ok(json!({"null":true})),
            ast::Constant::Bool(v) => Ok(json!({"bool":v})),
            ast::Constant::Str(v) => Ok(json!({"text":v})),
            ast::Constant::Int(_) | ast::Constant::Float(_) => {
                Ok(json!({"num":segment(source,node)?}))
            }
            _ => Err(err("unsupported core/v1 expression syntax")),
        },
        ast::Expr::Call(call) => {
            if let ast::Expr::Name(name) = call.func.as_ref() {
                let _ = name;
                let normalized = segment(source, call.func.as_ref())?
                    .nfkc()
                    .collect::<String>();
                if call.keywords.is_empty() {
                    if normalized == "ref"
                        && call.args.len() == 1
                        && let Some(id) = string_literal(&call.args[0])
                    {
                        return Ok(json!({"ref":id}));
                    }
                    if normalized == "field"
                        && call.args.len() == 2
                        && let Some(key) = string_literal(&call.args[1])
                    {
                        return Ok(json!({"field":visit(source,&call.args[0],depth+1)?,"key":key}));
                    }
                }
            }
            Err(err("unsupported core/v1 expression syntax"))
        }
        ast::Expr::BoolOp(b) => {
            let name = if b.op == ast::BoolOp::And {
                "and"
            } else {
                "or"
            };
            let mut values = b.values.iter().map(|c| visit(source, c, depth + 1));
            let mut result = values
                .next()
                .ok_or_else(|| err("unsupported core/v1 expression syntax"))??;
            for next in values {
                result = op(name, vec![result, next?]);
            }
            Ok(result)
        }
        ast::Expr::UnaryOp(u) => {
            let child = visit(source, &u.operand, depth + 1)?;
            match u.op {
                ast::UnaryOp::Not => Ok(op("not", vec![child])),
                ast::UnaryOp::UAdd => Ok(op("add", vec![json!({"num":"0"}), child])),
                ast::UnaryOp::USub => {
                    if let Some(s) = child.get("num").and_then(J::as_str) {
                        Ok(
                            json!({"num":s.strip_prefix('-').map_or_else(||format!("-{s}"),str::to_owned)}),
                        )
                    } else {
                        Ok(op("sub", vec![json!({"num":"0"}), child]))
                    }
                }
                _ => Err(err("unsupported core/v1 expression syntax")),
            }
        }
        ast::Expr::IfExp(i) => Ok(
            json!({"if":visit(source,&i.test,depth+1)?,"then":visit(source,&i.body,depth+1)?,"else":visit(source,&i.orelse,depth+1)?}),
        ),
        ast::Expr::List(a) => Ok(
            json!({"list":a.elts.iter().map(|c|visit(source,c,depth+1)).collect::<Result<Vec<_>>>()?}),
        ),
        ast::Expr::Dict(d) => {
            let mut fields = JsonMap::new();
            for (key, value) in d.keys.iter().zip(&d.values) {
                let key = key
                    .as_ref()
                    .and_then(string_literal)
                    .ok_or_else(|| err("record keys must be literal strings"))?;
                require(!fields.contains_key(key), "duplicate record key")?;
                fields.insert(key.into(), visit(source, value, depth + 1)?);
            }
            Ok(json!({"record":fields}))
        }
        ast::Expr::BinOp(b) => {
            let name = match b.op {
                ast::Operator::Add => "add",
                ast::Operator::Sub => "sub",
                ast::Operator::Mult => "mul",
                ast::Operator::Div => "div",
                _ => return Err(err("unsupported core/v1 expression syntax")),
            };
            Ok(op(
                name,
                vec![
                    visit(source, &b.left, depth + 1)?,
                    visit(source, &b.right, depth + 1)?,
                ],
            ))
        }
        ast::Expr::Compare(c) if c.ops.len() == 1 => {
            let name = match c.ops[0] {
                ast::CmpOp::Eq => "eq",
                ast::CmpOp::NotEq => "ne",
                ast::CmpOp::Lt => "lt",
                ast::CmpOp::LtE => "le",
                ast::CmpOp::Gt => "gt",
                ast::CmpOp::GtE => "ge",
                _ => return Err(err("unsupported core/v1 expression syntax")),
            };
            Ok(op(
                name,
                vec![
                    visit(source, &c.left, depth + 1)?,
                    visit(source, &c.comparators[0], depth + 1)?,
                ],
            ))
        }
        _ => Err(err("unsupported core/v1 expression syntax")),
    }
}
pub fn lower(value: &V) -> Result<J> {
    if let V::Map(m) = value
        && keys(m, &["expr"])
    {
        let source = text(&m["expr"])
            .filter(|s| !trim(s).is_empty() && s.chars().count() <= 4000)
            .ok_or_else(|| err("expr must be nonempty text within 4000 characters"))?;
        let source = trim(source);
        let parsed_source = parser_source(source)?;
        let ast = ast::Expr::parse(&parsed_source, "<expression>")
            .map_err(|_| err("invalid expression syntax"))?;
        let lowered = visit(source, &ast, 0)?;
        return structured(&ir_value(&lowered, 0)?, 0);
    }
    structured(value, 0)
}
/// Legacy claim selection normalizes only the arithmetic grammar from
/// expressions.lower. Core grammar extensions must not change conflict identity.
pub(crate) fn legacy_rule(value: &V) -> Result<V> {
    legacy_expression(value, false)
}
pub(crate) fn legacy_references(value: &V) -> Vec<String> {
    legacy_expression(value, false)
        .or_else(|_| legacy_expression(value, true))
        .and_then(|v| v.to_json())
        .map(|v| references(&v))
        .unwrap_or_default()
}
fn legacy_expression(value: &V, predicate: bool) -> Result<V> {
    fn legacy_visit(source: &str, node: &ast::Expr, depth: usize) -> Result<J> {
        require(depth < 64, "invalid_legacy_expression")?;
        Ok(match node {
            ast::Expr::Name(_) | ast::Expr::Attribute(_) => {
                let name = segment(source, node)?;
                require(name.split('.').all(name_part), "invalid_legacy_expression")?;
                match name {
                    "true" | "True" => json!({"bool":true}),
                    "false" | "False" => json!({"bool":false}),
                    _ => json!({"ref":name}),
                }
            }
            ast::Expr::Constant(c) => match &c.value {
                ast::Constant::Bool(v) => json!({"bool":v}),
                ast::Constant::Str(v) => json!({"text":v}),
                ast::Constant::Int(_) | ast::Constant::Float(_) => {
                    json!({"num":segment(source,node)?})
                }
                _ => return Err(err("invalid_legacy_expression")),
            },
            ast::Expr::UnaryOp(u) if u.op == ast::UnaryOp::USub => {
                let child = legacy_visit(source, &u.operand, depth + 1)?;
                if let Some(number) = child.get("num").and_then(J::as_str) {
                    json!({"num":format!("-{number}")})
                } else {
                    op("sub", vec![json!({"num":"0"}), child])
                }
            }
            ast::Expr::BinOp(b) => {
                let operator = match b.op {
                    ast::Operator::Add => "add",
                    ast::Operator::Sub => "sub",
                    ast::Operator::Mult => "mul",
                    ast::Operator::Div => "div",
                    _ => return Err(err("invalid_legacy_expression")),
                };
                op(
                    operator,
                    vec![
                        legacy_visit(source, &b.left, depth + 1)?,
                        legacy_visit(source, &b.right, depth + 1)?,
                    ],
                )
            }
            ast::Expr::Compare(c) if c.ops.len() == 1 && c.comparators.len() == 1 => {
                let operator = match c.ops[0] {
                    ast::CmpOp::Eq => "eq",
                    ast::CmpOp::NotEq => "ne",
                    ast::CmpOp::Lt => "lt",
                    ast::CmpOp::LtE => "le",
                    ast::CmpOp::Gt => "gt",
                    ast::CmpOp::GtE => "ge",
                    _ => return Err(err("invalid_legacy_expression")),
                };
                op(
                    operator,
                    vec![
                        legacy_visit(source, &c.left, depth + 1)?,
                        legacy_visit(source, &c.comparators[0], depth + 1)?,
                    ],
                )
            }
            ast::Expr::Call(c)
                if c.args.len() == 1
                    && c.keywords.is_empty()
                    && matches!(c.func.as_ref(), ast::Expr::Name(_))
                    && segment(source, c.func.as_ref())?.nfkc().collect::<String>() == "ref" =>
            {
                json!({"ref":string_literal(&c.args[0]).ok_or_else(|| err("invalid_legacy_expression"))?})
            }
            _ => return Err(err("invalid_legacy_expression")),
        })
    }
    fn validate(value: &V, depth: usize, predicate: bool) -> Result<()> {
        require(depth < 64, "invalid_legacy_expression")?;
        let V::Map(m) = value else {
            return Err(err("invalid_legacy_expression"));
        };
        if keys(m, &["op", "args"]) {
            let operators = if predicate {
                &["eq", "ne", "lt", "le", "gt", "ge"][..]
            } else {
                &["add", "sub", "mul", "div"][..]
            };
            require(
                text(&m["op"]).is_some_and(|v| operators.contains(&v)),
                "invalid_legacy_expression",
            )?;
            let V::List(args) = &m["args"] else {
                return Err(err("invalid_legacy_expression"));
            };
            require(args.len() == 2, "invalid_legacy_expression")?;
            for child in args {
                validate(child, depth + 1, false)?;
            }
            if predicate {
                require(
                    !references(&value.to_json()?).is_empty(),
                    "invalid_legacy_expression",
                )?;
            }
        } else if predicate {
            return Err(err("invalid_legacy_expression"));
        } else if keys(m, &["ref"]) {
            require(
                text(&m["ref"]).is_some_and(|v| !v.is_empty() && v.chars().count() <= 500),
                "invalid_legacy_expression",
            )?;
        } else if keys(m, &["num"]) {
            static NUMBER: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
                regex::Regex::new(r"^-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?$")
                    .unwrap()
            });
            let n = text(&m["num"]).ok_or_else(|| err("invalid_legacy_expression"))?;
            require(
                n.len() <= 512 && NUMBER.is_match(n),
                "invalid_legacy_expression",
            )?;
            let (mantissa, exponent) = n.split_once(['e', 'E']).unwrap_or((n, "0"));
            let exponent = exponent
                .parse::<i32>()
                .map_err(|_| err("invalid_legacy_expression"))?;
            let fraction = mantissa.split_once('.').map_or(0, |(_, v)| v.len() as i32);
            require(
                (-256..=256).contains(&exponent) && fraction - exponent <= 256,
                "invalid_legacy_expression",
            )?;
        } else if keys(m, &["text"]) {
            require(text(&m["text"]).is_some(), "invalid_legacy_expression")?;
        } else if keys(m, &["bool"]) {
            require(matches!(m["bool"], V::Bool(_)), "invalid_legacy_expression")?;
        } else {
            return Err(err("invalid_legacy_expression"));
        }
        Ok(())
    }
    let lowered = if let V::Map(m) = value
        && keys(m, &["expr"])
    {
        let source = text(&m["expr"])
            .filter(|s| !trim(s).is_empty() && s.chars().count() <= 4000)
            .ok_or_else(|| err("invalid_legacy_expression"))?;
        let source = trim(source);
        let ast = ast::Expr::parse(&parser_source(source)?, "<expression>")
            .map_err(|_| err("invalid_legacy_expression"))?;
        V::from_json(&legacy_visit(source, &ast, 0)?)?
    } else {
        value.clone()
    };
    validate(&lowered, 0, predicate)?;
    Ok(lowered)
}
pub fn literal(value: &V) -> Result<J> {
    fn convert(v: &V, depth: usize) -> Result<J> {
        require(
            depth <= 128,
            "literal exceeds core/v1 bounds or contains a cycle",
        )?;
        match v {
            V::Null => Ok(json!({"null":true})),
            V::Bool(v) => Ok(json!({"bool":v})),
            V::Integer(v) => lower(&V::Map(Map::from([(
                "num".into(),
                V::Text(v.as_str().into()),
            )]))),
            V::Float(v) => lower(&V::Map(Map::from([(
                "num".into(),
                V::Text(python_float(v.get())),
            )]))),
            V::Text(v) => Ok(json!({"text":v})),
            V::List(a) => {
                Ok(json!({"list":a.iter().map(|v|convert(v,depth+1)).collect::<Result<Vec<_>>>()?}))
            }
            V::Map(m) => Ok(
                json!({"record":m.iter().map(|(k,v)|convert(v,depth+1).map(|v|(k.clone(),v))).collect::<Result<JsonMap<_,_>>>()?}),
            ),
            _ => Err(err("input is outside core/v1 finite value types")),
        }
    }
    convert(value, 0)
}
pub fn node_expression(node: &V) -> Result<J> {
    let node = map(node)?;
    let body = node.get("body").unwrap_or(&V::Null);
    let V::Map(m) = body else {
        return literal(body);
    };
    if let Some(rule) = m.get("rule").filter(|v| **v != V::Null) {
        return lower(rule);
    }
    for key in ["v", "quoted"] {
        if let Some(v) = m.get(key) {
            return literal(v);
        }
    }
    Ok(json!({"unavailable":"missing_reference"}))
}
pub fn references(expression: &J) -> Vec<String> {
    let mut found = BTreeSet::new();
    let mut pending = vec![expression];
    while let Some(v) = pending.pop() {
        if let Some(id) = v.get("ref").and_then(J::as_str) {
            found.insert(id.into());
        }
        pending.extend(crate::reasoning_query::children(v));
    }
    found.into_iter().collect()
}
pub fn required_modules(expression: &J) -> Vec<String> {
    let mut found = BTreeSet::from(["arithmetic/v1".into()]);
    let mut pending = vec![expression];
    while let Some(v) = pending.pop() {
        if ["if", "list", "record", "field"]
            .iter()
            .any(|k| v.get(*k).is_some())
            || v.get("op")
                .and_then(J::as_str)
                .is_some_and(|s| ["and", "or", "not"].contains(&s))
        {
            found.insert("composition/v1".into());
        }
        pending.extend(crate::reasoning_query::children(v));
    }
    found.into_iter().collect()
}
pub fn closure(snapshot: &V, roots: &[String]) -> Result<J> {
    let snapshot = map(snapshot)?;
    let nodes = map(snapshot
        .get("nodes")
        .ok_or_else(|| err("invalid snapshot"))?)?;
    let empty = Map::new();
    let context = snapshot
        .get("context")
        .map(map)
        .transpose()?
        .unwrap_or(&empty);
    let conflicts = context.get("conflicts");
    let mut pending = roots.to_vec();
    let mut seen = BTreeSet::new();
    let mut expressions = JsonMap::new();
    let mut errors = JsonMap::new();
    let mut edges = 0;
    while let Some(id) = pending.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        require(seen.len() <= 20000, "node_limit")?;
        let Some(node) = nodes.get(&id) else {
            continue;
        };
        let expr = match node_expression(node) {
            Ok(v) => v,
            Err(_) => {
                errors.insert(id.clone(), json!("invalid_expression"));
                expressions.insert(id, json!({"unavailable":"invalid_expression"}));
                continue;
            }
        };
        let refs = references(&expr);
        edges += refs.len();
        require(edges <= 100000, "edge_limit")?;
        pending.extend(refs);
        let contested = match conflicts {
            None => false,
            Some(V::Map(m)) => m.contains_key(&id),
            Some(V::List(a)) => a.contains(&V::Text(id.clone())),
            Some(V::Text(s)) => s.contains(&id),
            _ => return Err(err("invalid snapshot context")),
        };
        expressions.insert(
            id,
            if contested {
                json!({"unavailable":"contested"})
            } else {
                expr
            },
        );
    }
    Ok(json!({"ids":seen,"nodes":expressions,"errors":errors}))
}

// ASTs have an expression-depth limit independent of history's data-container
// limit. The trusted visitor emits only this algebra; validate its expression
// depth before handing it to any later, separately bounded report container.
fn ir_value(j: &J, depth: usize) -> Result<V> {
    require(
        depth <= 258,
        "expression nesting exceeds core/v1 bounds or contains a cycle",
    )?;
    Ok(match j {
        J::Null => V::Null,
        J::Bool(v) => V::Bool(*v),
        J::String(s) => V::Text(s.clone()),
        J::Array(a) => V::List(
            a.iter()
                .map(|v| ir_value(v, depth + 1))
                .collect::<Result<_>>()?,
        ),
        J::Object(m) => V::Map(
            m.iter()
                .map(|(k, v)| ir_value(v, depth + 1).map(|v| (k.clone(), v)))
                .collect::<Result<_>>()?,
        ),
        J::Number(_) => unreachable!("IR numbers are lexemes"),
    })
}

// The parser's bundled Unicode identifier data predates the pinned oracle.
// Replace identifier scalars only in its scratch input, preserving byte offsets.
// All reference spelling and validation still use the original Unicode-16 source.
fn parser_source(source: &str) -> Result<String> {
    let raw = source.as_bytes();
    let mut output = raw.to_vec();
    let mut i = 0;
    let mut quote: Option<(u8, usize, bool)> = None;
    let mut comment = false;
    let mut nesting = 0usize;
    while i < raw.len() {
        let c = source[i..].chars().next().unwrap();
        let width = c.len_utf8();
        if comment {
            if c == '\n' || c == '\r' {
                comment = false;
            }
            i += width;
            continue;
        }
        if let Some((q, count, raw_string)) = quote {
            if c == '\\' {
                // Rust strings cannot retain Python surrogate code points.
                // Refuse before the AST parser can replace them with U+FFFD.
                if !raw_string && let Some(escape) = raw.get(i + 1) {
                    let digits = match escape {
                        b'u' => 4,
                        b'U' => 8,
                        _ => 0,
                    };
                    if digits > 0
                        && let Some(bytes) = raw.get(i + 2..i + 2 + digits)
                        && bytes.iter().all(u8::is_ascii_hexdigit)
                    {
                        let code =
                            u32::from_str_radix(std::str::from_utf8(bytes).unwrap(), 16).unwrap();
                        require(
                            code <= 0x10ffff && !(0xd800..=0xdfff).contains(&code),
                            "unsupported_unicode_scalar",
                        )?;
                    }
                }
                i += width;
                if i < raw.len() {
                    i += source[i..].chars().next().unwrap().len_utf8();
                }
                continue;
            }
            if raw[i] == q
                && raw[i..].iter().take(count).count() == count
                && raw[i..i + count].iter().all(|b| *b == q)
            {
                quote = None;
                i += count;
                continue;
            }
            i += width;
            continue;
        }
        if c == '#' {
            comment = true;
        } else if c == '\'' || c == '"' {
            let q = raw[i];
            let count = if i + 3 <= raw.len() && raw[i..i + 3].iter().all(|b| *b == q) {
                3
            } else {
                1
            };
            let mut prefix_start = i;
            while prefix_start > 0 && raw[prefix_start - 1].is_ascii_alphabetic() {
                prefix_start -= 1;
            }
            let prefix = source[prefix_start..i].to_ascii_lowercase();
            quote = Some((
                q,
                count,
                ["r", "br", "rb", "fr", "rf"].contains(&prefix.as_str()),
            ));
            i += count;
            continue;
        } else if matches!(c, '(' | '[' | '{') {
            nesting += 1;
            require(nesting <= 200, "invalid expression syntax")?;
        } else if matches!(c, ')' | ']' | '}') {
            nesting = nesting.saturating_sub(1);
        } else if !c.is_ascii() && crate::python_identifiers::continues(c) {
            output[i..i + width].fill(b'_');
        }
        i += width;
    }
    Ok(String::from_utf8(output).expect("whole scalars replaced with ASCII"))
}
