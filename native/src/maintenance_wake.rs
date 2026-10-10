//! Local wake compatibility; host readback is attested data, never execution or permission.
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};

// A report can remain live through one scheduled interval and a two-hour delay.
// This is only freshness of caller-supplied evidence, not host authentication.
pub(crate) fn reported_liveness_window(days: u64) -> Duration {
    Duration::days(days.min(36500) as i64) + Duration::hours(2)
}

pub(crate) fn assess(data: &Value, now: DateTime<Utc>) -> Value {
    let mut result = assess_phase(data, now);
    result["assurance_scope"] = json!("calendar_phase_only");
    result["capacity_and_age_guarantee"] = json!("not_established");
    result["host_authentication"] = json!("unestablished");
    result
}

fn assess_phase(data: &Value, now: DateTime<Utc>) -> Value {
    let daily = &data["daily"];
    let selection = &daily["maintenance_mode"];
    let mode = selection["mode"].as_str().unwrap_or("existing");
    if mode == "paused" {
        return json!({"state":"paused_locally","mode":mode,"host_mutation":"none","automatic_continuity":"paused"});
    }
    if mode == "manual" {
        return json!({"state":"manual","mode":mode,"host_mutation":"none","automatic_continuity":"uninstalled"});
    }
    let binding = &daily["binding"];
    if binding.is_null() {
        return json!({"state":"uninstalled","mode":mode,"reason":"no_workspace_wake_owner"});
    }
    if binding["state"] != "active" {
        return json!({"state":"paused_or_missing","mode":mode,"host_state":binding["state"]});
    }
    if binding["cadence"] == "daily" {
        let phase = binding["time"]
            .as_str()
            .and_then(|v| chrono::NaiveTime::parse_from_str(v, "%H:%M").ok());
        let zone = binding["timezone"]
            .as_str()
            .and_then(|v| v.parse::<chrono_tz::Tz>().ok());
        // A present null marker means a state transition deliberately invalidated
        // the former phase readback. Only legacy bindings lack the field entirely.
        let phase_timestamp = binding.get("phase_observed_at")
            .and_then(Value::as_str)
            .or_else(|| if binding.get("phase_observed_at").is_none() {
                binding["observed_at"].as_str()
            } else { None });
        let phase_observed = phase_timestamp
            .and_then(|v| DateTime::parse_from_rfc3339(v).ok())
            .map(|v| v.with_timezone(&Utc));
        let inspection_recent = phase_observed.is_some_and(|t| t <= now && now.signed_duration_since(t) < reported_liveness_window(1));
        let executed = crate::followup_daily::latest_reported_host_execution(daily, binding, now);
        let execution_day = match (executed.as_ref(), phase.as_ref(), zone.as_ref()) {
            (Some(executed), Some(phase), Some(zone)) => {
                let local = executed.with_timezone(zone).naive_local();
                let same_day = local.date().and_time(*phase);
                let scheduled = if local < same_day {
                    local.date().pred_opt().map(|day| day.and_time(*phase))
                } else { Some(same_day) };
                scheduled.filter(|scheduled| {
                    let delay = local.signed_duration_since(*scheduled);
                    delay >= Duration::zero() && delay <= Duration::hours(2)
                }).map(|scheduled| scheduled.date())
            }
            _ => None,
        };
        let execution_recent = execution_day.is_some();
        if !inspection_recent && !execution_recent {
            return json!({"state":"unknown","mode":mode,
                "reason":if executed.is_some(){"scheduled_execution_phase_mismatch"}else{"daily_phase_readback_expired_or_missing"},
                "reported_liveness_days":1,"grace_hours":2});
        }
        let mut reasons = Vec::new();
        let mut unproven = false;
        let mut maintenance_phases = serde_json::Map::new();
        for (id, item) in data["items"].as_object().into_iter().flatten() {
            if matches!(item["state"].as_str(), Some("done" | "cancelled")) {
                continue;
            }
            let Some(m) = item["spec"].get("maintenance") else {
                continue;
            };
            let check_time = m["check_time"]
                .as_str()
                .and_then(|v| chrono::NaiveTime::parse_from_str(v, "%H:%M").ok());
            let policy_zone = m["timezone"]
                .as_str()
                .and_then(|v| v.parse::<chrono_tz::Tz>().ok());
            // Recurrence is calendar N days from the admitted check, at check_time.
            // first_due_at may have another time and can be caught up once; it is
            // not proof of the recurring phase. Cross-zone/DST phases need proof.
            if phase.is_none()
                || zone.is_none()
                || check_time.is_none()
                || policy_zone.is_none()
                || zone != policy_zone
            {
                unproven = true;
                reasons.push(json!({"id":id,"reason":"daily_recurring_phase_unestablished"}));
                maintenance_phases.insert(id.clone(), json!("unknown"));
            } else if phase.unwrap() < check_time.unwrap() {
                reasons.push(json!({"id":id,"reason":"daily_wake_precedes_recurring_check_time","check_time":m["check_time"]}));
                maintenance_phases.insert(id.clone(), json!("incompatible"));
            } else {
                maintenance_phases.insert(id.clone(), json!("compatible"));
            }
        }
        return json!({"state":if reasons.is_empty(){"compatible"}else if unproven{"unknown"}else{"incompatible"},
            "mode":mode,"basis":"daily_recurring_phase","phase_liveness_basis":if inspection_recent{"reported_schedule_readback"}else{"self_reported_scheduled_execution"},
            "reasons":reasons,"maintenance_phases":maintenance_phases,"reported_liveness_days":1,"grace_hours":2,
            "required_choice":if reasons.is_empty(){json!([])}else{json!(["repair_daily_phase_with_authorization","manual"])},
            "execution":"unestablished_by_configuration","host_mutation":"none"});
    }
    let descriptor = &selection["native_readback"];
    if descriptor["host"] != binding["host"] || descriptor["id"] != binding["id"] {
        return json!({"state":"unknown","mode":mode,"reason":"native_phase_readback_unestablished","required_choice":["daily_fallback","manual"]});
    }
    let Some(days) = descriptor["cadence_days"]
        .as_u64()
        .filter(|n| *n > 0 && *n <= 36500)
    else {
        return json!({"state":"unknown","mode":mode,"reason":"native_interval_unestablished","required_choice":["daily_fallback","manual"]});
    };
    let observed = descriptor["observed_at"]
        .as_str()
        .and_then(|v| DateTime::parse_from_rfc3339(v).ok())
        .map(|v| v.with_timezone(&Utc));
    if observed
        .is_none_or(|t| t > now || now.signed_duration_since(t) >= reported_liveness_window(days))
    {
        return json!({"state":"unknown","mode":mode,"reason":"native_phase_readback_expired_or_missing","required_choice":["daily_fallback","manual"],"reported_liveness_days":days,"grace_hours":2});
    }
    if descriptor["interval_semantics"] != "calendar_days" {
        return json!({"state":"unknown","mode":mode,"reason":"native_calendar_semantics_unestablished","required_choice":["daily_fallback","manual"]});
    }
    let timezone = descriptor["timezone"]
        .as_str()
        .and_then(|v| v.parse::<chrono_tz::Tz>().ok());
    let anchor = descriptor["anchor_at"]
        .as_str()
        .and_then(|v| DateTime::parse_from_rfc3339(v).ok())
        .map(|v| v.with_timezone(&Utc));
    let (Some(zone), Some(anchor)) = (timezone, anchor) else {
        return json!({"state":"unknown","mode":mode,"reason":"native_anchor_or_timezone_unestablished","required_choice":["daily_fallback","manual"]});
    };
    let mut reasons = Vec::new();
    let mut ordinary_unknown = false;
    let mut maintenance_incompatible = false;
    let mut maintenance_phases = serde_json::Map::new();
    for (id, item) in data["items"].as_object().into_iter().flatten() {
        if matches!(item["state"].as_str(), Some("done" | "cancelled")) {
            continue;
        }
        let Some(m) = item["spec"].get("maintenance") else {
            ordinary_unknown = true;
            reasons.push(
                json!({"id":id,"reason":"ordinary_obligation_requires_separate_compatibility"}),
            );
            continue;
        };
        let next = item["next_at"]
            .as_str()
            .or_else(|| m["due_at"].as_str())
            .and_then(|v| DateTime::parse_from_rfc3339(v).ok())
            .map(|v| v.with_timezone(&Utc));
        let cadence = m["cadence_days"].as_u64();
        let compatible = next.is_some_and(|next| {
            let local = next.with_timezone(&zone);
            let anchor = anchor.with_timezone(&zone);
            let delta = local
                .date_naive()
                .signed_duration_since(anchor.date_naive())
                .num_days();
            m["timezone"] == descriptor["timezone"]
                && local.time() == anchor.time()
                && delta >= 0
                && delta % days as i64 == 0
                && cadence.is_some_and(|n| n % days == 0)
        });
        if !compatible {
            maintenance_incompatible = true;
            reasons.push(json!({"id":id,"reason":"fixed_native_phase_cannot_serve_next_calendar_check","next_at":next.map(|v|v.to_rfc3339())}));
        }
        maintenance_phases.insert(id.clone(), json!(if compatible { "compatible" } else { "incompatible" }));
    }
    json!({"state":if maintenance_incompatible{"incompatible"}else{"compatible"},"mode":mode,"reasons":reasons,"maintenance_phases":maintenance_phases,
        "ordinary_compatibility":if ordinary_unknown{"unknown"}else{"not_applicable_or_separate"},"assessed_at":now.to_rfc3339(),
        "reported_liveness_days":days,"grace_hours":2,
        "required_choice":if maintenance_incompatible{json!(["daily_fallback","manual"])}else{json!([])},
        "execution":"unestablished_by_attested_configuration","host_mutation":"none"})
}

pub(crate) fn select(
    store: &crate::followup_store::Store,
    mode: &str,
    authority: &str,
    acknowledge_empty_run_cost: bool,
    native_readback: Option<Value>,
) -> crate::Result<Value> {
    use crate::{Error, require};
    require(
        ["manual", "paused", "daily_fallback", "native"].contains(&mode),
        "Unknown maintenance execution mode",
    )?;
    require(
        !authority.trim().is_empty() && authority.len() <= 512,
        "Supply an existing or new user authorization reference (at most512 bytes)",
    )?;
    require(
        mode != "daily_fallback" || acknowledge_empty_run_cost,
        "Daily fallback requires acknowledgement that empty agent runs can cost tokens",
    )?;
    if let Some(ref descriptor) = native_readback {
        let object = descriptor
            .as_object()
            .ok_or_else(|| Error("Native readback must be a mapping".into()))?;
        require(
            mode == "native"
                && object.keys().all(|key| {
                    [
                        "host",
                        "id",
                        "cadence_days",
                        "anchor_at",
                        "timezone",
                        "evidence",
                        "observed_at",
                        "interval_semantics",
                    ]
                    .contains(&key.as_str())
                }),
            "Unknown native readback fields",
        )?;
        for key in [
            "host",
            "id",
            "anchor_at",
            "timezone",
            "evidence",
            "observed_at",
        ] {
            require(
                descriptor[key]
                    .as_str()
                    .is_some_and(|s| !s.trim().is_empty() && s.len() <= 512),
                "Native readback needs bounded identity, phase, time and evidence references",
            )?;
        }
        require(
            descriptor["cadence_days"]
                .as_u64()
                .is_some_and(|n| n > 0 && n <= 36500),
            "Native interval must be whole calendar days",
        )?;
        crate::followup_triggers::parse_time(descriptor["anchor_at"].as_str().unwrap(), "UTC")?;
        crate::followup_triggers::parse_time(descriptor["observed_at"].as_str().unwrap(), "UTC")?;
        descriptor["timezone"]
            .as_str()
            .unwrap()
            .parse::<chrono_tz::Tz>()
            .map_err(|e| Error(e.to_string()))?;
    }
    store.transaction(|data| {
        require(data["daily"]["claim"].is_null(), "Reconcile the active daily claim before changing local execution intent")?;
        data["version"]=json!(2);
        data["daily"]["maintenance_mode"]=json!({"mode":mode,"selected_at":store.now().to_rfc3339(),"authorization_reference":authority,
            "empty_run_cost_acknowledged":acknowledge_empty_run_cost,"native_readback":native_readback,
            "attestation":"host-supplied normalized phase; no configuration/execution authentication"});
        Ok(json!({"selection":data["daily"]["maintenance_mode"],"wake":assess(data,store.now()),"host_mutation":"none"}))
    })
}

#[cfg(test)]
mod tests {
    use super::{assess, reported_liveness_window};
    use chrono::{Duration, TimeZone, Utc};
    use serde_json::json;

    #[test]
    fn weekly_phase_survives_one_day_and_ordinary_items_stay_separate() {
        let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
        let data = json!({"daily":{"binding":{"host":"host","id":"schedule","state":"active"},
            "maintenance_mode":{"mode":"native","native_readback":{"host":"host","id":"schedule",
                "observed_at":"2026-10-05T11:00:00Z","cadence_days":7,"interval_semantics":"calendar_days",
                "timezone":"UTC","anchor_at":"2026-10-06T09:00:00Z"}}},
            "items":{"weekly":{"id":"weekly","state":"waiting","next_at":"2026-10-13T09:00:00Z",
                "spec":{"maintenance":{"timezone":"UTC","cadence_days":7}}},
                "ordinary":{"id":"ordinary","state":"waiting","spec":{"when":{"at":"2027-01-01T00:00:00Z"}}}}});
        let result = assess(&data, now);
        assert_eq!(result["state"], "compatible");
        assert_eq!(result["maintenance_phases"]["weekly"], "compatible");
        assert_eq!(result["ordinary_compatibility"], "unknown");
        assert_eq!(reported_liveness_window(7), Duration::days(7) + Duration::hours(2));
    }

    #[test]
    fn one_incompatible_maintenance_phase_does_not_change_another() {
        let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
        let data = json!({"daily":{"binding":{"host":"host","id":"schedule","state":"active"},
            "maintenance_mode":{"mode":"native","native_readback":{"host":"host","id":"schedule",
                "observed_at":"2026-10-06T10:00:00Z","cadence_days":7,"interval_semantics":"calendar_days",
                "timezone":"UTC","anchor_at":"2026-10-06T09:00:00Z"}}},
            "items":{"good":{"id":"good","state":"waiting","next_at":"2026-10-13T09:00:00Z",
                "spec":{"maintenance":{"timezone":"UTC","cadence_days":7}}},
                "bad":{"id":"bad","state":"waiting","next_at":"2026-10-13T10:00:00Z",
                "spec":{"maintenance":{"timezone":"UTC","cadence_days":7}}}}});
        let result = assess(&data, now);
        assert_eq!(result["maintenance_phases"]["good"], "compatible");
        assert_eq!(result["maintenance_phases"]["bad"], "incompatible");
    }

    #[test]
    fn daily_phase_renewal_requires_aligned_current_epoch_execution() {
        let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
        let data = json!({"daily":{"binding":{"host":"host","id":"schedule","state":"active",
            "cadence":"daily","time":"09:00","timezone":"UTC","observed_at":"2026-10-06T11:00:00Z",
            "phase_observed_at":"2026-10-05T01:00:00Z","binding_epoch":"epoch",
            "epoch_started_at":"2026-10-01T00:00:00Z"},
            "receipts":[{"outcome":"complete","execution_origin":"self_reported_scheduled",
                "binding_epoch":"epoch","started_at":"2026-10-06T09:02:00Z","finished_at":"2026-10-06T09:05:00Z",
                "host_execution":{"schema":"kpopper.host-execution/v1","trigger":"scheduled",
                    "host":"host","id":"schedule","executed_at":"2026-10-06T09:00:00Z",
                    "observed_at":"2026-10-06T09:01:00Z","evidence":"run"}}]},
            "items":{"check":{"id":"check","state":"waiting","spec":{"maintenance":{
                "timezone":"UTC","check_time":"08:45"}}}}});
        let result = assess(&data, now);
        assert_eq!(result["state"], "compatible");
        assert_eq!(result["phase_liveness_basis"], "self_reported_scheduled_execution");
        let mut early = data.clone();
        early["daily"]["receipts"][0]["started_at"] = json!("2026-10-06T08:32:00Z");
        early["daily"]["receipts"][0]["finished_at"] = json!("2026-10-06T08:35:00Z");
        early["daily"]["receipts"][0]["host_execution"]["executed_at"] = json!("2026-10-06T08:30:00Z");
        early["daily"]["receipts"][0]["host_execution"]["observed_at"] = json!("2026-10-06T08:31:00Z");
        let rejected = assess(&early, now);
        assert_eq!(rejected["state"], "unknown");
        assert_eq!(rejected["reason"], "scheduled_execution_phase_mismatch");
        let mut prior_epoch = data.clone();
        prior_epoch["daily"]["receipts"][0]["binding_epoch"] = json!("old");
        assert_eq!(assess(&prior_epoch, now)["state"], "unknown");
    }

    #[test]
    fn state_only_reactivation_with_null_phase_proof_is_unknown() {
        let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
        let data = json!({"daily":{"binding":{"host":"host","id":"schedule","state":"active",
            "cadence":"daily","time":"09:00","timezone":"UTC","observed_at":"2026-10-06T11:00:00Z",
            "phase_observed_at":null,"binding_epoch":"new","epoch_started_at":"2026-10-06T11:00:00Z"}},
            "items":{"check":{"id":"check","state":"waiting","spec":{"maintenance":{
                "timezone":"UTC","check_time":"08:00"}}}}});
        assert_eq!(assess(&data, now)["state"], "unknown");
    }
    #[test]
    fn nanosecond_mode_selection_remains_readable_for_daily_start() {
        use std::fs;
        let t = tempfile::tempdir().unwrap();
        let work = t.path().join("work");
        fs::create_dir(&work).unwrap();
        fs::write(work.join("PROVENANCE.yaml"), "meta: {name: Precision}
    known:
      facts.count: {v: 1}
    ").unwrap();
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-10T09:00:00.123456789Z").unwrap().with_timezone(&Utc);
        let store = crate::followup_store::Store::at_in_state(&work, &t.path().join("state"), now).unwrap();
        store.setup(None, "UTC", None, true).unwrap();
        super::select(&store, "manual", "fixture://manual-scope", false, None).unwrap();
        let started = crate::followup_daily::start_with_mode(&store, "owner", |_| Ok(None), Some("fixture://current-manual-grant")).unwrap();
        assert_eq!(started["claim"]["execution_origin"], "manual");
        let stamped = started["claim"]["started_at"].as_str().unwrap();
        assert_eq!(crate::followup_triggers::parse_time(stamped, "UTC").unwrap(), now);
    }

}
