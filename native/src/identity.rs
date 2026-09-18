//! Compatibility wrappers for typed identity and physical subject paths.
use crate::{Result, require, value::TypedValue};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn identity(value: &Value) -> Result<String> {
    TypedValue::from_json(value)?.digest()
}

pub fn object_identity(value: &Value) -> Result<String> {
    let mut value = value.clone();
    require(
        value["id_scheme"] == "typed-history/v2",
        "unsupported_identity",
    )?;
    value
        .as_object_mut()
        .ok_or_else(|| crate::Error("invalid_object".into()))?
        .remove("id");
    identity(&value)
}

pub fn subject_path(subject: &str, version: &str) -> Result<String> {
    require(
        subject.len() <= 1_048_576
            && version.len() == 64
            && version
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid_subject_or_version",
    )?;
    let mut bytes = b"kpopper-history-subject-path/v1\0".to_vec();
    bytes.extend_from_slice(subject.as_bytes());
    Ok(format!("~{}/{version}.yaml", sha256(&bytes)))
}

/// Python's finite float repr: shortest digits, exponent below -4 or at least 16.
/// Ryu supplies shortest round-trip digits; only the presentation policy changes.
pub(crate) fn python_float(value: f64) -> String {
    let mut buffer = ryu::Buffer::new();
    let source = buffer.format_finite(value);
    let (sign, source) = source.strip_prefix('-').map_or(("", source), |s| ("-", s));
    let (mantissa, exponent) = source
        .split_once('e')
        .map_or((source, 0), |(m, e)| (m, e.parse::<i32>().unwrap()));
    let point = mantissa.find('.').unwrap_or(mantissa.len()) as i32;
    let digits = mantissa.replace('.', "");
    let leading = digits.bytes().take_while(|c| *c == b'0').count();
    let digits = digits[leading..].trim_end_matches('0');
    if digits.is_empty() {
        return format!("{sign}0.0");
    }
    let power = exponent + point - leading as i32 - 1;
    if !(-4..16).contains(&power) {
        let tail = if digits.len() > 1 {
            format!(".{}", &digits[1..])
        } else {
            String::new()
        };
        format!(
            "{sign}{}{tail}e{}{:#02}",
            &digits[..1],
            if power < 0 { "-" } else { "+" },
            power.abs()
        )
    } else if power < 0 {
        format!("{sign}0.{}{digits}", "0".repeat((-power - 1) as usize))
    } else {
        let point = (power + 1) as usize;
        if point >= digits.len() {
            format!("{sign}{digits}{}.0", "0".repeat(point - digits.len()))
        } else {
            format!("{sign}{}.{}", &digits[..point], &digits[point..])
        }
    }
}

/// Legacy JSON preimage, excluding only the top-level id, preserving default=str.
/// Datetime uses a space here; typed-history/v2 uses datetime.isoformat with T.
pub fn legacy_preimage(value: &TypedValue) -> Result<Vec<u8>> {
    value.validate()?;
    let TypedValue::Map(fields) = value else {
        return Err(crate::Error("invalid_object".into()));
    };
    fn write(value: &TypedValue, out: &mut String) -> Result<()> {
        match value {
            TypedValue::Null => out.push_str("null"),
            TypedValue::Bool(v) => out.push_str(if *v { "true" } else { "false" }),
            TypedValue::Integer(v) => out.push_str(v.as_str()),
            TypedValue::Float(v) => out.push_str(&python_float(v.get())),
            TypedValue::Text(v) => out.push_str(&serde_json::to_string(v)?),
            TypedValue::Date(v) => out.push_str(&serde_json::to_string(v.as_str())?),
            TypedValue::DateTime(v) => {
                out.push_str(&serde_json::to_string(&v.as_str().replacen('T', " ", 1))?)
            }
            TypedValue::List(values) => {
                out.push('[');
                for (i, v) in values.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write(v, out)?;
                }
                out.push(']');
            }
            TypedValue::Map(fields) => {
                out.push('{');
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(&serde_json::to_string(k)?);
                    out.push(':');
                    write(v, out)?;
                }
                out.push('}');
            }
        }
        Ok(())
    }
    let fields = fields
        .iter()
        .filter(|(k, _)| k.as_str() != "id")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let mut out = String::new();
    write(&TypedValue::Map(fields), &mut out)?;
    Ok(out.into_bytes())
}

/// Dispatch only by the declared scheme. Unsupported declarations never fall back.
/// This computes an identity, not object-schema or history-closure validation.
pub fn typed_object_identity(value: &TypedValue) -> Result<String> {
    value.validate()?;
    let TypedValue::Map(fields) = value else {
        return Err(crate::Error("invalid_object".into()));
    };
    match fields.get("id_scheme") {
        None => Ok(sha256(&legacy_preimage(value)?)[..40].into()),
        Some(TypedValue::Text(scheme)) if scheme == "typed-history/v2" => {
            let fields = fields
                .iter()
                .filter(|(k, _)| k.as_str() != "id")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            TypedValue::Map(fields).digest()
        }
        _ => Err(crate::Error("unsupported_identity".into())),
    }
}
