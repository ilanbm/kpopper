//! Lossless canonical typed JSON. Ordinary strings never acquire a date type.
use crate::{Error, Result, identity::sha256, require};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const MAX_DEPTH: usize = 128;
pub const MAX_VALUES: usize = 100_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Integer(String);
impl Integer {
    pub fn new(text: &str) -> Result<Self> {
        let digits = text.strip_prefix('-').unwrap_or(text);
        require(
            !digits.is_empty()
                && digits.bytes().all(|b| b.is_ascii_digit())
                && (digits == "0" || !digits.starts_with('0'))
                && text != "-0",
            "noncanonical_integer",
        )?;
        Ok(Self(text.into()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Equality uses finite IEEE754 bits, keeping negative and positive zero distinct.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FiniteFloat(u64);
impl FiniteFloat {
    pub fn new(value: f64) -> Result<Self> {
        require(value.is_finite(), "nonfinite_float")?;
        Ok(Self(value.to_bits()))
    }
    pub fn get(self) -> f64 {
        f64::from_bits(self.0)
    }
    pub fn hex(self) -> String {
        let sign = if self.0 >> 63 == 1 { "-" } else { "" };
        let fraction = self.0 & ((1_u64 << 52) - 1);
        let exponent = ((self.0 >> 52) & 0x7ff) as i32;
        if exponent == 0 && fraction == 0 {
            return format!("{sign}0x0.0p+0");
        }
        let leading = if exponent == 0 { 0 } else { 1 };
        let power = if exponent == 0 {
            -1022
        } else {
            exponent - 1023
        };
        format!("{sign}0x{leading}.{fraction:013x}p{power:+}")
    }
    pub fn from_hex(text: &str) -> Result<Self> {
        let (sign, rest) = text
            .strip_prefix('-')
            .map_or((0, text), |s| (1_u64 << 63, s));
        if rest == "0x0.0p+0" {
            return Ok(Self(sign));
        }
        let (sig, power) = rest
            .split_once('p')
            .ok_or_else(|| Error("noncanonical_float".into()))?;
        require(
            sig.is_ascii()
                && sig.len() == 17
                && (sig.starts_with("0x1.") || sig.starts_with("0x0.")),
            "noncanonical_float",
        )?;
        let fraction =
            u64::from_str_radix(&sig[4..], 16).map_err(|_| Error("noncanonical_float".into()))?;
        let power: i32 = power
            .parse()
            .map_err(|_| Error("noncanonical_float".into()))?;
        let exponent = if sig.starts_with("0x0.") {
            require(power == -1022 && fraction != 0, "noncanonical_float")?;
            0
        } else {
            require((-1022..=1023).contains(&power), "nonfinite_float")?;
            (power + 1023) as u64
        };
        let result = Self(sign | (exponent << 52) | fraction);
        require(result.hex() == text, "noncanonical_float")?;
        Ok(result)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Date(String);
impl Date {
    pub fn new(text: &str) -> Result<Self> {
        require(text.is_ascii() && text.len() == 10, "invalid_date")?;
        require(&text[4..5] == "-" && &text[7..8] == "-", "invalid_date")?;
        let year = decimal(&text[..4])?;
        let month = decimal(&text[5..7])?;
        let day = decimal(&text[8..])?;
        require(year >= 1 && (1..=12).contains(&month), "invalid_date")?;
        let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
        let days = match month {
            2 => {
                if leap {
                    29
                } else {
                    28
                }
            }
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        };
        require((1..=days).contains(&day), "invalid_date")?;
        Ok(Self(text.into()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Canonical datetime.isoformat, retaining naive/aware and the original offset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DateTime(String);
impl DateTime {
    pub fn new(text: &str) -> Result<Self> {
        require(text.is_ascii() && text.len() >= 19, "invalid_datetime")?;
        Date::new(&text[..10])?;
        require(&text[10..11] == "T", "noncanonical_datetime")?;
        clock(&text[11..19])?;
        let mut tail = &text[19..];
        if let Some(fraction) = tail.strip_prefix('.') {
            require(fraction.len() >= 6, "invalid_datetime")?;
            require(decimal(&fraction[..6])? != 0, "noncanonical_datetime")?;
            tail = &fraction[6..];
        }
        if !tail.is_empty() {
            require(
                tail.starts_with(['+', '-']) && tail.len() >= 6,
                "invalid_offset",
            )?;
            let negative = tail.starts_with('-');
            let offset = &tail[1..];
            require(&offset[2..3] == ":", "invalid_offset")?;
            let hour = decimal(&offset[..2])?;
            let minute = decimal(&offset[3..5])?;
            require(hour <= 23 && minute <= 59, "invalid_offset")?;
            let mut seconds = 0;
            let mut micros = 0;
            let extra = &offset[5..];
            if !extra.is_empty() {
                require(extra.starts_with(':') && extra.len() >= 3, "invalid_offset")?;
                seconds = decimal(&extra[1..3])?;
                require(seconds <= 59, "invalid_offset")?;
                let fractional = &extra[3..];
                if !fractional.is_empty() {
                    require(
                        fractional.starts_with('.') && fractional.len() == 7,
                        "invalid_offset",
                    )?;
                    micros = decimal(&fractional[1..])?;
                    require(micros != 0, "noncanonical_offset")?;
                }
                require(seconds != 0 || micros != 0, "noncanonical_offset")?;
            }
            require(
                !negative || hour + minute + seconds + micros != 0,
                "noncanonical_offset",
            )?;
        }
        Ok(Self(text.into()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
fn decimal(text: &str) -> Result<u32> {
    require(
        !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()),
        "invalid_decimal",
    )?;
    text.parse().map_err(|_| Error("invalid_decimal".into()))
}
fn clock(text: &str) -> Result<()> {
    require(
        text.len() == 8 && &text[2..3] == ":" && &text[5..6] == ":",
        "invalid_datetime",
    )?;
    require(
        decimal(&text[..2])? <= 23 && decimal(&text[3..5])? <= 59 && decimal(&text[6..])? <= 59,
        "invalid_datetime",
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypedValue {
    Null,
    Bool(bool),
    Integer(Integer),
    Float(FiniteFloat),
    Text(String),
    Date(Date),
    DateTime(DateTime),
    List(Vec<Self>),
    Map(BTreeMap<String, Self>),
}
struct Budget {
    visits: usize,
    maximum: usize,
}
impl Budget {
    fn visit(&mut self, depth: usize) -> Result<()> {
        self.visits += 1;
        require(
            depth <= MAX_DEPTH && self.visits <= self.maximum,
            "value_limit",
        )
    }
}
impl TypedValue {
    pub fn from_json(value: &Value) -> Result<Self> {
        Self::from_json_bounded(value, MAX_VALUES)
    }
    pub(crate) fn from_json_bounded(value: &Value, maximum: usize) -> Result<Self> {
        Self::json_inner(value, 0, &mut Budget { visits: 0, maximum })
    }
    fn json_inner(value: &Value, depth: usize, budget: &mut Budget) -> Result<Self> {
        budget.visit(depth)?;
        Ok(match value {
            Value::Null => Self::Null,
            Value::Bool(v) => Self::Bool(*v),
            Value::String(v) => Self::Text(v.clone()),
            Value::Number(v) => {
                let spelling = v.to_string();
                if spelling.contains(['.', 'e', 'E']) {
                    Self::Float(FiniteFloat::new(
                        v.as_f64().ok_or_else(|| Error("invalid_float".into()))?,
                    )?)
                } else {
                    Self::Integer(Integer::new(if spelling == "-0" {
                        "0"
                    } else {
                        &spelling
                    })?)
                }
            }
            Value::Array(v) => Self::List(
                v.iter()
                    .map(|v| Self::json_inner(v, depth + 1, budget))
                    .collect::<Result<_>>()?,
            ),
            Value::Object(v) => Self::Map(
                v.iter()
                    .map(|(k, v)| Ok((k.clone(), Self::json_inner(v, depth + 1, budget)?)))
                    .collect::<Result<_>>()?,
            ),
        })
    }
    pub fn from_tagged(value: &Value) -> Result<Self> {
        Self::from_tagged_bounded(value, MAX_VALUES)
    }
    pub(crate) fn from_tagged_bounded(value: &Value, maximum: usize) -> Result<Self> {
        Self::tagged_inner(value, 0, &mut Budget { visits: 0, maximum })
    }
    fn tagged_inner(value: &Value, depth: usize, budget: &mut Budget) -> Result<Self> {
        budget.visit(depth)?;
        let parts = value
            .as_array()
            .ok_or_else(|| Error("invalid_typed_value".into()))?;
        let tag = parts
            .first()
            .and_then(Value::as_str)
            .ok_or_else(|| Error("invalid_typed_tag".into()))?;
        if tag == "null" {
            require(parts.len() == 1, "invalid_typed_arity")?;
            return Ok(Self::Null);
        }
        require(parts.len() == 2, "invalid_typed_arity")?;
        let v = &parts[1];
        let text = || {
            v.as_str()
                .ok_or_else(|| Error("invalid_typed_scalar".into()))
        };
        Ok(match tag {
            "bool" => Self::Bool(
                v.as_bool()
                    .ok_or_else(|| Error("invalid_typed_bool".into()))?,
            ),
            "text" => Self::Text(text()?.into()),
            "int" => Self::Integer(Integer::new(text()?)?),
            "float" => Self::Float(FiniteFloat::from_hex(text()?)?),
            "date" => Self::Date(Date::new(text()?)?),
            "datetime" => Self::DateTime(DateTime::new(text()?)?),
            "list" => Self::List(
                v.as_array()
                    .ok_or_else(|| Error("invalid_typed_list".into()))?
                    .iter()
                    .map(|v| Self::tagged_inner(v, depth + 1, budget))
                    .collect::<Result<_>>()?,
            ),
            "map" => {
                let pairs = v
                    .as_array()
                    .ok_or_else(|| Error("invalid_typed_map".into()))?;
                let mut map = BTreeMap::new();
                let mut previous: Option<&str> = None;
                for pair in pairs {
                    let pair = pair
                        .as_array()
                        .ok_or_else(|| Error("invalid_typed_pair".into()))?;
                    require(pair.len() == 2, "invalid_typed_pair")?;
                    let key = pair[0]
                        .as_str()
                        .ok_or_else(|| Error("invalid_typed_key".into()))?;
                    require(previous.is_none_or(|p| p < key), "noncanonical_map_order")?;
                    previous = Some(key);
                    map.insert(key.into(), Self::tagged_inner(&pair[1], depth + 1, budget)?);
                }
                Self::Map(map)
            }
            _ => return Err(Error("unknown_typed_tag".into())),
        })
    }
    pub fn validate(&self) -> Result<()> {
        self.validate_bounded(MAX_VALUES)
    }
    pub(crate) fn validate_bounded(&self, maximum: usize) -> Result<()> {
        let mut pending = vec![(self, 0)];
        let mut budget = Budget { visits: 0, maximum };
        while let Some((v, depth)) = pending.pop() {
            budget.visit(depth)?;
            match v {
                Self::List(v) => {
                    require(
                        v.len() + pending.len() <= maximum - budget.visits,
                        "value_limit",
                    )?;
                    pending.extend(v.iter().map(|v| (v, depth + 1)));
                }
                Self::Map(v) => {
                    require(
                        v.len() + pending.len() <= maximum - budget.visits,
                        "value_limit",
                    )?;
                    pending.extend(v.values().map(|v| (v, depth + 1)));
                }
                _ => {}
            }
        }
        Ok(())
    }
    pub fn to_tagged(&self) -> Result<Value> {
        self.to_tagged_bounded(MAX_VALUES)
    }
    pub(crate) fn to_tagged_bounded(&self, maximum: usize) -> Result<Value> {
        self.validate_bounded(maximum)?;
        Ok(self.tagged_unchecked())
    }
    fn tagged_unchecked(&self) -> Value {
        match self {
            Self::Null => json!(["null"]),
            Self::Bool(v) => json!(["bool", v]),
            Self::Text(v) => json!(["text", v]),
            Self::Integer(v) => json!(["int", v.as_str()]),
            Self::Float(v) => json!(["float", v.hex()]),
            Self::Date(v) => json!(["date", v.as_str()]),
            Self::DateTime(v) => json!(["datetime", v.as_str()]),
            Self::List(v) => json!([
                "list",
                v.iter().map(Self::tagged_unchecked).collect::<Vec<_>>()
            ]),
            Self::Map(v) => json!([
                "map",
                v.iter()
                    .map(|(k, v)| json!([k, v.tagged_unchecked()]))
                    .collect::<Vec<_>>()
            ]),
        }
    }
    pub fn to_json(&self) -> Result<Value> {
        self.validate()?;
        self.json_unchecked()
    }
    fn json_unchecked(&self) -> Result<Value> {
        Ok(match self {
            Self::Null => Value::Null,
            Self::Bool(v) => json!(v),
            Self::Text(v) => json!(v),
            Self::Integer(v) => Value::Number(v.as_str().parse()?),
            Self::Float(v) => json!(v.get()),
            Self::Date(_) | Self::DateTime(_) => return Err(Error("lossy_json_conversion".into())),
            Self::List(v) => json!(
                v.iter()
                    .map(Self::json_unchecked)
                    .collect::<Result<Vec<_>>>()?
            ),
            Self::Map(v) => Value::Object(
                v.iter()
                    .map(|(k, v)| Ok((k.clone(), v.json_unchecked()?)))
                    .collect::<Result<_>>()?,
            ),
        })
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(&self.to_tagged()?)?)
    }
    pub fn digest(&self) -> Result<String> {
        Ok(sha256(&self.canonical_bytes()?))
    }
}
