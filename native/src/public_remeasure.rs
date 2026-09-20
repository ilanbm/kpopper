//! Read-only ordinary `remeasure` plan and bounded local recipe execution.
use crate::{Error, Result, history_contract::*, history_view::list, history_yaml, public_workspace, require, source_capture::{self, ReadMode}, value::TypedValue as V};
use std::{collections::BTreeMap, path::{Path, PathBuf}, process::{Command, Stdio}, time::{Duration, SystemTime, UNIX_EPOCH}};

#[derive(Clone, Debug, clap::Args)]
pub struct Options {
    #[arg(long)] pub run: bool,
    pub record: Option<PathBuf>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Output {
    pub text: String,
    pub stderr: String,
    pub code: i32,
}

fn output(lines: Vec<String>, code: i32) -> Output {
    Output { text: lines.join("\n") + "\n", stderr: String::new(), code }
}

fn recipe_path(record: &Path) -> PathBuf { record.parent().unwrap_or(Path::new(".")).join(".kpopper/measure.yaml") }
fn scalar(v: &V) -> String { crate::source_text::ordinary_python_str(v) }
fn resolve(exe: &str, root: &Path) -> Option<PathBuf> {
    let path = Path::new(exe);
    if path.components().count() > 1 {
        let candidate = if path.is_absolute() { path.to_path_buf() } else { root.join(path) };
        return candidate.is_file().then_some(candidate);
    }
    env_path().into_iter().map(|dir| dir.join(exe)).find(|candidate| candidate.is_file())
}
fn env_path() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .collect()
}
fn allowlist(path: &Path) -> Result<BTreeMap<String, Vec<String>>> {
    if !path.is_file() { return Ok(BTreeMap::new()); }
    let value = history_yaml::decode_document(&std::fs::read(path)?)?;
    let map = map(&value)?;
    let mut out = BTreeMap::new();
    for (name, argv) in map {
        require(regex::Regex::new(r"^[A-Za-z][A-Za-z0-9_-]*$").unwrap().is_match(name), &format!("refused - .kpopper/measure.yaml:\n  '{name}' is not a recipe name - letters, digits, underscores and dashes, opening with a letter; quote it if the loader read it as something else"))?;
        let values = list(argv)?.iter().map(|v| text(v).map(str::to_owned)).collect::<Result<Vec<_>>>()?;
        require(!values.is_empty() && values.iter().all(|v| !v.is_empty() && !v.contains('\0')), &format!("refused - {name}: a recipe is a non-empty list of non-empty strings - the executable and its arguments - never a line for a shell"))?;
        out.insert(name.clone(), values);
    }
    Ok(out)
}
fn run_recipe(argv: &[String], root: &Path) -> Result<(String, String)> {
    let exe = resolve(&argv[0], root).ok_or_else(|| Error(format!("no executable {:?} on the path{}", argv[0], if argv[0].contains('/') { format!(" or under {}", root.display()) } else { String::new() })))?;
    let mut command = Command::new(exe);
    command.args(&argv[1..]).current_dir(root).stdin(Stdio::null());
    let output = crate::reasoning_runtime::run_command_capture(&mut command, vec![], Duration::from_secs(60), 64 * 1024)
        .map_err(|e| Error(match e.0.as_str() {
            "runtime_timeout" => "did not finish in 60 s and was stopped".into(),
            "output_limit" => "printed more than 64 KiB and was stopped".into(),
            other => other.to_owned(),
        }))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !output.status.success() {
        let tail = stderr.split_whitespace().collect::<Vec<_>>().join(" ");
        return Err(Error(format!("exited {}{}", output.status.code().map_or("by signal".into(), |n| n.to_string()), if tail.is_empty() { String::new() } else { format!(" - stderr: {}", tail.chars().rev().take(200).collect::<String>().chars().rev().collect::<String>()) })));
    }
    Ok((stdout, stderr))
}
fn utc_day() -> String {
    // Gregorian conversion without a dependency; the CLI only needs UTC's calendar date.
    let days = (SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() / 86400) as i64;
    let z = days + 719468; let era = if z >= 0 { z } else { z - 146096 } / 146097; let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); let mp = (5 * doy + 2) / 153; let d = doy - (153 * mp + 2) / 5 + 1; let m = mp + if mp < 10 { 3 } else { -9 };
    format!("{:04}-{:02}-{:02}", y + if m <= 2 { 1 } else { 0 }, m, d)
}
fn parse_reading(out: &str, recorded: &V) -> Result<V> {
    let lines = out.lines().map(str::trim).filter(|s| !s.is_empty()).collect::<Vec<_>>();
    require(!lines.is_empty(), "printed nothing - the value is the one line a recipe prints")?;
    require(lines.len() == 1, &format!("printed {} lines - the value is the one line a recipe prints; diagnostics go to stderr", lines.len()))?;
    let line = lines[0];
    match recorded {
        V::Bool(_) => match line { "true" => Ok(V::Bool(true)), "false" => Ok(V::Bool(false)), _ => Err(Error(format!("printed {line:?} where the record holds true or false"))) },
        V::Integer(_) => line.parse::<i64>().map(|n| V::Integer(crate::value::Integer::new(&n.to_string()).unwrap())).map_err(|_| Error(format!("printed '{}' where the record holds a number", line.replace('\'', "\\'")))),
        V::Float(_) => line.parse::<f64>().map_err(|_| Error(format!("printed '{}' where the record holds a number", line.replace('\'', "\\'")))).and_then(|n| crate::value::FiniteFloat::new(n).map(V::Float)),
        _ => Ok(V::Text(line.into())),
    }
}
fn agrees(a: &V, b: &V) -> bool {
    match (a,b) { (V::Integer(x), V::Integer(y)) => x.as_str() == y.as_str(), (V::Float(x), V::Float(y)) => x.get() == y.get(), (V::Integer(x), V::Float(y)) => x.as_str().parse::<f64>().ok() == Some(y.get()), (V::Float(x), V::Integer(y)) => Some(x.get()) == y.as_str().parse::<f64>().ok(), _ => a == b }
}
pub fn run(options: &Options, cwd: &Path, frozen: bool) -> Result<Output> {
    let record = options.record.clone().unwrap_or_else(|| cwd.join("GROUNDING.yaml"));
    let runtime = public_workspace::runtime_for_paths(std::slice::from_ref(&record), cwd, None)?;
    let capture = source_capture::capture_source_with_runtime(std::slice::from_ref(&record), cwd, if frozen { ReadMode::Frozen } else { ReadMode::Live }, None, runtime.as_ref())?;
    let doc = capture.ordinary_document();
    let collections = crate::reasoning_fields::collections(doc)?;
    let mut named = BTreeMap::<String, Vec<String>>::new();
    for (_section, members) in &collections {
        for (id, body) in members {
            if let Ok(body) = map(&body) {
                if let Some(name) = body.get("measure").and_then(|v| text(v).ok()) { named.entry(name.to_owned()).or_default().push(id.clone()); }
            }
        }
    }
    let recipes = match allowlist(&recipe_path(&record)) {
        Ok(recipes) => recipes,
        Err(error) => return Ok(Output { text: String::new(), stderr: format!("{error}\n"), code: 1 }),
    };
    if named.is_empty() {
        let lines = if recipes.is_empty() {
            vec!["no measures beside the record - nothing to re-measure".into()]
        } else {
            vec![format!(".kpopper/measure.yaml holds {} recipe{}, and no entry names one - nothing to re-measure", recipes.len(), if recipes.len() == 1 { "" } else { "s" })]
        };
        return Ok(output(lines, 0));
    }
    let root = record.parent().unwrap_or(cwd);
    let cited = named.keys().cloned().collect::<Vec<_>>();
    for name in &cited { require(recipes.contains_key(name), &format!("refused - the record names recipe .kpopper/measure.yaml does not hold: {name} - a measurement nothing takes is a hole, and a falsifier reading it tests nothing"))?; }
    let entries = named.values().map(Vec::len).sum::<usize>();
    let mut out = vec![format!("{} recipe{} named by {} entr{}, from .kpopper/measure.yaml, run from {}:", cited.len(), if cited.len()==1{""}else{"s"}, entries, if entries==1{"y"}else{"ies"}, root.display())];
    for name in &cited { let ids = &named[name]; let argv = &recipes[name]; let exe = resolve(&argv[0], root).map(|p| p.display().to_string()).unwrap_or_else(|| format!("{} (not found)", argv[0])); let args = argv[1..].iter().map(|v| shell_quote(v)).collect::<Vec<_>>().join(" "); out.push(format!("  {} <- {}: {} {}", ids.join(", "), name, exe, args)); }
    for name in recipes.keys().filter(|name| !named.contains_key(name.as_str())) { out.push(format!("  named by no entry, never run: {name}")); }
    if !options.run { out.extend(["".into(), "nothing ran - add --run to measure this tree".into()]); return Ok(output(out, 0)); }
    out.push("".into());
    out.push(format!("measured on {} (UTC): {} entr{} by {} recipe{}", utc_day(), entries, if entries==1{"y"}else{"ies"}, cited.len(), if cited.len()==1{""}else{"s"}));
    let mut changed = false;
    let mut failed = 0usize;
    for (name, ids) in named {
        let (text, _) = match run_recipe(recipes.get(&name).unwrap(), root) { Ok(v) => v, Err(e) => { failed += 1; out.push(format!("  FAIL {name} ({}): {}", ids.join(", "), e)); continue; } };
        for id in ids { let body = collections.values().find_map(|m|m.get(&id)).unwrap(); let body=map(body)?; let field=body.get("v").or_else(||body.get("quoted")).ok_or_else(||Error(format!("{id} has no stored reading")))?; match parse_reading(&text, field) { Ok(measured) if agrees(field, &measured) => out.push(format!("  {id}: {} - as recorded ({name})", scalar(field))), Ok(measured) => { changed=true; out.push(format!("  {id}: {} -> {} measured by {name}", scalar(field), scalar(&measured))); }, Err(e) => { failed += 1; out.push(format!("  FAIL {name} ({id}): {}", e)); } } }
    }
    out.push("".into());
    let code = if failed != 0 || changed { 1 } else { 0 };
    out.push(if failed != 0 {
        format!("not clean: a hole - {failed} recipe{} failed, and a measurement nothing takes is a hole", if failed == 1 { "" } else { "s" })
    } else if changed {
        "the tree reads entries differently, none across a line - refresh them".into()
    } else {
        "the record holds what this tree measures".into()
    });
    Ok(output(out, code))
}
fn shell_quote(value: &str) -> String {
    if value.chars().all(|c| c.is_ascii_alphanumeric() || "._/-".contains(c)) { value.into() } else { format!("'{}'", value.replace('\'', "'\\''")) }
}
