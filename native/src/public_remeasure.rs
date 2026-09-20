//! Read-only ordinary `remeasure` plan and bounded local recipe execution.
use crate::{
    Error, Result,
    history_contract::{map as hmap, text as htext},
    history_view::list as hlist,
    history_yaml,
    ordinary_value::{Map, Value as V, map, map_mut, text},
    public_consolidation::{
        OrdinaryPreviewHypothesis, OrdinaryPreviewRequest, PreviewEvidence, PreviewHypothesis,
    },
    public_workspace,
    reasoning_runtime::OperationalBounds,
    require,
    source_capture::{self, ReadMode},
    value::TypedValue as CV,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::LazyLock,
    time::Duration,
};

static RECIPE_NAME: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[A-Za-z][A-Za-z0-9_-]*$").unwrap());
static NUMBER: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^-?\d+(?:\.\d+)?$").unwrap());
static BARE_TEXT: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[A-Za-z_][A-Za-z0-9_.-]*$").unwrap());
type NamedRecipes = BTreeMap<String, Vec<String>>;
type RecipeHolders = BTreeMap<String, BTreeMap<String, Vec<Option<String>>>>;
struct CorePreview {
    report: String,
    code: i32,
    falsified: bool,
    holes: bool,
    moved: usize,
}

#[derive(Clone, Debug, clap::Args)]
pub struct Options {
    #[arg(long)]
    pub run: bool,
    pub record: Option<PathBuf>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Output {
    pub text: String,
    pub stderr: String,
    pub code: i32,
}

fn output(lines: Vec<String>, code: i32) -> Output {
    Output {
        text: lines.join("\n") + "\n",
        stderr: String::new(),
        code,
    }
}

fn recipe_file(record: &Path) -> (PathBuf, &'static str) {
    let parent = record.parent().unwrap_or(Path::new("."));
    if record
        .file_name()
        .is_some_and(|name| name == "GROUNDING.yaml")
    {
        (
            parent.join(".kpopper/measure.yaml"),
            ".kpopper/measure.yaml",
        )
    } else {
        (
            parent.join("PROVENANCE.measure.yaml"),
            "PROVENANCE.measure.yaml",
        )
    }
}
fn brief_file(record: &Path) -> PathBuf {
    let parent = record.parent().unwrap_or(Path::new("."));
    if record
        .file_name()
        .is_some_and(|name| name == "GROUNDING.yaml")
    {
        parent.join(".kpopper/view.yaml")
    } else {
        let stem = record
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("PROVENANCE");
        parent.join(format!("{stem}.view.yaml"))
    }
}
fn scalar(v: &V) -> String {
    match v {
        V::Text(value)
            if BARE_TEXT.is_match(value)
                && !matches!(
                    value.as_str(),
                    "null"
                        | "Null"
                        | "NULL"
                        | "true"
                        | "True"
                        | "TRUE"
                        | "false"
                        | "False"
                        | "FALSE"
                        | "yes"
                        | "Yes"
                        | "YES"
                        | "no"
                        | "No"
                        | "NO"
                        | "on"
                        | "On"
                        | "ON"
                        | "off"
                        | "Off"
                        | "OFF"
                ) =>
        {
            value.clone()
        }
        V::Text(value) => serde_json::to_string(value).unwrap(),
        _ => v.python_str(),
    }
}
fn refresh_line(
    id: &str,
    value: &V,
    recipe: &str,
    said: Option<&str>,
    today: &str,
    record: &Path,
    holder: Option<&str>,
) -> String {
    let text = value.python_str();
    let reason = if text.starts_with('-') || text.contains(['\n', '\r']) {
        Some("a value shaped like an option or spanning lines is not carried by set".into())
    } else if matches!(value, V::Text(_))
        && (NUMBER.is_match(&text) || matches!(text.as_str(), "true" | "false" | "null"))
    {
        Some(format!(
            "set would read {} as {}, which is not what was measured",
            value.python_repr(),
            text
        ))
    } else {
        None
    };
    if let Some(reason) = reason {
        let shown = format!("'{}'", text.replace('\'', "''"));
        return format!("edit it by hand in this pull request - {id}: v: {shown} - {reason}");
    }
    format!(
        "refresh: kpop set {id} {} --why {} --as-of {today}{} {}",
        shell_quote(&text),
        shell_quote(&measured_by(recipe, said)),
        holder
            .map(|name| format!(" --hypothesis {}", shell_quote(name)))
            .unwrap_or_default(),
        shell_quote(record.to_str().unwrap_or("GROUNDING.yaml"))
    )
}
fn executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    true
}
fn resolve(exe: &str, root: &Path) -> Option<PathBuf> {
    let path = Path::new(exe);
    if path.components().count() > 1 {
        let candidate = if path.is_absolute() {
            path.to_path_buf()
        } else {
            root.join(path)
        };
        return executable(&candidate).then_some(candidate);
    }
    env_path()
        .into_iter()
        .map(|dir| dir.join(exe))
        .find(|candidate| executable(candidate))
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
    command.args([
        "-C",
        parent.to_str().unwrap_or("."),
        "rev-parse",
        "--show-toplevel",
    ]);
    crate::reasoning_runtime::run_command_capture(
        &mut command,
        vec![],
        Duration::from_secs(5),
        16 * 1024,
    )
    .ok()
    .filter(|output| output.status.success())
    .and_then(|output| String::from_utf8(output.stdout).ok())
    .map(|output| output.trim().to_owned())
    .filter(|output| !output.is_empty())
    .map(PathBuf::from)
    .unwrap_or_else(|| parent.to_path_buf())
}
fn git_output(root: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(root).args(args);
    crate::reasoning_runtime::run_command_capture(
        &mut command,
        vec![],
        Duration::from_secs(5),
        16 * 1024,
    )
    .ok()
    .filter(|output| output.status.success())
    .and_then(|output| String::from_utf8(output.stdout).ok())
    .map(|output| output.trim().to_owned())
}
fn tree_identity(root: &Path) -> (Option<String>, Option<String>) {
    let commit =
        git_output(root, &["rev-parse", "--short=7", "HEAD"]).filter(|value| !value.is_empty());
    let said = commit.as_ref().map(|commit| {
        if git_output(root, &["status", "--porcelain"]).is_some_and(|value| !value.is_empty()) {
            format!("{commit} with the working tree changed")
        } else {
            commit.clone()
        }
    });
    (commit, said)
}
fn measured_by(recipe: &str, said: Option<&str>) -> String {
    format!(
        "measured by {recipe}{}",
        said.map(|value| format!(" at {value}")).unwrap_or_default()
    )
}
fn reading_day(body: &Map, raw: &BTreeMap<String, V>) -> Option<(String, String)> {
    for field in ["of", "read"] {
        if let Some(value) = body.get(field) {
            let day = value.python_str();
            if chrono::NaiveDate::parse_from_str(&day, "%Y-%m-%d").is_ok() {
                return Some((day, format!("its own {field}:")));
            }
        }
    }
    if let Some(source) = body.get("from").and_then(|value| text(value).ok())
        && let Some(source) = raw.get(source).and_then(|value| map(value).ok())
    {
        for field in ["read", "of"] {
            if let Some(value) = source.get(field) {
                let day = value.python_str();
                if chrono::NaiveDate::parse_from_str(&day, "%Y-%m-%d").is_ok() {
                    return Some((
                        day,
                        format!("the {field}: of {}", body["from"].python_str()),
                    ));
                }
            }
        }
    }
    None
}
fn hypothesis_holder(id: &str, base: &V, hypotheses: &V) -> Option<String> {
    if crate::ordinary_fields::collections(base)
        .ok()?
        .values()
        .any(|members| members.contains_key(id))
    {
        return None;
    }
    map(hypotheses).ok()?.iter().find_map(|(name, hypothesis)| {
        let hypothesis = map(hypothesis).ok()?;
        let document = hypothesis
            .get("doc")
            .or_else(|| hypothesis.get("document"))?;
        crate::ordinary_fields::collections(document)
            .ok()?
            .values()
            .any(|members| members.contains_key(id))
            .then(|| name.clone())
    })
}
fn remeasure_report(report: &str, core: bool) -> Vec<String> {
    const GENERIC_CONTESTED: &str = "re-read against the merged tree: pull each id to see every reading beside the base's, then set what holds today - in the base with a later day, or in the hypothesis that read it - or refute one, and consolidate again";
    const GENERIC_LATER: &str = "  read again on a later day - set it in the base or in the hypothesis with --as-of - or refute the hypothesis";
    let mut lines = report
        .lines()
        .filter(|line| {
            !line.starts_with("clean - and nothing here folds:")
                && (core
                    || *line
                        != "not clean: a falsifier holds - nothing folds until it is read again")
                && *line != GENERIC_CONTESTED
                && *line != GENERIC_LATER
                && !line.starts_with("  consolidate ")
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}
fn typed_layer(base: &CV, layer: &CV) -> Result<CV> {
    let mut result = base.clone();
    for (section, members) in crate::reasoning_fields::collections(layer)? {
        let target = crate::history_view::map_mut(
            crate::history_view::map_mut(&mut result)?
                .entry(section)
                .or_insert_with(|| CV::Map(BTreeMap::new())),
        )?;
        target.extend(members);
    }
    Ok(result)
}
fn core_preview(
    capture: &source_capture::OrdinaryCapture,
    current: &CV,
    hypotheses: &CV,
    tree: &PreviewHypothesis,
    differing: &BTreeMap<String, (String, V, V, String)>,
    runtime: Option<&crate::reasoning_runtime::Runtime>,
    today: &str,
) -> Result<CorePreview> {
    let candidate = typed_layer(current, &tree.document)?;
    let mut selection = hmap(hypotheses)?.keys().cloned().collect::<Vec<_>>();
    selection.push(tree.name.clone());
    selection.sort();
    let mut proposals = Vec::new();
    for (name, hypothesis) in hmap(hypotheses)? {
        let hypothesis = hmap(hypothesis)?;
        if hypothesis
            .get("error")
            .is_some_and(|value| *value != CV::Null)
        {
            continue;
        }
        proposals.push(CV::Map(BTreeMap::from([
            ("name".into(), CV::Text(name.clone())),
            (
                "doc".into(),
                hypothesis
                    .get("doc")
                    .or_else(|| hypothesis.get("document"))
                    .ok_or_else(|| Error("invalid_snapshot".into()))?
                    .clone(),
            ),
            (
                "head".into(),
                hypothesis
                    .get("head")
                    .cloned()
                    .unwrap_or_else(|| CV::Map(BTreeMap::new())),
            ),
        ])));
    }
    proposals.push(CV::Map(BTreeMap::from([
        ("name".into(), CV::Text(tree.name.clone())),
        ("doc".into(), tree.document.clone()),
        ("head".into(), tree.head.clone()),
    ])));
    let base = crate::reasoning_operations::OperationDocument::from_snapshot(
        capture.snapshot()?.clone(),
        false,
    )?;
    let derived = base.derive(&candidate, &selection, &proposals)?;
    let world = crate::reasoning_operations::OperationWorld::new(
        &derived,
        runtime,
        OperationalBounds::default(),
    )?;
    let findings = crate::reasoning_operations::findings(world.context())?;
    let findings = hmap(&findings)?;
    let falsified = hlist(&findings["falsified"])?
        .iter()
        .map(|v| htext(v).map(str::to_owned))
        .collect::<Result<Vec<_>>>()?;
    let holes = hlist(&findings["holes"])?
        .iter()
        .map(|v| htext(v).map(str::to_owned))
        .collect::<Result<Vec<_>>>()?;
    let moved = hlist(&findings["moved"])?
        .iter()
        .map(|v| htext(v).map(str::to_owned))
        .collect::<Result<Vec<_>>>()?;
    let names = selection.join(", ");
    let mut out = vec![
        format!(
            "core/v1 prospective snapshot {}; findings {}",
            world.context().snapshot_id(),
            world.context().findings_revision()
        ),
        format!(
            "the base with {names} laid over it{}",
            if selection.len() > 1 {
                ", in name order"
            } else {
                ""
            }
        ),
    ];
    for (name, hypothesis) in hmap(hypotheses)? {
        let head = hmap(hypothesis)?
            .get("head")
            .and_then(|value| hmap(value).ok());
        let claim = head
            .and_then(|head| head.get("claim"))
            .and_then(|v| htext(v).ok())
            .unwrap_or("");
        let born = head
            .and_then(|head| head.get("born"))
            .map(crate::source_text::ordinary_python_str)
            .unwrap_or_default();
        out.push(format!(
            "  {name}{}: {claim}",
            if born.is_empty() {
                String::new()
            } else {
                format!(" (born {born})")
            }
        ));
    }
    out.push(format!(
        "  {} (born {today}, today, never folds): {}",
        tree.name,
        htext(&hmap(&tree.head)?["claim"])?
    ));
    out.push(String::new());
    out.push("arrived (0): what the fold would add".into());
    out.push(format!(
        "updates ({}): what the base holds that a hypothesis replaces, and what rests on each",
        differing.len()
    ));
    for (id, (_recipe, body, measured, _section)) in differing {
        let body = map(body)?;
        let old = body.get("v").or_else(|| body.get("quoted")).unwrap();
        out.push(format!(
            "  {id}: {} -> {}, from {}",
            scalar(old),
            scalar(measured),
            tree.name
        ));
        out.push(format!(
            "    a reading from {today} that is newer than the base's"
        ));
        for line in &falsified {
            out.push(format!("    FIRED     {line}"));
        }
        if falsified.is_empty() {
            for line in &moved {
                let id = line.split(':').next().unwrap_or(line);
                out.push(format!("    MOVED     {id}: dependency value, rule or computational basis changed since review"));
            }
        }
    }
    out.push("reversed (0): a verdict, or other grounds, laid over a standing judgment - by its own condition, by a person's name, or waiting for one".into());
    out.push(format!(
        "moved / falsified ({}): what the union moves or breaks",
        moved.len() + falsified.len()
    ));
    out.extend(falsified.iter().map(|line| format!("  FALSIFIED {line}")));
    out.extend(moved.iter().map(|line| format!("  MOVED {line}")));
    out.push("contested (0)".into());
    out.push("candidates (0): pairs for a person to judge as the same subject or distinct".into());
    out.push("new subjects (0): prefixes the base does not hold".into());
    out.push(String::new());
    if !falsified.is_empty() || !holes.is_empty() {
        out.push("not clean: a falsifier holds - nothing folds until it is read again".into());
    } else if !moved.is_empty() {
        out.push(format!(
            "moved: {} judgment{} to re-review before the fold - a premise moved under {}",
            moved.len(),
            if moved.len() == 1 { "" } else { "s" },
            if moved.len() == 1 { "it" } else { "them" }
        ));
    }
    Ok(CorePreview {
        report: out.join("\n") + "\n",
        code: i32::from(!falsified.is_empty() || !holes.is_empty()),
        falsified: !falsified.is_empty(),
        holes: !holes.is_empty(),
        moved: moved.len(),
    })
}
fn history_head_lines(
    snapshot: crate::reasoning_snapshot::Snapshot,
    heads: &[(String, CV)],
    runtime: Option<&crate::reasoning_runtime::Runtime>,
) -> Result<(Vec<String>, bool)> {
    let operation = crate::reasoning_operations::OperationDocument::from_snapshot(snapshot, true)?;
    let world = crate::reasoning_operations::OperationWorld::new(
        &operation,
        runtime,
        OperationalBounds::default(),
    )?;
    let mut lines = Vec::new();
    let mut bad = false;
    for (name, head) in heads {
        let Some(condition) = hmap(head)?.get("wrong_if") else {
            continue;
        };
        let (truth, result) = world.condition(condition)?;
        if truth == Some(true) {
            bad = true;
            lines.push(format!(
                "  FALSIFIED hypothesis {name}: wrong_if holds on the measured candidate"
            ));
        } else if truth.is_none() {
            bad = true;
            let status = result
                .as_ref()
                .and_then(|value| value.get("status"))
                .and_then(|value| value.as_str())
                .unwrap_or("unavailable");
            lines.push(format!("  HOLE hypothesis {name}: wrong_if {status}"));
        }
    }
    Ok((lines, bad))
}
fn allowlist(path: &Path, display_name: &str) -> Result<BTreeMap<String, Vec<String>>> {
    if !path.is_file() {
        return Ok(BTreeMap::new());
    }
    let value = history_yaml::decode_document(&std::fs::read(path)?)?;
    let map = hmap(&value)?;
    let mut out = BTreeMap::new();
    for (name, argv) in map {
        require(
            RECIPE_NAME.is_match(name),
            &format!(
                "refused - {display_name}:\n  '{name}' is not a recipe name - letters, digits, underscores and dashes, opening with a letter; quote it if the loader read it as something else"
            ),
        )?;
        let values = hlist(argv)?
            .iter()
            .map(|v| htext(v).map(str::to_owned))
            .collect::<Result<Vec<_>>>()?;
        require(
            !values.is_empty() && values.iter().all(|v| !v.is_empty() && !v.contains('\0')),
            &format!(
                "refused - {name}: a recipe is a non-empty list of non-empty strings - the executable and its arguments - never a line for a shell"
            ),
        )?;
        out.insert(name.clone(), values);
    }
    Ok(out)
}
fn run_recipe(argv: &[String], root: &Path) -> Result<(String, String)> {
    let exe = resolve(&argv[0], root).ok_or_else(|| {
        Error(format!(
            "no executable {:?} on the path{}",
            argv[0],
            if argv[0].contains('/') {
                format!(" or under {}", root.display())
            } else {
                String::new()
            }
        ))
    })?;
    let mut command = Command::new(exe);
    command
        .args(&argv[1..])
        .current_dir(root)
        .stdin(Stdio::null());
    let output = crate::reasoning_runtime::run_command_capture(
        &mut command,
        vec![],
        Duration::from_secs(60),
        64 * 1024,
    )
    .map_err(|e| {
        Error(match e.0.as_str() {
            "runtime_timeout" => "did not finish in 60 s and was stopped".into(),
            "output_limit" => "printed more than 64 KiB and was stopped".into(),
            other => other.to_owned(),
        })
    })?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !output.status.success() {
        let tail = stderr.split_whitespace().collect::<Vec<_>>().join(" ");
        return Err(Error(format!(
            "exited {}{}",
            output
                .status
                .code()
                .map_or("by signal".into(), |n| n.to_string()),
            if tail.is_empty() {
                String::new()
            } else {
                format!(
                    " - stderr: {}",
                    tail.chars()
                        .rev()
                        .take(200)
                        .collect::<String>()
                        .chars()
                        .rev()
                        .collect::<String>()
                )
            }
        )));
    }
    Ok((stdout, stderr))
}
fn utc_day() -> String {
    chrono::Utc::now().date_naive().to_string()
}
fn parse_reading(out: &str, recorded: &V) -> Result<V> {
    let lines = out
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    require(
        !lines.is_empty(),
        "printed nothing - the value is the one line a recipe prints",
    )?;
    require(
        lines.len() == 1,
        &format!(
            "printed {} lines - the value is the one line a recipe prints; diagnostics go to stderr",
            lines.len()
        ),
    )?;
    let line = lines[0];
    match recorded {
        V::Bool(_) => match line {
            "true" => Ok(V::Bool(true)),
            "false" => Ok(V::Bool(false)),
            _ => Err(Error(format!(
                "printed {line:?} where the record holds true or false"
            ))),
        },
        V::Integer(_) => {
            if !NUMBER.is_match(line) {
                Err(Error(format!(
                    "printed '{}' where the record holds a number",
                    line.replace('\'', "\\'")
                )))
            } else if line.contains('.') {
                line.parse::<f64>()
                    .map_err(|_| Error("nonfinite_float".into()))
                    .and_then(|n| crate::value::FiniteFloat::new(n).map(V::Float))
            } else {
                crate::value::Integer::new(line).map(V::Integer)
            }
        }
        V::Float(_) | V::NonFinite(_) => {
            if !NUMBER.is_match(line) {
                Err(Error(format!(
                    "printed '{}' where the record holds a number",
                    line.replace('\'', "\\'")
                )))
            } else if line.contains('.') {
                line.parse::<f64>()
                    .map_err(|_| Error("nonfinite_float".into()))
                    .and_then(|n| crate::value::FiniteFloat::new(n).map(V::Float))
            } else {
                crate::value::Integer::new(line).map(V::Integer)
            }
        }
        _ => Ok(V::Text(line.into())),
    }
}
fn agrees(a: &V, b: &V) -> bool {
    match (a, b) {
        (V::Bool(x), V::Bool(y)) => x == y,
        (
            V::Integer(_) | V::Float(_) | V::NonFinite(_),
            V::Integer(_) | V::Float(_) | V::NonFinite(_),
        ) => crate::ordinary_value::python_equal(a, b),
        _ => a.python_str() == b.python_str(),
    }
}

fn measurement_declarations(base: &V, hypotheses: &V) -> Result<(NamedRecipes, V, Vec<String>)> {
    let mut declarations = RecipeHolders::new();
    let mut base_measures = BTreeMap::<String, String>::new();
    let mut current = base.clone();
    for (_section, members) in crate::ordinary_fields::collections(base)? {
        for (id, body) in members {
            if let Ok(body) = map(&body)
                && let Some(name) = body.get("measure").and_then(|value| text(value).ok())
            {
                declarations
                    .entry(id.clone())
                    .or_default()
                    .entry(name.into())
                    .or_default()
                    .push(None);
                base_measures.insert(id, name.into());
            }
        }
    }
    let mut problems = Vec::new();
    for (hypothesis_name, hypothesis) in map(hypotheses)? {
        let hypothesis = map(hypothesis)?;
        if hypothesis
            .get("error")
            .is_some_and(|value| *value != V::Null)
        {
            continue;
        }
        let document = hypothesis
            .get("doc")
            .or_else(|| hypothesis.get("document"))
            .ok_or_else(|| Error("invalid_snapshot".into()))?;
        for (section, members) in crate::ordinary_fields::collections(document)? {
            let target = map(&current)?
                .get(&section)
                .cloned()
                .unwrap_or_else(|| V::Map(Map::new()));
            let mut target = map(&target)?.clone();
            for (id, body) in members {
                if let Ok(body_map) = map(&body) {
                    if let Some(name) = body_map.get("measure").and_then(|value| text(value).ok()) {
                        declarations
                            .entry(id.clone())
                            .or_default()
                            .entry(name.into())
                            .or_default()
                            .push(Some(hypothesis_name.clone()));
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
            let said = by_recipe
                .iter()
                .map(|(recipe, holders)| {
                    let holders = holders
                        .iter()
                        .map(|holder| holder.as_deref().unwrap_or("the base"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{recipe} by {holders}")
                })
                .collect::<Vec<_>>()
                .join("; ");
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
    let record = options
        .record
        .clone()
        .unwrap_or_else(|| cwd.join("GROUNDING.yaml"));
    let record = if record.is_absolute() {
        record
    } else {
        cwd.join(record)
    }
    .canonicalize()?;
    let runtime = public_workspace::runtime_for_paths(std::slice::from_ref(&record), cwd, None)?;
    let capture = source_capture::capture_ordinary_source_with_runtime(
        std::slice::from_ref(&record),
        cwd,
        if frozen {
            ReadMode::Frozen
        } else {
            ReadMode::Live
        },
        None,
        runtime.as_ref(),
    )?;
    let mut history_candidate = None;
    let mut history_document = None;
    let mut history_heads = Vec::new();
    if let Some(history) = capture.history_capture() {
        use crate::history_authoring::{Options as AuthoringOptions, s as hs};
        use crate::history_hypothesis_authoring::{Fold, prepare_fold};
        use crate::history_paths::Scheme;
        use crate::history_store::Store;
        let mut selected = Vec::new();
        let mut physical = Vec::new();
        for (name, hypothesis) in map(capture.hypotheses())? {
            let kind = map(hypothesis)?
                .get("kind")
                .and_then(|value| text(value).ok())
                .unwrap_or("");
            if kind == crate::history_hypotheses::KIND {
                selected.push(name.clone());
                history_heads.push((
                    name.clone(),
                    map(hypothesis)?
                        .get("head")
                        .unwrap_or(&V::Null)
                        .finite_projection()?,
                ));
            } else if kind != "contribution" {
                physical.push(name.clone());
            }
        }
        if !physical.is_empty() {
            return Ok(Output {
                text: String::new(),
                stderr: format!(
                    "incomplete - physical hypotheses have no faithful admitted history path: {}\n",
                    physical.join(", ")
                ),
                code: 1,
            });
        }
        let mut virtual_capture = history.clone();
        if !selected.is_empty() {
            let now = chrono::Utc::now();
            let authoring = AuthoringOptions {
                operation: format!(
                    "remeasure-fold-{}",
                    now.timestamp_nanos_opt().unwrap_or_default()
                ),
                recorded_at: now.to_rfc3339(),
                recording_day: utc_day(),
                by: hs("remeasure"),
                strict: true,
                paths: Scheme::Hashed,
                receipt_version: Some(8),
            };
            let fold = Fold {
                names: selected,
                because: "prospective remeasure of selected named history".into(),
                take: vec![],
                drops: CV::Map(BTreeMap::new()),
                assessment_version: 1,
            };
            let store = Store::new(&record)?;
            let mutation = match prepare_fold(&store, history, &fold, &authoring, runtime.as_ref())
            {
                Ok(mutation) => mutation,
                Err(error) => {
                    return Ok(Output {
                        text: String::new(),
                        stderr: format!(
                            "refused - history remeasure cannot admit the named fold: {error}\n"
                        ),
                        code: 1,
                    });
                }
            };
            virtual_capture = crate::history_prospective::capture_after(history, &mutation)?;
        }
        history_document = Some(V::from_typed(&crate::history_authoring::document(
            &virtual_capture,
        )?));
        history_candidate = Some(virtual_capture);
    }
    let doc = history_document
        .as_ref()
        .unwrap_or_else(|| capture.ordinary_document());
    let no_hypotheses = V::Map(Map::new());
    let hypotheses = if history_candidate.is_some() {
        &no_hypotheses
    } else {
        capture.hypotheses()
    };
    let (named, current, problems) = measurement_declarations(doc, hypotheses)?;
    let capabilities = crate::ordinary_fields::capabilities(doc, None)?;
    let is_core = map(&capabilities)?
        .get("profile")
        .is_some_and(|value| value == &V::Text("core/v1".into()));
    if !problems.is_empty() {
        let mut lines = vec!["refused - the record names recipes it cannot: ".into()];
        lines.extend(problems.into_iter().map(|problem| format!("  {problem}")));
        return Ok(output(lines, 1));
    }
    let collections = crate::ordinary_fields::collections(&current)?;
    let (allowlist_path, allowlist_name) = recipe_file(&record);
    let allowlist_exists = allowlist_path.is_file();
    let recipes = match allowlist(&allowlist_path, allowlist_name) {
        Ok(recipes) => recipes,
        Err(error) => {
            return Ok(Output {
                text: String::new(),
                stderr: format!("{error}\n"),
                code: 1,
            });
        }
    };
    if named.is_empty() {
        let lines = if recipes.is_empty() {
            vec!["no measures beside the record - nothing to re-measure".into()]
        } else {
            vec![format!(
                "{allowlist_name} holds {} recipe{}, and no entry names one - nothing to re-measure",
                recipes.len(),
                if recipes.len() == 1 { "" } else { "s" }
            )]
        };
        return Ok(output(lines, 0));
    }
    let root = checkout_root(&record);
    let cited = named.keys().cloned().collect::<Vec<_>>();
    if !allowlist_exists {
        return Ok(output(
            vec![format!(
                "refused - {} recipe{} named and no {allowlist_name} beside the record to hold {}: {}",
                cited.len(),
                if cited.len() == 1 { "" } else { "s" },
                if cited.len() == 1 { "it" } else { "them" },
                cited.join(", ")
            )],
            1,
        ));
    }
    let missing = cited
        .iter()
        .filter(|name| !recipes.contains_key(name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Ok(output(
            vec![format!(
                "refused - the record names recipe{} {allowlist_name} does not hold: {} - a measurement nothing takes is a hole, and a falsifier reading it tests nothing",
                if missing.len() == 1 { "" } else { "s" },
                missing.join(", ")
            )],
            1,
        ));
    }
    let entries = named.values().map(Vec::len).sum::<usize>();
    let mut out = vec![format!(
        "{} recipe{} named by {} entr{}, from {allowlist_name}, run from {}:",
        cited.len(),
        if cited.len() == 1 { "" } else { "s" },
        entries,
        if entries == 1 { "y" } else { "ies" },
        root.display()
    )];
    for name in &cited {
        let ids = &named[name];
        let argv = &recipes[name];
        let exe = resolve(&argv[0], &root)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| format!("{} (not found)", argv[0]));
        let args = argv[1..]
            .iter()
            .map(|v| shell_quote(v))
            .collect::<Vec<_>>()
            .join(" ");
        out.push(format!(
            "  {} <- {}: {} {}",
            ids.join(", "),
            name,
            exe,
            args
        ));
    }
    for name in recipes
        .keys()
        .filter(|name| !named.contains_key(name.as_str()))
    {
        out.push(format!("  named by no entry, never run: {name}"));
    }
    if !options.run {
        out.extend([
            "".into(),
            "nothing ran - add --run to measure this tree".into(),
        ]);
        return Ok(output(out, 0));
    }
    out.push("".into());
    let today = utc_day();
    let (commit, said) = tree_identity(&root);
    out.push(format!(
        "measured on {today} (UTC){}: {} entr{} by {} recipe{}",
        said.as_ref()
            .map(|value| format!(", at {value}"))
            .unwrap_or_default(),
        entries,
        if entries == 1 { "y" } else { "ies" },
        cited.len(),
        if cited.len() == 1 { "" } else { "s" }
    ));
    let mut failed = 0usize;
    let mut failed_lines = Vec::new();
    let mut results = BTreeMap::new();
    for name in &cited {
        match run_recipe(recipes.get(name).unwrap(), &root) {
            Ok((value, _)) => {
                results.insert(name.clone(), value);
            }
            Err(error) => {
                failed += 1;
                failed_lines.push(format!(
                    "  FAIL {name} ({}): {error}",
                    named[name].join(", ")
                ));
            }
        }
    }
    if (is_core || capture.files().contains_key(&brief_file(&record)))
        && let Err(error) = capture.verify()
    {
        return Ok(output(vec![format!("refused - {}: {error}", error.0)], 1));
    }
    let by_id = named
        .iter()
        .flat_map(|(recipe, ids)| ids.iter().map(move |id| (id.clone(), recipe.clone())))
        .collect::<BTreeMap<_, _>>();
    let raw = collections
        .values()
        .flat_map(|members| members.iter().map(|(id, body)| (id.clone(), body.clone())))
        .collect::<BTreeMap<_, _>>();
    let mut agreed = Vec::new();
    let mut differing = BTreeMap::<String, (String, V, V, String)>::new();
    for (id, name) in by_id {
        let Some(result) = results.get(&name) else {
            continue;
        };
        let (section, body) = collections
            .iter()
            .find_map(|(section, members)| {
                members.get(&id).map(|body| (section.clone(), body.clone()))
            })
            .unwrap();
        let Ok(fields) = map(&body) else {
            failed += 1;
            failed_lines.push(format!("  FAIL {name} ({id}): what holds this id now is not an entry with a reading of its own, so there is nothing to measure against"));
            continue;
        };
        let Some(field) = fields.get("v").or_else(|| fields.get("quoted")) else {
            failed += 1;
            failed_lines.push(format!("  FAIL {name} ({id}): what holds this id now is not an entry with a reading of its own, so there is nothing to measure against"));
            continue;
        };
        match parse_reading(result, field) {
            Ok(measured) if agrees(field, &measured) => agreed.push(format!(
                "  {id}: {} - as recorded ({name})",
                scalar(&measured)
            )),
            Ok(measured) => {
                differing.insert(id, (name, body, measured, section));
            }
            Err(error) => {
                failed += 1;
                failed_lines.push(format!("  FAIL {name} ({id}): {error}"));
            }
        }
    }
    out.extend(agreed);
    out.extend(failed_lines);
    let mut unchanged_history_findings = None;
    let mut unchanged_history_heads = (Vec::new(), false);
    if is_core {
        let operation = crate::reasoning_operations::OperationDocument::from_snapshot(
            capture.snapshot()?.clone(),
            true,
        )?;
        let world = crate::reasoning_operations::OperationWorld::new(
            &operation,
            runtime.as_ref(),
            OperationalBounds::default(),
        )?;
        let findings = crate::reasoning_operations::findings(world.context())?;
        if history_candidate.is_some() {
            unchanged_history_findings = Some(findings);
            let snapshot = if let Some(candidate) = history_candidate.as_ref() {
                crate::history_adapter::from_store_capture(candidate)?
                    .snapshot(Default::default())?
            } else {
                unreachable!()
            };
            unchanged_history_heads =
                history_head_lines(snapshot, &history_heads, runtime.as_ref())?;
        } else {
            let holes = hlist(&hmap(&findings)?["holes"])?;
            failed += holes.len();
            for hole in holes {
                out.push(format!("  FAIL core assessment: {}", htext(hole)?));
            }
        }
    }
    if differing.is_empty() {
        if let Some(findings) = unchanged_history_findings {
            let findings = hmap(&findings)?;
            for (label, key) in [
                ("FALSIFIED", "falsified"),
                ("HOLE", "holes"),
                ("MOVED", "moved"),
                ("NOTE", "notes"),
            ] {
                for line in hlist(&findings[key])? {
                    out.push(format!("  {label} {}", htext(line)?));
                }
            }
            out.extend(unchanged_history_heads.0);
            let red = failed != 0
                || unchanged_history_heads.1
                || ["falsified", "holes"]
                    .iter()
                    .any(|key| hlist(&findings[*key]).is_ok_and(|items| !items.is_empty()));
            out.push(String::new());
            out.push("prospective history candidate only - neither the named fold nor the measured values were committed".into());
            out.push(String::new());
            out.push(if red {
                "not clean: the prospective measured history has a falsifier or incomplete finding"
                    .into()
            } else {
                "the prospective history holds what this tree measures".into()
            });
            return Ok(output(out, i32::from(red)));
        }
        out.push("".into());
        out.push(if failed != 0 {
        format!("not clean: a hole - {failed} recipe{} failed, and a measurement nothing takes is a hole", if failed == 1 { "" } else { "s" })
        } else {
            if history_candidate.is_some() {
                "the prospective history holds what this tree measures".into()
            } else {
                "the record holds what this tree measures".into()
            }
        });
        return Ok(output(out, i32::from(failed != 0)));
    }
    if let Some(history) = history_candidate.as_ref() {
        use crate::history_authoring::{Options as AuthoringOptions, obj as hobj, s as hs};
        use crate::history_authoring_batch::{BatchOptions, prepare_batch};
        use crate::history_paths::Scheme;
        use crate::history_store::Store;
        let store = Store::new(&record)?;
        let actions = differing
            .iter()
            .map(|(id, (recipe, _body, measured, _section))| {
                Ok(hobj([
                    ("kind", hs("set")),
                    ("id", hs(id)),
                    ("value", measured.try_typed()?),
                    ("as_of", hs(&today)),
                    ("why", hs(&measured_by(recipe, said.as_deref()))),
                ]))
            })
            .collect::<Result<Vec<_>>>()?;
        let now = chrono::Utc::now();
        let operation = format!(
            "remeasure-values-{}",
            now.timestamp_nanos_opt().unwrap_or_default()
        );
        let options = BatchOptions {
            authoring: AuthoringOptions {
                operation,
                recorded_at: now.to_rfc3339(),
                recording_day: today.clone(),
                by: hs("remeasure"),
                strict: true,
                paths: Scheme::Hashed,
                receipt_version: Some(8),
            },
            receipt_version: 8,
            context: hobj([
                ("kind", hs("prospective_remeasure")),
                ("observation_date", hs(&today)),
            ]),
            evidence: BTreeMap::new(),
        };
        let mutation = match prepare_batch(&store, history, &actions, &options, runtime.as_ref()) {
            Ok(mutation) => mutation,
            Err(error) => {
                out.push(String::new());
                out.push(format!(
                    "refused - history remeasure cannot admit the measured values: {error}"
                ));
                return Ok(output(out, 1));
            }
        };
        let assessment =
            crate::history_prospective::assess(history, &mutation, None, None, runtime.as_ref())?;
        let findings = crate::reasoning_operations::findings(&assessment.after)?;
        let findings = hmap(&findings)?;
        for (label, key) in [
            ("FALSIFIED", "falsified"),
            ("HOLE", "holes"),
            ("MOVED", "moved"),
            ("NOTE", "notes"),
        ] {
            for line in hlist(&findings[key])? {
                out.push(format!("  {label} {}", htext(line)?));
            }
        }
        let (head_lines, head_bad) = history_head_lines(
            assessment.after.snapshot().clone(),
            &history_heads,
            runtime.as_ref(),
        )?;
        out.extend(head_lines);
        out.push(String::new());
        out.push("prospective history candidate only - neither the named fold nor the measured values were committed".into());
        for (id, (recipe, body, measured, _section)) in &differing {
            let body = map(body)?;
            let field = body.get("v").or_else(|| body.get("quoted")).unwrap();
            let dated = reading_day(body, &raw);
            out.push(format!(
                "  {id}: {} recorded{} -> {} {}",
                scalar(field),
                dated
                    .as_ref()
                    .map(|(day, by)| format!(" ({day}, by {by})"))
                    .unwrap_or_else(|| " (undated)".into()),
                scalar(measured),
                measured_by(recipe, said.as_deref())
            ));
            out.push(format!(
                "    {}",
                refresh_line(
                    id,
                    measured,
                    recipe,
                    said.as_deref(),
                    &today,
                    &record,
                    hypothesis_holder(id, doc, capture.hypotheses()).as_deref(),
                )
            ));
        }
        let red = failed != 0
            || head_bad
            || ["falsified", "holes"]
                .iter()
                .any(|key| hlist(&findings[*key]).is_ok_and(|items| !items.is_empty()));
        out.push(String::new());
        out.push(if red {
            "not clean: the prospective measured history has a falsifier or incomplete finding".into()
        } else {
            format!("the prospective history reads {} entr{} differently, none across a line - refresh them", differing.len(), if differing.len() == 1 { "y" } else { "ies" })
        });
        return Ok(output(out, i32::from(red)));
    }
    let mut proposal_document = V::Map(Map::new());
    for (id, (recipe, body, measured, section)) in &differing {
        let mut body = map(body)?.clone();
        let value_field = if body.get("v").is_none_or(|value| *value == V::Null)
            && body.get("quoted").is_some_and(|value| *value != V::Null)
        {
            "quoted"
        } else {
            "v"
        };
        body.insert(value_field.into(), measured.clone());
        body.insert("of".into(), V::Text(today.clone()));
        body.insert("at".into(), V::Text(measured_by(recipe, said.as_deref())));
        let members = map_mut(
            map_mut(&mut proposal_document)?
                .entry(section.clone())
                .or_insert_with(|| V::Map(Map::new())),
        )?;
        members.insert(id.clone(), V::Map(body));
    }
    let tree_name = format!("tree/{}", commit.as_deref().unwrap_or("here"));
    let tree_claim = format!(
        "what the tree{} measures",
        commit
            .as_ref()
            .map(|value| format!(" at {value}"))
            .unwrap_or_default()
    );
    let (preview_report, preview_code, preview_facts, core_falsified, core_holes, core_moved) =
        if is_core {
            let tree_proposal = PreviewHypothesis {
                name: tree_name,
                document: proposal_document.finite_projection()?,
                head: CV::Map(BTreeMap::from([
                    ("claim".into(), CV::Text(tree_claim)),
                    ("born".into(), CV::Text(today.clone())),
                    ("folds".into(), CV::Text("never".into())),
                ])),
            };
            let finite_hypotheses = capture.hypotheses().finite_projection()?;
            core_preview(
                &capture,
                &current.finite_projection()?,
                &finite_hypotheses,
                &tree_proposal,
                &differing,
                runtime.as_ref(),
                &today,
            )
            .map(|preview| {
                (
                    preview.report,
                    preview.code,
                    None,
                    preview.falsified,
                    preview.holes,
                    preview.moved,
                )
            })?
        } else {
            let proposals = [OrdinaryPreviewHypothesis {
                name: tree_name,
                document: proposal_document,
                head: V::Map(Map::from([
                    ("claim".into(), V::Text(tree_claim)),
                    ("born".into(), V::Text(today.clone())),
                    ("folds".into(), V::Text("never".into())),
                ])),
            }];
            let request = OrdinaryPreviewRequest {
                document: doc,
                hypotheses: map(capture.hypotheses())?,
                proposals: &proposals,
                context: None,
                as_of: Some(&today),
                runtime: runtime.as_ref(),
            };
            let brief_path = brief_file(&record);
            let brief = capture.files().get(&brief_path);
            let preview = crate::public_consolidation::preview_ordinary_with_evidence(
                &request,
                &PreviewEvidence {
                    brief: brief.map(Vec::as_slice),
                },
            )?;
            (
                preview.report,
                preview.exit_code,
                Some(preview.facts),
                false,
                false,
                0,
            )
        };
    let changed_ids = differing
        .keys()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let page_bound = preview_facts
        .as_ref()
        .map(|facts| facts.page_bound(&changed_ids))
        .unwrap_or_default();
    let mut report_lines = remeasure_report(&preview_report, is_core);
    if !page_bound.is_empty() {
        report_lines.retain(|line| !line.starts_with("moved:"));
        while report_lines.last().is_some_and(String::is_empty) {
            report_lines.pop();
        }
    }
    out.push("".into());
    out.extend(report_lines);
    out.push("".into());
    if !page_bound.is_empty() {
        for item in &page_bound {
            out.push(format!("  FAIL {}: wrong_if reads {} beside {}, which the tree measured differently - the page decides it, and the page does not see the tree", item.id, item.pages.join(", "), item.readings.join(", ")));
        }
        out.push(String::new());
    }
    for (id, (recipe, body, measured, _section)) in &differing {
        let body = map(body)?;
        let field = body.get("v").or_else(|| body.get("quoted")).unwrap();
        let dated = reading_day(body, &raw);
        out.push(format!(
            "  {id}: {} recorded{} -> {} {}",
            scalar(field),
            dated
                .as_ref()
                .map(|(day, by)| format!(" ({day}, by {by})"))
                .unwrap_or_else(|| " (undated)".into()),
            scalar(measured),
            measured_by(recipe, said.as_deref())
        ));
        let refused = preview_facts.as_ref().is_some_and(|facts| {
            facts.contested.contains_key(id) || facts.refused.contains_key(id)
        });
        if refused {
            out.push("    correct the recorded claim in this pull request, or run the measurement again on a later day; do not future-date this result".into());
        } else {
            out.push(format!(
                "    {}",
                refresh_line(
                    id,
                    measured,
                    recipe,
                    said.as_deref(),
                    &today,
                    &record,
                    hypothesis_holder(id, doc, capture.hypotheses()).as_deref(),
                )
            ));
        }
    }
    out.push("".into());
    let facts_red = preview_facts.as_ref().is_some_and(|facts| facts.red);
    let code = if preview_facts.is_some() {
        i32::from(facts_red || failed != 0 || !page_bound.is_empty())
    } else {
        i32::from(preview_code != 0 || failed != 0)
    };
    let moved = preview_facts
        .as_ref()
        .map(|facts| facts.moved.len())
        .unwrap_or(core_moved);
    out.push(if code != 0 {
        let facts = preview_facts.as_ref();
        let mut causes = Vec::new();
        if facts.is_some_and(|facts| !facts.contested.is_empty() || !facts.refused.is_empty()) { causes.push("a reading the tree contests"); }
        if facts.is_some_and(|facts| !facts.falsified.is_empty() || !facts.head_falsified.is_empty())
            || core_falsified
        { causes.push("a falsifier that holds on what the tree measures"); }
        if failed != 0 || facts.is_some_and(|facts| !facts.holes.is_empty()) || core_holes
        { causes.push("a hole"); }
        if !page_bound.is_empty() { causes.push("a sign the page decides"); }
        if facts.is_some_and(|facts| !facts.untaken.is_empty()) { causes.push("a reversal a person has not taken by name"); }
        if facts.is_some_and(|facts| !facts.drops_needed.is_empty()) { causes.push("a dropped dependency to name at the fold"); }
        format!("not clean: {} - red until the record and the tree agree", causes.join(", "))
    } else if preview_facts.is_some() && moved != 0 {
        format!("the tree moved {} reading{} under {moved} judgment{} - refresh, then re-review; a move flags and fails nothing", differing.len(), if differing.len() == 1 { "" } else { "s" }, if moved == 1 { "" } else { "s" })
    } else {
        format!("the tree reads {} entr{} differently, none across a line - refresh them", differing.len(), if differing.len() == 1 { "y" } else { "ies" })
    });
    Ok(output(out, code))
}
fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "._/-".contains(c))
    {
        value.into()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}
