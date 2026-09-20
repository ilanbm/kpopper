//! Complete, ordered ordinary source values. The historical OrdinaryValue API
//! remains an explicit finite façade, converted only through try_finite.
use crate::{
    Error, Result, history_yaml as Y,
    ordinary_value::{Map, NonFiniteFloat, Scalar, Value},
    require,
    value::TypedValue as V,
};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    scalar: Scalar,
    constructor: Option<usize>,
}
impl Key {
    pub fn new(scalar: Scalar, constructor: Option<usize>) -> Result<Self> {
        require(
            constructor.is_none_or(|n| n < crate::value::MAX_VALUES * 2),
            "history_limit",
        )?;
        if let Scalar::Finite(v) = &scalar {
            Y::ordinary_key(v.clone())?;
        }
        Ok(Self {
            scalar,
            constructor,
        })
    }
    pub fn text_key(value: impl Into<String>) -> Self {
        Self {
            scalar: Scalar::Finite(V::Text(value.into())),
            constructor: None,
        }
    }
    pub fn scalar(&self) -> &Scalar {
        &self.scalar
    }
    pub fn text(&self) -> Option<&str> {
        match &self.scalar {
            Scalar::Finite(V::Text(v)) => Some(v),
            _ => None,
        }
    }
    pub fn python_eq(&self, other: &Self) -> bool {
        match (&self.scalar, &other.scalar) {
            (Scalar::Finite(a), Scalar::Finite(b)) => Y::ordinary_key(a.clone())
                .unwrap()
                .python_eq(&Y::ordinary_key(b.clone()).unwrap()),
            (Scalar::NonFinite(NonFiniteFloat::NaN), Scalar::NonFinite(NonFiniteFloat::NaN)) => {
                self.constructor == other.constructor
            }
            (Scalar::NonFinite(a), Scalar::NonFinite(b)) => a == b,
            _ => false,
        }
    }
    fn index(&self) -> String {
        match &self.scalar {
            Scalar::Finite(v) => Y::ordinary_key_index(&Y::ordinary_key(v.clone()).unwrap()),
            Scalar::NonFinite(v) => format!(
                "\0kpopper:ordinary-key:k:nonfinite:{}:{:020}",
                v.python_str(),
                self.constructor.unwrap_or(usize::MAX)
            ),
        }
    }
}
#[derive(Clone, Debug)]
pub enum Source {
    Scalar(Scalar),
    List(Vec<Source>),
    Map(Vec<(Key, Source)>),
}
impl Source {
    pub fn from_typed(value: &V) -> Self {
        match value {
            V::List(v) => Self::List(v.iter().map(Self::from_typed).collect()),
            V::Map(v) => Self::Map(
                v.iter()
                    .map(|(k, v)| (Key::text_key(k.clone()), Self::from_typed(v)))
                    .collect(),
            ),
            _ => Self::Scalar(Scalar::Finite(value.clone())),
        }
    }
    pub fn from_finite(value: &Y::OrdinaryValue) -> Self {
        match value {
            Y::OrdinaryValue::Scalar(v) => Self::Scalar(Scalar::Finite(v.clone())),
            Y::OrdinaryValue::List(v) => Self::List(v.iter().map(Self::from_finite).collect()),
            Y::OrdinaryValue::Map(v) => Self::Map(
                v.iter()
                    .map(|(k, v)| {
                        (
                            Key::new(Scalar::Finite(k.scalar().clone()), None).unwrap(),
                            Self::from_finite(v),
                        )
                    })
                    .collect(),
            ),
        }
    }
    pub fn get(&self, key: &str) -> Option<&Self> {
        let Self::Map(m) = self else { return None };
        m.iter()
            .find(|(k, _)| k.text() == Some(key))
            .map(|(_, v)| v)
    }
    pub fn projected(&self) -> Value {
        match self {
            Self::Scalar(v) => v.value(),
            Self::List(v) => Value::List(v.iter().map(Self::projected).collect()),
            Self::Map(v) => {
                let mut out = Map::new();
                for (k, v) in v {
                    out.insert_source_key(k.index(), k.scalar.clone(), v.projected());
                }
                Value::Map(out)
            }
        }
    }
    pub fn try_finite(&self) -> Result<Y::OrdinaryValue> {
        Ok(match self {
            Self::Scalar(Scalar::Finite(v)) => Y::OrdinaryValue::Scalar(v.clone()),
            Self::Scalar(Scalar::NonFinite(_)) => {
                return Err(Error(
                    "invalid_history_value: snapshot data contains a nonfinite value".into(),
                ));
            }
            Self::List(v) => {
                Y::OrdinaryValue::List(v.iter().map(Self::try_finite).collect::<Result<_>>()?)
            }
            Self::Map(v) => Y::OrdinaryValue::Map(
                v.iter()
                    .map(|(k, v)| {
                        let key = k.scalar.try_typed().map_err(|_| {
                            Error(
                                "invalid_history_value: snapshot mappings require string keys"
                                    .into(),
                            )
                        })?;
                        Ok((Y::ordinary_key(key)?, v.try_finite()?))
                    })
                    .collect::<Result<_>>()?,
            ),
        })
    }
    pub fn strict_typed(&self) -> Result<V> {
        self.try_finite()?.strict_typed()
    }
    /// Source-body matching ignores constructor numbering from a separate parse,
    /// never the scalar kind, duplicate-key multiplicity or authored value.
    pub fn same_content(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Scalar(a), Self::Scalar(b)) => a == b,
            (Self::List(a), Self::List(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.same_content(b))
            }
            (Self::Map(a), Self::Map(b)) => {
                if a.len() != b.len() {
                    return false;
                }
                let mut used = vec![false; b.len()];
                for (k, v) in a {
                    let Some(i) = b.iter().enumerate().position(|(i, (l, w))| {
                        !used[i] && k.scalar == l.scalar && v.same_content(w)
                    }) else {
                        return false;
                    };
                    used[i] = true;
                }
                true
            }
            _ => false,
        }
    }
}
pub fn update(map: &mut Vec<(Key, Source)>, key: Key, value: Source) {
    if let Some((_, old)) = map.iter_mut().find(|(old, _)| old.python_eq(&key)) {
        *old = value
    } else {
        map.push((key, value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_source_values_are_distinct_from_finite_facades_and_preserve_key_identity() {
        for token in [
            ".nan",
            ".NaN",
            ".NAN",
            ".inf",
            "+.inf",
            "-.INF",
            "1.0e+999",
            "!!float nan",
        ] {
            let raw = format!("known:\n  p.value: {{name: Sample, v: {token}}}\n");
            let source = Y::decode_full_ordinary_source_value(raw.as_bytes()).unwrap();
            assert!(source.try_finite().is_err());
            assert!(source.strict_typed().is_err());
            assert!(Y::decode_ordinary_source_value(raw.as_bytes()).is_err());
            assert!(Y::decode_document(raw.as_bytes()).is_err());
            assert!(source.projected().python_json(false).unwrap().contains(
                if token.contains("nan") || token.contains("NaN") || token.contains("NAN") {
                    "NaN"
                } else {
                    "Infinity"
                }
            ));
        }
        for (token, count) in [
            (".nan: first\n.nan: second\n", 1),
            ("!!float nan: first\n!!float nan: second\n", 2),
            ("&key !!float nan: first\n*key: second\n", 1),
            (".inf: first\n+.inf: second\n", 1),
        ] {
            let source = Y::decode_full_ordinary_source_value(token.as_bytes()).unwrap();
            let Source::Map(m) = &source else { panic!() };
            assert_eq!(m.len(), count);
            assert_eq!(
                source
                    .projected()
                    .python_json(false)
                    .unwrap()
                    .matches("NaN")
                    .count(),
                if token.contains("nan") { count } else { 0 }
            );
            assert!(source.try_finite().is_err());
            assert!(source.projected().try_typed().is_err());
            assert!(crate::ordinary_value::finite_payload(&source.projected()).is_err());
        }
        let quoted =
            Y::decode_full_ordinary_source_value(b"known: {p.value: {v: '.nan'}}\n").unwrap();
        assert!(quoted.try_finite().is_ok());
        let marker = Y::decode_full_ordinary_source_value(
            b"known: {p.value: {v: {kind: nonfinite, value: nan}}}\n",
        )
        .unwrap();
        assert!(marker.try_finite().is_ok());
    }
    #[test]
    fn mixed_key_json_falls_back_for_the_whole_document_without_losing_duplicates() {
        let source = Y::decode_full_ordinary_source_value(
            b"z: 1\na:\n  name: Sample\n  !!float nan: first\n  !!float nan: second\n",
        )
        .unwrap();
        assert_eq!(
            source.projected().python_json(true).unwrap(),
            "{\"z\": 1, \"a\": {\"name\": \"Sample\", \"NaN\": \"first\", \"NaN\": \"second\"}}"
        );
        assert_eq!(
            source.projected().python_str(),
            "{'z': 1, 'a': {'name': 'Sample', nan: 'first', nan: 'second'}}"
        );
        let replay = Y::decode_full_ordinary_source_value(
            b"z: 1\na:\n  name: Sample\n  !!float nan: first\n  !!float nan: second\n",
        )
        .unwrap();
        assert!(source.same_content(&replay));
    }
}
