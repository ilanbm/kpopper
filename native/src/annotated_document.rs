//! Native Annotated Documents application.
//!
//! This module packages complete authored HTML with finite, selected evidence
//! snapshots.  It never executes author code or reads outside the caller's root.

use chrono::{SecondsFormat, Utc};
use num_bigint::BigInt;
use serde_json::{Map, Number, Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

use crate::{Error, Result, annotated_document_html as html};

pub const MAX_SOURCE: usize = 8_000_000;
pub const MAX_ARTIFACT: usize = 40_000_000;
pub const GUIDE: &str = include_str!("annotated_document_guide.md");
const FRAME_CSP: &str = "default-src 'none'; script-src 'unsafe-inline' 'unsafe-eval' data: blob:; style-src 'unsafe-inline'; img-src data:; font-src data:; media-src data:; connect-src 'none'; frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'; worker-src 'none'";

fn err(message: impl Into<String>) -> Error {
    Error(format!("document: {}", message.into()))
}
fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, false)
}
fn digest(bytes: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(bytes.as_ref()))
}
pub fn valid_id(value: &str) -> bool {
    let mut bytes = value.bytes();
    value.len() <= 128
        && bytes.next().is_some_and(|b| b.is_ascii_alphabetic())
        && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}
fn object<'a>(value: &'a Value, name: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| err(format!("{name} has unknown fields or is not an object")))
}
fn text<'a>(value: Option<&'a Value>, name: &str, empty: bool, limit: usize) -> Result<&'a str> {
    let value = value.and_then(Value::as_str).ok_or_else(|| {
        err(format!(
            "{name} must be {}text",
            if empty { "" } else { "nonempty " }
        ))
    })?;
    if (!empty && value.trim().is_empty()) || value.chars().count() > limit {
        return Err(err(format!(
            "{name} must be {}text",
            if empty { "" } else { "nonempty " }
        )));
    }
    Ok(value)
}
fn keys(value: &Map<String, Value>, allowed: &[&str], name: &str) -> Result<()> {
    if value.keys().any(|k| !allowed.contains(&k.as_str())) {
        Err(err(format!(
            "{name} has unknown fields or is not an object"
        )))
    } else {
        Ok(())
    }
}
fn canonical(value: &Value) -> Result<String> {
    fn write(value: &Value, out: &mut String) -> Result<()> {
        match value {
            Value::Null => out.push_str("null"),
            Value::Bool(v) => out.push_str(if *v { "true" } else { "false" }),
            Value::Number(v) => out.push_str(&v.to_string()),
            Value::String(v) => out.push_str(&serde_json::to_string(v)?),
            Value::Array(values) => {
                out.push('[');
                for (i, v) in values.iter().enumerate() {
                    if i > 0 {
                        out.push(',')
                    }
                    write(v, out)?;
                }
                out.push(']');
            }
            Value::Object(values) => {
                out.push('{');
                let mut fields: Vec<_> = values.iter().collect();
                fields.sort_by_key(|(k, _)| *k);
                for (i, (k, v)) in fields.into_iter().enumerate() {
                    if i > 0 {
                        out.push(',')
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
    let mut out = String::new();
    write(value, &mut out)?;
    Ok(out)
}
fn safe_json(value: &Value) -> Result<String> {
    Ok(canonical(value)?
        .replace('<', "\\u003c")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029"))
}

/// Strict JSON input with duplicate-key and non-finite rejection.
pub fn read_json(raw: &str) -> Result<Value> {
    use serde::{Deserialize, de};
    use serde_json::value::RawValue;

    fn decode(raw: &RawValue) -> Result<Value> {
        struct ObjectVisitor;
        impl<'de> de::Visitor<'de> for ObjectVisitor {
            type Value = Map<String, Value>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: de::MapAccess<'de>>(
                self,
                mut fields: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut result = Map::new();
                while let Some(key) = fields.next_key::<String>()? {
                    if result.contains_key(&key) {
                        return Err(de::Error::custom(format!("Duplicate JSON key: {key}")));
                    }
                    let raw = fields.next_value::<Box<RawValue>>()?;
                    let value = decode(&raw).map_err(de::Error::custom)?;
                    result.insert(key, value);
                }
                Ok(result)
            }
        }

        let text = raw.get().trim();
        let mut decoder = serde_json::Deserializer::from_str(text);
        let value = match text.as_bytes().first() {
            Some(b'{') => {
                serde::Deserializer::deserialize_map(&mut decoder, ObjectVisitor).map(Value::Object)
            }
            Some(b'[') => Vec::<Box<RawValue>>::deserialize(&mut decoder).and_then(|items| {
                items
                    .iter()
                    .map(|item| decode(item).map_err(de::Error::custom))
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map(Value::Array)
            }),
            Some(b'"') => String::deserialize(&mut decoder).map(Value::String),
            Some(b'n') => <()>::deserialize(&mut decoder).map(|()| Value::Null),
            Some(b't' | b'f') => bool::deserialize(&mut decoder).map(Value::Bool),
            Some(_) => Number::deserialize(&mut decoder).map(Value::Number),
            None => unreachable!("RawValue cannot be empty"),
        }
        .map_err(|e| err(format!("Invalid JSON: {e}")))?;
        decoder
            .end()
            .map_err(|e| err(format!("Invalid JSON: {e}")))?;
        Ok(value)
    }

    // Parse once as a raw token tree. This preserves serde_json's nesting bound
    // while letting the recursive decoder distinguish JSON number tokens from
    // literal objects that use serde_json's private arbitrary-precision key.
    let mut decoder = serde_json::Deserializer::from_str(raw);
    let raw_value = Box::<RawValue>::deserialize(&mut decoder)
        .map_err(|e| err(format!("Invalid JSON: {e}")))?;
    decoder
        .end()
        .map_err(|e| err(format!("Invalid JSON: {e}")))?;
    decode(&raw_value)
}
fn read_limited(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let data = fs::read(path)?;
    if data.len() > limit {
        return Err(err("Input exceeds the supported size limit"));
    }
    Ok(data)
}
fn scoped(root: &Path, relative: &str) -> Result<PathBuf> {
    let rel = Path::new(relative);
    if relative.is_empty()
        || rel.is_absolute()
        || rel.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(err("Source paths must be relative paths inside --root"));
    }
    let root = fs::canonicalize(root)?;
    let mut candidate = root.clone();
    for component in rel.components() {
        candidate.push(component);
        if fs::symlink_metadata(&candidate).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(err("Source paths must not traverse symlinks"));
        }
    }
    if candidate.exists() {
        let resolved = fs::canonicalize(&candidate)?;
        if !resolved.starts_with(&root) {
            return Err(err("Source path escapes --root"));
        }
        if !resolved.is_file() {
            return Err(err("Source must be a regular file"));
        }
        Ok(resolved)
    } else {
        Ok(candidate)
    }
}

#[derive(Clone, Debug)]
struct Dec {
    n: BigInt,
    scale: i32,
}
impl Dec {
    fn parse(value: &str) -> Result<Self> {
        if value.len() > 512 {
            return Err(err("Number is outside the supported finite range"));
        }
        let value = value.trim();
        let (base, exp) = value
            .split_once(['e', 'E'])
            .map_or((value, 0), |(a, b)| (a, b.parse::<i32>().unwrap_or(10000)));
        let negative = base.starts_with('-');
        let base = base.trim_start_matches(['+', '-']);
        let (whole, frac) = base.split_once('.').map_or((base, ""), |(a, b)| (a, b));
        if whole.is_empty() && frac.is_empty()
            || !whole
                .bytes()
                .chain(frac.bytes())
                .all(|b| b.is_ascii_digit())
        {
            return Err(err("Expected a finite number"));
        }
        let adjusted = whole.trim_start_matches('0').len() as i32 + exp;
        if adjusted.abs() > 101 {
            return Err(err("Number is outside the supported finite range"));
        }
        let digits = format!("{whole}{frac}");
        let mut n = digits
            .parse::<BigInt>()
            .map_err(|_| err("Expected a finite number"))?;
        if negative {
            n = -n
        }
        Ok(Self {
            n,
            scale: frac.len() as i32 - exp,
        })
    }
    fn align(&self, other: &Self) -> (BigInt, BigInt, i32) {
        let s = self.scale.max(other.scale);
        (
            &self.n * pow10(s - self.scale),
            &other.n * pow10(s - other.scale),
            s,
        )
    }
    fn add(&self, o: &Self) -> Self {
        let (a, b, s) = self.align(o);
        Self { n: a + b, scale: s }
    }
    fn sub(&self, o: &Self) -> Self {
        let (a, b, s) = self.align(o);
        Self { n: a - b, scale: s }
    }
    fn mul(&self, o: &Self) -> Self {
        Self {
            n: &self.n * &o.n,
            scale: self.scale + o.scale,
        }
    }
    fn div(&self, o: &Self, precision: i32) -> Result<Self> {
        if o.n == BigInt::from(0) {
            return Err(err("The divisor is zero; no ratio can be checked"));
        }
        let shift = precision + o.scale - self.scale;
        let numerator = if shift >= 0 {
            &self.n * pow10(shift)
        } else {
            &self.n / pow10(-shift)
        };
        Ok(Self {
            n: numerator / &o.n,
            scale: precision,
        })
    }
    fn round(&self, decimals: i32) -> Self {
        if self.scale <= decimals {
            return Self {
                n: &self.n * pow10(decimals - self.scale),
                scale: decimals,
            };
        }
        let factor = pow10(self.scale - decimals);
        let q = &self.n / &factor;
        let r = &self.n % &factor;
        let twice = if r < BigInt::from(0) { -r * 2 } else { r * 2 };
        let sign = if self.n < BigInt::from(0) { -1 } else { 1 };
        Self {
            n: if twice >= factor { q + sign } else { q },
            scale: decimals,
        }
    }
    fn plain(&self) -> String {
        let neg = self.n < BigInt::from(0);
        let digits = self.n.to_string().trim_start_matches('-').to_owned();
        let result = if self.scale <= 0 {
            format!("{digits}{}", "0".repeat((-self.scale) as usize))
        } else if digits.len() <= self.scale as usize {
            format!(
                "0.{}{}",
                "0".repeat(self.scale as usize - digits.len()),
                digits
            )
        } else {
            let p = digits.len() - self.scale as usize;
            format!("{}.{}", &digits[..p], &digits[p..])
        };
        if neg && self.n != BigInt::from(0) {
            format!("-{result}")
        } else {
            result
        }
    }
}
fn pow10(exp: i32) -> BigInt {
    BigInt::from(10u8).pow(exp as u32)
}

fn normalize_input(value: &Value) -> Result<Value> {
    let o = object(value, "claim input")?;
    keys(o, &["source", "pointer", "quote"], "claim input")?;
    let source = text(o.get("source"), "source", false, 128)?;
    if !valid_id(source) {
        return Err(err(
            "IDs must start with a letter and use letters, digits, dot, dash or underscore",
        ));
    }
    if o.contains_key("pointer") && o.contains_key("quote") {
        return Err(err(
            "A source input has one selector, not both pointer and quote",
        ));
    }
    if let Some(p) = o.get("pointer") {
        let p = text(Some(p), "pointer", true, 2000)?;
        if !p.is_empty()
            && (!p.starts_with('/')
                || p.split('~')
                    .skip(1)
                    .any(|x| !x.starts_with('0') && !x.starts_with('1')))
        {
            return Err(err("Expected a JSON pointer"));
        }
    }
    if let Some(q) = o.get("quote") {
        text(Some(q), "source quote", false, 20000)?;
    }
    Ok(value.clone())
}
fn selection_key(input: &Value) -> Result<String> {
    let mut v = input.clone();
    v.as_object_mut().unwrap().remove("source");
    canonical(&v)
}
fn normalize_claims(value: &Value) -> Result<Vec<Value>> {
    let claims = value
        .as_array()
        .ok_or_else(|| err("Expected at most 500 claims"))?;
    if claims.len() > 500 {
        return Err(err("Expected at most 500 claims"));
    }
    let mut seen = BTreeSet::new();
    let mut out = vec![];
    for claim in claims {
        let o = object(claim, "claim")?;
        keys(
            o,
            &["id", "label", "kind", "inputs", "format", "reason", "group"],
            "claim",
        )?;
        let id = text(o.get("id"), "id", false, 128)?;
        if !valid_id(id) || !seen.insert(id.to_owned()) {
            return Err(err(format!("Duplicate or invalid claim ID: {id}")));
        }
        let kind = text(o.get("kind"), "kind", false, 20)?;
        if !matches!(
            kind,
            "value" | "quote" | "sum" | "difference" | "product" | "ratio" | "inference"
        ) {
            return Err(err(format!("Unsupported check kind: {kind}")));
        }
        text(o.get("label"), "claim label", false, 300)?;
        let inputs = o
            .get("inputs")
            .and_then(Value::as_array)
            .ok_or_else(|| err("A claim needs at most 32 explicit source inputs"))?;
        if inputs.len() > 32 {
            return Err(err("A claim needs at most 32 explicit source inputs"));
        }
        let inputs: Vec<_> = inputs.iter().map(normalize_input).collect::<Result<_>>()?;
        let arity = match kind {
            "value" | "quote" => Some(1),
            "difference" | "ratio" => Some(2),
            _ => None,
        };
        if arity.is_some_and(|n| inputs.len() != n)
            || matches!(kind, "sum" | "product") && inputs.is_empty()
        {
            return Err(err(format!("Wrong number of inputs for {kind}")));
        }
        if kind != "inference" && inputs.iter().any(|i| i.as_object().unwrap().len() != 2) {
            return Err(err("A mechanical check requires an exact source selector"));
        }
        if kind == "quote" && !inputs[0].get("quote").is_some() {
            return Err(err("A quotation check requires an exact text quote"));
        }
        if kind == "ratio"
            && !o
                .get("format")
                .and_then(Value::as_object)
                .is_some_and(|f| f.contains_key("decimals"))
        {
            return Err(err(
                "A ratio requires an explicit display precision in format.decimals",
            ));
        }
        if kind == "inference" {
            text(o.get("reason"), "inference reason", false, 20000)?;
        }
        if let Some(group) = o.get("group")
            && !valid_id(text(Some(group), "group", false, 128)?)
        {
            return Err(err("Invalid group ID"));
        }
        if let Some(format) = o.get("format") {
            validate_format(format)?;
        }
        let mut normalized = o.clone();
        normalized.insert("inputs".into(), Value::Array(inputs));
        out.push(Value::Object(normalized));
    }
    Ok(out)
}
fn validate_format(value: &Value) -> Result<()> {
    let o = object(value, "format")?;
    keys(
        o,
        &[
            "decimals",
            "scale",
            "prefix",
            "suffix",
            "thousands",
            "decimal",
        ],
        "format",
    )?;
    let decimals = o.get("decimals").and_then(Value::as_i64).unwrap_or(0);
    if !(0..=8).contains(&decimals) {
        return Err(err("format.decimals must be an integer from 0 to 8"));
    }
    if let Some(scale) = o.get("scale") {
        Dec::parse(&scalar_number(scale)?)?;
    }
    for k in ["prefix", "suffix"] {
        if let Some(v) = o.get(k) {
            text(Some(v), &format!("format.{k}"), true, 64)?;
        }
    }
    let thousands = o.get("thousands").and_then(Value::as_str).unwrap_or("");
    let decimal = o.get("decimal").and_then(Value::as_str).unwrap_or(".");
    if !matches!(thousands, "" | "," | "." | " " | "\u{a0}")
        || !matches!(decimal, "." | ",")
        || thousands == decimal
    {
        return Err(err("Unsupported numeric separators"));
    }
    Ok(())
}
fn scalar_number(value: &Value) -> Result<String> {
    match value {
        Value::Number(n) => Ok(n.to_string()),
        Value::String(s) => Ok(s.clone()),
        _ => Err(err("Expected a finite number")),
    }
}
fn pointer(mut data: &Value, pointer: &str) -> Result<Value> {
    if pointer.is_empty() {
        return Ok(data.clone());
    }
    for p in pointer[1..].split('/') {
        let key = p.replace("~1", "/").replace("~0", "~");
        data = match data {
            Value::Object(o) => o.get(&key),
            Value::Array(a)
                if key == "0"
                    || (!key.starts_with('0') && key.bytes().all(|b| b.is_ascii_digit())) =>
            {
                key.parse::<usize>().ok().and_then(|i| a.get(i))
            }
            _ => None,
        }
        .ok_or_else(|| err("The selected field is unavailable"))?;
    }
    Ok(data.clone())
}
fn atom(value: &Value) -> Result<Value> {
    match value {
        Value::Bool(v) => Ok(json!({"type":"boolean","value":v})),
        Value::Number(v) => {
            Dec::parse(&v.to_string())?;
            Ok(json!({"type":"number","value":v.to_string()}))
        }
        Value::String(v) => Ok(json!({"type":"string","value":v})),
        Value::Null => Err(err("The selected field has no value")),
        _ => Err(err("The selected field is not a scalar value")),
    }
}

fn source_spec(value: &Value) -> Result<&Map<String, Value>> {
    let o = object(value, "source")?;
    keys(
        o,
        &[
            "name",
            "path",
            "format",
            "uri",
            "unavailable",
            "representation",
            "event_id",
            "state_dir",
            "profile",
        ],
        "source",
    )?;
    text(o.get("name"), "source name", false, 300)?;
    if let Some(uri) = o.get("uri") {
        let u = text(Some(uri), "source URI", false, 4000)?;
        if !(u.starts_with("http://") || u.starts_with("https://"))
            || u.chars().any(char::is_whitespace)
        {
            return Err(err("Original-source links must be HTTP(S) citation URLs"));
        }
    }
    if o.contains_key("unavailable") {
        text(
            o.get("unavailable"),
            "unavailable-source reason",
            false,
            20000,
        )?;
        if o.contains_key("path") || o.contains_key("event_id") || o.contains_key("state_dir") {
            return Err(err("An unavailable source cannot supply a path or event"));
        }
    } else {
        if !o
            .get("format")
            .and_then(Value::as_str)
            .is_some_and(|f| matches!(f, "json" | "text" | "record"))
            || !o.contains_key("path")
        {
            return Err(err(
                "A source needs a local path and json, text or record format",
            ));
        }
    }
    if o.get("representation")
        .and_then(Value::as_str)
        .is_some_and(|v| !matches!(v, "file" | "extraction"))
    {
        return Err(err("representation must be file or extraction"));
    }
    if let Some(profile) = o.get("profile")
        && (profile != "core/v1" || o.get("format") != Some(&json!("record")))
    {
        return Err(err(
            "source.profile supports only explicit core/v1 record sources",
        ));
    }
    if let Some(event) = o.get("event_id") {
        let event = text(Some(event), "event_id", false, 32)?;
        if event.len() != 32
            || !event
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || o.get("format") != Some(&json!("record"))
        {
            return Err(err(
                "event_id must be the canonical 32-character ID returned by ingestion",
            ));
        }
    }
    if o.contains_key("state_dir") && !o.contains_key("event_id") {
        return Err(err("state_dir requires an ingestion event"));
    }
    Ok(o)
}

fn event_binding(
    record: &Path,
    spec: &Map<String, Value>,
    root: &Path,
    selections: &Map<String, Value>,
) -> Result<Value> {
    let event_id = spec["event_id"].as_str().unwrap();
    let state = spec
        .get("state_dir")
        .and_then(Value::as_str)
        .map(|relative| scoped_directory(root, relative))
        .transpose()?;
    let receipt=crate::recording_receipt::applied_event_binding(event_id,record,state.as_deref()).map_err(|e|err(if e.0=="ingestion_event_not_applied"{"The ingestion event is not durably applied; inspect it by event ID"}else{"The ingestion outcome could not be read; inspect or recover the event before refreshing"}))?;
    let mut readings = Vec::new();
    let batched = receipt.get("updates").and_then(Value::as_array).is_some();
    if let Some(updates) = receipt.get("updates").and_then(Value::as_array) {
        for op in updates {
            match op["kind"].as_str() {
                Some("set") => readings.push((
                    op["id"].as_str().unwrap_or(""),
                    op["value"].clone(),
                    vec!["v", "quoted"],
                    op.get("at").cloned(),
                )),
                Some("add") => {
                    if let Some(body) = op.get("body").and_then(Value::as_object) {
                        for field in ["v", "quoted"] {
                            if let Some(value) = body.get(field) {
                                readings.push((
                                    op["id"].as_str().unwrap_or(""),
                                    value.clone(),
                                    vec![field],
                                    op.get("at").cloned(),
                                ))
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    } else {
        readings.push((
            receipt["target"].as_str().unwrap_or(""),
            receipt["value"].clone(),
            vec!["v", "quoted"],
            None,
        ));
    }
    let cited = receipt
        .get("cited_source")
        .or_else(|| receipt.get("source"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut targets = BTreeSet::new();
    let mut matched = 0;
    for (target, value, fields, at) in readings {
        let encoded = target.replace('~', "~0").replace('/', "~1");
        for field in fields {
            let pointer = format!("/{encoded}/{field}");
            for selection in selections
                .values()
                .filter(|s| s["selector"]["pointer"] == pointer)
            {
                if selection["status"] != "available"
                    || selection["value"] != atom(&value)?
                    || selection["citation"]["source"] != cited
                {
                    return Err(err(
                        "Applied event does not match the selected current reading and citation",
                    ));
                }
                if receipt.get("cited_source").is_some() {
                    let expected_at = at.as_ref().or_else(|| receipt.get("at"));
                    if expected_at.is_some_and(|v| selection["citation"]["at"] != *v)
                        || receipt
                            .get("date")
                            .is_some_and(|v| selection["citation"]["date"] != *v)
                    {
                        return Err(err(
                            "Applied event does not match the selected current reading and citation",
                        ));
                    }
                }
                matched += 1;
                targets.insert(target.to_owned());
            }
        }
    }
    if matched == 0 {
        return Err(err(
            "Applied event does not match the selected current reading and citation",
        ));
    }
    let mut binding = json!({"event_id":receipt.get("event_id").cloned().unwrap_or(Value::Null),"state":receipt["state"],"target":receipt.get("target").cloned().unwrap_or(Value::Null),"source_sha256":receipt.get("source_sha256").cloned().unwrap_or(Value::Null),"envelope_sha256":receipt.get("envelope_sha256").cloned().unwrap_or(Value::Null)});
    if batched {
        let target = if targets.len() == 1 {
            json!(targets.iter().next().unwrap())
        } else {
            Value::Null
        };
        binding["targets"] = json!(targets);
        binding["target"] = target;
    }
    Ok(binding)
}
fn scoped_directory(root: &Path, relative: &str) -> Result<PathBuf> {
    let path = root.join(relative);
    if Path::new(relative).is_absolute()
        || Path::new(relative)
            .components()
            .any(|c| matches!(c, Component::ParentDir))
    {
        return Err(err("Source paths must be relative paths inside --root"));
    }
    let root = fs::canonicalize(root)?;
    let path = fs::canonicalize(path)?;
    if !path.starts_with(root) || !path.is_dir() {
        return Err(err("Source must be a regular directory"));
    }
    Ok(path)
}

fn record_selection(raw: &[u8], input: &Value) -> Result<Value> {
    use crate::{
        history_contract::{map as typed_map, text as typed_text},
        value::TypedValue,
    };
    let pointer = input.get("pointer").and_then(Value::as_str).unwrap_or("");
    let pieces = pointer
        .strip_prefix('/')
        .unwrap_or("")
        .split('/')
        .map(|p| p.replace("~1", "/").replace("~0", "~"))
        .collect::<Vec<_>>();
    if pieces.len() != 2 || !matches!(pieces[1].as_str(), "v" | "quoted") {
        return Err(err(
            "Record selectors are /ENTRY/v or /ENTRY/quoted for stored readings",
        ));
    }
    let document = crate::history_yaml::decode_document(raw)
        .map_err(|_| err("The source could not be read as a record"))?;
    let root = typed_map(&document).map_err(|_| err("The source could not be read as a record"))?;
    if root.get("record").is_some_and(|v| *v != TypedValue::Null)
        || root.get("also").is_some_and(|v| *v != TypedValue::Null)
    {
        return Err(err("Pointer records require an explicit source extraction"));
    }
    let mut entries = BTreeMap::new();
    for (collection, values) in root {
        if matches!(collection.as_str(), "meta" | "schema" | "record" | "also") {
            continue;
        }
        if let Ok(values) = typed_map(values) {
            for (id, body) in values {
                entries.insert(id.as_str(), body);
            }
        }
    }
    let body = entries
        .get(pieces[0].as_str())
        .and_then(|v| typed_map(v).ok())
        .ok_or_else(|| err("The selected record field is unavailable"))?;
    let value = body
        .get(&pieces[1])
        .ok_or_else(|| err("The selected record field is unavailable"))?
        .to_json()?;
    let source_id = body
        .get("from")
        .and_then(|v| typed_text(v).ok())
        .ok_or_else(|| err("The reading has no recorded source"))?;
    let cited = entries
        .get(source_id)
        .and_then(|v| typed_map(v).ok())
        .ok_or_else(|| err("The reading has no recorded source"))?;
    let name = ["name", "title", "label"]
        .iter()
        .find_map(|key| cited.get(*key).and_then(|v| typed_text(v).ok()))
        .unwrap_or(source_id);
    let at = body
        .get("at")
        .and_then(|v| typed_text(v).ok())
        .unwrap_or("Location not recorded");
    let date = body
        .get("of")
        .or_else(|| cited.get("read"))
        .map(|v| v.to_json())
        .transpose()?
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "Date not recorded".into());
    Ok(
        json!({"value":atom(&value)?,"citation":{"source":source_id,"name":name,"at":at,"date":date}}),
    )
}

fn typed_atom(value: &crate::value::TypedValue) -> Result<Value> {
    atom(&value.to_json()?)
}
fn core_record_source(
    path: &Path,
    inputs: &BTreeMap<String, Value>,
    at: &str,
) -> Result<(Map<String, Value>, Value)> {
    use crate::{
        history_contract::{map as typed_map, text as typed_text},
        history_view::list as typed_list,
        reasoning_context::CapturedAssessment,
        reasoning_runtime::OperationalBounds,
        source_capture::{ReadMode, capture_source_with_runtime},
        value::TypedValue,
    };
    let cwd = path.parent().ok_or_else(|| err("invalid record path"))?;
    let runtime = crate::public_workspace::core_runtime()
        .map_err(|e| err(format!("The source could not be assessed as core/v1: {e}")))?;
    let capture = capture_source_with_runtime(
        &[path.to_owned()],
        cwd,
        ReadMode::Frozen,
        None,
        runtime.as_ref(),
    )
    .map_err(|e| err(format!("The source could not be assessed as core/v1: {e}")))?;
    let context = CapturedAssessment::from_snapshot(
        capture.snapshot()?.clone(),
        None,
        "focused-review/v1",
        runtime.as_ref(),
        OperationalBounds::default(),
        None,
    )
    .map_err(|e| err(format!("The source could not be assessed as core/v1: {e}")))?;
    let snapshot = capture.snapshot()?.to_data();
    let snapshot_nodes = typed_map(&typed_map(&snapshot)?["nodes"])?;
    let report = context.assessment();
    let report_map = typed_map(report)?;
    let report_nodes = typed_map(&report_map["nodes"])?;
    let report_history = typed_map(&report_map["history_subjects"])?;
    let mut selected_ids = BTreeSet::new();
    let mut selections = Map::new();
    for (key, input) in inputs {
        let mut selector = input.as_object().unwrap().clone();
        selector.remove("source");
        let mut selection = json!({"selector":selector,"status":"unavailable"});
        if selector.is_empty() {
            selection["status"] = json!("available");
            selection["note"] = json!("Source identity captured; no excerpt selected");
            selections.insert(key.clone(), selection);
            continue;
        }
        let pointer = input.get("pointer").and_then(Value::as_str).unwrap_or("");
        let pieces = pointer
            .strip_prefix('/')
            .unwrap_or("")
            .split('/')
            .map(|p| p.replace("~1", "/").replace("~0", "~"))
            .collect::<Vec<_>>();
        if pieces.len() != 2 || !matches!(pieces[1].as_str(), "v" | "quoted") {
            selection["reason"] =
                json!("Record selectors are /ENTRY/v or /ENTRY/quoted for stored readings");
            selections.insert(key.clone(), selection);
            continue;
        }
        selected_ids.insert(pieces[0].clone());
        let outcome = (|| -> Result<Value> {
            let node = typed_map(
                snapshot_nodes
                    .get(&pieces[0])
                    .ok_or_else(|| err("The selected record field is unavailable"))?,
            )?;
            let body = typed_map(&node["body"])?;
            let value = body
                .get(&pieces[1])
                .ok_or_else(|| err("The selected record field is unavailable"))?;
            let source_id = body
                .get("from")
                .and_then(|v| typed_text(v).ok())
                .ok_or_else(|| err("The reading has no recorded source"))?;
            let cited = typed_map(
                &typed_map(
                    snapshot_nodes
                        .get(source_id)
                        .ok_or_else(|| err("The reading has no recorded source"))?,
                )?["body"],
            )?;
            let name = ["name", "title", "label"]
                .iter()
                .find_map(|k| cited.get(*k).and_then(|v| typed_text(v).ok()))
                .unwrap_or(source_id);
            let at_value = body
                .get("at")
                .and_then(|v| typed_text(v).ok())
                .unwrap_or("Location not recorded");
            let date = body
                .get("of")
                .or_else(|| cited.get("read"))
                .map(|v| v.to_json())
                .transpose()?
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_else(|| "Date not recorded".into());
            Ok(
                json!({"value":typed_atom(value)?,"citation":{"source":source_id,"name":name,"at":at_value,"date":date}}),
            )
        })();
        match outcome {
            Ok(v) => {
                selection["status"] = json!("available");
                selection["value"] = v["value"].clone();
                selection["citation"] = v["citation"].clone();
            }
            Err(e) => selection["reason"] = json!(e.0.trim_start_matches("document: ")),
        }
        selections.insert(key.clone(), selection);
    }
    let encode = |value: &TypedValue| value.to_tagged_bounded(2_000_000).map_err(|e| err(e.0));
    let nodes = selected_ids
        .iter()
        .filter_map(|id| report_nodes.get(id).map(|v| (id.clone(), encode(v))))
        .map(|(id, v)| Ok((id, v?)))
        .collect::<Result<Map<String, Value>>>()?;
    let history = selected_ids
        .iter()
        .filter_map(|id| report_history.get(id).map(|v| (id.clone(), encode(v))))
        .map(|(id, v)| Ok((id, v?)))
        .collect::<Result<Map<String, Value>>>()?;
    let assessment_selection = typed_list(&report_map["assessment_selection"])?
        .iter()
        .map(|v| v.to_json())
        .collect::<Result<Vec<_>>>()?;
    let mut receipt = json!({"version":1,"assessment_profile":"core/v1","schema_version":report_map["schema_version"].to_json()?,"finding_encoding":"kpopper-typed-json/v1","captured_at":at,"snapshot_id":context.snapshot_id(),"findings_revision":context.findings_revision(),"assessment_selection":assessment_selection,"embedded_selection":selected_ids.iter().cloned().collect::<Vec<_>>(),"history_selection":history.keys().cloned().collect::<Vec<_>>(),"nodes":nodes,"history_subjects":history});
    receipt["receipt_revision"] = json!(digest(canonical(&receipt)?));
    capture
        .verify()
        .map_err(|e| err(format!("The source changed during capture: {e}")))?;
    Ok((selections, receipt))
}

fn capture_sources(
    specs: &Value,
    claims: &[Value],
    root: &Path,
    at: &str,
    previous: Option<&Value>,
) -> Result<Value> {
    let specs = object(specs, "source inputs")?;
    if specs.len() > 100 {
        return Err(err("Expected at most 100 sources"));
    }
    let mut needed = BTreeMap::<String, BTreeMap<String, Value>>::new();
    for claim in claims {
        for input in claim["inputs"].as_array().unwrap() {
            needed
                .entry(input["source"].as_str().unwrap().into())
                .or_default()
                .insert(selection_key(input)?, input.clone());
        }
    }
    if specs.keys().any(|k| !needed.contains_key(k)) {
        return Err(err("Source inputs include an unreferenced source"));
    }
    let previous = previous.and_then(Value::as_object);
    if needed
        .keys()
        .any(|k| !specs.contains_key(k) && previous.is_none_or(|p| !p.contains_key(k)))
    {
        return Err(err(
            "Every source reference must be declared, including unavailable sources",
        ));
    }
    let mut result = Map::new();
    for (sid, inputs) in needed {
        if !valid_id(&sid) {
            return Err(err("Invalid source ID"));
        }
        if !specs.contains_key(&sid) {
            let mut old = previous.unwrap()[&sid].clone();
            old["reread"] = Value::Bool(false);
            result.insert(sid, old);
            continue;
        }
        let spec = source_spec(&specs[&sid])?;
        let mut source = json!({"name":spec["name"],"uri":spec.get("uri").cloned().unwrap_or(Value::Null),"format":spec.get("format").cloned().unwrap_or(Value::Null),"representation":spec.get("representation").cloned().unwrap_or(json!("file")),"read_at":Value::Null,"attempted_at":at,"reread":true,"sha256":Value::Null,"status":"unavailable","selections":{},"reason":spec.get("unavailable").cloned().unwrap_or(json!("Source unavailable"))});
        if spec.contains_key("unavailable") {
            result.insert(sid, source);
            continue;
        }
        let path = scoped(
            root,
            spec["path"]
                .as_str()
                .ok_or_else(|| err("source path must be text"))?,
        )?;
        if spec.get("profile") == Some(&json!("core/v1")) {
            source["profile"] = json!("core/v1");
            match core_record_source(&path, &inputs, at) {
                Ok((selections, assessment)) => {
                    source["status"] = json!("available");
                    source["sha256"] = assessment["snapshot_id"].clone();
                    source["read_at"] = json!(at);
                    source["reason"] = Value::Null;
                    source["selections"] = Value::Object(selections);
                    source["assessment"] = assessment;
                    if spec.contains_key("event_id") {
                        source["event"] = event_binding(
                            &path,
                            spec,
                            root,
                            source["selections"].as_object().unwrap(),
                        )?;
                    }
                }
                Err(_) => source["reason"] = json!("The source could not be assessed as core/v1"),
            }
            result.insert(sid, source);
            continue;
        }
        let raw = match read_limited(&path, MAX_SOURCE) {
            Ok(v) => v,
            Err(_) => {
                source["reason"] = json!(format!(
                    "The source could not be read as {}",
                    spec["format"]
                ));
                result.insert(sid, source);
                continue;
            }
        };
        let text = match String::from_utf8(raw.clone()) {
            Ok(v) => v.trim_start_matches('\u{feff}').to_owned(),
            Err(_) => {
                source["reason"] = json!(format!(
                    "The source could not be read as {}",
                    spec["format"]
                ));
                result.insert(sid, source);
                continue;
            }
        };
        if spec["format"] == "record" {
            let hypotheses = if path.file_name().and_then(|n| n.to_str()) == Some("GROUNDING.yaml")
            {
                path.parent().unwrap().join(".kpopper/hypotheses")
            } else {
                path.parent().unwrap().join("PROVENANCE.d")
            };
            let has_hypotheses = fs::symlink_metadata(&hypotheses)
                .is_ok_and(|m| m.file_type().is_symlink())
                || fs::read_dir(&hypotheses).ok().is_some_and(|entries| {
                    entries.filter_map(|e| e.ok()).any(|e| {
                        matches!(
                            e.path().extension().and_then(|x| x.to_str()),
                            Some("yaml" | "yml")
                        )
                    })
                });
            if has_hypotheses {
                source["reason"] =
                    json!("Records with hypotheses require an explicit source extraction");
                result.insert(sid, source);
                continue;
            }
        }
        let data = if spec["format"] == "json" {
            match read_json(&text) {
                Ok(v) => Some(v),
                Err(_) => {
                    source["reason"] = json!("The source could not be read as json");
                    result.insert(sid, source);
                    continue;
                }
            }
        } else {
            None
        };
        source["status"] = json!("available");
        source["sha256"] = json!(digest(&raw));
        source["read_at"] = json!(at);
        source["reason"] = Value::Null;
        for (key, input) in inputs {
            let mut selector = input.as_object().unwrap().clone();
            selector.remove("source");
            let mut selected = json!({"selector":selector,"status":"unavailable"});
            let selected_value = if selector.is_empty() {
                selected["note"] = json!("Source identity captured; no excerpt selected");
                Ok(Value::Null)
            } else if spec["format"] == "record" {
                record_selection(&raw, &input).map(|record| {
                    if let Some(citation) = record.get("citation") {
                        selected["citation"] = citation.clone();
                    }
                    record["value"].clone()
                })
            } else if let Some(p) = input.get("pointer").and_then(Value::as_str) {
                if spec["format"] != "json" {
                    Err(err("The selector does not match the source format"))
                } else {
                    pointer(data.as_ref().unwrap(), p).and_then(|v| atom(&v))
                }
            } else if let Some(q) = input.get("quote").and_then(Value::as_str) {
                if spec["format"] != "text" {
                    Err(err("The selector does not match the source format"))
                } else {
                    let positions: Vec<_> = text.match_indices(q).collect();
                    if positions.len() != 1 {
                        Err(err("The selected quotation is absent or ambiguous"))
                    } else {
                        let (start, _) = positions[0];
                        let end = start + q.len();
                        selected["location"] = json!(format!(
                            "lines {}–{}",
                            text[..start].matches('\n').count() + 1,
                            text[..end].matches('\n').count() + 1
                        ));
                        atom(&json!(q))
                    }
                }
            } else {
                Err(err("The selector does not match the source format"))
            };
            match selected_value {
                Ok(v) => {
                    selected["status"] = json!("available");
                    if !selector.is_empty() {
                        selected["value"] = v
                    }
                }
                Err(e) => selected["reason"] = json!(e.0.trim_start_matches("document: ")),
            }
            source["selections"][&key] = selected;
        }
        if spec.contains_key("event_id") {
            source["event"] =
                event_binding(&path, spec, root, source["selections"].as_object().unwrap())?;
        }
        if read_limited(&path, MAX_SOURCE).ok().as_deref() != Some(&raw) {
            return Err(err(
                "A source changed during capture; retry with a stable source copy",
            ));
        }
        result.insert(sid, source);
    }
    Ok(Value::Object(result))
}

fn selected(input: &Value, sources: &Value) -> Result<Value> {
    let source = &sources[&input["source"].as_str().unwrap()];
    let selection = &source["selections"][selection_key(input)?];
    if source["status"] != "available" || selection["status"] != "available" {
        return Err(err(selection
            .get("reason")
            .or_else(|| source.get("reason"))
            .and_then(Value::as_str)
            .unwrap_or("Selected evidence is unavailable")));
    }
    Ok(selection["value"].clone())
}
fn literal(atom: &Value, format: Option<&Value>) -> Result<String> {
    let kind = atom["type"].as_str().unwrap_or("");
    if kind != "number" {
        if format.is_some() {
            return Err(err(
                "A numeric format cannot be applied to a nonnumeric reading",
            ));
        }
        return Ok(match kind {
            "boolean" => atom["value"].as_bool().unwrap().to_string(),
            "string" => atom["value"].as_str().unwrap().into(),
            _ => return Err(err("Invalid reading")),
        });
    }
    let mut n = Dec::parse(atom["value"].as_str().unwrap())?;
    let Some(format) = format else {
        let mut s = n.plain();
        if s.contains('.') {
            while s.ends_with('0') {
                s.pop();
            }
            if s.ends_with('.') {
                s.pop();
            }
        }
        return Ok(if matches!(s.as_str(), "" | "-0") {
            "0".into()
        } else {
            s
        });
    };
    let f = format.as_object().unwrap();
    if let Some(scale) = f.get("scale") {
        n = n.mul(&Dec::parse(&scalar_number(scale)?)?)
    }
    let decimals = f.get("decimals").and_then(Value::as_i64).unwrap_or(0) as i32;
    n = n.round(decimals);
    let mut s = n.plain();
    let mut parts = s.split('.');
    let integer = parts.next().unwrap().to_owned();
    let fraction = parts.next().unwrap_or("").to_owned();
    let thousands = f.get("thousands").and_then(Value::as_str).unwrap_or("");
    if !thousands.is_empty() {
        let (negative, digits) = integer
            .strip_prefix('-')
            .map_or(("", integer.as_str()), |d| ("-", d));
        let mut grouped = String::new();
        for (i, ch) in digits.chars().enumerate() {
            if i > 0 && (digits.len() - i) % 3 == 0 {
                grouped.push_str(thousands)
            }
            grouped.push(ch)
        }
        s = format!("{negative}{grouped}");
    } else {
        s = integer
    }
    if decimals > 0 {
        s.push_str(f.get("decimal").and_then(Value::as_str).unwrap_or("."));
        s.push_str(&fraction)
    }
    Ok(format!(
        "{}{}{}",
        f.get("prefix").and_then(Value::as_str).unwrap_or(""),
        s,
        f.get("suffix").and_then(Value::as_str).unwrap_or("")
    ))
}
fn expected(claim: &Value, sources: &Value) -> Result<String> {
    let atoms: Vec<_> = claim["inputs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| selected(i, sources))
        .collect::<Result<_>>()?;
    let format = claim.get("format");
    match claim["kind"].as_str().unwrap() {
        "value" | "quote" => literal(&atoms[0], format),
        kind => {
            if atoms.iter().any(|a| a["type"] != "number") {
                return Err(err(
                    "Arithmetic needs numeric source readings, not interpreted text",
                ));
            }
            let nums: Vec<_> = atoms
                .iter()
                .map(|a| Dec::parse(a["value"].as_str().unwrap()))
                .collect::<Result<_>>()?;
            let value = match kind {
                "sum" => nums.iter().fold(
                    Dec {
                        n: 0.into(),
                        scale: 0,
                    },
                    |a, b| a.add(b),
                ),
                "difference" => nums[0].sub(&nums[1]),
                "product" => nums.iter().fold(
                    Dec {
                        n: 1.into(),
                        scale: 0,
                    },
                    |a, b| a.mul(b),
                ),
                "ratio" => nums[0].div(&nums[1], 240)?,
                _ => return Err(err("This claim has no supported mechanical check")),
            };
            literal(&json!({"type":"number","value":value.plain()}), format)
        }
    }
}

fn evaluate(
    claims: &[Value],
    parsed: &Value,
    sources: &Value,
    at: &str,
    previous: Option<&Value>,
) -> Value {
    let mut checks = Map::new();
    for claim in claims {
        let id = claim["id"].as_str().unwrap();
        let anchor = &parsed["anchors"][id];
        let changed = previous.is_some_and(|p| {
            claim["inputs"].as_array().unwrap().iter().any(|i| {
                let sid = i["source"].as_str().unwrap();
                sources[sid]["sha256"] != p[sid]["sha256"]
                    || sources[sid]["status"] != p[sid]["status"]
            })
        });
        let mut check = json!({"actual":anchor["text"],"status":"unchecked","expected":Value::Null,"detail":"","checked_at":Value::Null,"source_changed":changed});
        if claim["kind"] == "inference" {
            let mut detail = claim["reason"].as_str().unwrap().to_owned();
            if changed {
                detail.push_str(" Sources changed; this inference needs another review.")
            }
            check["detail"] = json!(detail);
        } else if anchor["text_only"] != true {
            check["status"] = json!("invalid");
            check["detail"] = json!(format!(
                "Mechanical claim {id} must anchor a text-only element"
            ));
        } else {
            match expected(claim, sources) {
                Ok(expected) => {
                    check["status"] = json!(if anchor["text"] == expected {
                        "match"
                    } else {
                        "mismatch"
                    });
                    check["expected"] = json!(expected);
                    check["checked_at"] = json!(at);
                    check["detail"] = json!(format!(
                        "Exact {} check against the captured source readings.",
                        claim["kind"].as_str().unwrap()
                    ));
                }
                Err(e) => {
                    check["status"] = json!("unavailable");
                    check["detail"] = json!(e.0.trim_start_matches("document: "));
                }
            }
        }
        checks.insert(id.into(), check);
    }
    Value::Object(checks)
}

struct Dsu(BTreeMap<String, String>);
impl Dsu {
    fn find(&mut self, x: &str) -> String {
        let p = self.0[x].clone();
        if p == x {
            p
        } else {
            let r = self.find(&p);
            self.0.insert(x.into(), r.clone());
            r
        }
    }
    fn union(&mut self, a: &str, b: &str) {
        let ar = self.find(a);
        let br = self.find(b);
        self.0.insert(br, ar);
    }
}
fn proposal_groups(
    authored: &str,
    claims: &[Value],
    parsed: &Value,
    sources: &Value,
    checks: &Value,
    at: &str,
) -> Result<Value> {
    let mechanical: Vec<_> = claims.iter().filter(|c| c["kind"] != "inference").collect();
    let mut dsu = Dsu(mechanical
        .iter()
        .map(|c| {
            let id = c["id"].as_str().unwrap().to_owned();
            (id.clone(), id)
        })
        .collect());
    let mut bindings = BTreeMap::new();
    let mut explicit = BTreeMap::new();
    let mut contexts = BTreeMap::new();
    for c in &mechanical {
        let id = c["id"].as_str().unwrap();
        for input in c["inputs"].as_array().unwrap() {
            let key = format!(
                "{}:{}",
                input["source"].as_str().unwrap(),
                selection_key(input)?
            );
            if let Some(other) = bindings.insert(key, id.to_owned()) {
                dsu.union(id, &other)
            }
        }
        if let Some(group) = c.get("group").and_then(Value::as_str)
            && let Some(other) = explicit.insert(group, id.to_owned())
        {
            dsu.union(id, &other)
        }
        let a = &parsed["anchors"][id];
        let key = (
            a["context_start"].as_u64().unwrap(),
            a["context_end"].as_u64().unwrap(),
        );
        if let Some(other) = contexts.insert(key, id.to_owned()) {
            dsu.union(id, &other)
        }
    }
    let mut components = BTreeMap::<String, Vec<&Value>>::new();
    for c in mechanical {
        components
            .entry(dsu.find(c["id"].as_str().unwrap()))
            .or_default()
            .push(c)
    }
    let mut groups = vec![];
    for members in components.into_values() {
        if members
            .iter()
            .all(|c| checks[c["id"].as_str().unwrap()]["status"] == "match")
        {
            continue;
        }
        let ids: Vec<_> = members
            .iter()
            .map(|c| c["id"].as_str().unwrap().to_owned())
            .collect();
        let blocked = ids
            .iter()
            .any(|id| checks[id]["status"] == "unavailable" || checks[id]["status"] == "invalid");
        let mut edits = vec![];
        if !blocked {
            for id in &ids {
                if checks[id]["status"] == "mismatch" {
                    let a = &parsed["anchors"][id];
                    edits.push(json!({"id":id,"start":a["start"],"end":a["end"],"before_raw":a["raw"],"after_raw":html::escaped_replacement(checks[id]["expected"].as_str().unwrap()),"before":a["text"],"after":checks[id]["expected"]}));
                }
            }
        }
        let mut after_checks = json!({});
        let mut contexts_out = vec![];
        if !edits.is_empty() {
            let candidate = html::patch_html(authored, &edits)?;
            let reparsed = html::parse_html(
                &candidate,
                &claims
                    .iter()
                    .map(|c| c["id"].as_str().unwrap().to_owned())
                    .collect::<Vec<_>>(),
            )?;
            after_checks = evaluate(claims, &reparsed, sources, at, None);
            if ids.iter().any(|id| after_checks[id]["status"] != "match") {
                return Err(err(
                    "The proposed group failed its independent candidate recheck",
                ));
            }
            let authored_chars: Vec<char> = authored.chars().collect();
            let mut seen = BTreeSet::new();
            for id in &ids {
                let a = &parsed["anchors"][id];
                let key = (
                    a["context_start"].as_u64().unwrap() as usize,
                    a["context_end"].as_u64().unwrap() as usize,
                );
                if seen.insert(key) {
                    let part: String = authored_chars[key.0..key.1].iter().collect();
                    let local = edits
                        .iter()
                        .filter_map(|edit| {
                            let start = edit["start"].as_u64()? as usize;
                            let end = edit["end"].as_u64()? as usize;
                            if start >= key.0 && end <= key.1 {
                                let mut edit = edit.clone();
                                edit["start"] = json!(start - key.0);
                                edit["end"] = json!(end - key.0);
                                Some(edit)
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>();
                    let context_after = html::text_content(&html::patch_html(&part, &local)?);
                    contexts_out.push(json!({"before":a["context_text"],"after":context_after}));
                }
            }
        }
        let gid = format!(
            "g-{}",
            &digest(canonical(&json!(
                ids.iter().cloned().collect::<BTreeSet<_>>()
            ))?)[..16]
        );
        let mut selected_checks = Map::new();
        for id in &ids {
            if !edits.is_empty() {
                selected_checks.insert(id.clone(), after_checks[id].clone());
            }
        }
        groups.push(json!({"id":gid,"label":members.iter().map(|c|c["label"].as_str().unwrap()).collect::<Vec<_>>().join(" / "),"members":ids,"status":if blocked{"blocked"}else{"ready"},"decision":"pending","decided_at":Value::Null,"reason":if blocked{json!("Required evidence is unavailable; the related edits stay together.")}else{Value::Null},"edits":edits,"contexts":contexts_out,"after_checks":selected_checks}));
    }
    Ok(Value::Array(groups))
}

#[allow(clippy::too_many_arguments)]
fn make_artifact(
    authored: &str,
    claims: &[Value],
    sources: Value,
    title: &str,
    language: &str,
    at: &str,
    previous: Value,
    history: Value,
) -> Result<Value> {
    let ids = claims
        .iter()
        .map(|c| c["id"].as_str().unwrap().into())
        .collect::<Vec<_>>();
    let parsed = html::parse_html(authored, &ids)?;
    for claim in claims {
        let id = claim["id"].as_str().unwrap();
        if claim["kind"] != "inference" && parsed["anchors"][id]["text_only"] != true {
            return Err(err(format!(
                "Mechanical claim {id} must anchor a text-only element"
            )));
        }
    }
    let checks = evaluate(
        claims,
        &parsed,
        &sources,
        at,
        if previous.is_null() {
            None
        } else {
            Some(&previous)
        },
    );
    let groups = proposal_groups(authored, claims, &parsed, &sources, &checks, at)?;
    Ok(
        json!({"version":1,"title":title,"language":language,"generated_at":at,"authored_html":authored,"authored_sha256":digest(authored),"head_end":parsed["head_end"],"claims":claims,"sources":sources,"checks":checks,"groups":groups,"coverage":parsed["coverage"],"history":history,"previous_sources":previous}),
    )
}
pub fn build(authored: &str, manifest: &Value, root: &Path, at: Option<&str>) -> Result<Value> {
    let m = object(manifest, "manifest")?;
    keys(
        m,
        &["version", "title", "language", "claims", "sources"],
        "manifest",
    )?;
    if m.get("version").and_then(Value::as_i64) != Some(1) {
        return Err(err("Expected document manifest version 1"));
    }
    let claims = normalize_claims(m.get("claims").unwrap_or(&Value::Null))?;
    let timestamp = at.map(str::to_owned).unwrap_or_else(now);
    let title = match m.get("title") {
        Some(value) => text(Some(value), "title", false, 500)?,
        None => "Document",
    };
    let language = match m.get("language") {
        Some(value) => text(Some(value), "language", false, 40)?,
        None => "en",
    };
    let sources = capture_sources(
        m.get("sources").unwrap_or(&json!({})),
        &claims,
        root,
        &timestamp,
        None,
    )?;
    make_artifact(
        authored,
        &claims,
        sources,
        title,
        language,
        &timestamp,
        Value::Null,
        json!([]),
    )
}

fn selected_html(data: &Value) -> Result<String> {
    let edits = data["groups"]
        .as_array()
        .ok_or_else(|| err("Invalid proposals"))?
        .iter()
        .filter(|g| g["decision"] == "accepted")
        .flat_map(|g| g["edits"].as_array().unwrap().iter().cloned())
        .collect::<Vec<_>>();
    html::patch_html(data["authored_html"].as_str().unwrap(), &edits)
}
fn validate_snapshots(sources: &Value, claims: &[Value]) -> Result<()> {
    let sources = object(sources, "embedded source snapshots")?;
    if sources.len() > 100 {
        return Err(err("Invalid embedded source snapshots"));
    }
    for claim in claims {
        for input in claim["inputs"].as_array().unwrap() {
            if !sources.contains_key(input["source"].as_str().unwrap()) {
                return Err(err("An embedded source is missing"));
            }
        }
    }
    for (sid, source) in sources {
        if !valid_id(sid) {
            return Err(err("Invalid embedded source ID"));
        }
        let source = object(source, "source snapshot")?;
        if !source
            .get("status")
            .and_then(Value::as_str)
            .is_some_and(|s| matches!(s, "available" | "unavailable"))
        {
            return Err(err("Invalid source snapshot"));
        }
        text(source.get("name"), "source name", false, 300)?;
        if source.get("status") == Some(&json!("available")) {
            let sha = source.get("sha256").and_then(Value::as_str).unwrap_or("");
            if sha.len() != 64
                || !sha
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                || source.get("read_at").and_then(Value::as_str).is_none()
            {
                return Err(err(
                    "An available snapshot needs its revision and read time",
                ));
            }
        }
        let selections = source
            .get("selections")
            .and_then(Value::as_object)
            .ok_or_else(|| err("Invalid selected evidence"))?;
        for (key, selection) in selections {
            let selection = object(selection, "selected evidence")?;
            if !selection
                .get("status")
                .and_then(Value::as_str)
                .is_some_and(|s| matches!(s, "available" | "unavailable"))
            {
                return Err(err("Invalid selected evidence"));
            }
            let mut input = selection
                .get("selector")
                .and_then(Value::as_object)
                .cloned()
                .ok_or_else(|| err("Invalid selected evidence"))?;
            input.insert("source".into(), json!(sid));
            let input = normalize_input(&Value::Object(input))?;
            if selection_key(&input)? != *key {
                return Err(err("Evidence selector identity differs"));
            }
            if selection.get("status") == Some(&json!("available"))
                && input.as_object().unwrap().len() > 1
            {
                let value = selection
                    .get("value")
                    .and_then(Value::as_object)
                    .ok_or_else(|| err("Invalid embedded scalar reading"))?;
                match value.get("type").and_then(Value::as_str) {
                    Some("number") => {
                        Dec::parse(
                            value
                                .get("value")
                                .and_then(Value::as_str)
                                .ok_or_else(|| err("Invalid embedded scalar reading"))?,
                        )?;
                    }
                    Some("string") if value.get("value").is_some_and(Value::is_string) => {}
                    Some("boolean") if value.get("value").is_some_and(Value::is_boolean) => {}
                    _ => return Err(err("Invalid embedded scalar reading")),
                }
            }
        }
        if source.get("profile") == Some(&json!("core/v1"))
            && source.get("status") == Some(&json!("available"))
        {
            let receipt = source
                .get("assessment")
                .and_then(Value::as_object)
                .ok_or_else(|| err("Invalid embedded core finding"))?;
            let expected = [
                "version",
                "assessment_profile",
                "schema_version",
                "finding_encoding",
                "captured_at",
                "snapshot_id",
                "findings_revision",
                "assessment_selection",
                "embedded_selection",
                "history_selection",
                "nodes",
                "history_subjects",
                "receipt_revision",
            ];
            if receipt.len() != expected.len()
                || expected.iter().any(|k| !receipt.contains_key(*k))
                || receipt["version"] != 1
                || receipt["assessment_profile"] != "core/v1"
                || receipt["schema_version"] != 3
                || receipt["finding_encoding"] != "kpopper-typed-json/v1"
            {
                return Err(err("Invalid embedded core finding"));
            }
            let mut preimage = receipt.clone();
            let revision = preimage
                .remove("receipt_revision")
                .and_then(|v| v.as_str().map(str::to_owned))
                .ok_or_else(|| err("Invalid embedded core finding"))?;
            if digest(canonical(&Value::Object(preimage))?) != revision {
                return Err(err("Invalid embedded core finding"));
            }
            if source.get("sha256") != receipt.get("snapshot_id") {
                return Err(err(
                    "Core source revision differs from its captured Snapshot",
                ));
            }
            let embedded = receipt["embedded_selection"]
                .as_array()
                .ok_or_else(|| err("Invalid embedded core finding"))?;
            let embedded_ids = embedded
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| err("Invalid embedded core finding"))
                })
                .collect::<Result<Vec<_>>>()?;
            let mut sorted = embedded_ids.clone();
            sorted.sort();
            sorted.dedup();
            if sorted != embedded_ids {
                return Err(err("Invalid embedded core finding"));
            }
            let nodes = receipt["nodes"]
                .as_object()
                .ok_or_else(|| err("Invalid embedded core finding"))?;
            if nodes.keys().cloned().collect::<Vec<_>>() != embedded_ids {
                return Err(err("Invalid embedded core finding"));
            }
            for value in nodes.values().chain(
                receipt["history_subjects"]
                    .as_object()
                    .ok_or_else(|| err("Invalid embedded core finding"))?
                    .values(),
            ) {
                let decoded = crate::value::TypedValue::from_tagged_bounded(value, 2_000_000)
                    .map_err(|_| err("Invalid embedded core finding"))?;
                if decoded.to_tagged_bounded(2_000_000)? != *value {
                    return Err(err("Invalid embedded core finding"));
                }
            }
        } else if source.get("assessment").is_some() {
            return Err(err(
                "Core assessment evidence needs an available core/v1 source",
            ));
        }
    }
    Ok(())
}
pub fn validate_artifact(data: &Value) -> Result<()> {
    if data.get("version").and_then(Value::as_i64) != Some(1) {
        return Err(err("Unsupported document artifact version"));
    }
    let authored = data
        .get("authored_html")
        .and_then(Value::as_str)
        .ok_or_else(|| err("Invalid embedded document"))?;
    if digest(authored) != data["authored_sha256"] {
        return Err(err(
            "The embedded authored document changed outside the review state",
        ));
    }
    let claims = normalize_claims(&data["claims"])?;
    validate_snapshots(&data["sources"], &claims)?;
    if let Some(previous) = data.get("previous_sources").filter(|v| !v.is_null()) {
        validate_snapshots(previous, &claims)?;
    }
    let regenerated = make_artifact(
        authored,
        &claims,
        data["sources"].clone(),
        text(data.get("title"), "title", false, 500)?,
        text(data.get("language"), "language", false, 40)?,
        text(data.get("generated_at"), "generation time", false, 50)?,
        data.get("previous_sources").cloned().unwrap_or(Value::Null),
        data.get("history").cloned().unwrap_or(json!([])),
    )?;
    for key in ["head_end", "coverage", "checks"] {
        if regenerated[key] != data[key] {
            return Err(err(format!(
                "Embedded {key} no longer matches its document and evidence"
            )));
        }
    }
    let stored = data["groups"]
        .as_array()
        .ok_or_else(|| err("Invalid proposals"))?;
    let generated = regenerated["groups"].as_array().unwrap();
    if stored.len() != generated.len() {
        return Err(err("Embedded proposals no longer match their checks"));
    }
    for (a, b) in stored.iter().zip(generated) {
        if !matches!(
            a["decision"].as_str(),
            Some("pending" | "accepted" | "rejected")
        ) {
            return Err(err("Invalid saved review decision"));
        }
        if b["status"] != "ready" && a["decision"] != "pending" {
            return Err(err(
                "A blocked proposal cannot have been accepted or rejected",
            ));
        }
        let mut normalized = a.clone();
        normalized["decision"] = json!("pending");
        normalized["decided_at"] = Value::Null;
        if normalized != *b {
            return Err(err(
                "An embedded proposal changed outside its review decision",
            ));
        }
    }
    Ok(())
}
pub fn refresh(
    data: &Value,
    source_inputs: &Value,
    root: &Path,
    at: Option<&str>,
) -> Result<Value> {
    validate_artifact(data)?;
    if data["groups"]
        .as_array()
        .unwrap()
        .iter()
        .any(|g| g["status"] == "ready" && g["decision"] == "pending")
    {
        return Err(err(
            "Decide the pending proposals and export the copy before refreshing again",
        ));
    }
    let timestamp = at.map(str::to_owned).unwrap_or_else(now);
    let authored = selected_html(data)?;
    let claims = normalize_claims(&data["claims"])?;
    let sources = capture_sources(
        source_inputs,
        &claims,
        root,
        &timestamp,
        Some(&data["sources"]),
    )?;
    let mut history = data["history"].as_array().cloned().unwrap_or_default();
    history.push(json!({"generated_at":data["generated_at"],"authored_sha256":data["authored_sha256"],"decisions":data["groups"].as_array().unwrap().iter().map(|g|json!({"id":g["id"],"decision":g["decision"],"decided_at":g["decided_at"]})).collect::<Vec<_>>() }));
    if history.len() > 100 {
        return Err(err(
            "This copy has reached 100 refreshes; author a new document from its current selected content",
        ));
    }
    make_artifact(
        &authored,
        &claims,
        sources,
        data["title"].as_str().unwrap(),
        data["language"].as_str().unwrap(),
        &timestamp,
        data["sources"].clone(),
        Value::Array(history),
    )
}

pub fn summary(data: &Value) -> Result<Value> {
    validate_artifact(data)?;
    let mut checks = data["checks"].clone();
    for group in data["groups"].as_array().unwrap() {
        if group["decision"] == "accepted" {
            for (k, v) in group["after_checks"].as_object().unwrap() {
                checks[k] = v.clone();
            }
        }
    }
    Ok(
        json!({"title":data["title"],"generated_at":data["generated_at"],"coverage":data["coverage"],"checks":checks,"sources":data["sources"],"proposals":data["groups"].as_array().unwrap().iter().map(|g|json!({"id":g["id"],"status":g["status"],"decision":g["decision"],"members":g["members"],"reason":g["reason"]})).collect::<Vec<_>>(),"history":data["history"]}),
    )
}
pub fn render(data: &Value) -> Result<String> {
    validate_artifact(data)?;
    let css = include_str!("../../scripts/document/layer.css");
    let js = include_str!("../../scripts/document/layer.js");
    let bridge = include_str!("../../scripts/document/frame.js");
    if js.to_ascii_lowercase().contains("</script") || css.to_ascii_lowercase().contains("</style")
    {
        return Err(err(
            "The packaged runtime contains an unsafe raw element terminator",
        ));
    }
    let runtime = safe_json(&json!({"bridge":bridge,"csp":FRAME_CSP}))?;
    let shell_csp = FRAME_CSP.replace("frame-src 'none'", "frame-src about:");
    Ok(format!(
        "<!doctype html>\n<html lang=\"{}\"><head><meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\" content=\"{}\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{}</title><style>{css}</style></head><body><noscript><p class=\"kp-noscript\">JavaScript is required to display this document and its evidence and review controls. Open this file in a browser with JavaScript enabled. No live sources are checked by opening it.</p></noscript><iframe id=\"kp-document\" title=\"{}\" sandbox=\"allow-scripts\"></iframe><button id=\"kp-open\" type=\"button\" aria-controls=\"kp-panel\" aria-expanded=\"false\">Evidence</button><aside id=\"kp-panel\" hidden aria-label=\"Document evidence\"></aside><div id=\"kp-notice\" role=\"status\" aria-live=\"polite\"></div><script id=\"kp-data\" type=\"application/json\">{}</script><script id=\"kp-runtime\" type=\"application/json\">{runtime}</script><script>{js}</script></body></html>\n",
        html_attr(data["language"].as_str().unwrap()),
        html_attr(&shell_csp),
        html_text(data["title"].as_str().unwrap()),
        html_attr(data["title"].as_str().unwrap()),
        safe_json(data)?
    ))
}
fn html_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
fn html_attr(s: &str) -> String {
    html_text(s).replace('"', "&quot;").replace('\'', "&#39;")
}
pub fn load_artifact(source: &str) -> Result<Value> {
    if source.len() > MAX_ARTIFACT {
        return Err(err("Input exceeds the supported size limit"));
    }
    let marker = "<script id=\"kp-data\" type=\"application/json\">";
    let mut matches = source.match_indices(marker);
    let Some((start, _)) = matches.next() else {
        return Err(err("Expected one standalone kpopper document payload"));
    };
    if matches.next().is_some() {
        return Err(err("Expected one standalone kpopper document payload"));
    }
    let rest = &source[start + marker.len()..];
    let end = rest
        .find("</script>")
        .ok_or_else(|| err("Invalid embedded document payload"))?;
    let data = read_json(&rest[..end])?;
    validate_artifact(&data)?;
    Ok(data)
}
pub fn inspect(source: &str) -> Result<Value> {
    summary(&load_artifact(source)?)
}

fn operation_result(data: &Value, output: &Path) -> Value {
    let mut counts = Map::new();
    for status in ["match", "mismatch", "unavailable", "unchecked"] {
        counts.insert(
            status.into(),
            json!(
                data["checks"]
                    .as_object()
                    .unwrap()
                    .values()
                    .filter(|check| check["status"] == status)
                    .count()
            ),
        );
    }
    json!({"output":output.to_string_lossy(),"checks":counts,"coverage":data["coverage"],"proposals":data["groups"].as_array().map_or(0,Vec::len),"source_files_changed":false})
}
fn protected_sources(specs: &Value, root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for spec in object(specs, "source inputs")?.values() {
        if let Some(path) = spec.get("path").and_then(Value::as_str) {
            paths.push(scoped(root, path)?);
        }
    }
    Ok(paths)
}
/// Path-level build boundary used by the CLI. Reads are bounded and the output
/// cannot alias either authoring input or any declared source.
pub fn build_files(
    html_path: &Path,
    manifest_path: &Path,
    root: Option<&Path>,
    output: &Path,
    overwrite: bool,
) -> Result<Value> {
    let authored = String::from_utf8(read_limited(html_path, MAX_ARTIFACT)?)
        .map_err(|_| err("Authored HTML must be UTF-8 text"))?;
    let manifest = read_json(
        &String::from_utf8(read_limited(manifest_path, MAX_SOURCE)?)
            .map_err(|_| err("Manifest must be UTF-8 JSON"))?,
    )?;
    let root = root.map(Path::to_owned).unwrap_or_else(|| {
        manifest_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_owned()
    });
    let data = build(&authored, &manifest, &root, None)?;
    let mut protected = vec![html_path.to_owned(), manifest_path.to_owned()];
    let empty_sources = Value::Object(Map::new());
    protected.extend(protected_sources(
        manifest.get("sources").unwrap_or(&empty_sources),
        &root,
    )?);
    let output = write_output(&data, output, overwrite, &protected)?;
    Ok(operation_result(&data, &output))
}
/// Path-level refresh boundary used by the CLI. Only named source definitions
/// are reread; omitted snapshots remain in the copy and are marked `reread=false`.
pub fn refresh_file(
    artifact_path: &Path,
    sources_path: &Path,
    root: Option<&Path>,
    output: &Path,
    overwrite: bool,
) -> Result<Value> {
    let artifact = String::from_utf8(read_limited(artifact_path, MAX_ARTIFACT)?)
        .map_err(|_| err("Artifact must be UTF-8 HTML"))?;
    let data = load_artifact(&artifact)?;
    let sources = read_json(
        &String::from_utf8(read_limited(sources_path, MAX_SOURCE)?)
            .map_err(|_| err("Source inputs must be UTF-8 JSON"))?,
    )?;
    let root = root.map(Path::to_owned).unwrap_or_else(|| {
        sources_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_owned()
    });
    let refreshed = refresh(&data, &sources, &root, None)?;
    let mut protected = vec![artifact_path.to_owned(), sources_path.to_owned()];
    protected.extend(protected_sources(&sources, &root)?);
    let output = write_output(&refreshed, output, overwrite, &protected)?;
    Ok(operation_result(&refreshed, &output))
}
pub fn inspect_file(artifact_path: &Path) -> Result<Value> {
    let artifact = String::from_utf8(read_limited(artifact_path, MAX_ARTIFACT)?)
        .map_err(|_| err("Artifact must be UTF-8 HTML"))?;
    inspect(&artifact)
}

pub fn write_output(
    data: &Value,
    output: &Path,
    overwrite: bool,
    protected: &[PathBuf],
) -> Result<PathBuf> {
    let absolute = if output.is_absolute() {
        output.to_owned()
    } else {
        std::env::current_dir()?.join(output)
    };
    if fs::symlink_metadata(&absolute).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(err(
            "Output must be a separate copy, not an input or source file",
        ));
    }
    let output_identity = absolute.canonicalize().unwrap_or_else(|_| absolute.clone());
    for p in protected {
        if p.canonicalize().unwrap_or_else(|_| p.clone()) == output_identity {
            return Err(err(
                "Output must be a separate copy, not an input or source file",
            ));
        }
    }
    let raw = render(data)?.into_bytes();
    if raw.len() > MAX_ARTIFACT {
        return Err(err("Standalone output exceeds the 40 MB limit"));
    }
    let parent = absolute
        .parent()
        .ok_or_else(|| err("Output has no parent directory"))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    use std::io::Write;
    temp.write_all(&raw)?;
    temp.as_file().sync_all()?;
    if overwrite {
        temp.persist(&absolute)
            .map_err(|e| Error(format!("io: {}", e.error)))?;
    } else {
        temp.persist_noclobber(&absolute)
            .map_err(|e| Error(format!("io: {}", e.error)))?;
    }
    Ok(absolute)
}
