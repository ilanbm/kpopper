//! Retained Python source-clock grouping uses str(value), including mapping order.
use crate::{history_yaml::SourceValue as S, identity::python_float, value::TypedValue as V};
use regex::Regex;
use std::sync::LazyLock;

static NONPRINTING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\p{C}\p{Z}]").unwrap());
fn quoted(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::from(quote);
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if c != ' ' && NONPRINTING.is_match(c.encode_utf8(&mut [0; 4])) => {
                let n = c as u32;
                out.push_str(&if n < 256 {
                    format!("\\x{n:02x}")
                } else if n < 65536 {
                    format!("\\u{n:04x}")
                } else {
                    format!("\\U{n:08x}")
                });
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}
fn numeric(s: &str) -> i64 {
    s.parse().expect("validated date component")
}
fn date_parts(s: &str) -> String {
    format!(
        "{}, {}, {}",
        numeric(&s[..4]),
        numeric(&s[5..7]),
        numeric(&s[8..10])
    )
}
fn datetime_repr(s: &str) -> String {
    let mut parts = format!(
        "{}, {}, {}",
        date_parts(s),
        numeric(&s[11..13]),
        numeric(&s[14..16])
    );
    let second = numeric(&s[17..19]);
    let mut tail = &s[19..];
    let micro = if let Some(f) = tail.strip_prefix('.') {
        let n = numeric(&f[..6]);
        tail = &f[6..];
        n
    } else {
        0
    };
    if second != 0 || micro != 0 {
        parts.push_str(&format!(", {second}"));
    }
    if micro != 0 {
        parts.push_str(&format!(", {micro}"));
    }
    if !tail.is_empty() {
        let mut offset = (numeric(&tail[1..3]) * 3600 + numeric(&tail[4..6]) * 60) * 1_000_000;
        if tail.len() > 6 {
            offset += numeric(&tail[7..9]) * 1_000_000;
            if tail.len() > 9 {
                offset += numeric(&tail[10..]);
            }
        }
        if tail.starts_with('-') {
            offset = -offset;
        }
        if offset == 0 {
            parts.push_str(", tzinfo=datetime.timezone.utc");
        } else {
            let days = offset.div_euclid(86_400_000_000);
            let remainder = offset.rem_euclid(86_400_000_000);
            let seconds = remainder / 1_000_000;
            let micros = remainder % 1_000_000;
            let mut args = Vec::new();
            if days != 0 {
                args.push(format!("days={days}"));
            }
            if seconds != 0 {
                args.push(format!("seconds={seconds}"));
            }
            if micros != 0 {
                args.push(format!("microseconds={micros}"));
            }
            parts.push_str(&format!(
                ", tzinfo=datetime.timezone(datetime.timedelta({}))",
                args.join(", ")
            ));
        }
    }
    format!("datetime.datetime({parts})")
}
pub fn python_repr(value: &S) -> String {
    match value {
        S::Scalar(V::Text(v)) => quoted(v),
        S::Scalar(V::Date(v)) => format!("datetime.date({})", date_parts(v.as_str())),
        S::Scalar(V::DateTime(v)) => datetime_repr(v.as_str()),
        S::List(a) => format!(
            "[{}]",
            a.iter().map(python_repr).collect::<Vec<_>>().join(", ")
        ),
        S::Map(a) => format!(
            "{{{}}}",
            a.iter()
                .map(|(k, v)| format!("{}: {}", quoted(k), python_repr(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => python_str(value),
    }
}
pub fn python_str(value: &S) -> String {
    match value {
        S::Scalar(V::Null) => "None".into(),
        S::Scalar(V::Bool(v)) => if *v { "True" } else { "False" }.into(),
        S::Scalar(V::Integer(v)) => v.as_str().into(),
        S::Scalar(V::Float(v)) => python_float(v.get()),
        S::Scalar(V::Text(v)) => v.clone(),
        S::Scalar(V::Date(v)) => v.as_str().into(),
        S::Scalar(V::DateTime(v)) => v.as_str().replacen('T', " ", 1),
        S::Scalar(v @ (V::Map(_) | V::List(_))) => python_str(&S::from_typed(v)),
        S::Map(_) | S::List(_) => python_repr(value),
    }
}

/// Python `str` for the ordinary reader's reversible `TypedValue` projection.
/// Reserved map keys are decoded back to their original scalar identity.
pub(crate) fn ordinary_python_repr(value: &V) -> String {
    match value {
        V::List(values) => format!(
            "[{}]",
            values
                .iter()
                .map(ordinary_python_repr)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        V::Map(values) => format!(
            "{{{}}}",
            values
                .iter()
                .map(|(key, value)| {
                    let key = crate::history_yaml::projected_ordinary_key(key)
                        .map(|key| python_repr(&S::Scalar(key)))
                        .unwrap_or_else(|| quoted(key));
                    format!("{key}: {}", ordinary_python_repr(value))
                })
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => python_repr(&S::from_typed(value)),
    }
}

pub(crate) fn ordinary_python_str(value: &V) -> String {
    match value {
        V::Map(_) | V::List(_) => ordinary_python_repr(value),
        _ => python_str(&S::from_typed(value)),
    }
}
