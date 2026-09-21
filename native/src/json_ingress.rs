//! Raw JSON ingress that keeps source tokens distinct from serde_json's
//! arbitrary-precision representation details.

use serde::{Deserialize, de};
use serde_json::{Map, Number, Value, value::RawValue};

const SERDE_DEFAULT_MAX_DEPTH: usize = 127;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DuplicateKeys {
    Reject,
    LastWins,
}

/// Parse JSON with serde_json's normal recursion envelope.
pub fn parse_slice(raw: &[u8], duplicate_keys: DuplicateKeys) -> serde_json::Result<Value> {
    parse(raw, duplicate_keys, None)
}

/// Parse JSON text with serde_json's normal recursion envelope.
pub fn parse_str(raw: &str, duplicate_keys: DuplicateKeys) -> serde_json::Result<Value> {
    parse_slice(raw.as_bytes(), duplicate_keys)
}

/// Parse JSON with an explicit nesting bound, including bounds deeper than
/// serde_json's normal recursion envelope.
pub fn parse_slice_bounded(
    raw: &[u8],
    duplicate_keys: DuplicateKeys,
    max_depth: usize,
) -> serde_json::Result<Value> {
    check_depth(raw, max_depth)?;
    parse(raw, duplicate_keys, Some(max_depth))
}

fn custom(message: impl std::fmt::Display) -> serde_json::Error {
    <serde_json::Error as de::Error>::custom(message)
}

fn check_depth(raw: &[u8], max_depth: usize) -> serde_json::Result<()> {
    let (mut quoted, mut escaped, mut depth) = (false, false, 0usize);
    for byte in raw {
        if quoted {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                quoted = false;
            }
        } else {
            match byte {
                b'"' => quoted = true,
                b'[' | b'{' => {
                    depth += 1;
                    if depth > max_depth {
                        return Err(custom("JSON nesting exceeds the supported depth"));
                    }
                }
                b']' | b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    Ok(())
}

fn parse(
    raw: &[u8],
    duplicate_keys: DuplicateKeys,
    explicit_depth: Option<usize>,
) -> serde_json::Result<Value> {
    if explicit_depth.is_none() {
        check_depth(raw, SERDE_DEFAULT_MAX_DEPTH)?;
    }
    let mut decoder = serde_json::Deserializer::from_slice(raw);
    if explicit_depth.is_some() {
        decoder.disable_recursion_limit();
    }
    let raw_value = <&RawValue>::deserialize(&mut decoder)?;
    decoder.end()?;
    decode(raw_value, duplicate_keys, explicit_depth.is_some())
}

fn decode(
    raw: &RawValue,
    duplicate_keys: DuplicateKeys,
    unbounded_decoder: bool,
) -> serde_json::Result<Value> {
    struct ObjectVisitor {
        duplicate_keys: DuplicateKeys,
        unbounded_decoder: bool,
    }
    impl<'de> de::Visitor<'de> for ObjectVisitor {
        type Value = Map<String, Value>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a JSON object")
        }

        fn visit_map<A: de::MapAccess<'de>>(
            self,
            mut fields: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            let mut result = Map::new();
            while let Some(key) = fields.next_key::<String>()? {
                if self.duplicate_keys == DuplicateKeys::Reject && result.contains_key(&key) {
                    return Err(de::Error::custom(format!("Duplicate JSON key: {key}")));
                }
                let raw = fields.next_value::<&RawValue>()?;
                let value = decode(raw, self.duplicate_keys, self.unbounded_decoder)
                    .map_err(de::Error::custom)?;
                result.insert(key, value);
            }
            Ok(result)
        }
    }

    let text = raw.get();
    let mut decoder = serde_json::Deserializer::from_str(text);
    if unbounded_decoder {
        decoder.disable_recursion_limit();
    }
    let value = match text.as_bytes().first() {
        Some(b'{') => serde::Deserializer::deserialize_map(
            &mut decoder,
            ObjectVisitor {
                duplicate_keys,
                unbounded_decoder,
            },
        )
        .map(Value::Object),
        Some(b'[') => Vec::<&RawValue>::deserialize(&mut decoder).and_then(|items| {
            items
                .into_iter()
                .map(|item| decode(item, duplicate_keys, unbounded_decoder))
                .collect::<serde_json::Result<Vec<_>>>()
                .map(Value::Array)
        }),
        Some(b'"') => String::deserialize(&mut decoder).map(Value::String),
        Some(b'n') => <()>::deserialize(&mut decoder).map(|()| Value::Null),
        Some(b't' | b'f') => bool::deserialize(&mut decoder).map(Value::Bool),
        Some(_) => Number::deserialize(&mut decoder).map(Value::Number),
        None => unreachable!("RawValue cannot be empty"),
    }?;
    decoder.end()?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn private_number_key_is_always_an_object_key() {
        let value = parse_str(
            r#"{"outer":[{"$serde_json::private::Number":"{\"injected\":true}"}]}"#,
            DuplicateKeys::Reject,
        )
        .unwrap();
        assert_eq!(
            value,
            json!({"outer":[{"$serde_json::private::Number":"{\"injected\":true}"}]})
        );
    }

    #[test]
    fn genuine_numbers_keep_arbitrary_precision_and_finite_float_spelling() {
        let value = parse_str(
            "[123456789012345678901234567890,5e-324,-0.0]",
            DuplicateKeys::Reject,
        )
        .unwrap();
        assert_eq!(
            value[0].as_number().unwrap().to_string(),
            "123456789012345678901234567890"
        );
        assert_eq!(value[1].as_number().unwrap().to_string(), "5e-324");
        assert_eq!(value[2].as_number().unwrap().to_string(), "-0.0");
    }

    #[test]
    fn duplicate_policy_is_explicit_at_every_depth() {
        let raw = br#"{"a":{"x":1,"x":2}}"#;
        assert!(parse_slice(raw, DuplicateKeys::Reject).is_err());
        assert_eq!(
            parse_slice(raw, DuplicateKeys::LastWins).unwrap(),
            json!({"a":{"x":2}})
        );
    }

    #[test]
    fn explicit_depth_can_exceed_serde_default_but_remains_bounded() {
        let nested = format!("{}0{}", "[".repeat(180), "]".repeat(180));
        assert!(parse_str(&nested, DuplicateKeys::Reject).is_err());
        assert!(parse_slice_bounded(nested.as_bytes(), DuplicateKeys::Reject, 180).is_ok());
        assert!(parse_slice_bounded(nested.as_bytes(), DuplicateKeys::Reject, 179).is_err());
    }

    #[test]
    fn deep_valid_typed_json_uses_only_the_explicit_envelope() {
        let mut typed = crate::value::TypedValue::Null;
        for _ in 0..100 {
            typed = crate::value::TypedValue::List(vec![typed]);
        }
        let encoded = serde_json::to_vec(&typed.to_tagged().unwrap()).unwrap();
        assert!(parse_slice(&encoded, DuplicateKeys::LastWins).is_err());
        let decoded = parse_slice_bounded(&encoded, DuplicateKeys::LastWins, 400).unwrap();
        assert_eq!(
            crate::value::TypedValue::from_tagged(&decoded).unwrap(),
            typed
        );
        assert!(parse_slice_bounded(&encoded, DuplicateKeys::LastWins, 200).is_err());
    }

    #[test]
    fn malformed_trailing_and_non_json_numbers_are_rejected() {
        for raw in ["", "1 2", "[1", "NaN", "Infinity", "01"] {
            assert!(parse_str(raw, DuplicateKeys::Reject).is_err(), "{raw}");
        }
    }
}
