//! Local wake compatibility; host readback is attested data, never execution or permission.
use chrono::{DateTime, Utc};
use serde_json::{Value, json};

pub(crate) fn assess(data: &Value, now: DateTime<Utc>) -> Value {
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
        return json!({"state":"compatible","mode":mode,"basis":"existing_daily_wake_serves_due_items","execution":"unestablished_by_configuration"});
    }
    let descriptor = &selection["native_readback"];
    if descriptor["host"] != binding["host"] || descriptor["id"] != binding["id"] {
        return json!({"state":"unknown","mode":mode,"reason":"native_phase_readback_unestablished","required_choice":["daily_fallback","manual"]});
    }
    let observed = descriptor["observed_at"]
        .as_str()
        .and_then(|v| DateTime::parse_from_rfc3339(v).ok())
        .map(|v| v.with_timezone(&Utc));
    if observed
        .is_none_or(|t| t > now || now.signed_duration_since(t) >= chrono::Duration::hours(24))
    {
        return json!({"state":"unknown","mode":mode,"reason":"native_phase_readback_expired_or_missing","required_choice":["daily_fallback","manual"]});
    }
    if descriptor["interval_semantics"] != "calendar_days" {
        return json!({"state":"unknown","mode":mode,"reason":"native_calendar_semantics_unestablished","required_choice":["daily_fallback","manual"]});
    }
    let Some(days) = descriptor["cadence_days"]
        .as_u64()
        .filter(|n| *n > 0 && *n <= 36500)
    else {
        return json!({"state":"unknown","mode":mode,"reason":"native_interval_unestablished","required_choice":["daily_fallback","manual"]});
    };
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
    for (id, item) in data["items"].as_object().into_iter().flatten() {
        if matches!(item["state"].as_str(), Some("done" | "cancelled")) {
            continue;
        }
        let Some(m) = item["spec"].get("maintenance") else {
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
            reasons.push(json!({"id":id,"reason":"fixed_native_phase_cannot_serve_next_calendar_check","next_at":next.map(|v|v.to_rfc3339())}));
        }
    }
    json!({"state":if reasons.is_empty(){"compatible"}else{"incompatible"},"mode":mode,"reasons":reasons,"assessed_at":now.to_rfc3339(),
        "required_choice":if reasons.is_empty(){json!([])}else{json!(["daily_fallback","manual"])},
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
