//! Explicit formula conversion and checked ordinary/core record migration.
//! Capture, conversion, byte edits and publication each retain their own guards.
#[path = "core_expression_conversion.rs"]
mod core_expression_conversion;
#[path = "core_expression_migration.rs"]
mod core_expression_migration;
#[path = "core_expression_yaml.rs"]
mod core_expression_yaml;
#[path = "ordinary_expression_migration.rs"]
mod ordinary_expression_migration;

use crate::{
    Error, Result, reasoning_language, require, source_capture::ReadMode, value::TypedValue,
};
use clap::Subcommand;
use serde_json::{Map, Value, json};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, clap::Args)]
pub struct Args {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Clone, Debug, Subcommand)]
pub enum Command {
    /// Convert one explicit legacy formula without writing a record.
    Convert {
        #[arg(allow_negative_numbers = true)]
        text: String,
        #[arg(long)]
        predicate: bool,
        #[arg(long)]
        readable: bool,
    },
    /// Preview or materialize a verified migration copy.
    Migrate {
        #[arg(long)]
        record: Option<PathBuf>,
        #[arg(long)]
        apply: bool,
        #[arg(long)]
        readable: bool,
        #[arg(long, value_parser = ["core/v1"])]
        profile: Option<String>,
        #[arg(long)]
        destination: Option<PathBuf>,
    },
}

fn refs(value: &Value, result: &mut BTreeSet<String>) {
    if let Some(id) = value.get("ref").and_then(Value::as_str) {
        result.insert(id.to_owned());
    }
    if let Some(args) = value.get("args").and_then(Value::as_array) {
        for item in args {
            refs(item, result);
        }
    }
}

fn display(value: &Value) -> String {
    if let Some(v) = value.get("expr").and_then(Value::as_str) {
        return v.to_owned();
    }
    if let Some(v) = value.get("ref").and_then(Value::as_str) {
        return v.to_owned();
    }
    if let Some(v) = value.get("num").and_then(Value::as_str) {
        return v.to_owned();
    }
    if let Some(v) = value.get("text").and_then(Value::as_str) {
        return serde_json::to_string(v).unwrap_or_else(|_| "\"\"".into());
    }
    if let Some(v) = value.get("bool").and_then(Value::as_bool) {
        return v.to_string();
    }
    let Some(op) = value.get("op").and_then(Value::as_str) else {
        return value.to_string();
    };
    let symbol = match op {
        "add" => "+",
        "sub" => "-",
        "mul" => "*",
        "div" => "/",
        "eq" => "==",
        "ne" => "!=",
        "lt" => "<",
        "le" => "<=",
        "gt" => ">",
        "ge" => ">=",
        _ => op,
    };
    let args = value.get("args").and_then(Value::as_array);
    match args {
        Some(a) if a.len() == 2 => format!("({} {} {})", display(&a[0]), symbol, display(&a[1])),
        Some(a) => format!(
            "{}({})",
            symbol,
            a.iter().map(display).collect::<Vec<_>>().join(", ")
        ),
        None => value.to_string(),
    }
}

fn explicit_syntax(source: &str) -> Result<()> {
    use rustpython_parser::{Parse, ast};
    require(
        source.chars().count() <= 4000,
        "legacy expression must be text within 4000 characters",
    )?;
    let ast = ast::Expr::parse(source, "<unknown>").map_err(|_| {
        Error(if source.starts_with([' ', '\t']) {
            "unexpected indent (<unknown>, line 1)".into()
        } else {
            "invalid syntax (<unknown>, line 1)".into()
        })
    })?;
    fn calls(node: &ast::Expr) -> bool {
        match node {
            ast::Expr::Call(_) => true,
            ast::Expr::BinOp(n) => calls(&n.left) || calls(&n.right),
            ast::Expr::UnaryOp(n) => calls(&n.operand),
            ast::Expr::Compare(n) => calls(&n.left) || n.comparators.iter().any(calls),
            _ => false,
        }
    }
    require(
        !calls(&ast),
        "unsupported legacy expression; it was not converted",
    )
}
fn authored_expression(source: &str, predicate: bool) -> Result<TypedValue> {
    explicit_syntax(source)?;
    let input = TypedValue::from_json(&json!({"expr":source}))?;
    let tree = reasoning_language::legacy_expression_detailed(&input, predicate)?;
    reasoning_language::convert_authored(source, predicate)?;
    Ok(tree)
}
fn convert(text: &str, predicate: bool, readable: bool) -> Result<Value> {
    explicit_syntax(text)?;
    let input = TypedValue::from_json(&json!({"expr": text}))?;
    let tree = reasoning_language::legacy_expression_detailed(&input, predicate)?.to_json()?;
    if readable {
        let source = display(&tree);
        let source = if source.starts_with('(') && source.ends_with(')') {
            source[1..source.len() - 1].to_owned()
        } else {
            source
        };
        let mut result = Map::new();
        result.insert("expr".into(), Value::String(source));
        let expression = Value::Object(result);
        let mut references = BTreeSet::new();
        refs(&tree, &mut references);
        return Ok(
            json!({"expression": expression, "display": display(&expression), "references": references}),
        );
    }
    let mut references = BTreeSet::new();
    refs(&tree, &mut references);
    Ok(json!({"expression": tree, "display": display(&tree), "references": references}))
}

fn expanded_path(cwd: &Path, path: &Path) -> Result<PathBuf> {
    if let Ok(relative) = path.strip_prefix("~") {
        Ok(PathBuf::from(
            std::env::var_os("HOME").ok_or_else(|| Error("home directory unavailable".into()))?,
        )
        .join(relative))
    } else {
        Ok(cwd.join(path))
    }
}
fn migrate(
    cwd: &Path,
    record: Option<&Path>,
    apply: bool,
    readable: bool,
    profile: Option<&str>,
    destination: Option<&Path>,
    _read_mode: Option<&str>,
) -> Result<Value> {
    let record = match record {
        Some(record) => expanded_path(cwd, record)?,
        None => crate::public_workspace::records(cwd)?
            .into_iter()
            .next()
            .ok_or_else(|| Error("record not found".into()))?,
    };
    if profile.is_none() {
        require(
            destination.is_none(),
            "copy migration needs an explicit --profile core/v1",
        )?;
        return ordinary_expression_migration::migrate(cwd, &record, apply, readable);
    }
    require(profile == Some("core/v1"), "unsupported migration profile")?;
    let runtime = crate::public_workspace::runtime()?;
    // The reference expression command supplies an explicit live capture even
    // when the outer CLI sets a frozen default. Ordinary migration reads frozen.
    let plan =
        core_expression_migration::Plan::prepare(&record, cwd, ReadMode::Live, runtime.as_ref())?;
    if apply && plan.problems.is_empty() {
        return if let Some(destination) = destination {
            plan.publish(&expanded_path(cwd, destination)?)
        } else {
            plan.apply()
        };
    }
    let mut result = plan.summary()?;
    if let Some(destination) = destination {
        result["destination"] = json!(crate::source_inventory::absolute(&expanded_path(
            cwd,
            destination
        )?)?);
    }
    Ok(result)
}

pub fn run(args: &Args, cwd: &Path) -> Result<String> {
    run_mode(args, cwd, ReadMode::Live)
}
fn run_mode(args: &Args, cwd: &Path, mode: ReadMode) -> Result<String> {
    let result = match &args.command {
        Command::Convert {
            text,
            predicate,
            readable,
        } => convert(text, *predicate, *readable)?,
        Command::Migrate {
            record,
            apply,
            readable,
            profile,
            destination,
        } => migrate(
            cwd,
            record.as_deref(),
            *apply,
            *readable,
            profile.as_deref(),
            destination.as_deref(),
            if mode == ReadMode::Frozen {
                Some("frozen")
            } else {
                None
            },
        )?,
    };
    Ok(json_output(&result) + "\n")
}

#[derive(Debug)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}
pub fn dispatch(args: &Args, cwd: &Path, mode: ReadMode) -> Output {
    match run_mode(args, cwd, mode) {
        Ok(stdout) => {
            let result: Value = serde_json::from_str(&stdout).unwrap();
            let code = if matches!(args.command, Command::Migrate { apply: true, .. })
                && result["problems"].as_array().is_some_and(|a| !a.is_empty())
            {
                1
            } else {
                0
            };
            Output {
                stdout,
                stderr: String::new(),
                code,
            }
        }
        Err(error) if error.0 == "ordinary_reader_requires_ordinary_profile" => Output {
            stdout: String::new(),
            stderr: "unsupported_capability: use core/v1 consumer\n".into(),
            code: 1,
        },
        Err(error) => Output {
            stdout: format!("{}\n", json_output(&json!({"error":error.to_string()}))),
            stderr: String::new(),
            code: 2,
        },
    }
}

// Public JSON retains the reference command's field order and separators. Receipt
// and snapshot bytes use their separate canonical encoders.
fn json_output(value: &Value) -> String {
    match value {
        Value::Array(a) => format!(
            "[{}]",
            a.iter().map(json_output).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(m) => {
            let order: &[&str] = if m.contains_key("expression") {
                &["expression", "display", "references"]
            } else if m.contains_key("before_sha256") {
                &[
                    "record",
                    "before_sha256",
                    "changes",
                    "skipped",
                    "applied",
                    "problems",
                    "fired",
                    "after_sha256",
                ]
            } else if m.contains_key("manifest") && m.contains_key("profile") {
                &[
                    "profile",
                    "state",
                    "record",
                    "complete",
                    "problems",
                    "changes",
                    "fired",
                    "reports",
                    "manifest",
                    "destination",
                    "receipt",
                    "read_mode",
                    "applied",
                    "backup",
                ]
            } else if m.contains_key("inventory_sha256") {
                &[
                    "version",
                    "transformation",
                    "record",
                    "state",
                    "publication_authority",
                    "source_snapshot_id",
                    "candidate_snapshot_id",
                    "observation_sha256",
                    "config_sha256",
                    "inventory_sha256",
                    "source",
                    "destination",
                    "reports",
                    "problems",
                    "complete",
                ]
            } else if m.contains_key("comparisons") {
                &[
                    "version",
                    "transformation",
                    "source_snapshot_id",
                    "candidate_snapshot_id",
                    "changes",
                    "blockers",
                    "problems",
                    "fired",
                    "preserved",
                    "comparisons",
                    "legacy_runtime",
                    "complete",
                    "origin",
                ]
            } else if m.contains_key("target") && m.contains_key("before") {
                &["collection", "id", "field", "target", "before", "after"]
            } else if m.contains_key("before") && m.contains_key("after") {
                &["id", "field", "status", "before", "after"]
            } else if m.contains_key("op") && m.contains_key("args") {
                &["op", "args"]
            } else if m.contains_key("code") && m.contains_key("detail") {
                &["code", "detail", "id", "field"]
            } else if m.contains_key("collection") && m.contains_key("reason") {
                &["collection", "id", "field", "reason", "value"]
            } else if m.contains_key("id") && m.contains_key("field") && m.contains_key("reason") {
                &["id", "field", "reason"]
            } else if m.contains_key("profile") && m.contains_key("status") {
                &["profile", "status", "identity", "reason"]
            } else if m.contains_key("origin") && m.contains_key("sha256") {
                &["origin", "path", "sha256"]
            } else if m.contains_key("status") && m.contains_key("value") {
                &["status", "value", "diagnostics"]
            } else if m.contains_key("type") {
                &[
                    "type",
                    "value",
                    "numerator",
                    "denominator",
                    "items",
                    "fields",
                ]
            } else {
                &[]
            };
            let mut keys = order
                .iter()
                .filter_map(|k| m.get_key_value(*k).map(|(k, _)| k))
                .collect::<Vec<_>>();
            keys.extend(m.keys().filter(|k| !order.contains(&k.as_str())));
            format!(
                "{{{}}}",
                keys.into_iter()
                    .map(|k| format!(
                        "{}: {}",
                        serde_json::to_string(k).unwrap(),
                        json_output(&m[k])
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        _ => value.to_string(),
    }
}

/// Adapt an expressions-only clap failure to the public argparse diagnostics.
/// The caller invokes this on `Args::try_parse` failure and otherwise uses the
/// original clap error. Global option values are skipped before command selection.
pub fn parse_failure(argv: &[std::ffi::OsString], error: &clap::Error) -> Option<Output> {
    if matches!(
        error.kind(),
        clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
    ) {
        return None;
    }
    let args = argv
        .iter()
        .map(|v| v.to_str())
        .collect::<Option<Vec<_>>>()?;
    let mut at = 1;
    while at < args.len() {
        match args[at] {
            "--workspace" => at += 2,
            "--json" | "--frozen" | "--no-cache" => at += 1,
            v if v.starts_with("--workspace=") => at += 1,
            _ => break,
        }
    }
    if args.get(at) != Some(&"expressions") {
        return None;
    }
    let tail = &args[at + 1..];
    let (action, tail) = tail
        .split_first()
        .map(|(a, t)| (*a, t))
        .unwrap_or(("", &[]));
    let root_usage = "usage: kpop expressions [-h] {convert,migrate} ...\n";
    let (program, usage, reason) = match action {
        "convert" => {
            let usage =
                "usage: kpop expressions convert [-h] [--predicate] [--readable] [--json] text\n";
            let mut positional = vec![];
            let mut unknown = vec![];
            let mut literal = false;
            let negative = regex::Regex::new(r"^-([0-9]+|[0-9]*\.[0-9]+)$").unwrap();
            for arg in tail {
                if literal {
                    positional.push(*arg);
                } else if *arg == "--" {
                    literal = true;
                } else if ["--predicate", "--readable", "--json"].contains(arg) {
                } else if arg.starts_with('-') && *arg != "-" && !negative.is_match(arg) {
                    unknown.push(*arg);
                } else {
                    positional.push(*arg);
                }
            }
            let reason = if positional.is_empty() {
                "the following arguments are required: text".into()
            } else {
                unknown.extend(positional.into_iter().skip(1));
                if unknown.is_empty() {
                    return None;
                }
                format!("unrecognized arguments: {}", unknown.join(" "))
            };
            ("kpop expressions convert", usage, reason)
        }
        "migrate" => {
            let usage = "usage: kpop expressions migrate [-h] [--record RECORD] [--apply] [--readable]\n                                [--profile {core/v1}]\n                                [--destination DESTINATION] [--json]\n";
            let mut unknown = vec![];
            let mut reason = None;
            let mut i = 0;
            while i < tail.len() {
                let arg = tail[i];
                if ["--apply", "--readable", "--json"].contains(&arg) {
                    i += 1;
                    continue;
                }
                let (key, inline) = arg
                    .split_once('=')
                    .map(|(k, v)| (k, Some(v)))
                    .unwrap_or((arg, None));
                if ["--record", "--destination", "--profile"].contains(&key) {
                    let value = if let Some(value) = inline {
                        value
                    } else if let Some(value) = tail.get(i + 1).filter(|v| !v.starts_with('-')) {
                        i += 1;
                        *value
                    } else {
                        reason = Some(format!("argument {key}: expected one argument"));
                        break;
                    };
                    if key == "--profile" && value != "core/v1" {
                        reason = Some(format!(
                            "argument --profile: invalid choice: {} (choose from 'core/v1')",
                            crate::source_text::ordinary_python_repr(&TypedValue::Text(
                                value.into()
                            ))
                        ));
                        break;
                    }
                } else {
                    unknown.push(arg);
                }
                i += 1;
            }
            let reason = reason.or_else(|| {
                (!unknown.is_empty())
                    .then(|| format!("unrecognized arguments: {}", unknown.join(" ")))
            })?;
            ("kpop expressions migrate", usage, reason)
        }
        "" => (
            "kpop expressions",
            root_usage,
            "the following arguments are required: action".into(),
        ),
        other => (
            "kpop expressions",
            root_usage,
            format!(
                "argument action: invalid choice: '{other}' (choose from 'convert', 'migrate')"
            ),
        ),
    };
    Some(Output {
        stdout: String::new(),
        stderr: format!("{usage}{program}: error: {reason}\n"),
        code: 2,
    })
}
