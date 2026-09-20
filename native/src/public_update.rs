//! Strict source-report parsing and planning for the synchronous update CLI.
//!
//! Publication is deliberately kept behind the existing captured transaction
//! writers. This module owns the report boundary and produces ordered native
//! actions; it never loops public commands or treats a shadow copy as authority.
use crate::{Error, Result, require, value::TypedValue as V};
use serde_json::Value as J;

#[derive(Clone, Debug)]
pub struct Report {
    pub date: V,
    pub source: Option<V>,
    pub at: Option<V>,
    pub scope: Option<V>,
    pub profile: Option<V>,
    pub updates: Vec<V>,
}

fn object<'a>(value: &'a J, code: &str) -> Result<&'a serde_json::Map<String, J>> {
    value.as_object().ok_or_else(|| Error(code.into()))
}
fn required_text(map: &serde_json::Map<String, J>, key: &str) -> Result<V> {
    let value = map.get(key).ok_or_else(|| Error(format!("missing_{key}")))?;
    let text = value.as_str().filter(|s| !s.is_empty()).ok_or_else(|| Error(format!("invalid_{key}")))?;
    Ok(V::Text(text.into()))
}
fn optional_value(map: &serde_json::Map<String, J>, key: &str) -> Result<Option<V>> {
    map.get(key).filter(|v| !v.is_null()).map(V::from_json).transpose()
}

/// Parse one source report without reading or writing a record.
pub fn parse(raw: &[u8]) -> Result<Report> {
    let input = crate::json_ingress::parse_slice(raw, crate::json_ingress::DuplicateKeys::Reject)?;
    let top = object(&input, "report_must_be_object")?;
    let date = required_text(top, "date")?;
    crate::value::Date::new(match &date { V::Text(s) => s, _ => unreachable!() })?;
    let updates = match top.get("updates") {
        Some(value) => value.as_array().ok_or_else(|| Error("updates_must_be_array".into()))?,
        None => return Err(Error("missing_updates".into())),
    };
    require(!updates.is_empty() && updates.len() <= 64, "invalid_updates")?;
    let mut actions = Vec::with_capacity(updates.len());
    for update in updates {
        let map = object(update, "update_must_be_object")?;
        let kind = required_text(map, "kind")?;
        require(matches!(&kind, V::Text(s) if ["add", "set", "review"].contains(&s.as_str())), "invalid_update_kind")?;
        let id = required_text(map, "id")?;
        crate::history_paths::subject(match &id { V::Text(s) => s, _ => unreachable!() })?;
        let mut action = serde_json::Map::new();
        action.insert("kind".into(), kind.to_json()?);
        action.insert("id".into(), id.to_json()?);
        action.insert("as_of".into(), date.to_json()?);
        match action["kind"].as_str() {
            Some("set") => {
                let value = map.get("value").ok_or_else(|| Error("set_requires_value".into()))?;
                action.insert("value".into(), value.clone());
            }
            Some("add") => {
                let body = map.get("body").ok_or_else(|| Error("add_requires_body".into()))?;
                require(body.is_object(), "add_body_must_be_object")?;
                action.insert("body".into(), body.clone());
                if let Some(into) = map.get("into") { action.insert("into".into(), into.clone()); }
            }
            Some("review") => {}
            _ => unreachable!(),
        }
        if let Some(at) = map.get("at") { action.insert("at".into(), at.clone()); }
        if let Some(drops) = map.get("drops") { action.insert("drops".into(), drops.clone()); }
        actions.push(V::from_json(&J::Object(action))?);
    }
    Ok(Report {
        date,
        source: optional_value(top, "source")?,
        at: optional_value(top, "at")?,
        scope: optional_value(top, "scope")?,
        profile: optional_value(top, "profile")?,
        updates: actions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_ordered_set_then_add_report() {
        let report = parse(br#"{"date":"2026-09-19","source":"s.report","updates":[{"kind":"set","id":"p.a","value":2},{"kind":"add","id":"p.b","body":{"rests_on":["p.a"],"verdict":"ok"}}]}"#).unwrap();
        assert_eq!(report.updates.len(), 2);
        assert_eq!(report.updates[0].to_json().unwrap()["kind"], "set");
        assert_eq!(report.updates[1].to_json().unwrap()["kind"], "add");
    }
    #[test]
    fn rejects_malformed_and_duplicate_reports() {
        assert_eq!(parse(br#"{}"#).unwrap_err().to_string(), "missing_date");
        assert!(parse(br#"{"date":"2026-09-19","updates":[]}"#).is_err());
        assert!(parse(br#"{"date":"2026-09-19","updates":[],"date":"2026-09-20"}"#).is_err());
    }
}
