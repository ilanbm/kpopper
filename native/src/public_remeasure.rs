//! Read-only ordinary `remeasure` plan and bounded local recipe execution.
use crate::{
    Error, Result,
    history_contract::{map as hmap, text as htext},
    history_view::list as hlist,
    history_yaml,
    ordinary_value::{Map, Value as V, map, map_mut, text},
    public_consolidation::{PreviewHypothesis, PreviewRequest},
    public_workspace, require,
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
type NamedRecipes = BTreeMap<String, Vec<String>>;
type RecipeHolders = BTreeMap<String, BTreeMap<String, Vec<Option<String>>>>;

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
fn scalar(v: &V) -> String {
    v.python_str()
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
fn remeasure_report(report: &str) -> Vec<String> {
    let mut lines = report
        .lines()
        .filter(|line| {
            !line.starts_with("clean - and nothing here folds:")
                && !line.starts_with("not clean:")
                && !line.starts_with("re-read against the merged tree:")
                && !line.starts_with("  read again on a later day")
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
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
    let doc = capture.ordinary_document();
    let (named, current, problems) = measurement_declarations(doc, capture.hypotheses())?;
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
        let fields = map(&body)?;
        let field = fields
            .get("v")
            .or_else(|| fields.get("quoted"))
            .ok_or_else(|| Error(format!("{id} has no stored reading")))?;
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
    if differing.is_empty() {
        out.push("".into());
        out.push(if failed != 0 {
        format!("not clean: a hole - {failed} recipe{} failed, and a measurement nothing takes is a hole", if failed == 1 { "" } else { "s" })
        } else {
            "the record holds what this tree measures".into()
        });
        return Ok(output(out, i32::from(failed != 0)));
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
    let tree_proposal = PreviewHypothesis {
        name: format!("tree/{}", commit.as_deref().unwrap_or("here")),
        document: proposal_document.finite_projection()?,
        head: CV::Map(BTreeMap::from([
            (
                "claim".into(),
                CV::Text(format!(
                    "what the tree{} measures",
                    commit
                        .as_ref()
                        .map(|value| format!(" at {value}"))
                        .unwrap_or_default()
                )),
            ),
            ("born".into(), CV::Text(today.clone())),
            ("folds".into(), CV::Text("never".into())),
        ])),
    };
    let finite_document = doc.finite_projection()?;
    let finite_hypotheses = capture.hypotheses().finite_projection()?;
    let proposals = [tree_proposal];
    let preview = crate::public_consolidation::preview(&PreviewRequest {
        document: &finite_document,
        hypotheses: hmap(&finite_hypotheses)?,
        proposals: &proposals,
        context: None,
        as_of: Some(&today),
        runtime: runtime.as_ref(),
    })?;
    out.push("".into());
    out.extend(remeasure_report(&preview.report));
    out.push("".into());
    let contested = preview
        .report
        .lines()
        .any(|line| line.starts_with("contested (") && line != "contested (0)");
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
        if contested {
            out.push("    correct the recorded claim in this pull request, or run the measurement again on a later day; do not future-date this result".into());
        } else {
            out.push(format!(
                "    refresh: kpop set {id} {} --why {} --as-of {today} {}",
                shell_quote(&scalar(measured)),
                shell_quote(&measured_by(recipe, said.as_deref())),
                shell_quote(record.to_str().unwrap_or("GROUNDING.yaml"))
            ));
        }
    }
    out.push("".into());
    let code = if preview.exit_code != 0 || failed != 0 {
        1
    } else {
        0
    };
    out.push(if code != 0 {
        let mut causes = Vec::new();
        if contested {
            causes.push("a reading the tree contests");
        }
        if preview.report.contains("  FALSIFIED ") {
            causes.push("a falsifier that holds on what the tree measures");
        }
        if failed != 0 || preview.report.contains("  HOLE ") {
            causes.push("a hole");
        }
        format!(
            "not clean: {} - red until the record and the tree agree",
            causes.join(", ")
        )
    } else {
        format!(
            "the tree reads {} entr{} differently, none across a line - refresh them",
            differing.len(),
            if differing.len() == 1 { "y" } else { "ies" }
        )
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
