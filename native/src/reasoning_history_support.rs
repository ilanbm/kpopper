//! Pure captured support traversal and historical pin decoding. Neither path
//! evaluates a predicate nor reads a source outside the supplied projection.
use crate::{
    Result, history_contract::*, history_projection, history_view::list,
    reasoning_capabilities::compare_basis, reasoning_snapshot::digest, require,
    value::TypedValue as V,
};
use serde_json::json;
use std::collections::BTreeSet;
pub const MAX_VISITS: usize = 100_000;
pub const MAX_PATH_ITEMS: usize = 200_000;
pub const REDUCER: &str = "necessary-support/v1";
pub const ACCEPTANCE: &[&str] = &[
    "accepted",
    "proposed",
    "contested",
    "refuted",
    "corrected",
    "retired",
    "unreviewed",
    "unavailable",
];
pub fn support_state(s: &str) -> bool {
    ACCEPTANCE.contains(&s) || ["moved", "fired", "unknown"].contains(&s)
}
fn s(v: &str) -> V {
    V::Text(v.into())
}
fn val(j: serde_json::Value) -> Result<V> {
    V::from_json_bounded(&j, 16 * 1024 * 1024 / 8)
}
fn strings(v: &V) -> Result<Vec<String>> {
    list(v)?
        .iter()
        .map(|v| text(v).map(str::to_owned))
        .collect()
}
#[derive(Clone, Debug)]
pub struct SupportBudget {
    pub remaining: usize,
    pub path_items: usize,
}
impl Default for SupportBudget {
    fn default() -> Self {
        Self {
            remaining: MAX_VISITS,
            path_items: MAX_PATH_ITEMS,
        }
    }
}
fn reservation(
    code: &str,
    state: &str,
    subject: &str,
    version: &str,
    path: &[String],
) -> Result<V> {
    val(json!({"code":code,"state":state,"subject":subject,"version":version,"path":path}))
}
pub fn reduce_support_graph(
    roots: &[String],
    graph: &V,
    outcomes: &V,
    max_visits: usize,
    budget: &mut SupportBudget,
) -> Result<V> {
    require(
        max_visits > 0 && max_visits <= MAX_VISITS,
        "invalid support visit limit",
    )?;
    let graph = map(graph)?;
    let outcomes = map(outcomes)?;
    let mut visited = BTreeSet::new();
    let mut reservations = Vec::new();
    let mut reservation_ids = BTreeSet::new();
    fn reserve(
        item: V,
        budget: &mut SupportBudget,
        ids: &mut BTreeSet<String>,
        items: &mut Vec<V>,
    ) -> Result<()> {
        let cost = list(&map(&item)?["path"])?.len();
        require(cost <= budget.path_items, "support_limit")?;
        let marker = digest(&item)?;
        if ids.insert(marker) {
            budget.path_items -= cost;
            items.push(item)
        }
        Ok(())
    }
    for root in roots {
        let mut stack = vec![(false, root.clone())];
        let mut active = BTreeSet::new();
        let mut path = Vec::new();
        while let Some((leave, current)) = stack.pop() {
            if leave {
                active.remove(&current);
                require(path.last() == Some(&current), "invalid support traversal")?;
                path.pop();
                continue;
            }
            if active.contains(&current) {
                let (subject, version) = current.split_once('@').unwrap_or((&current, ""));
                let mut p = path.clone();
                p.push(current.clone());
                reserve(
                    reservation("support_cycle", "unavailable", subject, version, &p)?,
                    budget,
                    &mut reservation_ids,
                    &mut reservations,
                )?;
                continue;
            }
            if visited.contains(&current) {
                continue;
            }
            require(
                visited.len() < max_visits && budget.remaining > 0,
                "support_limit",
            )?;
            visited.insert(current.clone());
            budget.remaining -= 1;
            let Some(V::Map(item)) = graph.get(&current) else {
                let (subject, version) = current.split_once('@').unwrap_or((&current, ""));
                let mut p = path.clone();
                p.push(current.clone());
                reserve(
                    reservation("missing_support", "unavailable", subject, version, &p)?,
                    budget,
                    &mut reservation_ids,
                    &mut reservations,
                )?;
                continue;
            };
            let subject = text(field(item, "subject")?)?;
            let version = text(field(item, "version")?)?;
            let state = text(field(item, "state")?)?;
            let dependencies = list(field(item, "dependencies")?)?;
            require(
                current == format!("{subject}@{version}") && support_state(state),
                "invalid support graph node",
            )?;
            active.insert(current.clone());
            path.push(current.clone());
            stack.push((true, current));
            if state != "accepted" {
                reserve(
                    reservation("support_state", state, subject, version, &path)?,
                    budget,
                    &mut reservation_ids,
                    &mut reservations,
                )?;
            }
            let outcomes = match outcomes.get(subject) {
                None => vec![],
                Some(V::Text(s)) => vec![s.clone()],
                Some(v) => strings(v)?,
            };
            for outcome in outcomes {
                require(
                    ["fired", "moved", "unknown", "unavailable"].contains(&outcome.as_str()),
                    "invalid support outcome",
                )?;
                reserve(
                    reservation(
                        &format!("current_{outcome}"),
                        &outcome,
                        subject,
                        version,
                        &path,
                    )?,
                    budget,
                    &mut reservation_ids,
                    &mut reservations,
                )?;
            }
            let mut children = Vec::new();
            for d in dependencies {
                let d = schema(d, &["subject", "version"], &[])?;
                children.push(format!("{}@{}", text(&d["subject"])?, text(&d["version"])?))
            }
            children.sort();
            for child in children.into_iter().rev() {
                stack.push((false, child));
            }
        }
    }
    reservations.sort_by_key(|v| {
        let m = map(v).unwrap();
        (
            strings(&m["path"]).unwrap(),
            text(&m["code"]).unwrap().to_owned(),
            text(&m["state"]).unwrap().to_owned(),
        )
    });
    let mut result=map(&val(json!({"reducer":REDUCER,"status":if reservations.is_empty(){"clear"}else{"reserved"},"visited":visited,"visited_count":visited.len(),"reservations":[]}))?)?.clone();
    result.insert("reservations".into(), V::List(reservations));
    Ok(V::Map(result))
}
fn finding(code: &str, subject: &str, version: &str, detail: &str) -> Result<V> {
    val(json!({"code":code,"subject":subject,"object_id":version,"detail":detail}))
}
fn literal(value: &V) -> Result<V> {
    if matches!(value,V::Map(m)if m.contains_key("type")) {
        crate::reasoning_values::validate(value)?;
        return Ok(value.clone());
    }
    require(
        !matches!(value, V::List(_))
            && !matches!(value,V::Map(m)if m.len()!=1||!m.contains_key("rational")),
        "unsupported stored value",
    )?;
    let value = crate::reasoning_scope::query_value(value, 0)?;
    require(
        !["list", "record"]
            .iter()
            .any(|t| string_is(&map(&value).unwrap()["type"], t)),
        "unsupported stored value",
    )?;
    Ok(value)
}
pub fn pin_review_evidence(projection: &V, version: &str, subject: Option<&str>) -> Result<V> {
    history_projection::validate_projection(projection)?;
    require(
        crate::history_paths::object_id(version),
        "invalid_identifier",
    )?;
    let projection = map(projection)?;
    let witness = map(&projection["pins"])?
        .get(version)
        .map(map)
        .transpose()?;
    let subject = if let Some(w) = witness {
        let observed = text(&w["subject"])?;
        require(subject.is_none_or(|s| s == observed), "reference_mismatch")?;
        Some(observed)
    } else {
        subject
    };
    let findings = list(&map(&projection["integrity"])?["findings"])?
        .iter()
        .filter(|v| {
            map(v)
                .ok()
                .and_then(|m| m.get("object_id"))
                .is_some_and(|v| string_is(v, version))
        })
        .cloned()
        .collect();
    let mut result=map(&val(json!({"status":"unavailable","evidence_kind":"version_pin","subject":subject,"version":version,"body":null,"profile":null,"value_status":"unavailable","value":null,"basis_status":"not_recorded","basis":null,"findings":[]}))?)?.clone();
    result.insert("findings".into(), V::List(findings));
    let add = |result: &mut Map, code: &str, detail: &str| -> Result<()> {
        let V::List(findings) = result.get_mut("findings").unwrap() else {
            unreachable!()
        };
        findings.push(finding(code, subject.unwrap_or(""), version, detail)?);
        Ok(())
    };
    let Some(w) = witness.filter(|w| string_is(&w["status"], "recorded")) else {
        let code = witness
            .map(|w| format!("pin_{}", text(&w["status"]).unwrap()))
            .unwrap_or_else(|| "missing_pin".into());
        add(&mut result, &code, "pin evidence is unavailable")?;
        return Ok(V::Map(result));
    };
    let obj = map(&w["object"])?;
    result.insert("status".into(), s("recorded"));
    result.insert("body".into(), obj["body"].clone());
    let Some(authored) = obj.get("authored").filter(|v| **v != V::Null) else {
        add(
            &mut result,
            "unresolved_authored_mapping",
            "recorded profile and value role are unavailable",
        )?;
        return Ok(V::Map(result));
    };
    if let Err(e) = require_interpretable_claim(&w["object"]) {
        result.insert("basis_status".into(), s("unavailable"));
        add(&mut result, &e.0, "original condition profile is unknown")?;
        return Ok(V::Map(result));
    }
    let authored = map(authored)?;
    result.insert("profile".into(), authored["profile"].clone());
    let body = &obj["body"];
    let read = |result: &mut Map, value: &V, detail: &str| -> Result<()> {
        match literal(value) {
            Ok(v) => {
                result.insert("value_status".into(), s("recorded"));
                result.insert("value".into(), v);
            }
            Err(_) => add(result, "unsupported_history_value", detail)?,
        }
        Ok(())
    };
    let V::Map(body) = body else {
        read(
            &mut result,
            body,
            "stored scalar is outside the computation value profile",
        )?;
        return Ok(V::Map(result));
    };
    let field = map(&authored["fields"])?
        .get("value")
        .and_then(|v| text(v).ok());
    let stored = if body.contains_key("computed") {
        Some(body)
    } else {
        field.and_then(|f| body.get(f)).and_then(|v| map(v).ok())
    };
    if let Some(computed) = stored
        .filter(|m| m.contains_key("computed"))
        .map(|m| &m["computed"])
    {
        let decoded = (|| -> Result<(V, V)> {
            let c = schema(computed, &["version", "value", "basis"], &["rule"])?;
            require(
                is_int(&c["version"], "2") && string_is(&authored["profile"], "core/v1"),
                "invalid_history",
            )?;
            crate::reasoning_values::validate(&c["value"])?;
            require(
                compare_basis(&c["basis"], &c["basis"])? == "same",
                "invalid_history",
            )?;
            Ok((c["value"].clone(), c["basis"].clone()))
        })();
        match decoded {
            Ok((v, b)) => {
                result.insert("value_status".into(), s("recorded"));
                result.insert("value".into(), v);
                result.insert("basis_status".into(), s("recorded"));
                result.insert("basis".into(), b);
            }
            Err(_) => {
                result.insert("basis_status".into(), s("unavailable"));
                add(
                    &mut result,
                    "invalid_history",
                    "stored computation cannot be decoded under its profile",
                )?;
            }
        }
        return Ok(V::Map(result));
    }
    if !body.contains_key("rule")
        && string_is(&obj["kind"], "reading")
        && let Some(v) = field.and_then(|f| body.get(f))
    {
        read(
            &mut result,
            v,
            "stored value is not a supported typed scalar",
        )?;
    }
    Ok(V::Map(result))
}
