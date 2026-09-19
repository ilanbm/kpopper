//! Bounded, pure, three-valued followup trigger evaluation.
use crate::{Error, Result, require};
use chrono::{DateTime, LocalResult, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use num_bigint::BigInt;
use serde_json::{Map, Value, json};
use std::{cmp::Ordering, collections::BTreeSet, str::FromStr};

pub const MAX_DEPTH: usize = 8;
pub const MAX_NODES: usize = 100;

#[derive(Clone, Debug, Default)]
pub struct Evaluation {
    pub value: Option<bool>,
    pub inputs: Map<String, Value>,
    pub reasons: Vec<String>,
    pub next_at: Option<DateTime<Utc>>,
}

pub fn stamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
}

pub fn parse_time(value: &str, zone: &str) -> Result<DateTime<Utc>> {
    if value.len() == 10 {
        let date = NaiveDate::parse_from_str(value, "%Y-%m-%d")
            .map_err(|e| Error(format!("invalid time or timezone: {e}")))?;
        let timezone =
            Tz::from_str(zone).map_err(|e| Error(format!("invalid time or timezone: {e}")))?;
        let local = date
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| Error("invalid time or timezone".into()))?;
        return match timezone.from_local_datetime(&local) {
            LocalResult::Single(value) | LocalResult::Ambiguous(value, _) => {
                Ok(value.with_timezone(&Utc))
            }
            LocalResult::None => Err(Error(
                "invalid time or timezone: nonexistent local time".into(),
            )),
        };
    }
    let mut normalized = value.replace('t', "T");
    if normalized.ends_with(['z', 'Z']) {
        normalized.truncate(normalized.len() - 1);
        normalized.push_str("+00:00");
    }
    if let Some(at) = normalized.find(' ') {
        normalized.replace_range(at..=at, "T");
    }
    DateTime::parse_from_rfc3339(&normalized)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|e| Error(format!("invalid time or timezone: {e}")))
}

fn text(value: &Value, label: &str) -> Result<String> {
    let value = value
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| Error(format!("{label} must be a nonempty string")))?;
    Ok(value.to_owned())
}

fn walk(root: &Value) -> Result<Vec<(&str, &Value, String)>> {
    let mut result = vec![];
    let mut stack = vec![(root, 1usize, "0".to_owned())];
    while let Some((node, depth, path)) = stack.pop() {
        require(
            depth <= MAX_DEPTH && result.len() < MAX_NODES,
            "trigger exceeds depth 8 or size 100",
        )?;
        let object = node
            .as_object()
            .filter(|object| object.len() == 1)
            .ok_or_else(|| Error("each trigger must be a mapping with exactly one key".into()))?;
        let (kind, value) = object.iter().next().unwrap();
        if matches!(kind.as_str(), "all" | "any") {
            let children = value
                .as_array()
                .filter(|children| !children.is_empty())
                .ok_or_else(|| Error(format!("{kind} requires a nonempty child list")))?;
            require(
                children.len() < MAX_NODES - result.len(),
                "trigger exceeds size 100",
            )?;
            for (index, child) in children.iter().enumerate().rev() {
                stack.push((child, depth + 1, format!("{path}.{index}")));
            }
        } else {
            require(
                matches!(
                    kind.as_str(),
                    "at" | "changed" | "condition" | "completed" | "external" | "manual"
                ),
                &format!("unknown trigger kind: {kind}"),
            )?;
        }
        result.push((kind.as_str(), value, path));
    }
    Ok(result)
}

pub fn validate(trigger: &Value, related: &[String]) -> Result<()> {
    let related = related.iter().collect::<BTreeSet<_>>();
    for (kind, value, _) in walk(trigger)? {
        match kind {
            "at" => {
                parse_time(
                    value
                        .as_str()
                        .ok_or_else(|| Error("at must be a time".into()))?,
                    "UTC",
                )?;
            }
            "changed" | "completed" | "manual" => {
                let identity = text(value, kind)?;
                if kind == "changed" {
                    require(
                        related.contains(&identity),
                        &format!("graph reference must be declared in related: {identity}"),
                    )?;
                }
            }
            "condition" => {
                let object = value
                    .as_object()
                    .ok_or_else(|| Error("condition requires exactly id, op and value".into()))?;
                require(
                    object.len() == 3
                        && ["id", "op", "value"]
                            .iter()
                            .all(|key| object.contains_key(*key)),
                    "condition requires exactly id, op and value",
                )?;
                let identity = text(&object["id"], "condition id")?;
                require(
                    related.contains(&identity),
                    &format!("graph reference must be declared in related: {identity}"),
                )?;
                let op = object["op"].as_str().unwrap_or("");
                require(
                    ["==", "!=", "<", "<=", ">", ">="].contains(&op),
                    "unknown condition operator",
                )?;
                require(
                    !object["value"].is_array()
                        && (!object["value"].is_object() || decimal(&object["value"]).is_some()),
                    "condition comparison value must be scalar",
                )?;
                if ["<", "<=", ">", ">="].contains(&op) {
                    require(
                        decimal(&object["value"]).is_some(),
                        "ordering requires a finite numeric comparison value",
                    )?;
                }
            }
            "external" => {
                let object = value.as_object().ok_or_else(|| {
                    Error("external requires ref, equals and optional max_age_hours".into())
                })?;
                require(
                    object.contains_key("ref")
                        && object.contains_key("equals")
                        && object
                            .keys()
                            .all(|key| ["ref", "equals", "max_age_hours"].contains(&key.as_str())),
                    "external requires ref, equals and optional max_age_hours",
                )?;
                text(&object["ref"], "external ref")?;
                let age = object.get("max_age_hours").unwrap_or(&json!(24)).as_f64();
                require(
                    age.is_some_and(|age| age.is_finite() && age > 0.0),
                    "max_age_hours must be a positive finite number",
                )?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn references(trigger: &Value, target: &str) -> Result<BTreeSet<String>> {
    let mut result = BTreeSet::new();
    for (kind, value, _) in walk(trigger)? {
        match (target, kind) {
            ("graph", "changed") | ("tasks", "completed") => {
                result.insert(text(value, kind)?);
            }
            ("graph", "condition") => {
                result.insert(text(&value["id"], "condition id")?);
            }
            ("external", "external") => {
                result.insert(text(&value["ref"], "external ref")?);
            }
            _ => {}
        }
    }
    Ok(result)
}

pub fn referenced_tasks(trigger: &Value) -> Result<BTreeSet<String>> {
    references(trigger, "tasks")
}
pub fn referenced_graph(trigger: &Value) -> Result<BTreeSet<String>> {
    references(trigger, "graph")
}
pub fn referenced_external(trigger: &Value) -> Result<BTreeSet<String>> {
    references(trigger, "external")
}

#[derive(Clone, Debug)]
struct Decimal {
    coefficient: BigInt,
    scale: i64,
}
fn decimal(value: &Value) -> Option<Decimal> {
    let source = value.as_number()?.to_string();
    let (mantissa, exponent) = if let Some((mantissa, exponent)) = source.split_once(['e', 'E']) {
        (mantissa, exponent.parse().ok()?)
    } else {
        (source.as_str(), 0i64)
    };
    let negative = mantissa.starts_with('-');
    let mantissa = mantissa.trim_start_matches('-');
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = format!("{whole}{fraction}");
    let mut coefficient = BigInt::parse_bytes(digits.as_bytes(), 10)?;
    if negative {
        coefficient = -coefficient;
    }
    Some(Decimal {
        coefficient,
        scale: fraction.len() as i64 - exponent,
    })
}
fn compare_numbers(left: &Value, right: &Value) -> Option<Ordering> {
    let left = decimal(left)?;
    let right = decimal(right)?;
    let scale = left.scale.max(right.scale);
    let a = left.coefficient * BigInt::from(10u8).pow((scale - left.scale) as u32);
    let b = right.coefficient * BigInt::from(10u8).pow((scale - right.scale) as u32);
    Some(a.cmp(&b))
}
fn equal(left: &Value, right: &Value) -> bool {
    if let Some(ordering) = compare_numbers(left, right) {
        return ordering == Ordering::Equal;
    }
    match (left, right) {
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| equal(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, value)| b.get(key).is_some_and(|other| equal(value, other)))
        }
        _ => left == right,
    }
}

fn core_scalar(value: &Value) -> Option<Value> {
    let core = value.as_object()?.get("core")?.as_object()?;
    if core.get("version") != Some(&json!(1)) || core.get("available") != Some(&json!(true)) {
        return None;
    }
    let typed = core.get("value")?.as_object()?;
    match typed.get("type")?.as_str()? {
        "number" => Some(json!({"rational":[typed.get("numerator")?,typed.get("denominator")?]})),
        "text" | "boolean" => typed.get("value").cloned(),
        "null" => Some(Value::Null),
        _ => typed.get("value").cloned(),
    }
}
pub fn unavailable(value: Option<&Value>, core: bool) -> bool {
    let Some(Value::Object(object)) = value else {
        return false;
    };
    if core {
        return object
            .get("core")
            .and_then(Value::as_object)
            .and_then(|core| core.get("available"))
            .and_then(Value::as_bool)
            != Some(true);
    }
    object.len() == 1 && object.get("unavailable").is_some_and(Value::is_string)
        || object.len() == 1
            && object
                .get("computed")
                .and_then(Value::as_object)
                .is_none_or(|computed| computed.get("value").is_none_or(Value::is_null))
}

struct Context<'a> {
    values: &'a Map<String, Value>,
    baseline: &'a Map<String, Value>,
    completed: &'a BTreeSet<String>,
    observations: &'a Map<String, Value>,
    now: DateTime<Utc>,
    zone: &'a str,
    current_core: &'a BTreeSet<String>,
    baseline_core: &'a BTreeSet<String>,
    inputs: Map<String, Value>,
    reasons: Vec<String>,
    wakes: Vec<DateTime<Utc>>,
}
impl Context<'_> {
    fn snapshot(&mut self, group: &str, identity: &str, baseline: bool) -> Option<Value> {
        let mapping = if baseline { self.baseline } else { self.values };
        let is_core = if baseline {
            self.baseline_core
        } else {
            self.current_core
        }
        .contains(identity);
        let raw = mapping.get(identity);
        let available = raw.is_some() && !unavailable(raw, is_core);
        let mut state = Map::new();
        state.insert("available".into(), json!(available));
        if let Some(value) = raw {
            if available {
                state.insert("value".into(), value.clone());
            } else {
                state.insert("detail".into(), value.clone());
            }
        }
        self.inputs.entry(group).or_insert_with(|| json!({}))[identity] = Value::Object(state);
        raw.filter(|_| available).cloned()
    }
    fn visit(&mut self, node: &Value, path: &str) -> Result<Option<bool>> {
        let (kind, spec) = node
            .as_object()
            .and_then(|m| m.iter().next())
            .ok_or_else(|| Error("invalid trigger".into()))?;
        match kind.as_str() {
            "all" | "any" => {
                let mut values = Vec::new();
                for (index, child) in spec.as_array().unwrap().iter().enumerate() {
                    values.push(self.visit(child, &format!("{path}.{index}"))?);
                }
                Ok(if kind == "all" {
                    if values.contains(&Some(false)) {
                        Some(false)
                    } else if values.contains(&None) {
                        None
                    } else {
                        Some(true)
                    }
                } else if values.contains(&Some(true)) {
                    Some(true)
                } else if values.contains(&None) {
                    None
                } else {
                    Some(false)
                })
            }
            "at" => {
                let target = parse_time(spec.as_str().unwrap(), self.zone)?;
                let reached = self.now >= target;
                self.inputs.entry("at").or_insert_with(|| json!({}))[path] =
                    json!({"time":stamp(target),"reached":reached});
                if !reached {
                    self.wakes.push(target);
                }
                self.reasons.push(format!(
                    "{} time {}",
                    if reached { "Reached" } else { "Awaiting" },
                    stamp(target)
                ));
                Ok(Some(reached))
            }
            "manual" => {
                self.inputs.entry("manual").or_insert_with(|| json!({}))[path] = spec.clone();
                self.reasons.push(format!(
                    "Manual confirmation required: {}",
                    spec.as_str().unwrap()
                ));
                Ok(None)
            }
            "completed" => {
                let identity = spec.as_str().unwrap();
                let done = self.completed.contains(identity);
                self.inputs.entry("completed").or_insert_with(|| json!({}))[identity] = json!(done);
                self.reasons.push(format!(
                    "{identity} is {}",
                    if done { "completed" } else { "not completed" }
                ));
                Ok(Some(done))
            }
            "changed" => {
                let identity = spec.as_str().unwrap();
                let current = self.snapshot("graph", identity, false);
                let previous = self.snapshot("baseline", identity, true);
                let result = current.zip(previous).map(|(a, b)| !equal(&a, &b));
                self.reasons.push(format!(
                    "{identity} {}",
                    match result {
                        None => "has unknown current/baseline value",
                        Some(true) => "changed",
                        Some(false) => "matches baseline",
                    }
                ));
                Ok(result)
            }
            "condition" => {
                let identity = spec["id"].as_str().unwrap();
                let raw = self.snapshot("graph", identity, false);
                let core = self.current_core.contains(identity);
                let current = raw.and_then(|value| {
                    if core {
                        core_scalar(&value)
                    } else if value
                        .as_object()
                        .is_some_and(|m| m.len() == 1 && m.contains_key("computed"))
                    {
                        value["computed"]
                            .get("value")
                            .cloned()
                            .filter(|v| !v.is_null())
                    } else {
                        Some(value)
                    }
                });
                let expected = &spec["value"];
                let result =
                    current
                        .as_ref()
                        .and_then(|value| match spec["op"].as_str().unwrap() {
                            "==" => Some(equal(value, expected)),
                            "!=" => Some(!equal(value, expected)),
                            "<" => compare_numbers(value, expected).map(|o| o == Ordering::Less),
                            "<=" => {
                                compare_numbers(value, expected).map(|o| o != Ordering::Greater)
                            }
                            ">" => compare_numbers(value, expected).map(|o| o == Ordering::Greater),
                            ">=" => compare_numbers(value, expected).map(|o| o != Ordering::Less),
                            _ => None,
                        });
                self.inputs.entry("condition").or_insert_with(|| json!({}))[path] =
                    json!({"id":identity,"op":spec["op"],"value":expected,"state":result});
                self.reasons.push(format!(
                    "Condition on {identity} is {}",
                    result.map_or("unknown".into(), |v| v.to_string())
                ));
                Ok(result)
            }
            "external" => {
                let identity = spec["ref"].as_str().unwrap();
                let max_age = spec
                    .get("max_age_hours")
                    .and_then(Value::as_f64)
                    .unwrap_or(24.0);
                let observation = self.observations.get(identity).and_then(Value::as_object);
                let mut state = json!({"status":"missing"});
                let result = if let Some(observation) = observation {
                    let complete = ["value", "observed_at", "evidence"]
                        .iter()
                        .all(|field| observation.contains_key(*field));
                    let evidence = observation.get("evidence").and_then(Value::as_str);
                    let observed = observation
                        .get("observed_at")
                        .and_then(Value::as_str)
                        .filter(|value| value.contains(['T', 't', ' ']))
                        .and_then(|value| parse_time(value, "UTC").ok());
                    if complete && evidence.is_some_and(|value| !value.trim().is_empty()) {
                        if let Some(observed) = observed {
                            let expiry = observed
                                + chrono::Duration::milliseconds((max_age * 3_600_000.0) as i64);
                            let status = if observed > self.now {
                                "future"
                            } else if self.now >= expiry {
                                "expired"
                            } else {
                                "fresh"
                            };
                            state = json!({"status":status,"value":observation["value"],"observed_at":stamp(observed),"evidence":observation["evidence"]});
                            if status == "fresh" {
                                self.wakes.push(expiry);
                                Some(equal(&observation["value"], &spec["equals"]))
                            } else {
                                None
                            }
                        } else {
                            state = json!({"status":"invalid"});
                            None
                        }
                    } else {
                        state = json!({"status":"invalid"});
                        None
                    }
                } else {
                    None
                };
                let status = state["status"].as_str().unwrap().to_owned();
                let mut input = json!({"ref":identity,"equals":spec["equals"],"max_age_hours":spec.get("max_age_hours").cloned().unwrap_or(json!(24))});
                input
                    .as_object_mut()
                    .unwrap()
                    .extend(state.as_object().unwrap().clone());
                self.inputs.entry("external").or_insert_with(|| json!({}))[path] = input;
                self.reasons
                    .push(format!("External observation {identity} is {status}"));
                Ok(result)
            }
            _ => Err(Error("invalid trigger".into())),
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn evaluate(
    trigger: &Value,
    values: &Map<String, Value>,
    baseline: &Map<String, Value>,
    completed: &BTreeSet<String>,
    observations: &Map<String, Value>,
    now: DateTime<Utc>,
    zone: &str,
    current_core: &BTreeSet<String>,
    baseline_core: &BTreeSet<String>,
) -> Result<Evaluation> {
    walk(trigger)?;
    let mut context = Context {
        values,
        baseline,
        completed,
        observations,
        now,
        zone,
        current_core,
        baseline_core,
        inputs: Map::new(),
        reasons: vec![],
        wakes: vec![],
    };
    let value = context.visit(trigger, "0")?;
    Ok(Evaluation {
        value,
        inputs: context.inputs,
        reasons: context.reasons,
        next_at: context.wakes.into_iter().min(),
    })
}
