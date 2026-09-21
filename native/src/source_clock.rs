//! Source-clock comparison retained from the history reader, without assessment.
use crate::{
    Error, Result,
    history_yaml::{SourceValue as S, numeric_text},
    source_text::python_str,
    value::{Date, TypedValue as V},
};
use num_bigint::BigInt;
use regex::Regex;
use std::{cmp::Ordering, sync::LazyLock};

pub type Ancestry<'a> = dyn Fn(&V, &V) -> bool + 'a;
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RevisionPart {
    Number(BigInt),
    Text(String),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ClockKey {
    Day(String),
    Stamp(i64),
    Revision(Vec<RevisionPart>),
}
fn invalid() -> Error {
    Error("invalid_source_clock".into())
}
impl ClockKey {
    pub fn compare(&self, other: &Self) -> Result<Ordering> {
        match (self, other) {
            (Self::Day(a), Self::Day(b)) => Ok(a.cmp(b)),
            (Self::Stamp(a), Self::Stamp(b)) => Ok(a.cmp(b)),
            (Self::Revision(a), Self::Revision(b)) => {
                for (a, b) in a.iter().zip(b) {
                    let cmp = match (a, b) {
                        (RevisionPart::Number(a), RevisionPart::Number(b)) => a.cmp(b),
                        (RevisionPart::Text(a), RevisionPart::Text(b)) => a.cmp(b),
                        _ => return Err(invalid()),
                    };
                    if cmp != Ordering::Equal {
                        return Ok(cmp);
                    }
                }
                Ok(a.len().cmp(&b.len()))
            }
            _ => Err(invalid()),
        }
    }
}
fn nondecimal_digit(c: char) -> bool {
    const RANGES: &[(u32, u32)] = &[
        (178, 179),
        (185, 185),
        (4969, 4977),
        (6618, 6618),
        (8304, 8304),
        (8308, 8313),
        (8320, 8329),
        (9312, 9320),
        (9332, 9340),
        (9352, 9360),
        (9450, 9450),
        (9461, 9469),
        (9471, 9471),
        (10102, 10110),
        (10112, 10120),
        (10122, 10130),
        (68160, 68163),
        (69216, 69224),
        (69714, 69722),
        (127232, 127242),
    ];
    RANGES.iter().any(|(a, b)| (*a..=*b).contains(&(c as u32)))
}
fn revision(s: &str) -> Result<Vec<RevisionPart>> {
    s.split(['.', '-'])
        .map(|p| {
            let normalized = numeric_text(p);
            if !p.is_empty()
                && normalized
                    .chars()
                    .all(|c| c.is_ascii_digit() || nondecimal_digit(c))
            {
                if normalized.len() > 4300 || !normalized.bytes().all(|c| c.is_ascii_digit()) {
                    return Err(invalid());
                }
                Ok(RevisionPart::Number(
                    normalized.parse().map_err(|_| invalid())?,
                ))
            } else {
                Ok(RevisionPart::Text(p.into()))
            }
        })
        .collect()
}
pub fn clock_parts(at: &S) -> Option<(&str, &S)> {
    if let S::Map(a) = at
        && a.len() == 1
    {
        Some((&a[0].0, &a[0].1))
    } else {
        None
    }
}
pub(crate) fn key(at: &S) -> Result<Option<ClockKey>> {
    let Some((kind, v)) = clock_parts(at) else {
        return Ok(None);
    };
    let text = python_str(v);
    Ok(match kind {
        "day" => {
            let digits = numeric_text(text.strip_suffix('\n').unwrap_or(&text));
            if digits.len() == 10
                && digits.bytes().enumerate().all(|(i, c)| {
                    if i == 4 || i == 7 {
                        c == b'-'
                    } else {
                        c.is_ascii_digit()
                    }
                })
            {
                Some(ClockKey::Day(text))
            } else {
                None
            }
        }
        "stamp" => instant(&text).map(ClockKey::Stamp),
        "revision" => Some(ClockKey::Revision(revision(&text)?)),
        _ => None,
    })
}
static STAMP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)\A(?P<date>[0-9]{4}-[0-9]{2}-[0-9]{2}|[0-9]{8}|[0-9]{4}-W[0-9]{2}(?:-[1-7])?|[0-9]{4}W[0-9]{2}[1-7]?)(?:.(?P<time>.+))?\z").unwrap()
});
static TIME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\A(?P<h>[0-9]{2})(?:(?P<m>[0-9]{2})(?:(?P<s>[0-9]{2})(?:[.,](?P<f>[0-9]+))?)?|:(?P<mc>[0-9]{2})(?::(?P<sc>[0-9]{2})(?:[.,](?P<fc>[0-9]+))?)?)?\z").unwrap()
});
fn leap(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}
fn ordinal(y: i64, m: i64, d: i64) -> i64 {
    let prior = y - 1;
    365 * prior + prior / 4 - prior / 100
        + prior / 400
        + [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334][m as usize - 1]
        + i64::from(m > 2 && leap(y))
        + d
}
fn date_ordinal(s: &str) -> Option<i64> {
    let compact = s.replace('-', "");
    let y = compact.get(..4)?.parse::<i64>().ok()?;
    if !(1..=9999).contains(&y) {
        return None;
    }
    if compact.as_bytes().get(4) == Some(&b'W') {
        let week = compact.get(5..7)?.parse::<i64>().ok()?;
        let day = if compact.len() == 8 {
            compact[7..].parse::<i64>().ok()?
        } else {
            1
        };
        let jan1 = (ordinal(y, 1, 1) - 1) % 7;
        let maxweek = if jan1 == 3 || jan1 == 2 && leap(y) {
            53
        } else {
            52
        };
        if !(1..=maxweek).contains(&week) || !(1..=7).contains(&day) {
            return None;
        }
        let jan4 = ordinal(y, 1, 4);
        let n = jan4 - (jan4 - 1) % 7 + (week - 1) * 7 + day - 1;
        return (n <= ordinal(9999, 12, 31)).then_some(n);
    }
    let m = compact.get(4..6)?.parse::<i64>().ok()?;
    let d = compact.get(6..8)?.parse::<i64>().ok()?;
    Date::new(&format!("{y:04}-{m:02}-{d:02}")).ok()?;
    Some(ordinal(y, m, d))
}
fn time_parts(s: &str) -> Option<(i64, i64, i64, i64)> {
    let c = TIME.captures(s)?;
    let n = |a, b| {
        c.name(a)
            .or_else(|| c.name(b))
            .map_or(Some(0), |s| s.as_str().parse::<i64>().ok())
    };
    let fraction = c
        .name("f")
        .or_else(|| c.name("fc"))
        .map_or("", |s| s.as_str());
    let micro = format!("{:0<6}", &fraction[..fraction.len().min(6)])
        .parse::<i64>()
        .ok()?;
    Some((n("h", "h")?, n("m", "mc")?, n("s", "sc")?, micro))
}
/// The later of local and UTC calendar days, matching the writer's date boundary.
pub fn latest_day() -> String {
    let utc = chrono::Utc::now();
    let local = utc.with_timezone(&chrono::Local);
    utc.date_naive().max(local.date_naive()).to_string()
}
pub fn instant(value: &str) -> Option<i64> {
    let c = STAMP.captures(value)?;
    let day = date_ordinal(c.name("date")?.as_str())?;
    let Some(time) = c.name("time") else {
        return Some(day * 86_400_000_000);
    };
    let time = time.as_str();
    let (clock, offset) = if let Some(clock) = time.strip_suffix('Z') {
        (clock, 0)
    } else if let Some((index, sign)) = time.char_indices().find(|(_, c)| *c == '+' || *c == '-') {
        let (h, m, s, u) = time_parts(&time[index + 1..])?;
        let seconds = h * 3600 + m * 60 + s;
        // Python treats an all-zero seconds offset as UTC, even with a fraction.
        let delta = if seconds == 0 {
            0
        } else {
            seconds * 1_000_000 + u
        };
        if delta >= 86_400_000_000 {
            return None;
        }
        (&time[..index], if sign == '-' { -delta } else { delta })
    } else {
        (time, 0)
    };
    let (h, m, s, u) = time_parts(clock)?;
    if h > 24
        || m > 59
        || s > 59
        || h == 24 && (m != 0 || s != 0 || u != 0 || day == ordinal(9999, 12, 31))
    {
        return None;
    }
    Some(day * 86_400_000_000 + (h * 3600 + m * 60 + s) * 1_000_000 + u - offset)
}
fn integer_value(v: &V) -> Option<BigInt> {
    match v {
        V::Integer(v) => v.as_str().parse().ok(),
        V::Bool(v) => Some(BigInt::from(u8::from(*v))),
        V::Float(v) if v.get().fract() == 0.0 => format!("{:.0}", v.get()).parse().ok(),
        _ => None,
    }
}
pub(crate) fn python_equal(a: &V, b: &V) -> bool {
    if let (Some(a), Some(b)) = (integer_value(a), integer_value(b)) {
        return a == b;
    }
    match (a, b) {
        (V::Float(a), V::Float(b)) => a.get() == b.get(),
        (V::List(a), V::List(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| python_equal(a, b))
        }
        (V::Map(a), V::Map(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(k, a)| b.get(k).is_some_and(|b| python_equal(a, b)))
        }
        (V::DateTime(a), V::DateTime(b)) => {
            let aware = |v: &str| v[19..].contains(['+', '-']);
            aware(a.as_str()) == aware(b.as_str()) && instant(a.as_str()) == instant(b.as_str())
        }
        _ => a == b,
    }
}
pub fn order(a: &S, b: &S, ancestry: Option<&Ancestry<'_>>) -> Result<Option<Ordering>> {
    let (Some((ak, av)), Some((bk, bv))) = (clock_parts(a), clock_parts(b)) else {
        return Ok(None);
    };
    if ak != bk {
        return Ok(None);
    }
    if ak == "commit" {
        let a = av.typed();
        let b = bv.typed();
        if python_equal(&a, &b) {
            return Ok(Some(Ordering::Equal));
        }
        if let Some(ancestry) = ancestry {
            if ancestry(&a, &b) {
                return Ok(Some(Ordering::Less));
            }
            if ancestry(&b, &a) {
                return Ok(Some(Ordering::Greater));
            }
        }
        return Ok(None);
    }
    match (key(a)?, key(b)?) {
        (Some(a), Some(b)) => a.compare(&b).map(Some),
        _ => Ok(None),
    }
}
