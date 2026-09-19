//! Bound query requests and validated response evidence. Lean owns row meaning.
use crate::{
    Error, Result, reasoning_query as Q, reasoning_scope::ScopeCapture, reasoning_snapshot,
    require, value::TypedValue as V,
};
use serde_json::{Value as J, json};
use std::collections::BTreeSet;
fn hash(value: &J) -> Result<String> {
    reasoning_snapshot::digest(&V::from_json_bounded(value, 16 * 1024 * 1024 / 8)?)
}
fn exact(value: &J, keys: &[&str]) -> bool {
    value
        .as_object()
        .is_some_and(|m| m.len() == keys.len() && keys.iter().all(|k| m.contains_key(*k)))
}
fn strings(value: &J) -> Result<Vec<String>> {
    value
        .as_array()
        .ok_or_else(|| Error("invalid_query_envelope".into()))?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| Error("invalid_query_envelope".into()))
        })
        .collect()
}
fn sign(value: &mut J) -> Result<()> {
    let mut preimage = value.clone();
    preimage
        .as_object_mut()
        .ok_or_else(|| Error("invalid_query_envelope".into()))?
        .remove("digest");
    value["digest"] = json!(hash(&preimage)?);
    Ok(())
}
fn valid_hash(value: &J) -> Result<bool> {
    let Some(m) = value.as_object() else {
        return Ok(false);
    };
    let mut preimage = m.clone();
    preimage.remove("digest");
    Ok(value["digest"] == json!(hash(&J::Object(preimage))?))
}
fn one(value: &J) -> bool {
    value.as_f64() == Some(1.0) || value == &J::Bool(true)
}

pub fn prepare(
    capture: &ScopeCapture<'_>,
    authored: &V,
    request_id: &str,
    declared: &[String],
    root: Option<&J>,
    limits: Option<&J>,
) -> Result<J> {
    let definition = capture.definition().to_json()?;
    let scope_basis = capture.basis().to_json()?;
    let columns = strings(&definition["fields"])?;
    let normalized = Q::lower(&authored.to_json()?, &columns)?;
    let scope = json!({"id":capture.witness().to_json()?["scope_id"],"witness":capture.witness().to_json()?,"fields":columns,"members":capture.query_rows()?.to_json()?});
    require(
        normalized["query"]["scope"] == scope["id"],
        "query scope does not match captured scope",
    )?;
    require(
        declared.windows(2).all(|w| w[0] < w[1]),
        "declared capabilities must be sorted unique text",
    )?;
    let modules = Q::required_modules(&normalized);
    let missing = modules
        .iter()
        .filter(|m| !declared.contains(m))
        .cloned()
        .collect::<Vec<_>>();
    require(
        missing.is_empty(),
        &format!("undeclared modules: {}", missing.join(", ")),
    )?;
    let root = root.cloned().unwrap_or(J::Null);
    if !root.is_null() {
        Q::witness(&root, "node")?;
    }
    let mut dependencies = Vec::new();
    let mut ids = Vec::new();
    if !root.is_null() {
        dependencies.push(root.clone());
        ids.push(root["id"].clone());
    }
    dependencies.push(scope["witness"].clone());
    ids.push(scope["id"].clone());
    ids.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
    let resources = Q::resources(limits)?;
    let request = json!({"version":4,"request_id":request_id,"resources":resources,"required_modules":modules,"operation":normalized,"root_witness":root,"scope":scope});
    let cost = Q::validate_request(&request)?;
    let mut basis = json!({"version":1,"recipe":"query-inputs/v1","profile":"core/v1","modules":modules,"as_of":scope_basis["as_of"],"operation":normalized,"scope":scope_basis,"dependencies":dependencies,"resources":resources,"preflight_cost":cost});
    sign(&mut basis)?;
    require(
        Q::canonical_json_bytes(&request)?.len() <= 16 * 1024 * 1024,
        "transport_limit",
    )?;
    Ok(
        json!({"version":1,"module":"query/v1","protocol":"KP4","resources":"resources/v4","normalized_operation":normalized,"required_modules":modules,"potential_dependencies":dependencies,"potential_ids":ids,"basis_template":basis,"request":request}),
    )
}
pub fn validate_prepared(prepared: &J) -> Result<()> {
    require(
        exact(
            prepared,
            &[
                "version",
                "module",
                "protocol",
                "resources",
                "normalized_operation",
                "required_modules",
                "potential_dependencies",
                "potential_ids",
                "basis_template",
                "request",
            ],
        ) && one(&prepared["version"])
            && prepared["module"] == "query/v1"
            && prepared["protocol"] == "KP4"
            && prepared["resources"] == "resources/v4",
        "invalid prepared query envelope",
    )?;
    let request = &prepared["request"];
    let cost = Q::validate_request(request)?;
    let root = &request["root_witness"];
    let mut dependencies = Vec::new();
    let mut ids = Vec::new();
    if !root.is_null() {
        dependencies.push(root.clone());
        ids.push(root["id"].clone());
    }
    dependencies.push(request["scope"]["witness"].clone());
    ids.push(request["scope"]["id"].clone());
    ids.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
    require(
        prepared["normalized_operation"] == request["operation"]
            && prepared["required_modules"] == request["required_modules"]
            && prepared["potential_dependencies"] == json!(dependencies)
            && prepared["potential_ids"] == json!(ids),
        "prepared request fields disagree",
    )?;
    let basis = &prepared["basis_template"];
    require(
        exact(
            basis,
            &[
                "version",
                "recipe",
                "profile",
                "modules",
                "as_of",
                "operation",
                "scope",
                "dependencies",
                "resources",
                "preflight_cost",
                "digest",
            ],
        ) && one(&basis["version"])
            && basis["recipe"] == "query-inputs/v1"
            && basis["profile"] == "core/v1"
            && basis["modules"] == request["required_modules"]
            && basis["operation"] == request["operation"]
            && basis["dependencies"] == json!(dependencies)
            && basis["resources"] == request["resources"]
            && valid_hash(basis)?,
        "invalid query basis template",
    )?;
    let scope = &basis["scope"];
    let members = request["scope"]["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].clone())
        .collect::<Vec<_>>();
    require(
        scope.is_object()
            && scope["witness"] == request["scope"]["witness"]
            && scope["members"] == json!(members)
            && scope["fields"] == request["scope"]["fields"]
            && valid_hash(scope)?,
        "query scope basis disagrees with request",
    )?;
    require(
        basis["as_of"] == scope["as_of"],
        "query basis time disagrees with scope basis",
    )?;
    require(
        basis["preflight_cost"] == cost,
        "query preflight cost disagrees with request",
    )
}
const COUNTS: &[&str] = &[
    "definite_match_count",
    "error_count",
    "input_count",
    "unknown_membership_count",
    "unknown_value_count",
];
const COST: &[&str] = &[
    "candidates",
    "evaluated_field_reads",
    "field_reads",
    "node_evaluations",
    "preflight_steps",
    "steps",
];
const DIAGNOSTICS: &[&str] = &[
    "missing_reference",
    "missing_input",
    "contested",
    "unavailable_input",
    "invalid_expression",
    "division_by_zero",
    "cyclic_reference",
    "type_error",
    "undeclared_dependency",
    "invalid_transport",
    "invalid_limits",
    "unsupported_capability",
    "step_limit",
    "depth_limit",
    "parser_depth_limit",
    "number_limit",
    "edge_limit",
    "expression_limit",
    "transport_limit",
    "node_limit",
    "missing_field",
    "collection_limit",
    "missing_column",
    "contested_column",
    "column_type_error",
    "where_unknown",
    "where_error",
    "value_error",
    "incomplete_history_scope",
    "candidate_limit",
    "field_read_limit",
    "value_limit",
    "digit_limit",
    "malformed_wire",
    "invalid_response",
];
fn natural(value: &J) -> Result<u64> {
    value
        .as_u64()
        .ok_or_else(|| Error("invalid query counter".into()))
}
fn identifier(value: &J) -> bool {
    value
        .as_str()
        .is_some_and(|s| !s.is_empty() && s.chars().count() <= 500)
}
pub fn validate_response(response: &J, prepared: &J) -> Result<J> {
    validate_prepared(prepared)?;
    require(
        exact(
            response,
            &[
                "version",
                "request_id",
                "status",
                "value",
                "diagnostics",
                "query_counts",
                "executed_reads",
                "cost",
            ],
        ) && response["version"].as_f64() == Some(4.0),
        "invalid KR4 response shape",
    )?;
    let request = &prepared["request"];
    require(
        response["request_id"] == request["request_id"],
        "KR4 request binding mismatch",
    )?;
    let status = response["status"].as_str().unwrap_or("");
    require(
        ["ok", "unknown", "error", "limit", "unsupported_capability"].contains(&status),
        "invalid KR4 status",
    )?;
    let diagnostics = response["diagnostics"]
        .as_array()
        .ok_or_else(|| Error("invalid KR4 diagnostics".into()))?;
    let scope = request["scope"]["id"].as_str().unwrap();
    let members = request["scope"]["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    let mut keys = Vec::new();
    for diagnostic in diagnostics {
        require(
            exact(diagnostic, &["code", "related_ids", "locations"])
                && diagnostic["code"]
                    .as_str()
                    .is_some_and(|s| DIAGNOSTICS.contains(&s)),
            "invalid query diagnostic",
        )?;
        let related = strings(&diagnostic["related_ids"])?;
        require(
            related.windows(2).all(|w| w[0] < w[1]),
            "diagnostic related IDs must be sorted unique text",
        )?;
        let locations = diagnostic["locations"]
            .as_array()
            .ok_or_else(|| Error("diagnostic locations must be a list".into()))?;
        let mut location_keys = Vec::new();
        for l in locations {
            require(
                exact(l, &["candidate", "column", "phase"])
                    && l["phase"]
                        .as_str()
                        .is_some_and(|s| ["where", "value", "preflight", "aggregate"].contains(&s))
                    && identifier(&l["candidate"])
                    && identifier(&l["column"]),
                "invalid diagnostic location",
            )?;
            let candidate = l["candidate"].as_str().unwrap();
            require(
                members.contains(candidate),
                "diagnostic candidate is outside captured scope",
            )?;
            require(
                related.iter().any(|s| s == scope) && related.iter().any(|s| s == candidate),
                "diagnostic location is not bound by related IDs",
            )?;
            location_keys.push((
                candidate.to_owned(),
                l["column"].as_str().unwrap().to_owned(),
                l["phase"].as_str().unwrap().to_owned(),
            ));
        }
        require(
            location_keys.windows(2).all(|w| w[0] < w[1]),
            "diagnostic locations must be sorted and unique",
        )?;
        keys.push((
            diagnostic["code"].as_str().unwrap().to_owned(),
            related,
            location_keys,
        ));
    }
    require(
        keys.windows(2).all(|w| w[0] < w[1]),
        "diagnostics must be sorted and unique",
    )?;
    require(
        response["executed_reads"] == prepared["potential_dependencies"],
        "KR4 executed reads disagree with prepared witnesses",
    )?;
    let counts = &response["query_counts"];
    if !counts.is_null() {
        require(exact(counts, COUNTS), "invalid query counts")?;
        for key in COUNTS {
            natural(&counts[key])?;
            if *key != "input_count" {
                require(
                    natural(&counts[key])? <= natural(&counts["input_count"])?,
                    "query row counters exceed input count",
                )?;
            }
        }
        require(
            natural(&counts["unknown_value_count"])? <= natural(&counts["definite_match_count"])?,
            "unknown value count exceeds definite membership",
        )?;
    }
    if status == "ok" {
        let value = &response["value"];
        require(
            !counts.is_null() && value.is_object(),
            "ok query response needs value and counts",
        )?;
        Q::typed_value(value)?;
        let mut fields = COUNTS.to_vec();
        fields.push("result");
        require(
            value["type"] == "record" && exact(&value["fields"], &fields),
            "ok query result has the wrong record shape",
        )?;
        for key in COUNTS {
            let field = &value["fields"][key];
            require(
                field["type"] == "number"
                    && field["denominator"] == "1"
                    && field["numerator"]
                        .as_str()
                        .and_then(|s| s.parse::<u64>().ok())
                        == Some(natural(&counts[key])?),
                "typed result counter disagrees with query_counts",
            )?;
        }
    } else {
        require(
            response["value"].is_null(),
            "non-ok query response must have null value",
        )?;
    }
    require(
        status != "unknown" || !counts.is_null(),
        "unknown row-scan response needs query counts",
    )?;
    require(
        !["limit", "unsupported_capability"].contains(&status) || counts.is_null(),
        "preflight/capability refusal cannot claim query counts",
    )?;
    let cost = &response["cost"];
    require(exact(cost, COST), "invalid KR4 cost shape")?;
    for key in COST {
        if *key != "node_evaluations" {
            natural(&cost[key])?;
        }
    }
    let expected = &prepared["basis_template"]["preflight_cost"];
    let resources = &request["resources"];
    for key in ["candidates", "field_reads", "preflight_steps"] {
        require(
            cost[key] == expected[key],
            "KR4 cost disagrees with prepared request",
        )?;
    }
    require(
        natural(&cost["evaluated_field_reads"])? <= natural(&cost["steps"])?
            && natural(&cost["steps"])? <= natural(&resources["steps"])?
            && natural(&cost["preflight_steps"])? <= natural(&resources["steps"])?
            && natural(&cost["candidates"])? <= natural(&resources["candidates"])?
            && natural(&cost["field_reads"])? <= natural(&resources["field_reads"])?,
        "KR4 cost disagrees with prepared request",
    )?;
    let root = &request["root_witness"];
    let expected_nodes = if root.is_null() {
        json!({})
    } else {
        let mut nodes = serde_json::Map::new();
        nodes.insert(root["id"].as_str().unwrap().into(), json!(1));
        J::Object(nodes)
    };
    require(
        crate::source_clock::python_equal(
            &V::from_json(&cost["node_evaluations"])?,
            &V::from_json(&expected_nodes)?,
        ),
        "KR4 node evaluation counts disagree with root witness",
    )?;
    require(
        counts.is_null() || counts["input_count"] == cost["candidates"],
        "query input count disagrees with captured candidates",
    )?;
    Ok(response.clone())
}
pub fn response_and_basis(response: &J, prepared: &J) -> Result<(J, J)> {
    let response = validate_response(response, prepared)?;
    let basis = if response["query_counts"].is_null() {
        J::Null
    } else {
        let mut basis = prepared["basis_template"].clone();
        basis["query_counts"] = response["query_counts"].clone();
        sign(&mut basis)?;
        basis
    };
    Ok((response, basis))
}
