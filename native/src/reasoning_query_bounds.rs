//! Conservative resources/v4 accounting over validated captured rows and IR.
use crate::{Error, Result, reasoning_query as Q, require};
use num_bigint::BigInt;
use serde_json::{Value as J, json};
#[derive(Clone, Copy, Default)]
struct Size {
    nodes: usize,
    depth: usize,
    bytes: usize,
    digits: usize,
}
impl Size {
    fn max(self, b: Self) -> Self {
        Self {
            nodes: self.nodes.max(b.nodes),
            depth: self.depth.max(b.depth),
            bytes: self.bytes.max(b.bytes),
            digits: self.digits.max(b.digits),
        }
    }
}
fn size(nodes: usize, depth: usize, bytes: usize, digits: usize) -> Size {
    Size {
        nodes,
        depth,
        bytes,
        digits,
    }
}
fn container(children: &[Size], keys: Option<&[String]>) -> Size {
    size(
        1 + children.iter().map(|s| s.nodes).sum::<usize>(),
        children.iter().map(|s| s.depth + 1).max().unwrap_or(0),
        2 + children
            .iter()
            .enumerate()
            .map(|(i, s)| s.bytes + keys.map_or(1, |keys| 2 + 2 * keys[i].len()))
            .sum::<usize>(),
        children.iter().map(|s| s.digits).max().unwrap_or(0),
    )
}
fn digits(value: &str) -> Result<usize> {
    let unsigned = value.strip_prefix('-').unwrap_or(value);
    let (mantissa, exponent) = unsigned.split_once(['e', 'E']).unwrap_or((unsigned, "0"));
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let mut coefficient = format!("{whole}{fraction}")
        .trim_start_matches('0')
        .to_owned();
    if coefficient.is_empty() {
        return Ok(1);
    }
    if exponent
        .trim_start_matches(['+', '-'])
        .trim_start_matches('0')
        .len()
        > 6
    {
        return Ok(257);
    }
    let exponent = exponent
        .parse::<i64>()
        .map_err(|_| Error("invalid exact number lexeme".into()))?;
    let mut scale = fraction.len() as i64 - exponent;
    while scale > 0 && coefficient.ends_with('0') {
        coefficient.pop();
        scale -= 1;
    }
    let rough = (coefficient.len() as i64 + (-scale).max(0)).max(scale.max(0) + 1);
    if rough > 512 {
        return Ok(257);
    }
    let mut a = coefficient
        .parse::<BigInt>()
        .map_err(|_| Error("invalid exact number lexeme".into()))?;
    let mut b = BigInt::from(1);
    if scale >= 0 {
        b = BigInt::from(10).pow(scale as u32);
    } else {
        a *= BigInt::from(10).pow((-scale) as u32);
    }
    let (mut x, mut y) = (a.clone(), b.clone());
    while y != BigInt::from(0) {
        let r = &x % &y;
        x = y;
        y = r;
    }
    a /= &x;
    b /= &x;
    Ok(a.to_string().len().max(b.to_string().len()))
}
fn typed(v: &J) -> Size {
    match v["type"].as_str().unwrap() {
        "number" => {
            let a = v["numerator"].as_str().unwrap();
            let b = v["denominator"].as_str().unwrap();
            size(
                1,
                0,
                3 + a.len() + b.len(),
                a.trim_start_matches('-').len().max(b.len()),
            )
        }
        "boolean" => size(1, 0, 3, 0),
        "text" => size(1, 0, 2 + 2 * v["value"].as_str().unwrap().len(), 0),
        "null" => size(1, 0, 1, 0),
        "list" => container(
            &v["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(typed)
                .collect::<Vec<_>>(),
            None,
        ),
        _ => {
            let m = v["fields"].as_object().unwrap();
            container(
                &m.values().map(typed).collect::<Vec<_>>(),
                Some(&m.keys().cloned().collect::<Vec<_>>()),
            )
        }
    }
}
fn upper(expr: &J, fields: &J) -> Result<(Size, Size)> {
    if let Some(column) = expr.get("column") {
        let cell = &fields[column.as_str().unwrap()];
        let s = if cell["status"] == "known" {
            typed(&cell["value"])
        } else {
            size(1, 0, 0, 0)
        };
        return Ok((s, s));
    }
    for (key, bytes) in [("bool", 3), ("null", 1)] {
        if expr.get(key).is_some() {
            let s = size(1, 0, bytes, 0);
            return Ok((s, s));
        }
    }
    if let Some(v) = expr.get("num") {
        let d = digits(v.as_str().unwrap())?;
        let s = size(1, 0, 4 + 2 * d, d);
        return Ok((s, s));
    }
    if let Some(v) = expr.get("text") {
        let s = size(1, 0, 2 + 2 * v.as_str().unwrap().len(), 0);
        return Ok((s, s));
    }
    let children = Q::children(expr)
        .into_iter()
        .map(|c| upper(c, fields))
        .collect::<Result<Vec<_>>>()?;
    let peak = children
        .iter()
        .map(|(_, p)| *p)
        .fold(Size::default(), Size::max);
    let output = if let Some(op) = expr.get("op").and_then(J::as_str) {
        match op {
            "add" | "sub" | "mul" | "div" => {
                let d = children.iter().map(|(v, _)| v.digits).sum::<usize>()
                    + usize::from(op == "add" || op == "sub");
                size(1, 0, 4 + 2 * d, d)
            }
            _ => size(1, 0, 3, 0),
        }
    } else if expr.get("if").is_some() {
        children[1].0.max(children[2].0)
    } else if expr.get("list").is_some() {
        container(&children.iter().map(|(s, _)| *s).collect::<Vec<_>>(), None)
    } else if let Some(m) = expr.get("record").and_then(J::as_object) {
        container(
            &children.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            Some(&m.keys().cloned().collect::<Vec<_>>()),
        )
    } else {
        children[0].0
    };
    Ok((output, output.max(peak)))
}
fn execution(expr: &J, fields: &J) -> Result<usize> {
    if ["column", "num", "bool", "text", "null"]
        .iter()
        .any(|k| expr.get(*k).is_some())
    {
        return Ok(1);
    }
    let children = Q::children(expr);
    let steps = children
        .iter()
        .map(|c| execution(c, fields))
        .collect::<Result<Vec<_>>>()?;
    let mut count = if expr.get("if").is_some() {
        1 + steps[0] + steps[1].max(steps[2])
    } else {
        1 + steps.iter().sum::<usize>()
    };
    if expr.get("field").is_some()
        || expr
            .get("op")
            .and_then(J::as_str)
            .is_some_and(|s| s == "eq" || s == "ne")
    {
        for child in children {
            count += upper(child, fields)?.0.nodes;
        }
    }
    Ok(count)
}
pub(crate) fn preflight(operation: &J, scope: &J, resources: &J) -> Result<J> {
    let members = scope["members"].as_array().unwrap();
    let columns = scope["fields"].as_array().unwrap();
    let query = &operation["query"];
    let candidate_count = members.len();
    let field_reads = candidate_count * columns.len();
    let (mut nodes, mut edges, mut depth, mut number_digits) = (0usize, 0usize, 0usize, 0usize);
    let mut pending = ["where", "value"]
        .iter()
        .filter_map(|k| query.get(*k).map(|v| (v, 1usize)))
        .collect::<Vec<_>>();
    while let Some((expr, d)) = pending.pop() {
        nodes += 1;
        depth = depth.max(d);
        if let Some(v) = expr.get("num") {
            number_digits = number_digits.max(digits(v.as_str().unwrap())?);
        }
        let children = Q::children(expr);
        edges += children.len();
        pending.extend(children.into_iter().map(|v| (v, d + 1)));
    }
    let preflight = nodes + edges + field_reads;
    let bound = |key: &str| resources[key].as_u64().unwrap() as usize;
    for (code, actual, max) in [
        ("candidate_limit", candidate_count, bound("candidates")),
        ("field_read_limit", field_reads, bound("field_reads")),
        ("depth_limit", depth, bound("depth")),
        ("digit_limit", number_digits, bound("digits")),
        (
            "step_limit",
            preflight.max(candidate_count * (nodes + 1)),
            bound("steps"),
        ),
    ] {
        require(actual <= max, code)?;
    }
    let mut peak = Size::default();
    let mut values = Vec::new();
    let fake = json!({"id":"","fields":columns.iter().map(|c|(c.as_str().unwrap().to_owned(),json!({"status":"missing"}))).collect::<serde_json::Map<_,_>>()});
    let rows = if members.is_empty() {
        vec![&fake]
    } else {
        members.iter().collect()
    };
    for (i, member) in rows.iter().enumerate() {
        for key in ["where", "value"] {
            if let Some(expr) = query.get(key) {
                let (o, p) = upper(expr, &member["fields"])?;
                peak = peak.max(p);
                if key == "value" && i < members.len() {
                    values.push(o);
                }
            }
        }
    }
    let d = members.len().to_string().len();
    let counter = size(1, 0, 4 + 2 * d, d);
    let raw = match query["op"].as_str().unwrap() {
        "filter" => container(
            &members
                .iter()
                .map(|m| size(1, 0, 2 + 2 * m["id"].as_str().unwrap().len(), 0))
                .collect::<Vec<_>>(),
            None,
        ),
        "project" | "select" => container(&values, None),
        "sum" => {
            let d = values.iter().map(|s| s.digits + 1).sum::<usize>().max(1);
            size(1, 0, 4 + 2 * d, d)
        }
        "count" => counter,
        _ => size(1, 0, 3, 0),
    };
    peak = peak.max(container(
        &[counter, counter, counter, raw, counter, counter],
        Some(
            &[
                "definite_match_count",
                "error_count",
                "input_count",
                "result",
                "unknown_membership_count",
                "unknown_value_count",
            ]
            .map(str::to_owned),
        ),
    ));
    for row in members {
        for cell in row["fields"].as_object().unwrap().values() {
            if cell["status"] == "known" {
                peak = peak.max(typed(&cell["value"]));
            }
        }
    }
    number_digits = number_digits.max(peak.digits);
    let mut step_upper = 0;
    for member in members {
        step_upper += 1;
        for key in ["where", "value"] {
            if let Some(expr) = query.get(key) {
                step_upper += execution(expr, &member["fields"])?;
            }
        }
    }
    for (code, actual, max) in [
        ("value_limit", peak.nodes, bound("value_nodes")),
        ("value_limit", peak.depth, bound("value_depth")),
        ("value_limit", peak.bytes, bound("value_bytes")),
        ("digit_limit", number_digits, bound("digits")),
        ("step_limit", preflight.max(step_upper), bound("steps")),
    ] {
        require(actual <= max, code)?;
    }
    Ok(
        json!({"candidates":candidate_count,"field_reads":field_reads,"preflight_steps":preflight,"step_upper_bound":step_upper}),
    )
}
