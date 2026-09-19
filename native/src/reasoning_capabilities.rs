//! Authorize the actual document's computational and historical declarations.
//! A valid declaration alone does not authorize malformed dependent evidence.
use crate::{
    Result,
    history_contract::*,
    history_view::{list, map_mut},
    reasoning_fields as F, reasoning_language as L, reasoning_query as Q,
    reasoning_snapshot::{CaptureOptions, Snapshot, digest, entries},
    require,
    value::{Integer, TypedValue as V},
};
fn s(value: &str) -> V {
    V::Text(value.into())
}
fn list_text(v: &V) -> Result<Vec<String>> {
    list(v)?
        .iter()
        .map(|v| text(v).map(str::to_owned))
        .collect()
}
fn scope_fields(document: &V, scope: &str) -> Result<Vec<String>> {
    let snapshot = Snapshot::from_data(document, CaptureOptions::default())?;
    let capture = snapshot.capture_query_scope(scope, None)?;
    list_text(&map(capture.definition())?["fields"])
}

fn expression(value: &V, document: Option<&V>, declared: &[String]) -> Result<()> {
    if let V::Map(m) = value
        && m.len() == 1
        && m.contains_key("query")
    {
        let validate = (|| -> Result<()> {
            let document = document.ok_or_else(|| error("invalid_expression"))?;
            let scope = text(field(map(&m["query"])?, "scope")?)?;
            let fields = scope_fields(document, scope)?;
            let lowered = Q::lower(&value.to_json()?, &fields)?;
            require(
                Q::required_modules(&lowered)
                    .iter()
                    .all(|m| declared.contains(m)),
                "invalid_capability",
            )
        })();
        return validate.map_err(|e| {
            if e.0 == "invalid_capability" {
                e
            } else {
                error("invalid_expression")
            }
        });
    }
    let lowered = L::lower(value).map_err(|e| {
        if e.0 == "unsupported_capability" {
            e
        } else {
            error("invalid_expression")
        }
    })?;
    require(
        !L::references(&lowered)
            .iter()
            .any(|s| F::BUILTINS.contains(&s.as_str())),
        "unsupported_core_builtin",
    )
}
pub fn compare_basis(current: &V, historical: &V) -> Result<&'static str> {
    if *historical == V::Null {
        return Ok("not_recorded");
    }
    let (Ok(a), Ok(b)) = (map(current), map(historical)) else {
        return Ok("unavailable");
    };
    for m in [a, b] {
        let version = m.get("version").is_some_and(|v| {
            crate::source_clock::python_equal(v, &V::Integer(Integer::new("1").unwrap()))
        });
        if !version
            || !m.get("profile").is_some_and(|v| string_is(v, "core/v1"))
            || !m.get("digest").is_some_and(|v| matches!(v, V::Text(_)))
        {
            return Ok("unavailable");
        }
        let preimage = V::Map(
            m.iter()
                .filter(|(k, _)| k.as_str() != "digest")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        );
        if m["digest"] != s(&digest(&preimage)?) {
            return Ok("unavailable");
        }
    }
    Ok(if a["digest"] == b["digest"] {
        "same"
    } else {
        "changed"
    })
}
fn historical_envelope(computed: &Map, document: &V) -> Result<()> {
    require(is_int(&computed["version"], "2"), "unsupported_history")?;
    let declaration = map(document)?
        .get("meta")
        .and_then(|v| map(v).ok())
        .and_then(|m| m.get("reasoning"))
        .and_then(|v| map(v).ok())
        .ok_or_else(|| error("invalid_capability"))?;
    require(
        declaration.get("version").is_some_and(|v| {
            crate::source_clock::python_equal(v, &V::Integer(Integer::new("2").unwrap()))
        }) && declaration
            .get("profile")
            .is_some_and(|v| string_is(v, "core/v1")),
        "invalid_capability",
    )?;
    let valid = (|| -> Result<()> {
        schema(
            &V::Map(computed.clone()),
            &["version", "value", "basis"],
            &["rule"],
        )?;
        crate::reasoning_values::validate(&computed["value"])?;
        require(
            compare_basis(&computed["basis"], &computed["basis"])? == "same",
            "invalid_history",
        )
    })();
    valid.map_err(|_| error("invalid_history"))
}
fn historical_rule(computed: &Map, dependency: &str) -> Result<()> {
    let rule = map(&computed["rule"])?;
    if rule.len() == 1 && rule.contains_key("query") {
        let check = (|| -> Result<()> {
            let basis = map(field(computed, "basis")?)?;
            require(
                basis
                    .get("recipe")
                    .is_some_and(|v| string_is(v, "query-inputs/v1")),
                "invalid_history",
            )?;
            let scope = map(field(basis, "scope")?)?;
            require(
                scope
                    .get("recipe")
                    .is_some_and(|v| string_is(v, "scope-inputs/v2")),
                "invalid_history",
            )?;
            let fields = list_text(field(scope, "fields")?)?;
            let members = list_text(field(scope, "members")?)?;
            for list in [&fields, &members] {
                require(
                    list.iter().all(|s| !s.is_empty()) && list.windows(2).all(|w| w[0] < w[1]),
                    "invalid_history",
                )?;
            }
            let lowered = Q::lower(&computed["rule"].to_json()?, &fields)?;
            let witness = map(field(scope, "witness")?)?;
            require(
                witness.get("kind").is_some_and(|v| string_is(v, "scope"))
                    && witness.get("scope_id").and_then(|v| text(v).ok())
                        == lowered["query"]["scope"].as_str()
                    && field(basis, "operation")? == &V::from_json(&lowered)?
                    && list_text(field(basis, "modules")?)? == Q::required_modules(&lowered),
                "invalid_history",
            )
        })();
        return check.map_err(|_| error("invalid_history"));
    }
    if rule.len() != 2 || !rule.contains_key("collection") || !rule.contains_key("fields") {
        return expression(&computed["rule"], None, &[]);
    }
    let check = (|| -> Result<()> {
        require(!text(&rule["collection"])?.is_empty(), "invalid_history")?;
        let fields = list_text(&rule["fields"])?;
        require(
            fields.iter().all(|s| !s.is_empty()) && fields.windows(2).all(|w| w[0] < w[1]),
            "invalid_history",
        )?;
        let basis = map(field(computed, "basis")?)?;
        if let Some(witness) = basis.get("witness").filter(|v| **v != V::Null) {
            let witness = map(witness)?;
            require(
                witness.get("kind").is_some_and(|v| string_is(v, "scope"))
                    && witness.get("scope_id") == Some(&s(dependency))
                    && witness.get("definition_digest") == Some(&s(&digest(&computed["rule"])?)),
                "invalid_history",
            )?;
        } else {
            require(
                !basis.contains_key("recipe")
                    && basis.contains_key("members")
                    && basis.contains_key("dependencies"),
                "invalid_history",
            )?;
        }
        require(
            map(field(computed, "value")?)?
                .get("type")
                .is_some_and(|v| string_is(v, "record")),
            "invalid_history",
        )
    })();
    check.map_err(|_| error("invalid_history"))
}
pub fn document_capabilities(document: &V) -> Result<V> {
    let doc = map(document)?;
    require(
        !doc.get("meta")
            .and_then(|v| map(v).ok())
            .is_some_and(|m| m.contains_key("history")),
        "unsupported_history_contribution",
    )?;
    let cap = F::capabilities(document, None)?;
    let c = map(&cap)?;
    let core = string_is(&c["profile"], "core/v1");
    let declared = list_text(&c["requires"])?;
    let entries = entries(document)?;
    let has_history = entries
        .values()
        .filter_map(|(_, v)| map(v).ok())
        .flat_map(|m| m.values())
        .filter_map(|v| map(v).ok())
        .flat_map(|m| m.values())
        .filter_map(|v| map(v).ok())
        .filter_map(|m| m.get("computed"))
        .filter_map(|v| map(v).ok())
        .any(|m| m.contains_key("version"));
    if !core && !has_history {
        return Ok(cap);
    }
    let fields = F::snapshot_fields(document)?;
    let snapshot = text(&fields["snapshot"])?;
    for (_, body) in entries.values() {
        let V::Map(body) = body else { continue };
        if let Some(V::Map(seen)) = body.get(snapshot) {
            for (dependency, old) in seen {
                let Some(computed) = map(old)
                    .ok()
                    .and_then(|m| m.get("computed"))
                    .and_then(|v| map(v).ok())
                else {
                    continue;
                };
                if let Some(V::Map(basis)) = computed.get("basis") {
                    let empty = V::List(Vec::new());
                    let modules = list_text(basis.get("modules").unwrap_or(&empty))
                        .map_err(|_| error("invalid_history"))?;
                    require(
                        modules.iter().all(|m| {
                            ["arithmetic/v1", "composition/v1", "query/v1"].contains(&m.as_str())
                        }) && basis.get("profile").is_none_or(|v| string_is(v, "core/v1")),
                        "unsupported_capability",
                    )?;
                    require(
                        modules.iter().all(|m| declared.contains(m)),
                        "invalid_capability",
                    )?;
                }
                if computed.contains_key("version") {
                    historical_envelope(computed, document)?;
                }
                if core && computed.get("rule").is_some_and(|v| matches!(v, V::Map(_))) {
                    historical_rule(computed, dependency)?;
                }
            }
        }
        if !core {
            continue;
        }
        for key in ["rule", text(&fields["predicate"])?] {
            if let Some(value @ V::Map(_)) = body.get(key) {
                expression(value, Some(document), &declared)?;
            }
        }
        if let Some(V::List(deps)) = body.get(text(&fields["deps"])?) {
            require(
                !deps
                    .iter()
                    .filter_map(|v| text(v).ok())
                    .any(|s| F::BUILTINS.contains(&s)),
                "unsupported_core_builtin",
            )?;
        }
    }
    Ok(cap)
}
pub(crate) fn history_document_capabilities(document: &V) -> Result<V> {
    let mut plain = document.clone();
    let doc = map_mut(&mut plain)?;
    if let Some(V::Map(meta)) = doc.get_mut("meta") {
        meta.remove("history");
    }
    document_capabilities(&plain)
}
