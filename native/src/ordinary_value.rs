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
    source_keys: BTreeMap<String, Scalar>,
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
    fn index_for_text(&self, key: &str) -> Option<usize> {
        if let Some(i) = self.index.get(key).copied() {
            if self
                .source_keys
                .get(key)
                .is_none_or(|s| matches!(s,Scalar::Finite(TypedValue::Text(t))if t==key))
            {
                return Some(i);
            }
        }
        self.entries.iter().position(|(index, _)| {
            self.source_keys
                .get(index)
                .is_some_and(|s| matches!(s,Scalar::Finite(TypedValue::Text(t))if t==key))
        })
    }
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.index_for_text(key).map(|i| &self.entries[i].1)
    }
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.index_for_text(key).map(|i| &mut self.entries[i].1)
    }
    pub fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }
    fn by_index(&self, key: &str) -> Option<&Value> {
        self.index.get(key).map(|i| &self.entries[*i].1)
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
    pub fn retain(&mut self, mut keep: impl FnMut(&String, &mut Value) -> bool) {
        self.entries.retain_mut(|(k, v)| keep(k, v));
        self.index = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, (k, _))| (k.clone(), i))
            .collect();
    }
    pub fn insert_source_key(&mut self, index: String, key: Scalar, value: Value) -> Option<Value> {
        if !self.index.contains_key(&index) {
            self.source_keys.insert(index.clone(), key);
        }
        self.insert(index, value)
    }
    pub fn source_key(&self, key: &str) -> Scalar {
        self.source_keys
            .get(key)
            .cloned()
            .unwrap_or_else(|| Scalar::Finite(TypedValue::Text(key.into())))
    }
    pub fn has_nonfinite_key(&self) -> bool {
        self.source_keys
            .values()
            .any(|k| matches!(k, Scalar::NonFinite(_)))
    }
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&String, &mut Value)> {
        self.entries.iter_mut().map(|(k, v)| (&*k, v))
    }
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut Value> {
        self.entries.iter_mut().map(|(_, v)| v)
    }
    pub fn entry(&mut self, key: String) -> Entry<'_> {
        Entry { map: self, key }
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
        self.source_keys.remove(key);
        let (_, value) = self.entries.remove(i);
        for (j, (k, _)) in self.entries.iter().enumerate().skip(i) {
            self.index.insert(k.clone(), j);
        }
        Some(value)
    }
}
pub struct Entry<'a> {
    map: &'a mut Map,
    key: String,
}
impl<'a> Entry<'a> {
    pub fn or_insert_with(self, make: impl FnOnce() -> Value) -> &'a mut Value {
        let i = if let Some(i) = self.map.index_for_text(&self.key) {
            i
        } else {
            let i = self.map.entries.len();
            self.map.insert(self.key, make());
            i
        };
        &mut self.map.entries[i].1
    }
    pub fn or_insert(self, value: Value) -> &'a mut Value {
        self.or_insert_with(|| value)
    }
}
impl PartialEq for Map {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().all(|(k, v)| other.by_index(k) == Some(v))
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
        self.by_index(key).expect("known ordinary field")
    }
}
impl std::ops::Index<&String> for Map {
    type Output = Value;
    fn index(&self, key: &String) -> &Value {
        self.by_index(key.as_str()).expect("known ordinary field")
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
    pub fn from_finite_projection(value: &TypedValue) -> Self {
        match value {
            TypedValue::Map(values) => {
                let mut out = Map::new();
                for (k, v) in values {
                    let key = crate::history_yaml::projected_ordinary_key(k)
                        .map(Scalar::Finite)
                        .unwrap_or_else(|| Scalar::Finite(TypedValue::Text(k.clone())));
                    out.insert_source_key(k.clone(), key, Self::from_finite_projection(v));
                }
                Self::Map(out)
            }
            TypedValue::List(values) => {
                Self::List(values.iter().map(Self::from_finite_projection).collect())
            }
            _ => Self::from_typed(value),
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
                Value::Map(v) => {
                    let mut result = BTreeMap::new();
                    for (k, item) in v.iter() {
                        let key = match v.source_key(k) {
                            Scalar::Finite(TypedValue::Text(k)) => k,
                            _ => return Err(Error("invalid_yaml_key".into())),
                        };
                        require(
                            result.insert(key, convert(item, depth + 1)?).is_none(),
                            "invalid_yaml_key",
                        )?;
                    }
                    TypedValue::Map(result)
                }
            })
        }
        let value = convert(self, 0)?;
        value.validate()?;
        Ok(value)
    }
    pub fn finite_projection(&self) -> Result<TypedValue> {
        Ok(match self {
            Self::Map(m) => {
                require(
                    !m.has_nonfinite_key(),
                    "invalid_history_value: snapshot mappings require string keys",
                )?;
                TypedValue::Map(
                    m.iter()
                        .map(|(k, v)| Ok((k.clone(), v.finite_projection()?)))
                        .collect::<Result<_>>()?,
                )
            }
            Self::List(v) => TypedValue::List(
                v.iter()
                    .map(Self::finite_projection)
                    .collect::<Result<_>>()?,
            ),
            Self::NonFinite(_) => {
                return Err(Error(
                    "invalid_history_value: snapshot data contains a nonfinite value".into(),
                ));
            }
            _ => self.try_typed()?,
        })
    }
    pub fn validate(&self) -> Result<()> {
        let mut pending = vec![(self, 0)];
        let mut count = 0;
        while let Some((v, depth)) = pending.pop() {
            count += 1;
            require(depth <= crate::value::MAX_DEPTH, "value_depth")?;
            require(count <= crate::value::MAX_VALUES, "value_limit")?;
            match v {
                Self::List(v) => pending.extend(v.iter().map(|v| (v, depth + 1))),
                Self::Map(v) => pending.extend(v.values().map(|v| (v, depth + 1))),
                _ => {}
            }
        }
        Ok(())
    }
    /// Strict JSON with ordinary date formatting; nonfinite values do not become
    /// null. Use python_json for Python-compatible ordinary output instead.
    pub fn to_json(&self) -> Result<Json> {
        self.json_value()
    }
    pub fn json_value(&self) -> Result<Json> {
        match self {
            Self::NonFinite(_) => Err(Error("nonfinite_value_not_json".into())),
            Self::Date(v) => Ok(Json::String(v.as_str().into())),
            Self::DateTime(v) => Ok(Json::String(v.as_str().replacen('T', " ", 1))),
            Self::List(v) => Ok(Json::Array(
                v.iter().map(Self::json_value).collect::<Result<_>>()?,
            )),
            Self::Map(m) => {
                let mut out = serde_json::Map::new();
                for (k, v) in m.iter() {
                    let key = m.source_key(k);
                    require(
                        !matches!(key, Scalar::NonFinite(_)),
                        "nonfinite_value_not_json",
                    )?;
                    let key = json_key(&key)?;
                    require(
                        out.insert(key, v.json_value()?).is_none(),
                        "ordinary_json_key_collision",
                    )?;
                }
                Ok(Json::Object(out))
            }
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
        fn category(key: &Scalar) -> u8 {
            match key {
                Scalar::Finite(TypedValue::Text(_)) => 0,
                Scalar::Finite(
                    TypedValue::Bool(_) | TypedValue::Integer(_) | TypedValue::Float(_),
                )
                | Scalar::NonFinite(_) => 1,
                Scalar::Finite(TypedValue::Null) => 2,
                _ => 3,
            }
        }
        fn sortable(value: &Value) -> bool {
            match value {
                Value::Map(m) => {
                    let mut kind = None;
                    for (k, v) in m.iter() {
                        let c = category(&m.source_key(k));
                        if kind.is_some_and(|old| old != c) || !sortable(v) {
                            return false;
                        }
                        kind = Some(c);
                    }
                    true
                }
                Value::List(v) => v.iter().all(sortable),
                _ => true,
            }
        }
        fn numeric(key: &Scalar) -> Option<(num_bigint::BigInt, num_bigint::BigInt)> {
            let v = match key {
                Scalar::Finite(TypedValue::Bool(v)) => {
                    TypedValue::Integer(Integer::new(if *v { "1" } else { "0" }).unwrap())
                }
                Scalar::Finite(v) => v.clone(),
                _ => return None,
            };
            let value = crate::reasoning_scope::query_value(&v, 0).ok()?;
            let m = crate::history_contract::map(&value).ok()?;
            Some((
                crate::history_contract::text(m.get("numerator")?)
                    .ok()?
                    .parse()
                    .ok()?,
                crate::history_contract::text(m.get("denominator")?)
                    .ok()?
                    .parse()
                    .ok()?,
            ))
        }
        fn compare(a: &Scalar, b: &Scalar) -> std::cmp::Ordering {
            use std::cmp::Ordering::*;
            match (a, b) {
                (Scalar::Finite(TypedValue::Text(a)), Scalar::Finite(TypedValue::Text(b))) => {
                    a.cmp(b)
                }
                (Scalar::NonFinite(NonFiniteFloat::NaN), _)
                | (_, Scalar::NonFinite(NonFiniteFloat::NaN)) => Equal,
                (Scalar::NonFinite(a), Scalar::NonFinite(b)) => {
                    a.get().partial_cmp(&b.get()).unwrap_or(Equal)
                }
                (Scalar::NonFinite(NonFiniteFloat::PositiveInfinity), _)
                | (_, Scalar::NonFinite(NonFiniteFloat::NegativeInfinity)) => Greater,
                (Scalar::NonFinite(NonFiniteFloat::NegativeInfinity), _)
                | (_, Scalar::NonFinite(NonFiniteFloat::PositiveInfinity)) => Less,
                _ => match (numeric(a), numeric(b)) {
                    (Some((a, b)), Some((c, d))) => (a * d).cmp(&(c * b)),
                    _ => Equal,
                },
            }
        }
        fn write(
            value: &Value,
            out: &mut String,
            depth: usize,
            spaces: bool,
            sorted: bool,
        ) -> Result<()> {
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
                        write(v, out, depth + 1, spaces, sorted)?;
                    }
                    out.push(']');
                }
                Value::Map(values) => {
                    let mut entries = values
                        .iter()
                        .map(|(k, v)| (values.source_key(k), v))
                        .collect::<Vec<_>>();
                    if sorted {
                        entries.sort_by(|(a, _), (b, _)| compare(a, b));
                    }
                    out.push('{');
                    for (i, (k, v)) in entries.into_iter().enumerate() {
                        if i > 0 {
                            out.push_str(if spaces { ", " } else { "," });
                        }
                        out.push_str(&serde_json::to_string(&json_key(&k)?)?);
                        out.push_str(if spaces { ": " } else { ":" });
                        write(v, out, depth + 1, spaces, sorted)?;
                    }
                    out.push('}');
                }
            }
            require(out.len() <= 512 * 1024 * 1024, "assessment byte limit")
        }
        let mut out = String::new();
        write(self, &mut out, 0, spaces, sortable(self))?;
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
            Self::Map(m) => format!(
                "{{{}}}",
                m.iter()
                    .map(|(k, v)| format!(
                        "{}: {}",
                        m.source_key(k).value().python_repr(),
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
pub fn json_key(key: &Scalar) -> Result<String> {
    match key {
        Scalar::NonFinite(v) => Ok(v.python_json().into()),
        Scalar::Finite(v) => Ok(match v {
            TypedValue::Text(v) => v.clone(),
            TypedValue::Null => "null".into(),
            TypedValue::Bool(v) => v.to_string(),
            TypedValue::Integer(v) => v.as_str().into(),
            TypedValue::Float(v) => crate::identity::python_float(v.get()),
            _ => {
                return Err(Error(
                    "ordinary JSON requires scalar JSON-compatible keys".into(),
                ));
            }
        }),
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
            Value::Map(m) => {
                let mut out = serde_json::Map::new();
                let mut key_kind = None;
                for (k, v) in m.iter() {
                    let key = m.source_key(k);
                    require(
                        !matches!(key, Scalar::NonFinite(_)),
                        "nonfinite_value_not_json",
                    )?;
                    let kind = match &key {
                        Scalar::Finite(TypedValue::Text(_)) => 0,
                        Scalar::Finite(
                            TypedValue::Bool(_) | TypedValue::Integer(_) | TypedValue::Float(_),
                        ) => 1,
                        _ => 2,
                    };
                    require(
                        key_kind.is_none_or(|old| old == kind),
                        "ordinary_json_key_order",
                    )?;
                    key_kind = Some(kind);
                    let key = json_key(&key)?;
                    path.push(key.clone());
                    let item = lower(v, path, unavailable, depth + 1)?;
                    path.pop();
                    require(
                        out.insert(key, item).is_none(),
                        "ordinary_json_key_collision",
                    )?;
                }
                Json::Object(out)
            }
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

pub fn list(value: &Value) -> Result<&[Value]> {
    if let Value::List(v) = value {
        Ok(v)
    } else {
        Err(Error("expected_sequence".into()))
    }
}
pub fn truth(value: &Value) -> bool {
    value.truth()
}
pub fn is_int(value: &Value, expected: &str) -> bool {
    matches!(value,Value::Integer(n)if n.as_str()==expected)
}
pub fn field<'a>(fields: &'a Map, key: &str) -> Result<&'a Value> {
    fields
        .get(key)
        .ok_or_else(|| Error(format!("missing_field: {key}")))
}
pub fn py(value: &Value) -> String {
    value.python_str()
}
pub fn named(body: &Value) -> String {
    map(body)
        .ok()
        .and_then(|m| {
            ["name", "title", "label", "what", "desc"]
                .into_iter()
                .find_map(|k| m.get(k).filter(|v| truth(v)))
        })
        .map(|v| py(v).trim().into())
        .unwrap_or_default()
}
pub fn blocked_text(body: &Value) -> String {
    let Ok(m) = map(body) else {
        return String::new();
    };
    for k in ["blocked_on", "unverified", "status"] {
        if let Some(v) = m.get(k).filter(|v| truth(v)) {
            if let Value::Map(m) = v {
                let why = m
                    .iter()
                    .filter(|(k, _)| *k != "missing")
                    .filter_map(|(_, v)| text(v).ok())
                    .collect::<Vec<_>>()
                    .join(" ");
                if !why.is_empty() {
                    return why.trim().into();
                }
                let missing = match m.get("missing") {
                    Some(Value::Text(v)) => vec![v.clone()],
                    Some(Value::List(a)) => a.iter().map(py).collect(),
                    _ => vec![],
                };
                return format!("waiting on {}", missing.join(", ")).trim().into();
            }
            return py(v);
        }
    }
    String::new()
}
pub fn reopened_text(body: &Value) -> String {
    map(body)
        .ok()
        .and_then(|m| m.get("reopened_by"))
        .filter(|v| truth(v))
        .map(py)
        .unwrap_or_default()
}
pub fn python_equal(a: &Value, b: &Value) -> bool {
    if let (Ok(a), Ok(b)) = (a.try_typed(), b.try_typed()) {
        return crate::source_clock::python_equal(&a, &b);
    }
    match (a, b) {
        (Value::NonFinite(NonFiniteFloat::NaN), Value::NonFinite(NonFiniteFloat::NaN)) => false,
        (Value::NonFinite(a), Value::NonFinite(b)) => a == b,
        (Value::List(a), Value::List(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a == b || python_equal(a, b))
        }
        (Value::Map(a), Value::Map(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(k, a)| b.get(k).is_some_and(|b| a == b || python_equal(a, b)))
        }
        _ => false,
    }
}

pub fn same(a: &Value, b: &Value) -> Result<bool> {
    match (a.try_typed(), b.try_typed()) {
        (Ok(a), Ok(b)) => crate::reasoning_authoring::same(&a, &b),
        _ => Ok(python_equal(a, b)),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) fn value(j: &Json) -> Value {
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
