//! Ordinary expression adapters select syntax explicitly without laundering raw
//! nonfinite source values into the canonical expression language.
use crate::{Result, ordinary_value::Value, value::TypedValue};
fn syntax(value: &Value) -> Result<TypedValue> {
    if let Value::Map(m) = value
        && m.len() == 1
        && let Some(Value::Text(expr)) = m.get("expr")
    {
        return Ok(TypedValue::Map(std::collections::BTreeMap::from([(
            "expr".into(),
            TypedValue::Text(expr.clone()),
        )])));
    }
    value.try_typed()
}
pub(crate) fn legacy_expression(value: &Value, predicate: bool) -> Result<Value> {
    let value = syntax(value).map_err(|_| crate::Error("invalid_legacy_expression".into()))?;
    crate::reasoning_language::legacy_expression(&value, predicate).map(|v| Value::from_typed(&v))
}
pub(crate) fn legacy_expression_detailed(value: &Value, predicate: bool) -> Result<Value> {
    let syntax = syntax(value).map_err(|error| {
        if error.0 == "invalid_yaml_key" {
            crate::Error("invalid expression fields".into())
        } else {
            error
        }
    })?;
    crate::reasoning_language::legacy_expression_detailed(&syntax, predicate)
        .map(|v| Value::from_typed(&v))
}
pub(crate) fn legacy_rule(value: &Value) -> Result<Value> {
    legacy_expression(value, false)
}
pub(crate) fn legacy_references(value: &Value) -> Vec<String> {
    syntax(value)
        .map(|v| crate::reasoning_language::legacy_references(&v))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nonfinite_extra_fields_do_not_widen_expression_grammar() {
        let value = Value::Map(crate::ordinary_value::Map::from([
            ("expr".into(), Value::Text("p.value + 1".into())),
            (
                "extra".into(),
                Value::NonFinite(crate::ordinary_value::NonFiniteFloat::NaN),
            ),
        ]));
        assert!(legacy_expression(&value, false).is_err());
    }
}
