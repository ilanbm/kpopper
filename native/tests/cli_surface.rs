//! The command surface `kpop --help` exposes, rendered as one line per command
//! path with its long options. Skills and docs are checked against the committed
//! copy, so a change to commands or flags shows up here first.
//!
//! Regenerate with:
//!   cd native && KPOP_UPDATE_CLI_SURFACE=1 cargo test --locked --test cli_surface

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::Command;

const UPDATE: &str = "KPOP_UPDATE_CLI_SURFACE";
const REGENERATE: &str =
    "cd native && KPOP_UPDATE_CLI_SURFACE=1 cargo test --locked --test cli_surface";
/// Hidden command trees that skills cite.
const HIDDEN: &[&str] = &["_agent"];

#[derive(Default)]
struct Help {
    commands: Vec<String>,
    operations: Vec<String>,
    /// Long option name, with a trailing `=` when it takes a value.
    options: BTreeSet<String>,
}

fn help(path: &[String]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .args(path)
        .arg("--help")
        .env("NO_COLOR", "1")
        .env_remove("CLICOLOR_FORCE")
        .output()
        .expect("run kpop --help");
    assert!(
        output.status.success(),
        "kpop {} --help failed: {}",
        path.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("help is UTF-8")
}

fn option(line: &str) -> Option<String> {
    // `  -h, --help`, `      --json`, `      --tokens <TOKENS>`, `      --out=<OUT>`
    let rest = line.strip_prefix("  ")?;
    let rest = match rest.strip_prefix("    ") {
        Some(rest) => rest,
        None => {
            let mut chars = rest.chars();
            if chars.next() != Some('-') {
                return None;
            }
            chars.next()?;
            chars.as_str().strip_prefix(", ")?
        }
    };
    let rest = rest.strip_prefix("--")?;
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    let tail = &rest[end..];
    let takes_value = tail.starts_with(" <")
        || tail.starts_with("=<")
        || tail.starts_with(" [<")
        || tail.starts_with("[=");
    Some(format!(
        "--{}{}",
        &rest[..end],
        if takes_value { "=" } else { "" }
    ))
}

fn parse(text: &str) -> Help {
    let mut parsed = Help::default();
    let mut section = "";
    for line in text.lines() {
        if !line.starts_with(' ') && line.ends_with(':') {
            section = line;
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        // The experimental listing is written by hand rather than by clap.
        if let Some(rest) = line.strip_prefix("  kpop experimental ") {
            if let Some(name) = rest
                .split_whitespace()
                .next()
                .filter(|name| !name.starts_with(['<', '-']))
            {
                parsed.commands.push(name.to_string());
            }
            continue;
        }
        match section {
            "Commands:" => {
                if let Some(name) = line
                    .strip_prefix("  ")
                    .and_then(|rest| rest.split_whitespace().next())
                    && !line.starts_with("   ")
                    && name != "help"
                {
                    parsed.commands.push(name.to_string());
                }
            }
            "Arguments:" => {
                if line.trim_start().starts_with("<OPERATION>")
                    && let Some(values) = line.split("[possible values: ").nth(1)
                {
                    let values = values.split(']').next().unwrap_or_default();
                    parsed
                        .operations
                        .extend(values.split(", ").map(str::to_string));
                }
            }
            // `Options:` and any custom heading such as `Output options:`.
            _ if section.to_ascii_lowercase().ends_with("options:") => {
                if let Some(option) = option(line) {
                    parsed.options.insert(option);
                }
            }
            _ => {}
        }
    }
    parsed
}

fn walk(path: Vec<String>, surface: &mut BTreeMap<Vec<String>, BTreeSet<String>>) {
    let mut parsed = parse(&help(&path));
    if path.is_empty() {
        parsed
            .commands
            .extend(HIDDEN.iter().map(|name| name.to_string()));
    }
    for operation in &parsed.operations {
        let mut child = path.clone();
        child.push(operation.clone());
        surface.entry(child).or_default();
    }
    for command in parsed.commands.iter().collect::<BTreeSet<_>>() {
        let mut child = path.clone();
        child.push(command.clone());
        if !surface.contains_key(&child) {
            walk(child, surface);
        }
    }
    surface.insert(path, parsed.options);
}

fn render() -> String {
    let mut surface = BTreeMap::new();
    walk(Vec::new(), &mut surface);
    let globals = surface[&Vec::new()].clone();
    let mut out = String::from(
        "# Generated from kpop --help by native/tests/cli_surface.rs; do not edit.\n\
         # One line per command path, then its long options; `=` marks an option that takes a value.\n\
         # The first line holds the global options, which every other line leaves out.\n",
    );
    for (path, options) in &surface {
        let mut line = std::iter::once("kpop".to_string())
            .chain(path.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ");
        for option in options
            .iter()
            .filter(|option| path.is_empty() || !globals.contains(*option))
        {
            line.push(' ');
            line.push_str(option);
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cli-surface.txt")
}

fn diff(expected: &str, actual: &str) -> String {
    let expected: BTreeSet<_> = expected.lines().collect();
    let actual: BTreeSet<_> = actual.lines().collect();
    let mut out = String::new();
    for line in expected.difference(&actual) {
        out.push_str(&format!("- {line}\n"));
    }
    for line in actual.difference(&expected) {
        out.push_str(&format!("+ {line}\n"));
    }
    out
}

#[test]
fn committed_cli_surface_matches_help() {
    let actual = render();
    let path = fixture();
    if std::env::var_os(UPDATE).is_some_and(|value| value == "1") {
        std::fs::write(&path, &actual).expect("write cli surface");
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        expected == actual,
        "native/tests/fixtures/cli-surface.txt differs from kpop --help \
         (- committed, + current):\n{}\nRegenerate it with:\n  {REGENERATE}",
        diff(&expected, &actual),
    );
}

#[test]
fn surface_parses_both_help_layouts() {
    let short = "Usage: kpop x [OPTIONS]\n\nOptions:\n      --out <OUT>  Where\n  -q, --quiet      Less\n  -h, --help       Print help\n";
    let long = "Options:\n      --tokens <TOKENS>\n          \n      --rebuild\n          Rebuild the cache\n          --not-an-option is prose\n  -h, --help\n          Print help\n";
    assert_eq!(
        parse(short).options.into_iter().collect::<Vec<_>>(),
        ["--help", "--out=", "--quiet"]
    );
    assert_eq!(
        parse(long).options.into_iter().collect::<Vec<_>>(),
        ["--help", "--rebuild", "--tokens="]
    );
    let headed = "Output options:\n      --format <FORMAT>  How\n";
    assert_eq!(
        parse(headed).options.into_iter().collect::<Vec<_>>(),
        ["--format="]
    );
    let commands = "Commands:\n  pull    Read entries\n  help    Print this message\n\nArguments:\n  <OPERATION>  [possible values: open, view]\n";
    let parsed = parse(commands);
    assert_eq!(parsed.commands, ["pull"]);
    assert_eq!(parsed.operations, ["open", "view"]);
    let listing = "Experimental applications:\n  kpop experimental hub          Render.\n  kpop experimental annotated-doc Author.\n";
    assert_eq!(parse(listing).commands, ["hub", "annotated-doc"]);
}
