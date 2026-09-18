//! Exact JSON-subset implementation of pending_grounding.identity.
use crate::{Result, require};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn float_hex(value: f64) -> String {
    let bits = value.to_bits();
    let sign = if bits >> 63 == 1 { "-" } else { "" };
    let fraction = bits & ((1_u64 << 52) - 1);
    let exponent = ((bits >> 52) & 0x7ff) as i32;
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

fn encode(value: &Value, depth: usize, count: &mut usize) -> Result<Value> {
    *count += 1;
    require(depth <= 100 && *count <= 100_000, "value_limit")?;
    Ok(match value {
        Value::Null => json!(["null"]),
        Value::Bool(v) => json!(["bool", v]),
        Value::String(v) => json!(["text", v]),
        Value::Number(v) => {
            let spelling = v.to_string();
            if spelling.contains(['.', 'e', 'E']) {
                let number = v
                    .as_f64()
                    .ok_or_else(|| crate::Error("invalid_float".into()))?;
                require(number.is_finite(), "nonfinite_float")?;
                json!(["float", float_hex(number)])
            } else {
                json!(["int", spelling])
            }
        }
        Value::Array(values) => json!([
            "list",
            values
                .iter()
                .map(|v| encode(v, depth + 1, count))
                .collect::<Result<Vec<_>>>()?
        ]),
        Value::Object(values) => {
            let mut pairs = values.iter().collect::<Vec<_>>();
            pairs.sort_by_key(|(key, _)| *key);
            json!([
                "map",
                pairs
                    .into_iter()
                    .map(|(key, v)| Ok(json!([key, encode(v, depth + 1, count)?])))
                    .collect::<Result<Vec<_>>>()?
            ])
        }
    })
}

pub fn identity(value: &Value) -> Result<String> {
    Ok(sha256(&serde_json::to_vec(&encode(value, 0, &mut 0)?)?))
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn python_float_spelling() {
        assert_eq!(float_hex(1.0), "0x1.0000000000000p+0");
        assert_eq!(float_hex(-0.0), "-0x0.0p+0");
        assert_eq!(float_hex(f64::from_bits(1)), "0x0.0000000000001p-1022");
    }
    #[test]
    fn different_types_and_orders_have_different_identities() {
        let values: Vec<Value> = serde_json::from_str(
            r#"[null,{},true,1,1.0,"1",[1,2],[2,1],0.0,-0.0,184467440737095516160]"#,
        )
        .unwrap();
        let ids = values
            .iter()
            .map(|v| identity(v).unwrap())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(values.len(), ids.len());
        assert_eq!(
            identity(&json!({"b":2,"a":1})).unwrap(),
            identity(&json!({"a":1,"b":2})).unwrap()
        );
    }
}
