//! Values for ordinary reading are broader than canonical history values.
//! Nonfinite source atoms are explicit here; only the derivative Lean payload
//! lowers unavailable readings. Canonical conversion always refuses them.
use crate::{
    Error, Result, require,
    value::{Date, DateTime, FiniteFloat, Integer, TypedValue},
};
use serde_json::Value as Json;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NonFiniteFloat {
    NaN,
    PositiveInfinity,
    NegativeInfinity,
}
impl NonFiniteFloat {
    pub fn get(self) -> f64 {
        match self {
            Self::NaN => f64::NAN,
            Self::PositiveInfinity => f64::INFINITY,
            Self::NegativeInfinity => f64::NEG_INFINITY,
        }
    }
    pub fn python_str(self) -> &'static str {
        match self {
            Self::NaN => "nan",
            Self::PositiveInfinity => "inf",
            Self::NegativeInfinity => "-inf",
        }
    }
    pub fn python_json(self) -> &'static str {
        match self {
            Self::NaN => "NaN",
            Self::PositiveInfinity => "Infinity",
            Self::NegativeInfinity => "-Infinity",
        }
    }
    pub fn yaml(self) -> &'static str {
        match self {
            Self::NaN => ".nan",
            Self::PositiveInfinity => ".inf",
            Self::NegativeInfinity => "-.inf",
        }
    }
}
/// Parser/emitter scalar seam. Construction identity for Python dictionary keys
/// belongs to the source graph, independently of this scalar's numeric kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scalar {
    Finite(TypedValue),
    NonFinite(NonFiniteFloat),
}
impl Scalar {
    pub fn from_float(value: f64) -> Self {
        if value.is_nan() {
            Self::NonFinite(NonFiniteFloat::NaN)
        } else if value == f64::INFINITY {
            Self::NonFinite(NonFiniteFloat::PositiveInfinity)
        } else if value == f64::NEG_INFINITY {
            Self::NonFinite(NonFiniteFloat::NegativeInfinity)
        } else {
            Self::Finite(TypedValue::Float(
                FiniteFloat::new(value).expect("finite float"),
            ))
        }
    }
    pub fn try_typed(&self) -> Result<TypedValue> {
        match self {
            Self::Finite(v) => Ok(v.clone()),
            Self::NonFinite(_) => Err(Error("nonfinite_value_not_canonical".into())),
        }
    }
    pub fn value(&self) -> Value {
        match self {
            Self::Finite(v) => Value::from_typed(v),
            Self::NonFinite(v) => Value::NonFinite(*v),
        }
    }
}
/// Ordinary semantic projection. Source mapping order and constructor/alias
/// identity are retained separately in the ordered source graph. This ordered map is an
/// entry/field lookup index, never an implicit typed-history transport.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Null,
    Bool(bool),
    Integer(Integer),
    Float(FiniteFloat),
    NonFinite(NonFiniteFloat),
    Text(String),
    Date(Date),
    DateTime(DateTime),
    List(Vec<Value>),
    Map(Map),
}
#[derive(Clone, Debug, Default)]
pub struct Map {
    entries: Vec<(String, Value)>,
    index: BTreeMap<String, usize>,
}
impl Map {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.index.get(key).map(|i| &self.entries[*i].1)
    }
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.index.get(key).copied().map(|i| &mut self.entries[i].1)
    }
    pub fn contains_key(&self, key: &str) -> bool {
        self.index.contains_key(key)
    }
    pub fn insert(&mut self, key: String, value: Value) -> Option<Value> {
        if let Some(i) = self.index.get(&key) {
            Some(std::mem::replace(&mut self.entries[*i].1, value))
        } else {
            self.index.insert(key.clone(), self.entries.len());
            self.entries.push((key, value));
            None
        }
    }
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Value)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }
    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.entries.iter().map(|(k, _)| k)
    }
    pub fn values(&self) -> impl Iterator<Item = &Value> {
        self.entries.iter().map(|(_, v)| v)
    }
    pub fn into_keys(self) -> impl Iterator<Item = String> {
        self.entries.into_iter().map(|(k, _)| k)
    }
    pub fn into_values(self) -> impl Iterator<Item = Value> {
        self.entries.into_iter().map(|(_, v)| v)
    }
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        let i = self.index.remove(key)?;
        let (_, value) = self.entries.remove(i);
        for (j, (k, _)) in self.entries.iter().enumerate().skip(i) {
            self.index.insert(k.clone(), j);
        }
        Some(value)
    }
}
impl PartialEq for Map {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().all(|(k, v)| other.get(k) == Some(v))
    }
}
impl Eq for Map {}
impl<const N: usize> From<[(String, Value); N]> for Map {
    fn from(v: [(String, Value); N]) -> Self {
        v.into_iter().collect()
    }
}
impl FromIterator<(String, Value)> for Map {
    fn from_iter<T: IntoIterator<Item = (String, Value)>>(values: T) -> Self {
        let mut out = Self::new();
        out.extend(values);
        out
    }
}
impl Extend<(String, Value)> for Map {
    fn extend<T: IntoIterator<Item = (String, Value)>>(&mut self, values: T) {
        for (k, v) in values {
            self.insert(k, v);
        }
    }
}
impl IntoIterator for Map {
    type Item = (String, Value);
    type IntoIter = std::vec::IntoIter<Self::Item>;
    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}
impl<'a> IntoIterator for &'a Map {
    type Item = &'a (String, Value);
    type IntoIter = std::slice::Iter<'a, (String, Value)>;
    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter()
    }
}
impl std::ops::Index<&str> for Map {
    type Output = Value;
    fn index(&self, key: &str) -> &Value {
        self.get(key).expect("known ordinary field")
    }
}
impl std::ops::Index<&String> for Map {
    type Output = Value;
    fn index(&self, key: &String) -> &Value {
        self.get(key.as_str()).expect("known ordinary field")
    }
}
impl Value {
    pub fn from_typed(value: &TypedValue) -> Self {
        match value {
            TypedValue::Null => Self::Null,
            TypedValue::Bool(v) => Self::Bool(*v),
            TypedValue::Integer(v) => Self::Integer(v.clone()),
            TypedValue::Float(v) => Self::Float(*v),
            TypedValue::Text(v) => Self::Text(v.clone()),
            TypedValue::Date(v) => Self::Date(v.clone()),
            TypedValue::DateTime(v) => Self::DateTime(v.clone()),
            TypedValue::List(v) => Self::List(v.iter().map(Self::from_typed).collect()),
            TypedValue::Map(v) => Self::Map(
                v.iter()
                    .map(|(k, v)| (k.clone(), Self::from_typed(v)))
                    .collect(),
            ),
        }
    }
    pub fn from_json(value: &Json) -> Result<Self> {
        Ok(Self::from_typed(&TypedValue::from_json(value)?))
    }
    pub fn try_typed(&self) -> Result<TypedValue> {
        fn convert(value: &Value, depth: usize) -> Result<TypedValue> {
            require(depth <= crate::value::MAX_DEPTH, "history_limit")?;
            Ok(match value {
                Value::Null => TypedValue::Null,
                Value::Bool(v) => TypedValue::Bool(*v),
                Value::Integer(v) => TypedValue::Integer(v.clone()),
                Value::Float(v) => TypedValue::Float(*v),
                Value::NonFinite(_) => return Err(Error("nonfinite_value_not_canonical".into())),
                Value::Text(v) => TypedValue::Text(v.clone()),
                Value::Date(v) => TypedValue::Date(v.clone()),
                Value::DateTime(v) => TypedValue::DateTime(v.clone()),
                Value::List(v) => TypedValue::List(
                    v.iter()
                        .map(|v| convert(v, depth + 1))
                        .collect::<Result<_>>()?,
                ),
                Value::Map(v) => TypedValue::Map(
                    v.iter()
                        .map(|(k, v)| Ok((k.clone(), convert(v, depth + 1)?)))
                        .collect::<Result<_>>()?,
                ),
            })
        }
        let value = convert(self, 0)?;
        value.validate()?;
        Ok(value)
    }
    /// Strict JSON with ordinary date formatting; nonfinite values do not become
    /// null. Use python_json for Python-compatible ordinary output instead.
    pub fn json_value(&self) -> Result<Json> {
        match self {
            Self::NonFinite(_) => Err(Error("nonfinite_value_not_json".into())),
            Self::Date(v) => Ok(Json::String(v.as_str().into())),
            Self::DateTime(v) => Ok(Json::String(v.as_str().replacen('T', " ", 1))),
            Self::List(v) => Ok(Json::Array(
                v.iter().map(Self::json_value).collect::<Result<_>>()?,
            )),
            Self::Map(v) => Ok(Json::Object(
                v.iter()
                    .map(|(k, v)| Ok((k.clone(), v.json_value()?)))
                    .collect::<Result<_>>()?,
            )),
            _ => self.try_typed()?.to_json(),
        }
    }
    pub fn truth(&self) -> bool {
        match self {
            Self::Null => false,
            Self::Bool(v) => *v,
            Self::Integer(v) => v.as_str() != "0",
            Self::Float(v) => v.get() != 0.0,
            Self::NonFinite(_) => true,
            Self::Text(v) => !v.is_empty(),
            Self::List(v) => !v.is_empty(),
            Self::Map(v) => !v.is_empty(),
            _ => true,
        }
    }
    /// Python's JSON extension is intentionally output-only. A generic strict
    /// JSON parser must not begin accepting these tokens as canonical values.
    pub fn python_json(&self, spaces: bool) -> Result<String> {
        fn write(value: &Value, out: &mut String, depth: usize, spaces: bool) -> Result<()> {
            require(depth <= 400, "assessment nesting limit")?;
            match value {
                Value::Null => out.push_str("null"),
                Value::Bool(v) => out.push_str(if *v { "true" } else { "false" }),
                Value::Integer(v) => out.push_str(v.as_str()),
                Value::Float(v) => out.push_str(&crate::identity::python_float(v.get())),
                Value::NonFinite(v) => out.push_str(v.python_json()),
                Value::Text(v) => out.push_str(&serde_json::to_string(v)?),
                Value::Date(v) => out.push_str(&serde_json::to_string(v.as_str())?),
                Value::DateTime(v) => {
                    out.push_str(&serde_json::to_string(&v.as_str().replacen('T', " ", 1))?)
                }
                Value::List(values) => {
                    out.push('[');
                    for (i, v) in values.iter().enumerate() {
                        if i > 0 {
                            out.push_str(if spaces { ", " } else { "," });
                        }
                        write(v, out, depth + 1, spaces)?;
                    }
                    out.push(']');
                }
                Value::Map(values) => {
                    out.push('{');
                    let mut sorted = values.iter().collect::<Vec<_>>();
                    sorted.sort_by(|a, b| a.0.cmp(b.0));
                    for (i, (k, v)) in sorted.into_iter().enumerate() {
                        if i > 0 {
                            out.push_str(if spaces { ", " } else { "," });
                        }
                        out.push_str(&serde_json::to_string(k)?);
                        out.push_str(if spaces { ": " } else { ":" });
                        write(v, out, depth + 1, spaces)?;
                    }
                    out.push('}');
                }
            }
            require(out.len() <= 512 * 1024 * 1024, "assessment byte limit")
        }
        let mut out = String::new();
        write(self, &mut out, 0, spaces)?;
        Ok(out)
    }
    pub fn python_str(&self) -> String {
        match self {
            Self::NonFinite(v) => v.python_str().into(),
            Self::Null => "None".into(),
            Self::Bool(v) => if *v { "True" } else { "False" }.into(),
            Self::Integer(v) => v.as_str().into(),
            Self::Float(v) => crate::identity::python_float(v.get()),
            Self::Text(v) => v.clone(),
            Self::Date(v) => v.as_str().into(),
            Self::DateTime(v) => v.as_str().replacen('T', " ", 1),
            Self::List(_) | Self::Map(_) => self.python_repr(),
        }
    }
    pub fn python_repr(&self) -> String {
        match self {
            Self::Text(v) => crate::source_text::python_repr(
                &crate::history_yaml::SourceValue::Scalar(TypedValue::Text(v.clone())),
            ),
            Self::Date(v) => crate::source_text::python_repr(
                &crate::history_yaml::SourceValue::Scalar(TypedValue::Date(v.clone())),
            ),
            Self::DateTime(v) => crate::source_text::python_repr(
                &crate::history_yaml::SourceValue::Scalar(TypedValue::DateTime(v.clone())),
            ),
            Self::List(v) => format!(
                "[{}]",
                v.iter()
                    .map(Self::python_repr)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Map(v) => format!(
                "{{{}}}",
                v.iter()
                    .map(|(k, v)| format!(
                        "{}: {}",
                        Self::Text(k.clone()).python_repr(),
                        v.python_repr()
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            _ => self.python_str(),
        }
    }
}
impl From<&TypedValue> for Value {
    fn from(value: &TypedValue) -> Self {
        Self::from_typed(value)
    }
}
pub fn map(value: &Value) -> Result<&Map> {
    if let Value::Map(v) = value {
        Ok(v)
    } else {
        Err(Error("expected_mapping".into()))
    }
}
pub fn map_mut(value: &mut Value) -> Result<&mut Map> {
    if let Value::Map(v) = value {
        Ok(v)
    } else {
        Err(Error("expected_mapping".into()))
    }
}
pub fn text(value: &Value) -> Result<&str> {
    if let Value::Text(v) = value {
        Ok(v)
    } else {
        Err(Error("expected_text".into()))
    }
}
pub fn string_is(value: &Value, expected: &str) -> bool {
    matches!(value,Value::Text(v)if v==expected)
}
pub fn get<'a>(value: &'a Map, key: &str) -> &'a Value {
    value.get(key).unwrap_or(&Value::Null)
}

/// This is a derivative computation request, not a replacement document. The
/// unavailable locations remain explicit so no caller mistakes it for source.
#[derive(Clone, Debug)]
pub struct FinitePayload {
    pub value: Json,
    pub unavailable: Vec<Vec<String>>,
}
pub fn finite_payload(value: &Value) -> Result<FinitePayload> {
    fn lower(
        value: &Value,
        path: &mut Vec<String>,
        unavailable: &mut Vec<Vec<String>>,
        depth: usize,
    ) -> Result<Json> {
        require(depth <= 128, "ordinary_input_limit")?;
        Ok(match value {
            Value::NonFinite(_) => {
                unavailable.push(path.clone());
                Json::Null
            }
            Value::List(v) => Json::Array(
                v.iter()
                    .enumerate()
                    .map(|(i, v)| {
                        path.push(i.to_string());
                        let result = lower(v, path, unavailable, depth + 1);
                        path.pop();
                        result
                    })
                    .collect::<Result<_>>()?,
            ),
            Value::Map(v) => Json::Object(
                v.iter()
                    .map(|(k, v)| {
                        path.push(k.clone());
                        let result = lower(v, path, unavailable, depth + 1);
                        path.pop();
                        Ok((k.clone(), result?))
                    })
                    .collect::<Result<_>>()?,
            ),
            _ => value.json_value()?,
        })
    }
    let mut unavailable = Vec::new();
    let value = lower(value, &mut Vec::new(), &mut unavailable, 0)?;
    Ok(FinitePayload { value, unavailable })
}
pub fn record_body(body: &Value) -> Value {
    let body = match body {
        Value::Map(v) => v.clone(),
        v => Map::from([("v".into(), v.clone())]),
    };
    let reading = body
        .get("v")
        .filter(|v| **v != Value::Null)
        .or_else(|| body.get("quoted"));
    if get(&body, "rule") == &Value::Null
        && reading.is_some_and(|v| matches!(v, Value::NonFinite(_)))
    {
        Value::Map(Map::from([("v".into(), Value::Null)]))
    } else {
        Value::Map(body)
    }
}
pub fn compute_record(raw: &Map, ids: &BTreeSet<String>) -> Result<FinitePayload> {
    let mut lost = Vec::new();
    let nodes = raw
        .iter()
        .filter(|(id, _)| ids.contains(*id))
        .map(|(id, body)| {
            let prepared = record_body(body);
            if prepared != *body && matches!(body, Value::NonFinite(_))
                || map(body).is_ok_and(|m| {
                    get(m, "rule") == &Value::Null
                        && m.get("v")
                            .filter(|v| **v != Value::Null)
                            .or_else(|| m.get("quoted"))
                            .is_some_and(|v| matches!(v, Value::NonFinite(_)))
                })
            {
                lost.push(vec!["nodes".into(), id.clone(), "body".into(), "v".into()]);
            }
            (
                id.clone(),
                Value::Map(Map::from([("body".into(), prepared)])),
            )
        })
        .collect();
    let mut payload = finite_payload(&Value::Map(Map::from([(
        "nodes".into(),
        Value::Map(nodes),
    )])))?;
    payload.unavailable.extend(lost);
    payload.unavailable.sort();
    payload.unavailable.dedup();
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn value(j: &Json) -> Value {
        let a = j.as_array().unwrap();
        match a[0].as_str().unwrap() {
            "nonfinite" => Value::NonFinite(match a[1].as_str().unwrap() {
                "nan" => NonFiniteFloat::NaN,
                "inf" => NonFiniteFloat::PositiveInfinity,
                "-inf" => NonFiniteFloat::NegativeInfinity,
                _ => panic!(),
            }),
            "list" => Value::List(a[1].as_array().unwrap().iter().map(value).collect()),
            "map" => Value::Map(
                a[1].as_array()
                    .unwrap()
                    .iter()
                    .map(|r| (r[0].as_str().unwrap().into(), value(&r[1])))
                    .collect(),
            ),
            _ => Value::from_typed(&TypedValue::from_tagged(j).unwrap()),
        }
    }
    fn corpus() -> Json {
        serde_json::from_str(include_str!(
            "../tests/fixtures/ordinary-nonfinite-values.json"
        ))
        .unwrap()
    }
    #[test]
    fn ordinary_bytes_and_finite_wire_match_frozen_python() {
        for row in corpus()["values"].as_array().unwrap() {
            let source = value(&row["value"]);
            let original = source.clone();
            assert_eq!(
                source.python_json(true).unwrap(),
                row["json"].as_str().unwrap()
            );
            assert_eq!(
                source.python_json(false).unwrap(),
                row["compact"].as_str().unwrap()
            );
            assert_eq!(source.python_str(), row["str"].as_str().unwrap());
            assert_eq!(source.python_repr(), row["repr"].as_str().unwrap());
            let finite = finite_payload(&source).unwrap();
            assert_eq!(
                finite.value,
                serde_json::from_str::<Json>(row["finite"].as_str().unwrap()).unwrap()
            );
            assert_eq!(source, original);
        }
    }
    #[test]
    fn derivative_primary_guard_never_falls_through_to_quoted_or_verdict() {
        for row in corpus()["bodies"].as_array().unwrap() {
            let body = value(&row["body"]);
            let original = body.clone();
            assert_eq!(record_body(&body), value(&row["expected"]));
            let payload = compute_record(
                &Map::from([("p.value".into(), body.clone())]),
                &BTreeSet::from(["p.value".into()]),
            )
            .unwrap();
            assert_eq!(
                payload.value,
                serde_json::from_str::<Json>(row["wire"].as_str().unwrap()).unwrap()
            );
            assert_eq!(body, original);
            if matches!(body, Value::NonFinite(_))
                || map(&body).is_ok_and(|m| matches!(get(m, "v"), Value::NonFinite(_)))
            {
                assert!(!payload.unavailable.is_empty());
            }
        }
    }
    #[test]
    fn canonical_history_and_strict_json_boundaries_remain_closed() {
        for special in [
            NonFiniteFloat::NaN,
            NonFiniteFloat::PositiveInfinity,
            NonFiniteFloat::NegativeInfinity,
        ] {
            let v = Value::NonFinite(special);
            assert!(v.try_typed().is_err());
            assert!(v.json_value().is_err());
            assert!(FiniteFloat::new(special.get()).is_err());
            assert!(
                crate::history_yaml::decode_document(format!("v: {}\n", special.yaml()).as_bytes())
                    .is_err()
            );
            assert!(
                TypedValue::from_tagged(&serde_json::json!(["float", special.python_str()]))
                    .is_err()
            );
            assert!(
                crate::json_ingress::parse_slice(
                    special.python_json().as_bytes(),
                    crate::json_ingress::DuplicateKeys::Reject
                )
                .is_err()
            );
        }
        let impostor = serde_json::json!({"kind":"nonfinite","value":"nan"});
        let v = Value::from_json(&impostor).unwrap();
        assert_eq!(v.json_value().unwrap(), impostor);
        assert!(v.try_typed().is_ok());
        assert_eq!(
            Value::Text("NaN".into()).python_json(false).unwrap(),
            "\"NaN\""
        );
        assert_ne!(Value::Null, Value::NonFinite(NonFiniteFloat::NaN));
    }
}
