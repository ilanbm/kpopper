//! Read-only ordinary `remeasure` plan and bounded local recipe execution.
use crate::{Error, Result, history_contract::{map as hmap, text as htext}, history_view::list as hlist, history_yaml, ordinary_value::{Map, Value as V, map, map_mut, text}, public_workspace, require, source_capture::{self, ReadMode}};
use std::{collections::BTreeMap, path::{Path, PathBuf}, process::{Command, Stdio}, time::Duration};

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
fn scalar(v: &V) -> String { v.python_str() }
fn executable(path: &Path) -> bool {
    if !path.is_file() { return false; }
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        return path.metadata().is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0);
    }
    #[cfg(not(unix))]
    true
}
fn resolve(exe: &str, root: &Path) -> Option<PathBuf> {
    let path = Path::new(exe);
    if path.components().count() > 1 {
        let candidate = if path.is_absolute() { path.to_path_buf() } else { root.join(path) };
        return executable(&candidate).then_some(candidate);
    }
    env_path().into_iter().map(|dir| dir.join(exe)).find(|candidate| executable(candidate))
}
fn env_path() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .collect()
}
fn checkout_root(record: &Path) -> PathBuf {
    let parent = record.parent().unwrap_or(Path::new("."));
    let mut command = Command::new("git");
    command.args(["-C", parent.to_str().unwrap_or("."), "rev-parse", "--show-toplevel"]);
    crate::reasoning_runtime::run_command_capture(&mut command, vec![], Duration::from_secs(5), 16 * 1024)
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|output| output.trim().to_owned())
        .filter(|output| !output.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| parent.to_path_buf())
}
fn allowlist(path: &Path) -> Result<BTreeMap<String, Vec<String>>> {
    if !path.is_file() { return Ok(BTreeMap::new()); }
    let value = history_yaml::decode_document(&std::fs::read(path)?)?;
    let map = hmap(&value)?;
    let mut out = BTreeMap::new();
    for (name, argv) in map {
        require(regex::Regex::new(r"^[A-Za-z][A-Za-z0-9_-]*$").unwrap().is_match(name), &format!("refused - .kpopper/measure.yaml:\n  '{name}' is not a recipe name - letters, digits, underscores and dashes, opening with a letter; quote it if the loader read it as something else"))?;
        let values = hlist(argv)?.iter().map(|v| htext(v).map(str::to_owned)).collect::<Result<Vec<_>>>()?;
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
    chrono::Utc::now().date_naive().to_string()
}
fn parse_reading(out: &str, recorded: &V) -> Result<V> {
    let lines = out.lines().map(str::trim).filter(|s| !s.is_empty()).collect::<Vec<_>>();
    require(!lines.is_empty(), "printed nothing - the value is the one line a recipe prints")?;
    require(lines.len() == 1, &format!("printed {} lines - the value is the one line a recipe prints; diagnostics go to stderr", lines.len()))?;
    let line = lines[0];
    match recorded {
        V::Bool(_) => match line { "true" => Ok(V::Bool(true)), "false" => Ok(V::Bool(false)), _ => Err(Error(format!("printed {line:?} where the record holds true or false"))) },
        V::Integer(_) => {
            let number = regex::Regex::new(r"^-?\d+(?:\.\d+)?$").unwrap();
            if !number.is_match(line) {
                Err(Error(format!("printed '{}' where the record holds a number", line.replace('\'', "\\'"))))
            } else if line.contains('.') {
                line.parse::<f64>().map_err(|_| Error("nonfinite_float".into())).and_then(|n| crate::value::FiniteFloat::new(n).map(V::Float))
            } else {
                crate::value::Integer::new(line).map(V::Integer)
            }
        }
        V::Float(_) | V::NonFinite(_) => {
            let number = regex::Regex::new(r"^-?\d+(?:\.\d+)?$").unwrap();
            if !number.is_match(line) {
                Err(Error(format!("printed '{}' where the record holds a number", line.replace('\'', "\\'"))))
            } else if line.contains('.') {
                line.parse::<f64>().map_err(|_| Error("nonfinite_float".into())).and_then(|n| crate::value::FiniteFloat::new(n).map(V::Float))
            } else {
                crate::value::Integer::new(line).map(V::Integer)
            }
        },
        _ => Ok(V::Text(line.into())),
    }
}
fn agrees(a: &V, b: &V) -> bool {
    match (a, b) {
        (V::Bool(x), V::Bool(y)) => x == y,
        (V::Integer(_) | V::Float(_) | V::NonFinite(_), V::Integer(_) | V::Float(_) | V::NonFinite(_)) => crate::ordinary_value::python_equal(a, b),
        _ => a.python_str() == b.python_str(),
    }
}

fn measurement_declarations(
    base: &V,
    hypotheses: &V,
) -> Result<(BTreeMap<String, Vec<String>>, V, Vec<String>)> {
    let mut declarations = BTreeMap::<String, BTreeMap<String, Vec<Option<String>>>>::new();
    let mut base_measures = BTreeMap::<String, String>::new();
    let mut current = base.clone();
    for (_section, members) in crate::ordinary_fields::collections(base)? {
        for (id, body) in members {
            if let Ok(body) = map(&body)
                && let Some(name) = body.get("measure").and_then(|value| text(value).ok())
            {
                declarations.entry(id.clone()).or_default().entry(name.into()).or_default().push(None);
                base_measures.insert(id, name.into());
            }
        }
    }
    let mut problems = Vec::new();
    for (hypothesis_name, hypothesis) in map(hypotheses)? {
        let hypothesis = map(hypothesis)?;
        if hypothesis.get("error").is_some_and(|value| *value != V::Null) {
            continue;
        }
        let document = hypothesis.get("doc").or_else(|| hypothesis.get("document")).ok_or_else(|| Error("invalid_snapshot".into()))?;
        for (section, members) in crate::ordinary_fields::collections(document)? {
            let target = map(&current)?.get(&section).cloned().unwrap_or_else(|| V::Map(Map::new()));
            let mut target = map(&target)?.clone();
            for (id, body) in members {
                if let Ok(body_map) = map(&body) {
                    if let Some(name) = body_map.get("measure").and_then(|value| text(value).ok()) {
                        declarations.entry(id.clone()).or_default().entry(name.into()).or_default().push(Some(hypothesis_name.clone()));
                    } else if let Some(name) = base_measures.get(&id) {
                        problems.push(format!("{id}: {hypothesis_name} replaces it without measure: {name} - the fold carries the block over whole, so the recipe would be dropped and nothing would take this reading again; carry measure: {name} into {hypothesis_name}, or drop it from the base first"));
                    }
                }
                target.insert(id, body);
            }
            map_mut(&mut current)?.insert(section, V::Map(target));
        }
    }
    for (id, by_recipe) in &declarations {
        if by_recipe.len() > 1 {
            let said = by_recipe.iter().map(|(recipe, holders)| {
                let holders = holders.iter().map(|holder| holder.as_deref().unwrap_or("the base")).collect::<Vec<_>>().join(", ");
                format!("{recipe} by {holders}")
            }).collect::<Vec<_>>().join("; ");
            problems.push(format!("{id}: two recipes named for one entry - {said} - a contested recipe; one of them, or neither"));
        }
    }
    let mut named = BTreeMap::<String, Vec<String>>::new();
    for (id, by_recipe) in declarations {
        for recipe in by_recipe.into_keys() {
            named.entry(recipe).or_default().push(id.clone());
        }
    }
    Ok((named, current, problems))
}

pub fn run(options: &Options, cwd: &Path, frozen: bool) -> Result<Output> {
    let record = options.record.clone().unwrap_or_else(|| cwd.join("GROUNDING.yaml"));
    let record = if record.is_absolute() { record } else { cwd.join(record) }.canonicalize()?;
    let runtime = public_workspace::runtime_for_paths(std::slice::from_ref(&record), cwd, None)?;
    let capture = source_capture::capture_ordinary_source_with_runtime(std::slice::from_ref(&record), cwd, if frozen { ReadMode::Frozen } else { ReadMode::Live }, None, runtime.as_ref())?;
    let doc = capture.ordinary_document();
    let (named, current, problems) = measurement_declarations(doc, capture.hypotheses())?;
    if !problems.is_empty() {
        let mut lines = vec!["refused - the record names recipes it cannot: ".into()];
        lines.extend(problems.into_iter().map(|problem| format!("  {problem}")));
        return Ok(output(lines, 1));
    }
    let collections = crate::ordinary_fields::collections(&current)?;
    let allowlist_path = recipe_path(&record);
    let allowlist_exists = allowlist_path.is_file();
    let recipes = match allowlist(&allowlist_path) {
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
    let root = checkout_root(&record);
    let cited = named.keys().cloned().collect::<Vec<_>>();
    if !allowlist_exists {
        return Ok(output(vec![format!("refused - {} recipe{} named and no .kpopper/measure.yaml beside the record to hold {}: {}", cited.len(), if cited.len() == 1 { "" } else { "s" }, if cited.len() == 1 { "it" } else { "them" }, cited.join(", "))], 1));
    }
    let missing = cited.iter().filter(|name| !recipes.contains_key(name.as_str())).cloned().collect::<Vec<_>>();
    if !missing.is_empty() {
        return Ok(output(vec![format!("refused - the record names recipe{} .kpopper/measure.yaml does not hold: {} - a measurement nothing takes is a hole, and a falsifier reading it tests nothing", if missing.len() == 1 { "" } else { "s" }, missing.join(", "))], 1));
    }
    let entries = named.values().map(Vec::len).sum::<usize>();
    let mut out = vec![format!("{} recipe{} named by {} entr{}, from .kpopper/measure.yaml, run from {}:", cited.len(), if cited.len()==1{""}else{"s"}, entries, if entries==1{"y"}else{"ies"}, root.display())];
    for name in &cited { let ids = &named[name]; let argv = &recipes[name]; let exe = resolve(&argv[0], &root).map(|p| p.display().to_string()).unwrap_or_else(|| format!("{} (not found)", argv[0])); let args = argv[1..].iter().map(|v| shell_quote(v)).collect::<Vec<_>>().join(" "); out.push(format!("  {} <- {}: {} {}", ids.join(", "), name, exe, args)); }
    for name in recipes.keys().filter(|name| !named.contains_key(name.as_str())) { out.push(format!("  named by no entry, never run: {name}")); }
    if !options.run { out.extend(["".into(), "nothing ran - add --run to measure this tree".into()]); return Ok(output(out, 0)); }
    out.push("".into());
    out.push(format!("measured on {} (UTC): {} entr{} by {} recipe{}", utc_day(), entries, if entries==1{"y"}else{"ies"}, cited.len(), if cited.len()==1{""}else{"s"}));
    let mut changed = false;
    let mut failed = 0usize;
    for (name, ids) in named {
        let (text, _) = match run_recipe(recipes.get(&name).unwrap(), &root) { Ok(v) => v, Err(e) => { failed += 1; out.push(format!("  FAIL {name} ({}): {}", ids.join(", "), e)); continue; } };
        for id in ids { let body = collections.values().find_map(|m|m.get(&id)).unwrap(); let body=map(body)?; let field=body.get("v").or_else(||body.get("quoted")).ok_or_else(||Error(format!("{id} has no stored reading")))?; match parse_reading(&text, field) { Ok(measured) if agrees(field, &measured) => out.push(format!("  {id}: {} - as recorded ({name})", scalar(&measured))), Ok(measured) => { changed=true; out.push(format!("  {id}: {} -> {} measured by {name}", scalar(field), scalar(&measured))); }, Err(e) => { failed += 1; out.push(format!("  FAIL {name} ({id}): {}", e)); } } }
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
