//! Pure consumer projections. Attention and clipping cannot change findings or
//! potential dependency edges; no computation or source collection occurs here.
use crate::{
    Error, Result,
    history_contract::{Map, map, text},
    history_view::{list, truth},
    reasoning_language as L, reasoning_query as Q, require,
    value::TypedValue as V,
};
use serde_json::{Value as J, json};
use std::collections::{BTreeMap, BTreeSet};
pub const VERSION: &str = "reasoning-projection/v1";
fn val(j: J) -> Result<V> {
    V::from_json_bounded(&j, 64 * 1024 * 1024 / 8)
}
fn s(v: &str) -> V {
    V::Text(v.into())
}
fn mapping(v: &V) -> Result<Map> {
    if *v == V::Null {
        Ok(Map::new())
    } else {
        Ok(map(v)?.clone())
    }
}
fn get<'a>(m: &'a Map, key: &str) -> &'a V {
    m.get(key).unwrap_or(&V::Null)
}
fn dimension<'a>(n: &'a Map, state: &'a Map, k: &str) -> &'a V {
    n.get(k).unwrap_or_else(|| get(state, k))
}
fn status(v: &V, default: &str) -> String {
    if let V::Text(v) = v {
        return v.clone();
    }
    if let V::Map(m) = v {
        if let Some(V::Text(v)) = m.get("status") {
            return v.clone();
        }
        if let Some(V::Bool(v)) = m.get("complete") {
            return if *v { "complete" } else { "incomplete" }.into();
        }
    }
    default.into()
}
pub fn render_value(v: &V) -> Result<String> {
    crate::reasoning_values::validate(v)?;
    fn render(v: &V) -> Result<String> {
        let m = map(v)?;
        Ok(match text(&m["type"])? {
            "number" => {
                if m["denominator"] == s("1") {
                    text(&m["numerator"])?.into()
                } else {
                    format!("{}/{}", text(&m["numerator"])?, text(&m["denominator"])?)
                }
            }
            "boolean" => if truth(&m["value"]) { "true" } else { "false" }.into(),
            "text" => serde_json::to_string(text(&m["value"])?)?,
            "null" => "null".into(),
            "list" => format!(
                "[{}]",
                list(&m["items"])?
                    .iter()
                    .map(render)
                    .collect::<Result<Vec<_>>>()?
                    .join(", ")
            ),
            "record" => format!(
                "{{{}}}",
                map(&m["fields"])?
                    .iter()
                    .map(|(k, v)| Ok(format!("{}: {}", serde_json::to_string(k)?, render(v)?)))
                    .collect::<Result<Vec<_>>>()?
                    .join(", ")
            ),
            _ => unreachable!(),
        })
    }
    render(v)
}
fn reference(id: &str) -> Result<String> {
    let keywords = [
        "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class",
        "continue", "def", "del", "elif", "else", "except", "finally", "for", "from", "global",
        "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return",
        "try", "while", "with", "yield",
    ];
    if id
        .split('.')
        .all(|p| crate::python_identifiers::identifier(p) && !keywords.contains(&p))
        && !["true", "false", "True", "False", "null", "None"].contains(&id)
    {
        return Ok(id.into());
    }
    Ok(format!("ref({})", serde_json::to_string(id)?))
}
pub fn render_expression(expression: &V) -> Result<String> {
    if matches!(expression,V::Map(m)if m.len()==1&&m.contains_key("query")) {
        let j = expression.to_json()?;
        let mut pending = vec![];
        if let Some(m) = j["query"].as_object() {
            for k in ["where", "value"] {
                if let Some(v) = m.get(k) {
                    pending.push(v);
                }
            }
        }
        let mut columns = BTreeSet::new();
        while let Some(v) = pending.pop() {
            if let Some(c) = v.get("column").and_then(J::as_str) {
                columns.insert(c.to_owned());
            }
            pending.extend(Q::children(v));
        }
        return Ok(serde_json::to_string(&Q::lower(
            &j,
            &columns.into_iter().collect::<Vec<_>>(),
        )?)?);
    }
    let tree = L::lower(expression)?;
    if let V::Map(m) = expression
        && m.len() == 1
        && m.contains_key("expr")
    {
        return Ok(text(&m["expr"])?
            .trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
            .into());
    }
    fn render(n: &J) -> Result<String> {
        if let Some(v) = n.get("ref").and_then(J::as_str) {
            return reference(v);
        }
        if let Some(v) = n.get("num").and_then(J::as_str) {
            return Ok(v.into());
        }
        if let Some(v) = n.get("bool").and_then(J::as_bool) {
            return Ok(if v { "true" } else { "false" }.into());
        }
        if let Some(v) = n.get("text").and_then(J::as_str) {
            return Ok(serde_json::to_string(v)?);
        }
        if n.get("null").is_some() {
            return Ok("null".into());
        }
        if let Some(a) = n.get("list").and_then(J::as_array) {
            return Ok(format!(
                "[{}]",
                a.iter().map(render).collect::<Result<Vec<_>>>()?.join(", ")
            ));
        }
        if let Some(m) = n.get("record").and_then(J::as_object) {
            return Ok(format!(
                "{{{}}}",
                m.iter()
                    .map(|(k, v)| Ok(format!("{}: {}", serde_json::to_string(k)?, render(v)?)))
                    .collect::<Result<Vec<_>>>()?
                    .join(", ")
            ));
        }
        if let Some(v) = n.get("field") {
            return Ok(format!(
                "field({}, {})",
                render(v)?,
                serde_json::to_string(&n["key"])?
            ));
        }
        if let Some(v) = n.get("if") {
            return Ok(format!(
                "({} if {} else {})",
                render(&n["then"])?,
                render(v)?,
                render(&n["else"])?
            ));
        }
        let op = n["op"]
            .as_str()
            .ok_or_else(|| Error("invalid rendered operator".into()))?;
        let args = n["args"]
            .as_array()
            .ok_or_else(|| Error("invalid rendered operands".into()))?;
        if op == "not" {
            return Ok(format!("(not {})", render(&args[0])?));
        }
        let symbol = match op {
            "add" => "+",
            "sub" => "-",
            "mul" => "*",
            "div" => "/",
            "eq" => "==",
            "ne" => "!=",
            "lt" => "<",
            "le" => "<=",
            "gt" => ">",
            "ge" => ">=",
            "and" => "and",
            "or" => "or",
            _ => return Err(Error("invalid rendered operator".into())),
        };
        Ok(format!(
            "({} {symbol} {})",
            render(&args[0])?,
            render(&args[1])?
        ))
    }
    render(&tree)
}
pub fn predicate_truth(v: &V) -> V {
    match text(v).ok() {
        Some("holds") => V::Bool(true),
        Some("does_not_hold") => V::Bool(false),
        _ => V::Null,
    }
}
pub fn computation_truth(v: &V) -> Result<V> {
    let m = mapping(v)?;
    if get(&m, "status") == &s("ok")
        && let Some(V::Map(value)) = m.get("value")
        && get(value, "type") == &s("boolean")
        && matches!(get(value, "value"), V::Bool(_))
    {
        return Ok(value["value"].clone());
    }
    Ok(V::Null)
}
fn assurance_status(a: &Map, c: &Map) -> String {
    let mut kinds = BTreeSet::new();
    if let Some(V::Map(actual)) = a.get("actual") {
        let mut items = vec![get(actual, "node"), get(actual, "falsifier")];
        if let Some(V::Map(d)) = actual.get("dependencies") {
            items.extend(d.values())
        }
        for item in items {
            if let Some(V::Text(kind)) = map(item).ok().and_then(|m| m.get("kind")) {
                kinds.insert(kind.clone());
            }
        }
    } else if let Some(V::Text(kind)) = a.get("kind") {
        kinds.insert(kind.clone());
    }
    if kinds.is_empty()
        && let Some(V::Text(kind)) = c
            .get("assurance")
            .and_then(|v| map(v).ok())
            .and_then(|m| m.get("kind"))
    {
        kinds.insert(kind.clone());
    }
    match kinds.len() {
        0 => "not_available".into(),
        1 => kinds.into_iter().next().unwrap(),
        _ => "mixed".into(),
    }
}
pub fn project_node_status(node: &V) -> Result<V> {
    let n = mapping(node)?;
    let state = mapping(get(&n, "state"))?;
    let c = mapping(dimension(&n, &state, "computation"))?;
    let falsifier = mapping(get(&state, "falsifier"))?;
    let value_text = if get(&c, "status") == &s("ok") && get(&c, "value") != &V::Null {
        V::Text(render_value(&c["value"])?)
    } else {
        V::Null
    };
    let a = dimension(&n, &state, "assurance");
    let a = mapping(if *a == V::Null {
        get(&c, "assurance")
    } else {
        a
    })?;
    let coverage = dimension(&n, &state, "coverage");
    let included = map(coverage)
        .ok()
        .and_then(|m| m.get("included"))
        .filter(|v| matches!(v, V::Bool(_)))
        .cloned()
        .unwrap_or(V::Null);
    let kinds = match a.get("recorded_evidence_kinds") {
        Some(V::List(a)) if a.iter().all(|v| matches!(v, V::Text(_))) => a
            .iter()
            .map(|v| text(v).unwrap().to_owned())
            .collect::<BTreeSet<_>>(),
        _ => BTreeSet::new(),
    };
    let support = mapping(dimension(&n, &state, "support"))?;
    let empty = V::List(vec![]);
    let reservations = list(support.get("reservations").unwrap_or(&empty))?;
    let values = |field: &str| {
        reservations
            .iter()
            .filter_map(|r| {
                map(r)
                    .ok()
                    .and_then(|m| m.get(field))
                    .and_then(|v| text(v).ok())
            })
            .collect::<BTreeSet<_>>()
    };
    let mut result=map(&val(json!({"acceptance":status(dimension(&n,&state,"acceptance"),"not_applicable"),"computation":{"status":c.get("status").map(|v|v.to_json()).transpose()?.unwrap_or(json!("not_available")),"truth":null,"value_text":null},"basis":status(get(&state,"basis"),"not_available"),"falsifier":{"status":falsifier.get("status").map(|v|v.to_json()).transpose()?.unwrap_or(json!("not_available")),"holds":null},"contention":status(get(&state,"contention"),"not_available"),"integrity":status(get(&state,"integrity"),"not_available"),"coverage":status(coverage,"not_available"),"coverage_included":null,"assurance":assurance_status(&a,&c),"recorded_evidence_kinds":kinds,"support":{"status":support.get("status").map(|v|v.to_json()).transpose()?.unwrap_or(json!("not_available")),"states":values("state"),"codes":values("code")}}))?)?.clone();
    let V::Map(computation) = result.get_mut("computation").unwrap() else {
        unreachable!()
    };
    computation.insert("truth".into(), computation_truth(&V::Map(c))?);
    computation.insert("value_text".into(), value_text);
    let V::Map(f) = result.get_mut("falsifier").unwrap() else {
        unreachable!()
    };
    f.insert("holds".into(), predicate_truth(get(&falsifier, "status")));
    result.insert("coverage_included".into(), included);
    Ok(V::Map(result))
}
pub fn render_node_status(node: &V) -> Result<String> {
    let status = project_node_status(node)?;
    let m = map(&status)?;
    let c = map(&m["computation"])?;
    let mut computation = text(&c["status"])?.to_owned();
    if c["value_text"] != V::Null {
        computation.push('=');
        computation.push_str(text(&c["value_text"])?)
    }
    let support = map(&m["support"])?;
    let states = list(&support["states"])?
        .iter()
        .map(|v| text(v))
        .collect::<Result<Vec<_>>>()?;
    Ok(format!(
        "acceptance={}; computation={}; basis={}; falsifier={}; contention={}; integrity={}; coverage={}; assurance={}; support={}{}",
        text(&m["acceptance"])?,
        computation,
        text(&m["basis"])?,
        text(&map(&m["falsifier"])?["status"])?,
        text(&m["contention"])?,
        text(&m["integrity"])?,
        text(&m["coverage"])?,
        text(&m["assurance"])?,
        text(&support["status"])?,
        if states.is_empty() {
            String::new()
        } else {
            format!("[{}]", states.join(","))
        }
    ))
}
fn witness_ids(v: &V) -> Result<BTreeSet<String>> {
    if *v == V::Null {
        return Ok(BTreeSet::new());
    }
    let mut out = BTreeSet::new();
    for v in list(v)? {
        let id = if let V::Map(m) = v {
            m.get("id")
                .filter(|v| truth(v))
                .or_else(|| m.get("scope_id"))
                .unwrap_or(&V::Null)
        } else {
            v
        };
        let id = text(id)?;
        require(!id.is_empty(), "invalid projected witness")?;
        out.insert(id.into());
    }
    Ok(out)
}
pub fn project_witnesses(computation: &V) -> Result<V> {
    let c = mapping(computation)?;
    let mut potential = witness_ids(get(&c, "potential_dependencies"))?;
    potential.extend(witness_ids(get(&c, "potential_ids"))?);
    let executed = witness_ids(get(&c, "executed_reads"))?;
    potential.extend(executed.iter().cloned());
    let unexecuted = potential.difference(&executed).cloned().collect::<Vec<_>>();
    let rows=potential.iter().map(|id|json!({"id":id,"classification":if executed.contains(id){"executed"}else{"potential"}})).collect::<Vec<_>>();
    val(
        json!({"potential":potential,"executed":executed,"unexecuted":unexecuted,"dependencies":rows}),
    )
}
fn computations(node: &V) -> Result<Vec<(String, V)>> {
    let n = mapping(node)?;
    let state = n
        .get("state")
        .and_then(|v| map(v).ok())
        .cloned()
        .unwrap_or_default();
    let mut out = vec![];
    if let v @ V::Map(_) = dimension(&n, &state, "computation") {
        out.push(("value".into(), v.clone()))
    }
    if let Some(v @ V::Map(_)) = state
        .get("falsifier")
        .and_then(|v| map(v).ok())
        .and_then(|m| m.get("computation"))
    {
        out.push(("falsifier".into(), v.clone()))
    }
    if let Some(deps) = state
        .get("basis")
        .and_then(|v| map(v).ok())
        .and_then(|m| m.get("dependencies"))
        .and_then(|v| map(v).ok())
    {
        for (id, d) in deps {
            if let Some(v @ V::Map(_)) = map(d).ok().and_then(|m| m.get("computation")) {
                out.push((format!("basis:{id}"), v.clone()))
            }
        }
    }
    Ok(out)
}
pub fn project_node_impacts(node: &V) -> Result<V> {
    let mut by_id = BTreeMap::<String, Vec<(String, String)>>::new();
    for (context, c) in computations(node)? {
        let w = project_witnesses(&c)?;
        for row in list(&map(&w)?["dependencies"])? {
            let row = map(row)?;
            by_id
                .entry(text(&row["id"])?.into())
                .or_default()
                .push((context.clone(), text(&row["classification"])?.into()));
        }
    }
    let rows=by_id.into_iter().map(|(id,mut ws)|{ws.sort();json!({"id":id,"classification":if ws.iter().any(|(_,c)|c=="executed"){"executed"}else{"potential"},"witnesses":ws.into_iter().map(|(context,classification)|json!({"context":context,"classification":classification})).collect::<Vec<_>>()})}).collect::<Vec<_>>();
    val(json!(rows))
}
pub fn project_impacts(nodes: &V) -> Result<V> {
    let mut edges = vec![];
    for (consumer, node) in mapping(nodes)? {
        map(&node)?;
        for impact in list(&project_node_impacts(&node)?)? {
            let impact = map(impact)?;
            if impact["id"] == s(&consumer) {
                continue;
            }
            edges.push(V::Map(Map::from([
                ("from".into(), impact["id"].clone()),
                ("to".into(), s(&consumer)),
                ("classification".into(), impact["classification"].clone()),
                ("witnesses".into(), impact["witnesses"].clone()),
            ])));
        }
    }
    Ok(V::List(edges))
}
pub fn project_findings(report: &V) -> Result<V> {
    let r = mapping(report)?;
    let nodes = mapping(get(&r, "nodes"))?;
    let mut projected = Map::new();
    for (id, node) in &nodes {
        projected.insert(
            id.clone(),
            V::Map(Map::from([
                ("status".into(), project_node_status(node)?),
                ("status_text".into(), s(&render_node_status(node)?)),
                ("dependencies".into(), project_node_impacts(node)?),
            ])),
        );
    }
    Ok(V::Map(Map::from([
        ("version".into(), s(VERSION)),
        ("snapshot_id".into(), get(&r, "snapshot_id").clone()),
        (
            "findings_revision".into(),
            get(&r, "findings_revision").clone(),
        ),
        ("nodes".into(), V::Map(projected)),
        ("impacts".into(), project_impacts(&V::Map(nodes))?),
    ])))
}
