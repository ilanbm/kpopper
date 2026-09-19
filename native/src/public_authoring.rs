//! Public unannotated authoring and guarded first-record creation.
use crate::{
    Result,
    history_authoring::{obj, s},
    history_bootstrap as B,
    history_contract::*,
    history_transaction_fs as F,
    history_view::map_mut,
    project_modes::WriteRoute,
    public_history::fresh_id,
    public_workspace, recording_privacy as Privacy, require,
    value::{FiniteFloat, Integer, TypedValue as V},
};
use std::{
    path::{Path, PathBuf},
    sync::LazyLock,
};

#[derive(Clone, Default, clap::Args)]
pub struct Options {
    pub subject: String,
    #[arg(allow_negative_numbers = true)]
    pub values: Vec<String>,
    #[arg(long, allow_hyphen_values = true)]
    pub value: Option<String>,
    #[arg(long)]
    pub source: Option<String>,
    #[arg(long)]
    pub at: Option<String>,
    #[arg(long)]
    pub why: Option<String>,
    #[arg(long)]
    pub as_of: Option<String>,
    #[arg(long = "in")]
    pub into: Option<String>,
    #[arg(long)]
    pub profile: Option<String>,
    #[arg(long)]
    pub hypothesis: Option<String>,
    #[arg(long = "drop")]
    pub drops: Vec<String>,
    #[arg(long)]
    pub operation: Option<String>,
    #[arg(long)]
    pub on: Option<String>,
    #[arg(long)]
    pub expected_revision: Option<String>,
}
static NUMBER: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^-?[0-9]+(?:\.[0-9]+)?$").unwrap());
fn typed(text: &str) -> Result<V> {
    // Decimal digit blocks used by the pinned Python Unicode tables. Python's
    // CLI number pattern accepts Nd digits, including mixed scripts.
    const ZEROES: &[u32] = &[
        0x30, 0x660, 0x6f0, 0x7c0, 0x966, 0x9e6, 0xa66, 0xae6, 0xb66, 0xbe6, 0xc66, 0xce6, 0xd66,
        0xde6, 0xe50, 0xed0, 0xf20, 0x1040, 0x1090, 0x17e0, 0x1810, 0x1946, 0x19d0, 0x1a80, 0x1a90,
        0x1b50, 0x1bb0, 0x1c40, 0x1c50, 0xa620, 0xa8d0, 0xa900, 0xa9d0, 0xa9f0, 0xaa50, 0xabf0,
        0xff10, 0x104a0, 0x10d30, 0x10d40, 0x11066, 0x110f0, 0x11136, 0x111d0, 0x112f0, 0x11450,
        0x114d0, 0x11650, 0x116c0, 0x116d0, 0x116da, 0x11730, 0x118e0, 0x11950, 0x11bf0, 0x11c50,
        0x11d50, 0x11da0, 0x11f50, 0x16130, 0x16a60, 0x16ac0, 0x16b50, 0x16d70, 0x1ccf0, 0x1d7ce,
        0x1d7d8, 0x1d7e2, 0x1d7ec, 0x1d7f6, 0x1e140, 0x1e2f0, 0x1e4f0, 0x1e5f1, 0x1e950, 0x1fbf0,
    ];
    let number = text
        .chars()
        .map(|c| {
            if c.is_ascii() {
                return c;
            }
            ZEROES
                .iter()
                .find_map(|zero| (c as u32).checked_sub(*zero).filter(|n| *n < 10))
                .map(|n| (b'0' + n as u8) as char)
                .unwrap_or(c)
        })
        .collect::<String>();
    if NUMBER.is_match(&number) {
        return if number.contains('.') {
            Ok(V::Float(FiniteFloat::new(
                number.parse().map_err(|_| error("invalid_number"))?,
            )?))
        } else {
            let integer: num_bigint::BigInt =
                number.parse().map_err(|_| error("invalid_number"))?;
            Ok(V::Integer(Integer::new(&integer.to_string())?))
        };
    }
    Ok(match text {
        "true" => V::Bool(true),
        "false" => V::Bool(false),
        _ => s(text),
    })
}
fn yaml(text: &str) -> Result<V> {
    Ok(crate::history_yaml::decode_source_value(text.as_bytes())?.typed())
}
fn action(kind: &str, options: &Options) -> Result<(V, Vec<PathBuf>)> {
    require(
        options.value.is_none()
            && options.operation.is_none()
            && options.on.is_none()
            && options.expected_revision.is_none(),
        "feasibility writer options require an explicitly marked store",
    )?;
    require(
        options.why.as_ref().is_none_or(|s| !s.contains('\n')),
        "--why is one line: a second line would be a line of the record",
    )?;
    if let Some(day) = &options.as_of {
        crate::value::Date::new(day)?;
        let today = chrono::Utc::now()
            .date_naive()
            .max(chrono::Local::now().date_naive())
            .to_string();
        require(day <= &today, "--as-of is after today")?;
    }
    let mut a = obj([
        ("kind", s(kind)),
        ("id", s(&options.subject)),
        ("as_of", options.as_of.as_deref().map(s).unwrap_or(V::Null)),
        ("why", options.why.as_deref().map(s).unwrap_or(V::Null)),
        ("into", options.into.as_deref().map(s).unwrap_or(V::Null)),
        (
            "hypothesis",
            options.hypothesis.as_deref().map(s).unwrap_or(V::Null),
        ),
        (
            "source",
            options.source.as_deref().map(s).unwrap_or(V::Null),
        ),
        ("at", options.at.as_deref().map(s).unwrap_or(V::Null)),
    ]);
    if let Some(profile) = &options.profile {
        map_mut(&mut a)?.insert("profile".into(), s(profile));
    }
    if !options.drops.is_empty() {
        require(
            kind == "add",
            "--drop goes with add, on a judgment that replaces a standing one",
        )?;
        let mut drops = Map::new();
        for value in &options.drops {
            let (id, why) = value
                .split_once(':')
                .ok_or_else(|| error("--drop takes <id>: <why>"))?;
            require(
                !id.trim().is_empty() && !why.trim().is_empty(),
                "--drop takes both an id and a reason",
            )?;
            drops.insert(id.trim().into(), s(why.trim()));
        }
        map_mut(&mut a)?.insert("drops".into(), V::Map(drops));
    }
    let is_file = |s: &&String| (s.ends_with(".yaml") || s.ends_with(".yml")) && !s.contains('=');
    let paths;
    if kind == "set" {
        let first = options
            .values
            .first()
            .ok_or_else(|| error("set needs a value: set <key> <value>"))?;
        map_mut(&mut a)?.insert("value".into(), typed(first)?);
        paths = options.values[1..]
            .iter()
            .filter(is_file)
            .map(PathBuf::from)
            .collect();
    } else if kind == "add" {
        paths = options
            .values
            .iter()
            .filter(is_file)
            .map(PathBuf::from)
            .collect();
        let given = options
            .values
            .iter()
            .filter(|s| !is_file(s))
            .collect::<Vec<_>>();
        let body = if given.len() == 1 && given[0].trim_start().starts_with('{') {
            let value = yaml(given[0])?;
            require(matches!(value, V::Map(_)), "the fields must be a mapping")?;
            value
        } else if given.len() == 1 && !given[0].contains('=') {
            s(given[0])
        } else {
            let mut fields = Map::new();
            for value in given {
                let (field, value) = value
                    .split_once('=')
                    .ok_or_else(|| error("fields are written field=value"))?;
                let value = if value.starts_with('[')
                    || value.starts_with('{') && !value.starts_with("{{")
                {
                    yaml(value)?
                } else {
                    typed(value)?
                };
                fields.insert(field.trim().into(), value);
            }
            require(
                !fields.is_empty(),
                "add needs fields: add <id> v=... from=...",
            )?;
            V::Map(fields)
        };
        map_mut(&mut a)?.insert("body".into(), body);
    } else {
        paths = options
            .values
            .iter()
            .filter(is_file)
            .map(PathBuf::from)
            .collect();
    }
    Ok((a, paths))
}
pub fn run(kind: &str, options: &Options, cwd: &Path) -> Result<String> {
    let cwd = cwd.canonicalize()?;
    let (action, files) = action(kind, options)?;
    let implicit = files.is_empty();
    let original = if implicit {
        public_workspace::records(&cwd)?
    } else {
        files.iter().map(|p| cwd.join(p)).collect()
    };
    let route = WriteRoute::capture(&original, &cwd)?;
    require(route.paths().len() == 1, "choose one logical record entry")?;
    let entry = &route.paths()[0];
    let _lock =
        F::DirectoryGuard::acquire(entry.parent().ok_or_else(|| error("invalid_path"))?, true)?;
    if entry.exists()
        && crate::legacy_authoring::route(entry, route.config())?
            == crate::legacy_authoring::AuthorityRoute::Legacy
    {
        return crate::legacy_authoring::write(&action, &route);
    }
    if entry.exists() {
        drop(_lock);
        drop(route);
        let result = crate::direct_history::write(&original, &cwd, &action)?;
        if string_is(&map(&result)?["state"], "private draft") {
            return Ok(format!(
                "{}\n",
                crate::public_core_readers::json_value(&result)?
            ));
        }
        return Ok(format!(
            "history committed: {} ({kind} {})\n",
            text(&map(&result)?["operation"])?,
            options.subject
        ));
    }
    require(kind == "add" && implicit, "record not found")?;
    let body = map(&action)?["body"].clone();
    if Privacy::private_marker(&action) {
        let document = obj([(
            "known",
            V::Map(Map::from([(options.subject.clone(), body)])),
        )]);
        let draft = Privacy::draft(
            route.project(),
            &action,
            &document,
            "private or unclear source permission",
        )?;
        return Ok(format!(
            "{}\n",
            crate::public_core_readers::json_value(&draft)?
        ));
    }
    let runtime = public_workspace::core_runtime()?;
    let now = chrono::Utc::now();
    let mutation = B::prepare(
        entry,
        &action,
        route.config(),
        &B::BootstrapOptions {
            operation: fresh_id("first")?,
            recorded_at: now.to_rfc3339(),
            recording_day: now
                .date_naive()
                .max(chrono::Local::now().date_naive())
                .to_string(),
            record_id: fresh_id("record")?,
            by: V::Null,
        },
        runtime.as_ref(),
    )?;
    B::publish(
        entry,
        &mutation,
        route.config(),
        runtime.as_ref(),
        &mut |_| route.verify(),
    )?;
    crate::session_activity::published(
        entry.parent().unwrap(),
        mutation.files(),
        Some(&std::collections::BTreeSet::from([options.subject.clone()])),
    );
    Ok(format!(
        "history committed: {} ({kind} {})\ncreated {} - this workspace's record, born with its first entry\n",
        text(&map(&mutation.to_data())?["operation"])?,
        options.subject,
        entry.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cli_scalars_keep_their_types_instead_of_using_yaml_implicit_types() {
        for input in ["on", "off", "null", "07:30", "1e3", "{{p.x}}"] {
            assert_eq!(typed(input).unwrap(), s(input));
        }
        assert_eq!(typed("true").unwrap(), V::Bool(true));
        assert_eq!(typed("false").unwrap(), V::Bool(false));
        assert_eq!(
            typed("001").unwrap(),
            V::Integer(Integer::new("1").unwrap())
        );
        assert_eq!(
            typed("-١２").unwrap(),
            V::Integer(Integer::new("-12").unwrap())
        );
        assert_eq!(
            typed("١.２").unwrap(),
            V::Float(FiniteFloat::new(1.2).unwrap())
        );
        assert_eq!(
            typed("-0.0").unwrap(),
            V::Float(FiniteFloat::new(-0.0).unwrap())
        );
        let (parsed, files) = action(
            "add",
            &Options {
                subject: "p.x".into(),
                values: vec!["v=[null, false, 1.0]".into(), "source.yaml".into()],
                ..Options::default()
            },
        )
        .unwrap();
        assert_eq!(files, vec![PathBuf::from("source.yaml")]);
        assert_eq!(
            map(&map(&parsed).unwrap()["body"]).unwrap()["v"],
            V::List(vec![
                V::Null,
                V::Bool(false),
                V::Float(FiniteFloat::new(1.0).unwrap())
            ])
        );
    }
}
