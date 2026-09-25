//! Public unannotated authoring and guarded first-record creation.
use crate::{
    Result,
    history_authoring::{obj, s},
    history_contract::*,
    history_transaction_fs as F,
    history_view::map_mut,
    history_yaml::SourceValue,
    project_modes::WriteRoute,
    public_workspace, recording_privacy as Privacy, require,
    value::{FiniteFloat, Integer, TypedValue as V},
};
use std::{
    path::{Path, PathBuf},
    sync::LazyLock,
};

#[path = "ordinary_contribution_authoring.rs"]
mod contribution_routing;

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
    /// Recorded author or reviewer; defaults to the current agent session when present.
    #[arg(long = "by")]
    pub actor: Option<String>,
    /// Replace a stored scalar by an equal-valued rule, preserving its identity and history.
    #[arg(long)]
    pub reframe: bool,
    /// Refuse if the locked active-history record differs from the caller's read.
    #[arg(long)]
    pub expected_record_sha256: Option<String>,
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
    #[arg(long, value_parser = ["project", "private", "unclear"])]
    pub shareability: Option<String>,
    #[arg(long, value_parser = ["project", "external", "code", "feature", "unclear"])]
    pub scope: Option<String>,
    #[arg(long)]
    pub environment: Option<String>,
    #[arg(long)]
    pub commit: Option<String>,
    #[arg(long)]
    pub event_id: Option<String>,
    #[arg(long)]
    pub contribution_id: Option<String>,
    #[arg(long)]
    pub evidence_root: Option<PathBuf>,
    #[arg(long = "disclose-locator")]
    pub disclose_locators: Vec<String>,
    /// Set by `answer` and `correct`: the whole new body of an existing entry.
    #[arg(skip)]
    pub amend: Option<Amend>,
}
/// A rewrite of one existing entry in place, prepared by `answer` or `correct`.
#[derive(Clone, Debug)]
pub struct Amend {
    /// `answer` or `correct`.
    pub kind: &'static str,
    pub body: V,
    /// Field order for ordinary records; `None` keeps the entry's own order.
    pub order: Option<SourceValue>,
    /// The entry that answered a question, pinned at its current head in history.
    pub by: Option<String>,
    /// The entry as the command read it; the write refuses if it changed since.
    pub was: V,
}
static NUMBER: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^-?[0-9]+(?:\.[0-9]+)?$").unwrap());
pub(crate) fn typed(text: &str) -> Result<V> {
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
pub(crate) fn yaml(text: &str) -> Result<SourceValue> {
    crate::history_yaml::decode_source_value(text.as_bytes())
}
fn action(kind: &str, options: &Options) -> Result<(V, Vec<PathBuf>, Option<SourceValue>)> {
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
    if options.reframe {
        require(kind == "add", "--reframe goes with add and a derived rule")?;
        map_mut(&mut a)?.insert("reframe".into(), V::Bool(true));
    }
    if let Some(expected) = &options.expected_record_sha256 {
        require(expected.len() == 64 && expected.bytes().all(|b| b.is_ascii_hexdigit()),
            "--expected-record-sha256 needs a SHA-256 digest")?;
        map_mut(&mut a)?.insert("expected_record_sha256".into(), s(&expected.to_ascii_lowercase()));
    }
    for (key, value) in [
        ("shareability", options.shareability.as_deref()),
        ("scope", options.scope.as_deref()),
        ("environment", options.environment.as_deref()),
        ("commit", options.commit.as_deref()),
        ("event_id", options.event_id.as_deref()),
        ("contribution_id", options.contribution_id.as_deref()),
    ] {
        if let Some(value) = value {
            map_mut(&mut a)?.insert(key.into(), s(value));
        }
    }
    if let Some(root) = &options.evidence_root {
        let root = root
            .to_str()
            .ok_or_else(|| error("invalid_evidence_root"))?;
        map_mut(&mut a)?.insert("evidence_root".into(), s(root));
    }
    if !options.disclose_locators.is_empty() {
        let mut disclosed = Vec::new();
        for locator in &options.disclose_locators {
            let (path, sha256) = locator
                .rsplit_once('=')
                .ok_or_else(|| error("--disclose-locator needs a relative PATH=SHA256"))?;
            crate::history_authority::relative_path(path)?;
            require(
                sha256.len() == 64 && sha256.bytes().all(|b| b.is_ascii_hexdigit()),
                "--disclose-locator needs a relative PATH=SHA256",
            )?;
            disclosed.push(obj([("path", s(path)), ("sha256", s(sha256))]));
        }
        map_mut(&mut a)?.insert("disclosed_locators".into(), V::List(disclosed));
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
    let mut source_body = None;
    if let Some(amend) = &options.amend {
        require(kind == "add", "an amendment rewrites an existing entry")?;
        let fields = map_mut(&mut a)?;
        fields.insert("amend".into(), s(amend.kind));
        if let Some(by) = &amend.by {
            fields.insert("answer_by".into(), s(by));
        }
        fields.insert("body".into(), amend.body.clone());
        return Ok((a, vec![], amend.order.clone()));
    }
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
            require(
                matches!(value, SourceValue::Map(_)),
                "the fields must be a mapping",
            )?;
            source_body = Some(value.clone());
            value.typed()
        } else if given.len() == 1 && !given[0].contains('=') {
            s(given[0])
        } else {
            let mut fields = Map::new();
            let mut source_fields: Vec<(String, SourceValue)> = Vec::new();
            for value in given {
                let (field, value) = value
                    .split_once('=')
                    .ok_or_else(|| error("fields are written field=value"))?;
                let value = if value.starts_with('[')
                    || value.starts_with('{') && !value.starts_with("{{")
                {
                    yaml(value)?
                } else {
                    SourceValue::Scalar(typed(value)?)
                };
                let field = field.trim();
                fields.insert(field.into(), value.typed());
                if let Some((_, current)) = source_fields.iter_mut().find(|(key, _)| key == field) {
                    *current = value;
                } else {
                    source_fields.push((field.into(), value));
                }
            }
            require(
                !fields.is_empty(),
                "add needs fields: add <id> v=... from=...",
            )?;
            source_body = Some(SourceValue::Map(source_fields));
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
    Ok((a, paths, source_body))
}
fn entry_is_legacy_or_pending(route: &WriteRoute) -> Result<bool> {
    Ok(route.pending_required()? || (route.paths()[0].exists()
        && crate::legacy_authoring::authority_route(&route.paths()[0])? == crate::legacy_authoring::AuthorityRoute::Legacy))
}
pub fn run(kind: &str, options: &Options, cwd: &Path) -> Result<String> {
    let cwd = cwd.canonicalize()?;
    require(options.actor.as_ref().is_none_or(|a| !a.trim().is_empty() && a.len() <= 200), "--by must be non-empty recorded actor text, at most 200 bytes")?;
    let (action, files, source_body) = action(kind, options)?;
    let implicit = files.is_empty();
    let original = if implicit {
        public_workspace::records(&cwd)?
    } else {
        files.iter().map(|p| cwd.join(p)).collect()
    };
    let route = WriteRoute::capture(&original, &cwd)?;
    require(route.paths().len() == 1, "choose one logical record entry")?;
    if options.actor.is_some() && entry_is_legacy_or_pending(&route)? {
        return Err(error("--by requires a direct active-history record; legacy records retain their existing review rule"));
    }
    if options.reframe || options.expected_record_sha256.is_some() {
        let entry = &route.paths()[0];
        require(entry.exists() && crate::legacy_authoring::authority_route(entry)?
            == crate::legacy_authoring::AuthorityRoute::History,
            "reframe and expected-record-sha256 require active core/v1 history")?;
        if let Some(expected) = &options.expected_record_sha256 {
            use sha2::Digest;
            let bytes = std::fs::read(entry)?;
            require(format!("{:x}", sha2::Sha256::digest(&bytes)) == expected.to_ascii_lowercase(),
                "record changed since the caller read it")?;
        }
    }
    let action =
        match contribution_routing::route(kind, options, action, source_body.as_ref(), route)? {
            contribution_routing::Outcome::Handled(value) => {
                return Ok(format!(
                    "{}\n",
                    crate::public_core_readers::json_value(&value)?
                ));
            }
            contribution_routing::Outcome::HandledWithNotice(value, notice) => {
                return Ok(format!(
                    "{notice}{}\n",
                    crate::public_core_readers::json_value(&value)?
                ));
            }
            contribution_routing::Outcome::Local(action) => action,
        };
    let route = WriteRoute::capture(&original, &cwd)?;
    require(route.paths().len() == 1, "choose one logical record entry")?;
    let entry = &route.paths()[0];
    let _lock =
        F::DirectoryGuard::acquire(entry.parent().ok_or_else(|| error("invalid_path"))?, true)?;
    if let Some(amend) = &options.amend {
        require(entry.exists(), "record not found")?;
        crate::public_amend::unchanged_or_refuse(
            route.paths(),
            &cwd,
            &options.subject,
            &amend.was,
            amend.kind,
        )?;
        if amend.kind == "correct" {
            crate::public_amend::unlanded_or_refuse(route.paths(), &cwd, &options.subject)?;
        }
    }
    if entry.exists()
        && crate::legacy_authoring::authority_route(entry)?
            == crate::legacy_authoring::AuthorityRoute::Legacy
    {
        require(!options.reframe && options.expected_record_sha256.is_none(),
            "reframe and expected-record-sha256 require active core/v1 history; migrate explicitly before reframing")?;
        return if route.pending_required()? {
            crate::legacy_authoring::write_advanced_local(&action, &route, source_body.as_ref())
        } else {
            crate::legacy_authoring::write(&action, &route, source_body.as_ref())
        };
    }
    if entry.exists() {
        drop(_lock);
        drop(route);
        let (result, notice) = crate::direct_history::write_as(&original, &cwd, &action, options.actor.as_deref())?;
        if string_is(&map(&result)?["state"], "private draft") {
            return Ok(format!(
                "{}\n",
                crate::public_core_readers::json_value(&result)?
            ));
        }
        return Ok(format!(
            "{notice}history committed: {} ({} {})\n",
            text(&map(&result)?["operation"])?,
            options.amend.as_ref().map_or(kind, |amend| amend.kind),
            options.subject
        ));
    }
    if kind != "add" || !implicit {
        let named = files.iter().filter_map(|p| p.to_str());
        return Err(crate::public_readers::no_record_here(named, &cwd, true));
    }
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
    let (result, notice) = crate::history_node_birth::create(
        &route, &original, &action, runtime.as_ref(),
        options.actor.as_deref().map(s).unwrap_or_else(crate::direct_history::actor),
    )?;
    if string_is(&map(&result)?["state"], "private draft") {
        return Ok(format!("{}\n", crate::public_core_readers::json_value(&result)?));
    }
    Ok(format!(
        "{notice}history committed: {} ({kind} {})\ncreated {} - this workspace's record, born with its first entry\n",
        text(&map(&result)?["operation"])?,
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
        let (parsed, files, _) = action(
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
