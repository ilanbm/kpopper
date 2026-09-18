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
