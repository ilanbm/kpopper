use crate::{Error, Result, followup_store, followup_triggers as triggers, require};
use chrono::{Duration, NaiveDate, NaiveTime, TimeZone};
use chrono_tz::Tz;
use serde_json::{Value, json};
use std::str::FromStr;

const DECLARATION_SCHEMA: &str = "kpopper.maintenance-declaration/v1";
const MAX_CADENCE_DAYS: u64 = 36_500;

fn error(message: impl Into<String>) -> Error {
    Error(message.into())
}

fn object<'a>(value: &'a Value, label: &str) -> Result<&'a serde_json::Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| error(format!("{label} must be a mapping")))
}

fn reject_unknown(object: &serde_json::Map<String, Value>, allowed: &[&str]) -> Result<()> {
    let unknown = object
        .keys()
        .filter(|key| !allowed.contains(&key.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    require(
        unknown.is_empty(),
        &format!("Unknown maintenance fields: {}", unknown.join(", ")),
    )
}

fn required_text(
    object: &serde_json::Map<String, Value>,
    key: &str,
    unresolved: &mut Vec<String>,
) -> Option<String> {
    let Some(value) = object
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.chars().count() <= 12_000)
    else {
        unresolved.push(key.to_owned());
        return None;
    };
    Some(value.to_owned())
}

fn is_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn utc_timestamp(value: &str) -> Result<String> {
    require(
        value.ends_with('Z') || value.ends_with("+00:00"),
        "maintenance deadlines must be explicit UTC timestamps",
    )?;
    Ok(triggers::stamp(triggers::parse_time(value, "UTC")?))
}

fn mapped_deadline(value: &Value) -> Result<String> {
    let deadline = object(value, "deadline")?;
    if deadline.len() == 1 && deadline.contains_key("utc") {
        let utc = deadline["utc"]
            .as_str()
            .ok_or_else(|| error("deadline.utc must be an explicit UTC timestamp"))?;
        return utc_timestamp(utc);
    }
    require(
        deadline.len() == 3
            && ["date", "timezone", "boundary"]
                .iter()
                .all(|key| deadline.contains_key(*key)),
        "deadline must contain utc, or date, timezone and boundary",
    )?;
    let date_text = deadline["date"]
        .as_str()
        .ok_or_else(|| error("deadline.date must be YYYY-MM-DD"))?;
    let date = NaiveDate::parse_from_str(date_text, "%Y-%m-%d")
        .map_err(|_| error("deadline.date must be YYYY-MM-DD"))?;
    let zone = deadline["timezone"]
        .as_str()
        .ok_or_else(|| error("deadline.timezone must be an IANA timezone"))?;
    let timezone =
        Tz::from_str(zone).map_err(|e| error(format!("invalid time or timezone: {e}")))?;
    let midnight = |date: NaiveDate| {
        timezone.from_local_datetime(&date.and_hms_opt(0,0,0).unwrap()).single().map(|time| time.with_timezone(&chrono::Utc)).ok_or_else(|| error("maintenance date boundary is ambiguous or nonexistent; resolve an explicit UTC deadline"))
    };
    let boundary = deadline["boundary"]
        .as_str()
        .ok_or_else(|| error("deadline.boundary must be midnight or end_of_day"))?;
    let instant = match boundary {
        "midnight" => midnight(date)?,
        "end_of_day" => {
            let following = date
                .succ_opt()
                .ok_or_else(|| error("deadline.date is out of range"))?;
            midnight(following)?
                .checked_sub_signed(Duration::microseconds(1))
                .ok_or_else(|| error("deadline.date is out of range"))?
        }
        _ => return Err(error("deadline.boundary must be midnight or end_of_day")),
    };
    Ok(triggers::stamp(instant))
}

fn cadence_days(value: Option<&Value>) -> Option<u64> {
    value
        .and_then(Value::as_u64)
        .filter(|days| (1..=MAX_CADENCE_DAYS).contains(days))
}

fn source_fields(
    value: Option<&Value>,
    unresolved: &mut Vec<String>,
) -> Result<Option<(Value, String, String, f64)>> {
    let Some(value) = value else {
        unresolved.extend(
            [
                "source.source_id",
                "source.locator",
                "source.publisher",
                "source.selection",
                "source.adapter",
                "source.evidence_format",
                "source.tool_policy",
                "source.allowed_roots",
                "source.max_age_hours",
            ]
            .map(str::to_owned),
        );
        return Ok(None);
    };
    let source = object(value, "source")?;
    reject_unknown(
        source,
        &[
            "source_id",
            "locator",
            "publisher",
            "selection",
            "adapter",
            "evidence_format",
            "tool_policy",
            "allowed_roots",
            "max_age_hours",
        ],
    )?;
    let mut read_text = |key: &str| {
        required_text(source, key, unresolved).map(|value| (key.to_owned(), json!(value)))
    };
    let identity = read_text("source_id").map(|(_, value)| value.as_str().unwrap().to_owned());
    let locator = read_text("locator");
    let publisher = read_text("publisher");
    let selection = read_text("selection");
    let adapter = read_text("adapter");
    let evidence_format = read_text("evidence_format");
    let tool_policy = read_text("tool_policy");
    let roots = source
        .get("allowed_roots")
        .and_then(Value::as_array)
        .filter(|roots| {
            !roots.is_empty()
                && roots.len() <= 100
                && roots.iter().all(|root| {
                    root.as_str()
                        .is_some_and(|s| !s.trim().is_empty() && s.len() <= 12000)
                })
        })
        .cloned();
    if roots.is_none() {
        unresolved.push("source.allowed_roots".into());
    }
    let max_age = source
        .get("max_age_hours")
        .and_then(Value::as_f64)
        .filter(|age| age.is_finite() && *age > 0.0 && *age < 24_000_000_000.0);
    if max_age.is_none() {
        unresolved.push("source.max_age_hours".to_owned());
    }
    let (
        Some(identity),
        Some(locator),
        Some(publisher),
        Some(selection),
        Some(adapter),
        Some(format),
        Some(tool_policy),
        Some(roots),
        Some(max_age),
    ) = (
        identity,
        locator,
        publisher,
        selection,
        adapter,
        evidence_format,
        tool_policy,
        roots,
        max_age,
    )
    else {
        return Ok(None);
    };
    if !is_identifier(&identity) {
        unresolved.push("source.source_id".to_owned());
        return Ok(None);
    }
    let inspection = json!({
        "locator":locator.1,
        "publisher":publisher.1,
        "selection":selection.1,
        "adapter":adapter.1,
        "evidence_format":format.1,
        "tool_policy":tool_policy.1,
        "allowed_roots":roots,
    });
    let inspection_digest = followup_store::digest(&inspection)?;
    Ok(Some((inspection, identity, inspection_digest, max_age)))
}

fn policy_digest(spec: &Value) -> Result<String> {
    followup_store::digest(&json!({
        "schema":"kpopper.maintenance-policy/v1",
        "id":spec["id"],
        "kind":spec["maintenance"]["kind"],
        "related":spec["related"],
        "scope":spec["scope"],
        "cadence_days":spec["maintenance"]["cadence_days"],
        "max_age_hours":spec["maintenance"]["max_age_hours"],
        "source_ref":spec["maintenance"]["source_ref"],
        "due_at":spec["maintenance"]["due_at"],
        "timezone":spec["maintenance"]["timezone"],
        "check_time":spec["maintenance"]["check_time"],
        "use_policy":spec["maintenance"]["use_policy"],
        "evidence_requirement":spec["maintenance"]["evidence_requirement"],
    }))
}

pub(crate) fn consent_digest(
    policy_digest: &str,
    scope: &str,
    workspace_key: &str,
    record: &str,
) -> Result<String> {
    followup_store::digest(&json!({
        "schema":"kpopper.maintenance-consent/v1",
        "authorization":"explicit-local-followup-registration",
        "workspace_key":workspace_key,
        "record":record,
        "host_execution":"not-established",
        "policy_digest":policy_digest,
        "scope":scope,
    }))
}

pub(crate) fn validate_spec(spec: &Value) -> Result<()> {
    let maintenance = object(
        spec.get("maintenance")
            .ok_or_else(|| error("maintenance metadata is missing"))?,
        "maintenance metadata",
    )?;
    reject_unknown(
        maintenance,
        &[
            "version",
            "kind",
            "policy_digest",
            "consent_digest",
            "cadence_days",
            "max_age_hours",
            "source_id",
            "source_ref",
            "inspection_digest",
            "inspection",
            "due_at",
            "timezone",
            "check_time",
            "use_policy",
            "evidence_requirement",
        ],
    )?;
    require(
        maintenance.len() == 15
            && [
                "version",
                "kind",
                "policy_digest",
                "consent_digest",
                "cadence_days",
                "max_age_hours",
                "source_id",
                "source_ref",
                "inspection_digest",
                "inspection",
                "due_at",
                "timezone",
                "check_time",
                "use_policy",
                "evidence_requirement",
            ]
            .iter()
            .all(|field| maintenance.contains_key(*field)),
        "maintenance metadata is incomplete",
    )?;
    require(
        maintenance.get("version") == Some(&json!(1)),
        "unsupported maintenance metadata version",
    )?;
    let kind = maintenance
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("");
    require(
        ["source", "clock"].contains(&kind),
        "maintenance kind must be source or clock",
    )?;
    let cadence = maintenance.get("cadence_days").and_then(Value::as_u64);
    require(
        cadence.is_some_and(|days| (1..=MAX_CADENCE_DAYS).contains(&days)),
        "maintenance cadence_days must be a positive whole number of days",
    )?;
    validate_use_inputs(maintenance)?;
    let due_at = maintenance
        .get("due_at")
        .and_then(Value::as_str)
        .unwrap_or("");
    require(
        utc_timestamp(due_at).is_ok(),
        "maintenance due_at must be an explicit UTC timestamp",
    )?;
    let policy = maintenance
        .get("policy_digest")
        .and_then(Value::as_str)
        .unwrap_or("");
    require(
        is_digest(policy),
        "maintenance policy_digest must be a SHA-256 digest",
    )?;
    let consent = maintenance
        .get("consent_digest")
        .and_then(Value::as_str)
        .unwrap_or("");
    require(
        is_digest(consent),
        "maintenance consent_digest must be a SHA-256 digest",
    )?;
    let age = maintenance.get("max_age_hours").and_then(Value::as_f64);
    let source_ref = maintenance
        .get("source_ref")
        .and_then(Value::as_str)
        .unwrap_or("");
    let inspection = maintenance
        .get("inspection_digest")
        .and_then(Value::as_str)
        .unwrap_or("");
    let source_id = maintenance
        .get("source_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    if kind == "source" {
        require(
            age.is_some_and(|value| value.is_finite() && value > 0.0 && value < 24_000_000_000.0),
            "maintenance source max_age_hours must be positive and finite",
        )?;
        require(
            is_digest(inspection),
            "maintenance inspection_digest must be a SHA-256 digest",
        )?;
        let descriptor = object(&maintenance["inspection"], "inspection")?;
        reject_unknown(
            descriptor,
            &[
                "locator",
                "publisher",
                "selection",
                "adapter",
                "evidence_format",
                "tool_policy",
                "allowed_roots",
            ],
        )?;
        require(
            descriptor.len() == 7
                && descriptor.iter().all(|(key, value)| {
                    if key == "allowed_roots" {
                        value.as_array().is_some_and(|roots| {
                            !roots.is_empty()
                                && roots.len() <= 100
                                && roots.iter().all(|root| {
                                    root.as_str()
                                        .is_some_and(|s| !s.trim().is_empty() && s.len() <= 12000)
                                })
                        })
                    } else {
                        value
                            .as_str()
                            .is_some_and(|s| !s.trim().is_empty() && s.chars().count() <= 12000)
                    }
                }),
            "inspection semantics must be complete and bounded",
        )?;
        require(
            followup_store::digest(&maintenance["inspection"])? == inspection,
            "inspection_digest does not match its locator/selection/tool semantics",
        )?;
        require(
            is_identifier(source_id) && source_ref == format!("{source_id}@{inspection}"),
            "maintenance source_ref must match source_id@inspection_digest",
        )?;
    } else {
        require(
            age.is_none()
                && source_ref.is_empty()
                && inspection.is_empty()
                && source_id.is_empty()
                && maintenance["inspection"].is_null(),
            "clock maintenance cannot declare a source inspection",
        )?;
    }
    let when = spec
        .get("when")
        .ok_or_else(|| error("maintenance followup needs an at trigger"))?;
    require(
        when.as_object().is_some_and(|trigger| {
            trigger.len() == 1 && trigger.get("at").and_then(Value::as_str) == Some(due_at)
        }) && spec
            .get("task")
            .and_then(Value::as_str)
            .is_none_or(|task| !task.starts_with("https://")),
        "maintenance followup must use its local at trigger and task",
    )?;
    require(
        triggers::referenced_external(when)?.is_empty(),
        "maintenance source checks cannot depend on external observations",
    )?;
    let expected_policy = policy_digest(spec)?;
    require(
        expected_policy == policy,
        "maintenance policy_digest does not match its declaration",
    )?;
    Ok(())
}

fn validate_use_inputs(maintenance: &serde_json::Map<String, Value>) -> Result<()> {
    maintenance["timezone"]
        .as_str()
        .unwrap_or("")
        .parse::<Tz>()
        .map_err(|e| error(format!("invalid maintenance timezone: {e}")))?;
    let time = maintenance["check_time"].as_str().unwrap_or("");
    require(
        time.len() == 5 && NaiveTime::parse_from_str(time, "%H:%M").is_ok(),
        "maintenance check_time must be HH:MM",
    )?;
    let policy = maintenance["use_policy"].as_str().unwrap_or("");
    let evidence = maintenance["evidence_requirement"].as_str().unwrap_or("");
    require(
        ["require_live", "allow_cached_until_expiry"].contains(&policy),
        "maintenance use_policy must be explicit",
    )?;
    require(
        if maintenance["kind"] == "clock" {
            policy == "require_live" && evidence == "trusted_clock"
        } else {
            ["host_attested", "trusted_origin"].contains(&evidence)
        },
        "maintenance evidence requirement must match its kind",
    )
}

fn is_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn compile(declaration: &Value) -> Result<Value> {
    let declaration = object(declaration, "maintenance declaration")?;
    reject_unknown(
        declaration,
        &[
            "schema",
            "kind",
            "id",
            "title",
            "why",
            "how",
            "scope",
            "related",
            "cadence_days",
            "first_due_at",
            "source",
            "deadline",
            "timezone",
            "check_time",
            "use_policy",
            "evidence_requirement",
        ],
    )?;
    require(
        declaration.get("schema").and_then(Value::as_str) == Some(DECLARATION_SCHEMA),
        "maintenance declaration schema must be kpopper.maintenance-declaration/v1",
    )?;
    let kind = declaration
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("");
    require(
        ["source", "clock"].contains(&kind),
        "maintenance kind must be source or clock",
    )?;

    let mut unresolved = Vec::new();
    let id = required_text(declaration, "id", &mut unresolved).filter(|value| is_identifier(value));
    if id.is_none() && !unresolved.contains(&"id".to_owned()) {
        unresolved.push("id".to_owned());
    }
    let title = required_text(declaration, "title", &mut unresolved);
    let why = required_text(declaration, "why", &mut unresolved);
    let how = required_text(declaration, "how", &mut unresolved);
    let scope = required_text(declaration, "scope", &mut unresolved);
    let related = declaration
        .get("related")
        .and_then(Value::as_array)
        .and_then(|values| {
            let strings = values
                .iter()
                .map(Value::as_str)
                .collect::<Option<Vec<_>>>()?;
            let unique = strings
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>();
            (!strings.is_empty()
                && strings.len() <= 100
                && strings.len() == unique.len()
                && strings.iter().all(|id| !id.is_empty() && id.len() <= 200))
            .then(|| json!(strings))
        });
    if related.is_none() {
        unresolved.push("related".to_owned());
    }
    let cadence = cadence_days(declaration.get("cadence_days"));
    if cadence.is_none() {
        unresolved.push("cadence_days".to_owned());
    }

    let timezone = required_text(declaration, "timezone", &mut unresolved)
        .filter(|zone| zone.parse::<Tz>().is_ok());
    if timezone.is_none() && !unresolved.contains(&"timezone".to_owned()) {
        unresolved.push("timezone".into());
    }
    let check_time = required_text(declaration, "check_time", &mut unresolved)
        .filter(|time| time.len() == 5 && NaiveTime::parse_from_str(time, "%H:%M").is_ok());
    if check_time.is_none() && !unresolved.contains(&"check_time".to_owned()) {
        unresolved.push("check_time".into());
    }
    let use_policy = required_text(declaration, "use_policy", &mut unresolved).filter(|value| {
        ["require_live", "allow_cached_until_expiry"].contains(&value.as_str())
            && (kind != "clock" || value == "require_live")
    });
    if use_policy.is_none() && !unresolved.contains(&"use_policy".to_owned()) {
        unresolved.push("use_policy".into());
    }
    let evidence_requirement = required_text(declaration, "evidence_requirement", &mut unresolved)
        .filter(|value| {
            if kind == "clock" {
                value == "trusted_clock"
            } else {
                ["host_attested", "trusted_origin"].contains(&value.as_str())
            }
        });
    if evidence_requirement.is_none() && !unresolved.contains(&"evidence_requirement".to_owned()) {
        unresolved.push("evidence_requirement".into());
    }
    let mut source_data: Option<(Value, String, String, f64)> = None;
    let due_at = if kind == "source" {
        require(
            !declaration.contains_key("deadline"),
            "source declaration cannot contain a clock deadline",
        )?;
        let source = source_fields(declaration.get("source"), &mut unresolved)?;
        source_data = source;
        declaration
            .get("first_due_at")
            .and_then(Value::as_str)
            .and_then(|value| utc_timestamp(value).ok())
            .or_else(|| {
                unresolved.push("first_due_at".to_owned());
                None
            })
    } else {
        require(
            !declaration.contains_key("source"),
            "clock declaration cannot contain source inspection fields",
        )?;
        require(
            !declaration.contains_key("first_due_at"),
            "clock declaration uses deadline instead of first_due_at",
        )?;
        declaration
            .get("deadline")
            .and_then(|value| mapped_deadline(value).ok())
            .or_else(|| {
                unresolved.push("deadline".to_owned());
                None
            })
    };

    unresolved.push("authorization".to_owned());
    unresolved.sort();
    unresolved.dedup();
    let complete = id.is_some()
        && title.is_some()
        && why.is_some()
        && how.is_some()
        && scope.is_some()
        && related.is_some()
        && cadence.is_some()
        && due_at.is_some()
        && timezone.is_some()
        && check_time.is_some()
        && use_policy.is_some()
        && evidence_requirement.is_some()
        && (kind != "source" || source_data.is_some());
    if !complete {
        return Ok(json!({"status":"unresolved","spec":null,"unresolved":unresolved}));
    }
    let id = id.unwrap();
    let scope = scope.unwrap();
    let related = related.unwrap();
    let (inspection, source_id, source_ref, inspection_digest, max_age) =
        if let Some((inspection, source_id, digest, age)) = source_data {
            (
                inspection,
                json!(source_id),
                json!(format!("{source_id}@{digest}")),
                json!(digest),
                json!(age),
            )
        } else {
            (
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
            )
        };
    let mut spec = json!({
        "id":id,
        "title":title.unwrap(),
        "why":why.unwrap(),
        "how":how.unwrap(),
        "scope":scope,
        "related":related,
        "when":{"at":due_at.as_ref().unwrap()},
        "maintenance":{
            "version":1,
            "kind":kind,
            "policy_digest":"",
            "consent_digest":null,
            "cadence_days":cadence.unwrap(),
            "max_age_hours":max_age,
            "source_id":source_id,
            "source_ref":source_ref,
            "inspection_digest":inspection_digest,
            "inspection":inspection,
            "due_at":due_at.unwrap(),
            "timezone":timezone.unwrap(),
            "check_time":check_time.unwrap(),
            "use_policy":use_policy.unwrap(),
            "evidence_requirement":evidence_requirement.unwrap(),
        }
    });
    let digest = policy_digest(&spec)?;
    spec["maintenance"]["policy_digest"] = json!(digest);
    Ok(json!({"status":"proposed","spec":spec,"unresolved":unresolved}))
}

#[cfg(test)]
mod tests {
    use super::compile;
    use serde_json::json;

    #[test]
    fn local_date_deadline_requires_and_applies_boundary_mapping() {
        let declaration = json!({
            "schema":"kpopper.maintenance-declaration/v1",
            "kind":"clock",
            "id":"deadline-review",
            "title":"Review deadline",
            "why":"Check the stated deadline.",
            "how":"Reassess the predicate at its declared boundary.",
            "scope":"Check the named record predicate only.",
            "related":["facts.count"],
            "cadence_days":1,
            "timezone":"Europe/Prague", "check_time":"09:00", "use_policy":"require_live", "evidence_requirement":"trusted_clock",
            "deadline":{"date":"2026-10-05","timezone":"Europe/Prague","boundary":"end_of_day"}
        });
        let result = compile(&declaration).unwrap();
        assert_eq!(result["unresolved"], json!(["authorization"]));
        assert_eq!(result["spec"]["when"]["at"], "2026-10-05T21:59:59.999999Z");
        assert!(result["spec"]["maintenance"]["consent_digest"].is_null());
    }
}
