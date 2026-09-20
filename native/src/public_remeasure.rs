//! Read-only ordinary `remeasure` plan and bounded local recipe execution.
use crate::{Error, Result, history_contract::*, history_view::list, history_yaml, public_workspace, require, source_capture::{self, ReadMode}, value::TypedValue as V};
use std::{collections::BTreeMap, path::{Path, PathBuf}, process::{Command, Stdio}, time::Duration};

#[derive(Clone, Debug, clap::Args)]
pub struct Options {
    #[arg(long)] pub run: bool,
    pub record: Option<PathBuf>,
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
        require(regex::Regex::new(r"^[A-Za-z][A-Za-z0-9_-]*$").unwrap().is_match(name), &format!("refused - {} is not a recipe name", name))?;
        let values = list(argv)?.iter().map(|v| text(v).map(str::to_owned)).collect::<Result<Vec<_>>>()?;
        require(!values.is_empty() && values.iter().all(|v| !v.is_empty() && !v.contains('\0')), "refused - recipe arguments must be non-empty strings")?;
        out.insert(name.clone(), values);
    }
    Ok(out)
}
fn run_recipe(argv: &[String], root: &Path) -> Result<String> {
    let exe = resolve(&argv[0], root).ok_or_else(|| Error(format!("no executable {:?} on the path or under {}", argv[0], root.display())))?;
    let mut command = Command::new(exe);
    command.args(&argv[1..]).current_dir(root).stdin(Stdio::null());
    let (_, out) = crate::reasoning_runtime::run_command_bounded_with_status(&mut command, vec![], Duration::from_secs(60), 64 * 1024)?;
    String::from_utf8(out).map_err(|e| Error(e.to_string()))
}
pub fn run(options: &Options, cwd: &Path, frozen: bool) -> Result<String> {
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
    let recipes = allowlist(&recipe_path(&record))?;
    if named.is_empty() { return Ok("no measures beside the record - nothing to re-measure\n".into()); }
    let root = record.parent().unwrap_or(cwd);
    let mut out = vec![format!("{} recipe{} named by {} entr{}, from .kpopper/measure.yaml, run from {}:", named.len(), if named.len()==1{""}else{"s"}, named.values().map(Vec::len).sum::<usize>(), if named.values().map(Vec::len).sum::<usize>()==1{"y"}else{"ies"}, root.display())];
    for (name, ids) in &named { let argv = recipes.get(name).ok_or_else(|| Error(format!("refused - recipe {name} is not held by .kpopper/measure.yaml")))?; let exe = resolve(&argv[0], root).map(|p| p.display().to_string()).unwrap_or_else(|| format!("{} (not found)", argv[0])); let args = argv[1..].iter().map(|v| shell_quote(v)).collect::<Vec<_>>().join(" "); out.push(format!("  {} <- {}: {} {}", ids.join(", "), name, exe, args)); }
    if !options.run { out.extend(["".into(), "nothing ran - add --run to measure this tree".into()]); return Ok(out.join("\n")+"\n"); }
    out.push("".into());
    for (name, ids) in named {
        let text = run_recipe(recipes.get(&name).unwrap(), root)?;
        let value = text.lines().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>();
        require(value.len()==1, &format!("recipe {name} must print one non-empty line"))?;
        for id in ids { let body = collections.values().find_map(|m|m.get(&id)).unwrap(); let body=map(body)?; let field=body.get("v").or_else(||body.get("quoted")).ok_or_else(||Error(format!("{id} has no stored reading")))?; let measured=V::Text(value[0].trim().into()); if scalar(field)==scalar(&measured) { out.push(format!("  {id}: {} - as recorded ({name})", scalar(field))); } else { out.push(format!("  {id}: {} -> {} measured by {name}", scalar(field), value[0].trim())); } }
    }
    out.push("".into()); out.push("the record holds what this tree measures".into()); Ok(out.join("\n")+"\n")
}
fn shell_quote(value: &str) -> String {
    if value.chars().all(|c| c.is_ascii_alphanumeric() || "._/-".contains(c)) { value.into() } else { format!("'{}'", value.replace('\'', "'\\''")) }
}
