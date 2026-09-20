//! Bounded, pure, three-valued followup trigger evaluation.
use crate::{Error, Result, require};
use chrono::{DateTime, Datelike, LocalResult, NaiveDate, Offset, TimeZone, Utc};
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
    value.to_rfc3339_opts(
        if value.timestamp_subsec_micros() == 0 {
            chrono::SecondsFormat::Secs
        } else {
            chrono::SecondsFormat::Micros
        },
        true,
    )
}

pub fn parse_time(value: &str, zone: &str) -> Result<DateTime<Utc>> {
    let timezone =
        Tz::from_str(zone).map_err(|e| Error(format!("invalid time or timezone: {e}")))?;
    let in_range = |value: DateTime<Utc>| {
        require(
            (1..=9999).contains(&value.year()),
            "invalid time or timezone: date value out of range",
        )?;
        Ok(value)
    };
    if value.len() == 10 {
        let date = NaiveDate::parse_from_str(value, "%Y-%m-%d")
            .map_err(|e| Error(format!("invalid time or timezone: {e}")))?;
        require(
            date.year() >= 1,
            "invalid time or timezone: year 0 is out of range",
        )?;
        let local = date
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| Error("invalid time or timezone".into()))?;
        return match timezone.from_local_datetime(&local) {
            LocalResult::Single(value) | LocalResult::Ambiguous(value, _) => {
                in_range(value.with_timezone(&Utc))
            }
            LocalResult::None => {
                // ZoneInfo fold=0 uses the pre-transition offset for a gap.
                for hours in 1..=48 {
                    let Some(previous) = local.checked_sub_signed(chrono::Duration::hours(hours))
                    else {
                        break;
                    };
                    let offset = match timezone.offset_from_local_datetime(&previous) {
                        LocalResult::Single(offset) | LocalResult::Ambiguous(offset, _) => {
                            offset.fix().local_minus_utc()
                        }
                        LocalResult::None => continue,
                    };
                    let value = local
                        .and_utc()
                        .checked_sub_signed(chrono::Duration::seconds(offset.into()))
                        .ok_or_else(|| {
                            Error("invalid time or timezone: date value out of range".into())
                        })?;
                    return in_range(value);
                }
                Err(Error(
                    "invalid time or timezone: nonexistent local time".into(),
                ))
            }
        };
    }
    static STAMP: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
        r"^([0-9]{4}-[0-9]{2}-[0-9]{2})[Tt ]([0-9]{2}:[0-9]{2})(?::([0-9]{2})(\.[0-9]{1,6})?)?([Zz]|[+-][0-9]{2}:[0-9]{2})$").unwrap()
    });
    let fields = STAMP.captures(value).ok_or_else(|| {
        Error(
            "invalid time or timezone: expected an ISO date or timestamp with an explicit offset"
                .into(),
        )
    })?;
    require(
        fields
            .get(3)
            .is_none_or(|seconds| seconds.as_str().parse::<u8>().is_ok_and(|n| n < 60)),
        "invalid time or timezone: second must be in 0..59",
    )?;
    let value = format!(
        "{}T{}:{}{}{}",
        &fields[1],
        &fields[2],
        fields.get(3).map_or("00", |s| s.as_str()),
        fields.get(4).map_or("", |s| s.as_str()),
        &fields[5]
    );
    let mut normalized = value.replace('t', "T");
    if normalized.ends_with(['z', 'Z']) {
        normalized.truncate(normalized.len() - 1);
        normalized.push_str("+00:00");
    }
    if let Some(at) = normalized.find(' ') {
        normalized.replace_range(at..=at, "T");
    }
    let parsed = DateTime::parse_from_rfc3339(&normalized)
        .map_err(|e| Error(format!("invalid time or timezone: {e}")))?;
    in_range(parsed.with_timezone(&Utc))
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
                        && (!object["value"].is_object() || number(&object["value"]).is_some()),
                    "condition comparison value must be scalar",
                )?;
                if ["<", "<=", ">", ">="].contains(&op) {
                    require(
                        number(&object["value"]).is_some(),
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
                require(
                    age.unwrap() < 24_000_000_000.0,
                    "max_age_hours is too large",
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

fn number(value: &Value) -> Option<(BigInt, BigInt)> {
    crate::ordinary_assessment::number(&crate::value::TypedValue::from_json(value).ok()?)
}
fn compare_numbers(left: &Value, right: &Value) -> Option<Ordering> {
    let (a, b) = number(left)?;
    let (c, d) = number(right)?;
    Some((a * d).cmp(&(c * b)))
}
fn equal(left: &Value, right: &Value) -> bool {
    if let Some(ordering) = compare_numbers(left, right) {
        return ordering == Ordering::Equal;
    }
    if let (Some(a), Some(b)) = (left.as_object(), right.as_object())
        && a.len() == 1
        && b.len() == 1
        && let (Some(a), Some(b)) = (
            a.get("computed").and_then(Value::as_object),
            b.get("computed").and_then(Value::as_object),
        )
        && a.len() == b.len()
        && ["rule", "value"]
            .iter()
            .all(|key| a.contains_key(*key) && b.contains_key(*key))
    {
        return a.iter().all(|(key, value)| {
            b.get(key).is_some_and(|other| {
                if key == "rule" {
                    match (
                        crate::value::TypedValue::from_json(value),
                        crate::value::TypedValue::from_json(other),
                    ) {
                        (Ok(a), Ok(b)) => {
                            crate::source_clock::python_equal(&a, &b)
                                || matches!(
                                    (&a, &b),
                                    (
                                        crate::value::TypedValue::Map(_),
                                        crate::value::TypedValue::Map(_)
                                    )
                                ) && crate::ordinary_counts::same_rule(&a, &b)
                        }
                        _ => false,
                    }
                } else {
                    equal(value, other)
                }
            })
        });
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
    let core = crate::followup_core::envelope(value).ok()?.as_object()?;
    if core.get("version") != Some(&json!(1)) || core.get("available") != Some(&json!(true)) {
        return None;
    }
    let typed = core.get("value")?.as_object()?;
    match typed.get("type")?.as_str()? {
        "number" => Some(json!({"rational":[typed.get("numerator")?,typed.get("denominator")?]})),
        "text" | "boolean" => typed.get("value").cloned(),
        "null" => Some(Value::Null),
        _ => Some(Value::Object(typed.clone())),
    }
}
pub fn unavailable(value: Option<&Value>, core: bool) -> bool {
    if core {
        return value
            .and_then(|value| crate::followup_core::envelope(value).ok())
            .and_then(|core| core.get("available"))
            .and_then(Value::as_bool)
            != Some(true);
    }
    let Some(Value::Object(object)) = value else {
        return false;
    };
    object.len() == 1 && object.get("unavailable").is_some_and(Value::is_string)
        || object.len() == 1
            && object.contains_key("computed")
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
                let matching_types =
                    self.current_core.contains(identity) == self.baseline_core.contains(identity);
                let result = current
                    .zip(previous)
                    .map(|(a, b)| !matching_types || !equal(&a, &b));
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
                            .filter(|_| value["computed"].get("rule").is_some())
                            .cloned()
                            .filter(|v| !v.is_null())
                    } else if value
                        .as_object()
                        .is_some_and(|m| m.len() == 1 && m.contains_key("rational"))
                        && number(&value).is_none()
                    {
                        None
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
                        let expiry = observed.and_then(|observed| {
                            let micros = (max_age * 3_600_000_000.0).round_ties_even();
                            if !micros.is_finite() || micros < 0.0 || micros >= i64::MAX as f64 {
                                return None;
                            }
                            observed
                                .checked_add_signed(chrono::Duration::microseconds(micros as i64))
                                .filter(|value| (1..=9999).contains(&value.year()))
                        });
                        if let (Some(observed), Some(expiry)) = (observed, expiry) {
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
