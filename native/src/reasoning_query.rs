//! Canonical query framing and validation; query meaning remains in Lean.
use crate::{Error, Result, require};
use num_bigint::BigInt;
use serde::Deserialize;
use serde_json::{Map, Value as J, json};
use std::collections::BTreeSet;
const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
fn err(s: &str) -> Error {
    Error(s.into())
}
fn m<'a>(v: &'a J, code: &str) -> Result<&'a Map<String, J>> {
    v.as_object().ok_or_else(|| err(code))
}
fn fields(v: &J, keys: &[&str]) -> bool {
    v.as_object()
        .is_some_and(|m| m.len() == keys.len() && keys.iter().all(|k| m.contains_key(*k)))
}
fn identifier<'a>(v: &'a J, label: &str) -> Result<&'a str> {
    v.as_str()
        .filter(|s| !s.is_empty() && s.chars().count() <= 500)
        .ok_or_else(|| err(&format!("invalid {label}")))
}
fn integer(s: &str) -> bool {
    let raw = s.strip_prefix('-').unwrap_or(s);
    !raw.is_empty()
        && raw.bytes().all(|b| b.is_ascii_digit())
        && (s == "0" || !raw.starts_with('0'))
}
fn rational(v: &J) -> Result<()> {
    require(
        fields(v, &["type", "numerator", "denominator"]) && v["type"] == "number",
        "invalid exact numeric value",
    )?;
    let a = v["numerator"]
        .as_str()
        .ok_or_else(|| err("noncanonical exact numeric value"))?;
    let b = v["denominator"]
        .as_str()
        .ok_or_else(|| err("noncanonical exact numeric value"))?;
    require(
        a.len() <= 4097
            && b.len() <= 4096
            && integer(a)
            && integer(b)
            && !b.starts_with('-')
            && b != "0",
        "noncanonical exact numeric value",
    )?;
    let (mut a, mut b) = (a.parse::<BigInt>().unwrap(), b.parse::<BigInt>().unwrap());
    while b != BigInt::from(0) {
        let r = &a % &b;
        a = b;
        b = r;
    }
    require(
        a == BigInt::from(1) || a == BigInt::from(-1),
        "noncanonical exact numeric value",
    )
}
pub fn typed_value(v: &J) -> Result<()> {
    fn check(v: &J, depth: usize) -> Result<()> {
        require(depth <= 128 && v.is_object(), "invalid typed value")?;
        match v["type"].as_str() {
            Some("number") => rational(v),
            Some("boolean") if fields(v, &["type", "value"]) && v["value"].is_boolean() => Ok(()),
            Some("text") if fields(v, &["type", "value"]) && v["value"].is_string() => Ok(()),
            Some("null") if fields(v, &["type"]) => Ok(()),
            Some("list")
                if fields(v, &["type", "items"])
                    && v["items"].as_array().is_some_and(|a| a.len() <= 10000) =>
            {
                for c in v["items"].as_array().unwrap() {
                    check(c, depth + 1)?;
                }
                Ok(())
            }
            Some("record")
                if fields(v, &["type", "fields"])
                    && v["fields"].as_object().is_some_and(|m| m.len() <= 10000) =>
            {
                for c in v["fields"].as_object().unwrap().values() {
                    check(c, depth + 1)?;
                }
                Ok(())
            }
            _ => Err(err("invalid typed value shape")),
        }
    }
    check(v, 0)
}
pub fn canonical_json_bytes(v: &J) -> Result<Vec<u8>> {
    fn check(v: &J, depth: usize) -> Result<()> {
        require(depth <= 256, "JSON nesting exceeds canonical bounds")?;
        match v {
            J::Number(n) => {
                let s = n.to_string();
                require(
                    !s.contains(['.', 'e', 'E']),
                    "canonical KP4 JSON does not contain floating point numbers",
                )?;
                require(
                    s.trim_start_matches('-').len() <= 4300,
                    "invalid canonical JSON value",
                )?;
            }
            J::Array(a) => {
                for v in a {
                    check(v, depth + 1)?;
                }
            }
            J::Object(m) => {
                if m.get("type").is_some_and(|v| v == "number") {
                    rational(v)?;
                }
                for v in m.values() {
                    check(v, depth + 1)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    check(v, 0)?;
    let bytes = serde_json::to_vec(v)?;
    require(
        bytes.len() <= MAX_RESPONSE_BYTES,
        "canonical JSON exceeds transport bounds",
    )?;
    Ok(bytes)
}
pub fn decode_canonical_json(raw: &[u8]) -> Result<J> {
    require(
        !raw.is_empty() && raw.len() <= MAX_RESPONSE_BYTES,
        "invalid canonical JSON payload length",
    )?;
    // Bound syntax before disabling serde's shallower default. Inspect number
    // spelling before a JSON value parser can normalize a forbidden -0 or float.
    let (mut string, mut escaped, mut depth, mut i) = (false, false, 0usize, 0usize);
    while i < raw.len() {
        let b = raw[i];
        if string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                string = false;
            }
        } else if b == b'"' {
            string = true;
        } else if b == b'[' || b == b'{' {
            depth += 1;
            require(depth <= 257, "JSON nesting exceeds canonical bounds")?;
        } else if b == b']' || b == b'}' {
            depth = depth.saturating_sub(1);
        } else if b == b'-' || b.is_ascii_digit() {
            let start = i;
            while i + 1 < raw.len() && !b",]} \t\r\n".contains(&raw[i + 1]) {
                i += 1;
            }
            let token = std::str::from_utf8(&raw[start..=i])
                .map_err(|_| err("malformed canonical JSON"))?;
            require(
                integer(token) && token.trim_start_matches('-').len() <= 4096,
                "malformed canonical JSON",
            )?;
        }
        i += 1;
    }
    let mut decoder = serde_json::Deserializer::from_slice(raw);
    decoder.disable_recursion_limit();
    crate::store::Unique::deserialize(&mut decoder).map_err(|_| err("malformed canonical JSON"))?;
    decoder.end().map_err(|_| err("malformed canonical JSON"))?;
    let mut decoder = serde_json::Deserializer::from_slice(raw);
    decoder.disable_recursion_limit();
    let raw_value = <&serde_json::value::RawValue>::deserialize(&mut decoder)
        .map_err(|_| err("malformed canonical JSON"))?;
    let value = raw_json_value(raw_value)?;
    require(canonical_json_bytes(&value)? == raw, "noncanonical JSON")?;
    Ok(value)
}

// RawValue distinguishes an actual JSON object from serde's synthetic map used
// for arbitrary-precision numbers. User keys never select a deserializer type.
fn raw_json_value(raw: &serde_json::value::RawValue) -> Result<J> {
    let text = raw.get();
    let mut decoder = serde_json::Deserializer::from_str(text);
    decoder.disable_recursion_limit();
    match text.as_bytes()[0] {
        b'{' => {
            let fields =
                std::collections::BTreeMap::<String, &serde_json::value::RawValue>::deserialize(
                    &mut decoder,
                )
                .map_err(|_| err("malformed canonical JSON"))?;
            Ok(J::Object(
                fields
                    .into_iter()
                    .map(|(k, v)| raw_json_value(v).map(|v| (k, v)))
                    .collect::<Result<_>>()?,
            ))
        }
        b'[' => {
            let items = Vec::<&serde_json::value::RawValue>::deserialize(&mut decoder)
                .map_err(|_| err("malformed canonical JSON"))?;
            Ok(J::Array(
                items
                    .into_iter()
                    .map(raw_json_value)
                    .collect::<Result<_>>()?,
            ))
        }
        b'"' => String::deserialize(&mut decoder)
            .map(J::String)
            .map_err(|_| err("malformed canonical JSON")),
        b'n' => Ok(J::Null),
        b't' => Ok(J::Bool(true)),
        b'f' => Ok(J::Bool(false)),
        _ => text
            .parse::<serde_json::Number>()
            .map(J::Number)
            .map_err(|_| err("malformed canonical JSON")),
    }
}
pub fn encode_frame(v: &J, prefix: &str) -> Result<Vec<u8>> {
    require(
        ["KP4", "KR4"].contains(&prefix),
        "unsupported frame protocol",
    )?;
    let body = canonical_json_bytes(v)?;
    require(
        body.len()
            <= if prefix == "KP4" {
                MAX_BYTES
            } else {
                MAX_RESPONSE_BYTES
            },
        "frame exceeds transport bounds",
    )?;
    let mut frame = format!("{prefix} {}\n", body.len()).into_bytes();
    frame.extend(body);
    frame.push(b'\n');
    Ok(frame)
}
pub fn decode_frame(frame: &[u8], prefix: &str) -> Result<J> {
    require(["KP4", "KR4"].contains(&prefix), "invalid frame")?;
    let marker = format!("{prefix} ");
    require(
        frame.starts_with(marker.as_bytes()),
        "unsupported frame protocol",
    )?;
    let end = frame[marker.len()..]
        .iter()
        .position(|b| *b == b'\n')
        .map(|n| n + marker.len())
        .ok_or_else(|| err("truncated frame header"))?;
    let length =
        std::str::from_utf8(&frame[marker.len()..end]).map_err(|_| err("invalid frame length"))?;
    require(
        integer(length) && !length.starts_with('-'),
        "invalid frame length",
    )?;
    let size = length
        .parse::<usize>()
        .map_err(|_| err("frame exceeds transport bounds"))?;
    require(
        size <= if prefix == "KP4" {
            MAX_BYTES
        } else {
            MAX_RESPONSE_BYTES
        },
        "frame exceeds transport bounds",
    )?;
    let stop = end + 1 + size;
    require(
        stop + 1 == frame.len() && frame.get(stop) == Some(&b'\n'),
        "frame length or terminator mismatch",
    )?;
    decode_canonical_json(&frame[end + 1..stop])
}
const ARITHMETIC: [&str; 10] = [
    "add", "sub", "mul", "div", "eq", "ne", "lt", "le", "gt", "ge",
];
const BOOLEAN: [&str; 3] = ["and", "or", "not"];
fn number_lexeme(s: &str) -> bool {
    use std::sync::LazyLock;
    static NUMBER: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"\A-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?\z").unwrap()
    });
    s.len() <= 8200 && NUMBER.is_match(s)
}
pub fn lower_row(v: &J, columns: &[String]) -> Result<J> {
    require(
        columns.windows(2).all(|w| w[0] < w[1]) && columns.iter().all(|s| !s.is_empty()),
        "scope fields must be sorted unique text",
    )?;
    fn lower(v: &J, columns: &[String], depth: usize) -> Result<J> {
        require(
            depth <= 128 && v.is_object(),
            "row expression exceeds bounds or contains a cycle",
        )?;
        if fields(v, &["column"]) {
            let name = identifier(&v["column"], "column")?;
            require(
                columns.iter().any(|c| c == name),
                &format!("column is not granted by the scope: {name}"),
            )?;
            return Ok(v.clone());
        }
        if fields(v, &["num"]) {
            require(
                v["num"].as_str().is_some_and(number_lexeme),
                "invalid exact number lexeme",
            )?;
            return Ok(v.clone());
        }
        if fields(v, &["bool"]) && v["bool"].is_boolean()
            || fields(v, &["text"]) && v["text"].is_string()
            || fields(v, &["null"]) && v["null"] == J::Bool(true)
        {
            return Ok(v.clone());
        }
        if fields(v, &["op", "args"]) {
            let name = v["op"].as_str().unwrap_or("");
            let a = v["args"]
                .as_array()
                .ok_or_else(|| err("invalid row operation or arity"))?;
            require(
                (ARITHMETIC.contains(&name) || BOOLEAN.contains(&name))
                    && a.len() == if name == "not" { 1 } else { 2 },
                "invalid row operation or arity",
            )?;
            for c in a {
                lower(c, columns, depth + 1)?;
            }
            return Ok(v.clone());
        }
        if fields(v, &["if", "then", "else"]) {
            for key in ["if", "then", "else"] {
                lower(&v[key], columns, depth + 1)?;
            }
            return Ok(v.clone());
        }
        if fields(v, &["list"]) && v["list"].as_array().is_some_and(|a| a.len() <= 10000) {
            for c in v["list"].as_array().unwrap() {
                lower(c, columns, depth + 1)?;
            }
            return Ok(v.clone());
        }
        if fields(v, &["record"])
            && v["record"]
                .as_object()
                .is_some_and(|m| m.len() <= 10000 && m.keys().all(|k| k.chars().count() <= 500))
        {
            for c in v["record"].as_object().unwrap().values() {
                lower(c, columns, depth + 1)?;
            }
            return Ok(v.clone());
        }
        if fields(v, &["field", "key"])
            && v["key"].as_str().is_some_and(|s| s.chars().count() <= 500)
        {
            lower(&v["field"], columns, depth + 1)?;
            return Ok(v.clone());
        }
        Err(err("invalid row expression fields"))
    }
    lower(v, columns, 0)
}
pub fn lower(operation: &J, columns: &[String]) -> Result<J> {
    require(
        fields(operation, &["query"]) && operation["query"].is_object(),
        "query operation needs exactly one query key",
    )?;
    let query = &operation["query"];
    let op = query["op"].as_str().unwrap_or("");
    require(
        ["filter", "project", "select", "count", "sum", "all", "any"].contains(&op)
            && query["version"].as_u64() == Some(1),
        "unknown query version or operation",
    )?;
    identifier(&query["scope"], "scope ID")?;
    let keys = if op == "project" {
        vec!["version", "scope", "op", "value"]
    } else if op == "select" || op == "sum" && query.get("where").is_some() {
        vec!["version", "scope", "op", "where", "value"]
    } else if op == "sum" {
        vec!["version", "scope", "op", "value"]
    } else {
        vec!["version", "scope", "op", "where"]
    };
    require(
        fields(query, &keys),
        &format!("invalid keys for {op} query"),
    )?;
    for key in ["where", "value"] {
        if let Some(v) = query.get(key) {
            lower_row(v, columns)?;
        }
    }
    Ok(operation.clone())
}
pub(crate) fn children(v: &J) -> Vec<&J> {
    if let Some(a) = v.get("args").and_then(J::as_array) {
        a.iter().collect()
    } else if v.get("if").is_some() {
        ["if", "then", "else"].iter().map(|k| &v[*k]).collect()
    } else if let Some(a) = v.get("list").and_then(J::as_array) {
        a.iter().collect()
    } else if let Some(m) = v.get("record").and_then(J::as_object) {
        m.values().collect()
    } else if let Some(c) = v.get("field") {
        vec![c]
    } else {
        Vec::new()
    }
}
pub fn required_modules(operation: &J) -> Vec<String> {
    let mut found = BTreeSet::from(["query/v1".to_owned()]);
    let mut pending = ["where", "value"]
        .iter()
        .filter_map(|k| operation["query"].get(*k))
        .collect::<Vec<_>>();
    while let Some(v) = pending.pop() {
        let op = v["op"].as_str().unwrap_or("");
        if ARITHMETIC.contains(&op) {
            found.insert("arithmetic/v1".into());
        }
        if BOOLEAN.contains(&op)
            || ["if", "list", "record", "field"]
                .iter()
                .any(|k| v.get(*k).is_some())
        {
            found.insert("composition/v1".into());
        }
        pending.extend(children(v));
    }
    found.into_iter().collect()
}
pub fn resources(limits: Option<&J>) -> Result<J> {
    let default = json!({"version":"resources/v4","steps":1000000,"depth":128,"digits":256,"value_nodes":10000,"value_depth":128,"value_bytes":16777216,"candidates":10000,"field_reads":100000});
    let mut result = default.clone();
    if let Some(v) = limits.filter(|v| !v.is_null()) {
        let m = m(v, "query resources must be a mapping")?;
        require(
            m.get("version").is_none_or(|v| v == "resources/v4")
                && m.keys().all(|k| default.get(k).is_some()),
            "unknown query resource profile or field",
        )?;
        for (k, v) in m {
            if k == "version" {
                continue;
            }
            require(
                v.as_u64()
                    .is_some_and(|n| n > 0 && n <= default[k].as_u64().unwrap()),
                &format!("invalid query resource limit: {k}"),
            )?;
            result[k] = v.clone();
        }
    }
    Ok(result)
}
pub fn witness(v: &J, kind: &str) -> Result<()> {
    m(v, "invalid dependency witness")?;
    require(v["kind"] == kind, "unexpected dependency witness kind")?;
    let keys = if kind == "node" {
        vec!["kind", "id", "fingerprint"]
    } else {
        vec![
            "kind",
            "scope_id",
            "definition_digest",
            "membership_digest",
            "projected_inputs_digest",
        ]
    };
    require(fields(v, &keys), "invalid dependency witness shape")?;
    identifier(
        &v[if kind == "node" { "id" } else { "scope_id" }],
        if kind == "node" {
            "node ID"
        } else {
            "scope ID"
        },
    )?;
    for k in if kind == "node" {
        vec!["fingerprint"]
    } else {
        vec![
            "definition_digest",
            "membership_digest",
            "projected_inputs_digest",
        ]
    } {
        require(
            v[k].as_str().is_some_and(|s| {
                s.len() == 64
                    && s.bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            }),
            if kind == "node" {
                "invalid node fingerprint"
            } else {
                "invalid scope witness digest"
            },
        )?;
    }
    Ok(())
}
pub fn validate_request(request: &J) -> Result<J> {
    require(
        fields(
            request,
            &[
                "version",
                "request_id",
                "resources",
                "required_modules",
                "operation",
                "root_witness",
                "scope",
            ],
        ) && request["version"].as_f64() == Some(4.0),
        "invalid KP4 request shape",
    )?;
    require(
        request["request_id"].as_str().is_some_and(|s| {
            s.len() == 64
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }),
        "invalid KP4 request ID",
    )?;
    let limits = resources(Some(&request["resources"]))?;
    let scope = &request["scope"];
    require(
        fields(scope, &["id", "witness", "fields", "members"]),
        "invalid KP4 scope shape",
    )?;
    let scope_id = identifier(&scope["id"], "scope ID")?;
    witness(&scope["witness"], "scope")?;
    require(
        scope["witness"]["scope_id"] == scope_id
            && scope["members"].is_array()
            && scope["fields"].is_array(),
        "invalid KP4 scope binding",
    )?;
    let columns = scope["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            v.as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| err("invalid KP4 scope binding"))
        })
        .collect::<Result<Vec<_>>>()?;
    require(
        columns.windows(2).all(|w| w[0] < w[1]),
        "invalid KP4 scope binding",
    )?;
    for column in &columns {
        identifier(&J::String(column.clone()), "column")?;
    }
    let members = scope["members"].as_array().unwrap();
    let ids = members
        .iter()
        .map(|m| {
            m.get("id")
                .and_then(J::as_str)
                .ok_or_else(|| err("scope members must be sorted and unique"))
        })
        .collect::<Result<Vec<_>>>()?;
    require(
        ids.windows(2).all(|w| w[0] < w[1]),
        "scope members must be sorted and unique",
    )?;
    for member in members {
        require(
            fields(member, &["id", "fields"]) && member["fields"].is_object(),
            "invalid scope member",
        )?;
        identifier(&member["id"], "member ID")?;
        let cells = member["fields"].as_object().unwrap();
        require(
            cells.keys().cloned().collect::<Vec<_>>() == columns,
            "scope member grants disagree",
        )?;
        for (column, cell) in cells {
            identifier(&J::String(column.clone()), "column")?;
            require(
                cell.is_object()
                    && cell["status"].as_str().is_some_and(|s| {
                        ["known", "missing", "contested", "unavailable"].contains(&s)
                    }),
                "invalid wire field state",
            )?;
            match cell["status"].as_str().unwrap() {
                "known" => {
                    require(
                        fields(cell, &["status", "value"]),
                        "invalid known wire field",
                    )?;
                    typed_value(&cell["value"])?;
                }
                "unavailable" => require(
                    fields(cell, &["status", "reason"])
                        && cell["reason"].as_str().is_some_and(|s| {
                            ["unsupported_type", "formula_value", "invalid_value"].contains(&s)
                        }),
                    "invalid unavailable wire field",
                )?,
                _ => require(
                    fields(cell, &["status"]),
                    "invalid missing/contested wire field",
                )?,
            }
        }
    }
    let normalized = lower(&request["operation"], &columns)?;
    require(
        normalized == request["operation"] && normalized["query"]["scope"] == scope_id,
        "noncanonical KP4 operation",
    )?;
    require(
        request["required_modules"] == json!(required_modules(&normalized)),
        "noncanonical closure-local module list",
    )?;
    if !request["root_witness"].is_null() {
        witness(&request["root_witness"], "node")?;
    }
    crate::reasoning_query_bounds::preflight(&normalized, scope, &limits)
}
pub fn encode_request(request: &J) -> Result<Vec<u8>> {
    validate_request(request)?;
    encode_frame(request, "KP4")
}
