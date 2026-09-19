//! Finite typed result algebra. This schema preserves unreduced recorded
//! rationals; query inputs impose their additional canonical-number check.
use crate::{Result, history_contract::*, history_view::list, require, value::TypedValue as V};
pub fn validate(value: &V) -> Result<()> {
    fn visit(value: &V, depth: usize) -> Result<()> {
        require(depth <= 128, "invalid_typed_value")?;
        let m = map(value)?;
        match m.get("type").and_then(|v| text(v).ok()) {
            Some("number") => {
                schema(value, &["type", "numerator", "denominator"], &[])?;
                let n = text(&m["numerator"])?;
                let d = text(&m["denominator"])?;
                let integer = |s: &str| {
                    s == "0"
                        || s.strip_prefix('-')
                            .unwrap_or(s)
                            .bytes()
                            .next()
                            .is_some_and(|b| (b'1'..=b'9').contains(&b))
                            && s.strip_prefix('-')
                                .unwrap_or(s)
                                .bytes()
                                .all(|b| b.is_ascii_digit())
                };
                require(
                    n.len() <= 4097
                        && d.len() <= 4096
                        && integer(n)
                        && integer(d)
                        && !d.starts_with('-')
                        && d != "0",
                    "invalid_typed_value",
                )?;
            }
            Some("boolean" | "text") => {
                schema(value, &["type", "value"], &[])?;
                require(
                    if string_is(&m["type"], "boolean") {
                        matches!(m["value"], V::Bool(_))
                    } else {
                        matches!(m["value"], V::Text(_))
                    },
                    "invalid_typed_value",
                )?;
            }
            Some("null") => {
                schema(value, &["type"], &[])?;
            }
            Some("list") => {
                schema(value, &["type", "items"], &[])?;
                let items = list(&m["items"])?;
                require(items.len() <= 10_000, "invalid_typed_value")?;
                for item in items {
                    visit(item, depth + 1)?;
                }
            }
            Some("record") => {
                schema(value, &["type", "fields"], &[])?;
                let fields = map(&m["fields"])?;
                require(fields.len() <= 10_000, "invalid_typed_value")?;
                for item in fields.values() {
                    visit(item, depth + 1)?;
                }
            }
            _ => return Err(error("invalid_typed_value")),
        }
        Ok(())
    }
    visit(value, 0)
}
