//! `answer` settles an open question in place; `correct` rewrites an entry nobody has
//! landed yet. Both prepare the whole new body here and write it through the ordinary
//! write path, which validates it, locks the record and answers with the reach.
use crate::{
    Result,
    history_contract::{error, map, text},
    history_yaml::{OrdinaryValue, SourceValue},
    public_authoring::{self as authoring, Amend, Options},
    public_workspace,
    reasoning_snapshot::entries,
    require,
    source_capture::{CapturedSource, ReadMode, capture_source},
    value::TypedValue as V,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    process::Command,
    sync::LazyLock,
};

#[derive(Clone, Default, clap::Args)]
pub struct AnswerOptions {
    /// The open question.
    pub question: String,
    /// The entry or judgment that answered it.
    pub by: Option<String>,
    /// Close the question with no answer, saying why it no longer matters.
    #[arg(long, conflicts_with = "by")]
    pub dropped: Option<String>,
    /// Why this answers the question.
    #[arg(long)]
    pub why: Option<String>,
    #[arg(long)]
    pub as_of: Option<String>,
}

#[derive(Clone, Default, clap::Args)]
pub struct CorrectOptions {
    /// The entry to correct.
    pub subject: String,
    /// Fields to replace, as field=value; a bare value replaces an entry that is one value.
    #[arg(allow_negative_numbers = true)]
    pub values: Vec<String>,
    /// A field the correction removes.
    #[arg(long = "unset")]
    pub unset: Vec<String>,
    /// What was wrong.
    #[arg(long)]
    pub why: Option<String>,
}

static DOTTED: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+").unwrap());
static REFS: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\{\{\s*([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+)\s*\}\}").unwrap()
});

fn s(value: &str) -> V {
    V::Text(value.into())
}

fn record_paths(cwd: &Path) -> Result<Vec<PathBuf>> {
    let original = public_workspace::records(cwd)?;
    let route = crate::project_modes::WriteRoute::capture(&original, cwd)?;
    require(route.paths().len() == 1, "choose one logical record entry")?;
    require(route.paths()[0].exists(), "record not found")?;
    Ok(route.paths().to_vec())
}

/// The record as its reader holds it: an ordinary capture, or the view of active history
/// read from its file with the history head taken off.
struct Record {
    entry: PathBuf,
    doc: V,
    capture: Option<CapturedSource>,
}

fn captured(paths: &[PathBuf], cwd: &Path) -> Result<Record> {
    let entry = paths[0].clone();
    if crate::legacy_authoring::authority_route(&entry)?
        == crate::legacy_authoring::AuthorityRoute::History
    {
        let raw = std::fs::read(&entry)?;
        let mut doc = crate::history_yaml::decode_ordinary_source_value(&raw)?.projected();
        if let V::Map(fields) = &mut doc
            && let Some(V::Map(meta)) = fields.get_mut("meta")
        {
            meta.remove("history");
        }
        return Ok(Record {
            entry,
            doc,
            capture: None,
        });
    }
    let capture = capture_source(paths, cwd, ReadMode::Frozen, None)?;
    let doc = capture.source().projected();
    Ok(Record {
        entry,
        doc,
        capture: Some(capture),
    })
}

/// The role names this record's reader inferred: dependencies, condition, snapshot.
fn roles(doc: &V) -> Result<(String, String, String)> {
    let runtime = public_workspace::runtime_for_document(doc)?;
    let reader = crate::history_authoring_reader::AuthoringReader::new(doc, runtime.as_ref())?;
    let fields = reader.fields();
    let role = |name: &str, fallback: &str| {
        fields
            .get(name)
            .and_then(|v| text(v).ok())
            .filter(|v| !v.is_empty())
            .unwrap_or(fallback)
            .to_owned()
    };
    Ok((
        role("deps", "rests_on"),
        role("predicate", "wrong_if"),
        role("snapshot", "seen"),
    ))
}

fn ordered(value: &OrdinaryValue) -> Option<SourceValue> {
    Some(match value {
        OrdinaryValue::Scalar(value) => SourceValue::Scalar(value.clone()),
        OrdinaryValue::List(values) => {
            SourceValue::List(values.iter().map(ordered).collect::<Option<_>>()?)
        }
        OrdinaryValue::Map(values) => SourceValue::Map(
            values
                .iter()
                .map(|(key, value)| Some((key.text()?.to_owned(), ordered(value)?)))
                .collect::<Option<_>>()?,
        ),
    })
}

fn today() -> String {
    chrono::Local::now().date_naive().to_string()
}

/// What an answering entry says: a judgment's verdict, or an entry's own value.
pub(crate) fn said(body: &V) -> Option<V> {
    match body {
        V::Map(m) => ["verdict", "v", "quoted"]
            .iter()
            .find_map(|k| m.get(*k).cloned()),
        V::Null => None,
        value => Some(value.clone()),
    }
}

/// A question that `answer` closed: answered by an entry, or dropped. What `answer`
/// writes sits under one key, so the question's own fields are never overwritten.
pub(crate) fn settled(question: &V) -> bool {
    map(question).is_ok_and(|m| m.contains_key("answered") || m.contains_key("dropped"))
}

/// The same, for a reader that holds the record in the ordinary value type.
pub(crate) fn settled_value(question: &crate::ordinary_value::Value) -> bool {
    crate::ordinary_value::map(question)
        .is_ok_and(|m| m.contains_key("answered") || m.contains_key("dropped"))
}

/// Why a closed question needs a person again: its answer moved after it was given.
pub(crate) enum AnswerFlag {
    Gone(String),
    Broken(String),
    Changed(String, V, V),
}

pub(crate) fn answer_flag(
    question: &V,
    raw: &BTreeMap<String, V>,
    falsified: &mut dyn FnMut(&str, &V) -> bool,
) -> Option<AnswerFlag> {
    let answered = map(map(question).ok()?.get("answered")?).ok()?;
    let by = text(answered.get("by")?).ok()?.to_owned();
    let Some(body) = raw.get(&by) else {
        return Some(AnswerFlag::Gone(by));
    };
    if falsified(&by, body) {
        return Some(AnswerFlag::Broken(by));
    }
    let was = answered.get("said")?;
    let now = said(body).unwrap_or(V::Null);
    (now != *was).then(|| AnswerFlag::Changed(by, was.clone(), now))
}

impl AnswerFlag {
    pub(crate) fn text(&self, apart: &dyn Fn(&V, &V) -> (String, String)) -> String {
        match self {
            Self::Gone(by) => format!("its answer {by} is no longer an entry"),
            Self::Broken(by) => format!("its answer {by} is broken by its own condition"),
            Self::Changed(by, was, now) => {
                let (was, now) = apart(was, now);
                format!("its answer moved - {by} now says {now}, it said {was}")
            }
        }
    }
}

/// The closed questions of a whole document whose answer moved, for readers that hold
/// the document rather than an ordinary reader.
pub(crate) fn document_flags(
    document: &V,
    falsified: &BTreeSet<String>,
) -> Vec<(String, AnswerFlag)> {
    let Ok(all) = entries(document) else {
        return vec![];
    };
    let raw = all
        .iter()
        .map(|(id, (_, body))| (id.clone(), body.clone()))
        .collect::<BTreeMap<_, _>>();
    all.iter()
        .filter(|(_, (collection, _))| ["open", "questions"].contains(&collection.as_str()))
        .filter_map(|(id, (_, question))| {
            answer_flag(question, &raw, &mut |by, _| falsified.contains(by))
                .map(|flag| (id.clone(), flag))
        })
        .collect()
}

/// How `pull` shows a closed question.
pub(crate) fn settled_line(id: &str, question: &V) -> Option<String> {
    let m = map(question).ok()?;
    let py = crate::source_text::ordinary_python_str;
    let asked = ["question", "v", "name"]
        .iter()
        .find_map(|k| m.get(*k))
        .map(py)
        .unwrap_or_else(|| id.to_owned());
    let closed = |c: &crate::history_contract::Map| {
        let of = c
            .get("of")
            .map(|v| format!(" on {}", py(v)))
            .unwrap_or_default();
        let why = c
            .get("because")
            .map(|v| format!(" - {}", py(v)))
            .unwrap_or_default();
        format!("{of}{why}")
    };
    if let Some(answered) = m.get("answered").and_then(|v| map(v).ok()) {
        let by = answered.get("by").map(py).unwrap_or_default();
        Some(format!(
            "{id}: {asked} - answered by {by}{}",
            closed(answered)
        ))
    } else {
        m.get("dropped")
            .and_then(|v| map(v).ok())
            .map(|dropped| format!("{id}: {asked} - dropped{}", closed(dropped)))
    }
}

pub fn answer(options: &AnswerOptions, cwd: &Path) -> Result<String> {
    require(
        options.by.is_some() != options.dropped.is_some(),
        "answer takes the entry that answered the question, or --dropped with the reason it no longer matters",
    )?;
    require(
        options
            .dropped
            .as_ref()
            .is_none_or(|why| !why.trim().is_empty()),
        "--dropped needs the reason the question no longer matters",
    )?;
    require(
        options.dropped.is_none() || options.why.is_none(),
        "--dropped carries the reason; --why goes with an answer",
    )?;
    let cwd = cwd.canonicalize()?;
    let paths = record_paths(&cwd)?;
    let record = captured(&paths, &cwd)?;
    let doc = &record.doc;
    let place = entries(doc)?;
    let id = &options.question;
    let (collection, old) = place
        .get(id)
        .ok_or_else(|| error(&format!("refused - {id} is not an entry of the record")))?;
    let source_order = record.capture.as_ref().and_then(|capture| {
        capture
            .source()
            .get(collection)
            .and_then(|members| members.get(id))
            .and_then(ordered)
    });
    let mut order: Vec<(String, SourceValue)> = match (old, source_order) {
        (V::Text(question), _) => vec![("question".into(), SourceValue::Scalar(s(question)))],
        (V::Map(_), Some(SourceValue::Map(fields))) => fields,
        (V::Map(_), _) => match SourceValue::from_typed(old) {
            SourceValue::Map(fields) => fields,
            _ => unreachable!(),
        },
        _ => {
            return Err(error(&format!(
                "refused - {id} is not a question this reader can close"
            )));
        }
    };
    let day = SourceValue::Scalar(s(&options.as_of.clone().unwrap_or_else(today)));
    let mut closing: Vec<(String, SourceValue)> = vec![];
    if let Some(by) = &options.by {
        closing.push(("by".into(), SourceValue::Scalar(s(by))));
        if let Some((_, answering)) = place.get(by)
            && let Some(said) = said(answering)
        {
            closing.push(("said".into(), SourceValue::from_typed(&said)));
        }
        closing.push(("of".into(), day));
        if let Some(why) = options.why.as_ref().filter(|why| !why.trim().is_empty()) {
            closing.push(("because".into(), SourceValue::Scalar(s(why.trim()))));
        }
        order.push(("answered".into(), SourceValue::Map(closing)));
    } else if let Some(dropped) = &options.dropped {
        closing.push(("of".into(), day));
        closing.push(("because".into(), SourceValue::Scalar(s(dropped.trim()))));
        order.push(("dropped".into(), SourceValue::Map(closing)));
    }
    let body = V::Map(order.iter().map(|(k, v)| (k.clone(), v.typed())).collect());
    let write = Options {
        subject: id.clone(),
        why: options.why.clone().or_else(|| options.dropped.clone()),
        as_of: options.as_of.clone(),
        amend: Some(Amend {
            kind: "answer",
            body,
            order: Some(SourceValue::Map(order)),
            by: options.by.clone(),
        }),
        ..Options::default()
    };
    authoring::run("add", &write, &cwd)
}

pub fn correct(options: &CorrectOptions, cwd: &Path) -> Result<String> {
    let cwd = cwd.canonicalize()?;
    let paths = record_paths(&cwd)?;
    let record = captured(&paths, &cwd)?;
    let place = entries(&record.doc)?;
    let id = &options.subject;
    let (_, old) = place
        .get(id)
        .ok_or_else(|| error(&format!("refused - {id} is not an entry of the record")))?;
    let (_, _, snapshot) = roles(&record.doc)?;
    // An entry that is one value takes one value, even text with an equals sign in it.
    let bare = options.values.len() == 1
        && (!options.values[0].contains('=') || !matches!(old, V::Map(_)));
    let body = if bare {
        require(
            !matches!(old, V::Map(_)) && options.unset.is_empty(),
            &format!("refused - {id} holds fields: correct it as field=value"),
        )?;
        authoring::typed(&options.values[0])?
    } else {
        let mut body = match old {
            V::Map(m) => m.clone(),
            _ => {
                return Err(error(&format!(
                    "refused - {id} is one value: correct {id} <value>"
                )));
            }
        };
        for value in &options.values {
            let (field, value) = value
                .split_once('=')
                .ok_or_else(|| error("fields are written field=value"))?;
            let field = field.trim();
            require(!field.is_empty(), "fields are written field=value")?;
            require(
                field != snapshot,
                &format!(
                    "refused - {snapshot} is written by this tool, from what the dependencies hold - leave it out"
                ),
            )?;
            let value =
                if value.starts_with('[') || value.starts_with('{') && !value.starts_with("{{") {
                    authoring::yaml(value)?.typed()
                } else {
                    authoring::typed(value)?
                };
            body.insert(field.into(), value);
        }
        for field in &options.unset {
            require(
                body.remove(field.trim()).is_some(),
                &format!("refused - {id} has no field {}", field.trim()),
            )?;
        }
        // The snapshot is the tool's: a corrected judgment has it taken again.
        body.remove(&snapshot);
        V::Map(body)
    };
    let mut unchanged = old.clone();
    if let V::Map(m) = &mut unchanged {
        m.remove(&snapshot);
    }
    require(
        body.digest()? != unchanged.digest()?,
        &format!("{id} already says that; nothing to correct"),
    )?;
    let write = Options {
        subject: id.clone(),
        why: options.why.clone(),
        amend: Some(Amend {
            kind: "correct",
            body,
            order: None,
            by: None,
        }),
        ..Options::default()
    };
    authoring::run("add", &write, &cwd)
}

fn mentions(body: &V, deps: &str, predicate: &str, target: &str) -> bool {
    let Ok(m) = map(body) else {
        return false;
    };
    if let Some(V::List(items)) = m.get(deps)
        && items.iter().any(|v| text(v).is_ok_and(|t| t == target))
    {
        return true;
    }
    // A reading cites its source; an answered question rests on its answer.
    let answered = m
        .get("answered")
        .and_then(|v| map(v).ok())
        .and_then(|a| a.get("by"));
    if [m.get("from"), answered]
        .into_iter()
        .flatten()
        .any(|v| text(v).is_ok_and(|t| t == target))
    {
        return true;
    }
    for key in [predicate, "rule"] {
        let reads = match m.get(key) {
            Some(value @ V::Map(_)) => crate::reasoning_language::lower(value)
                .map(|tree| crate::reasoning_language::references(&tree))
                .is_ok_and(|refs| refs.iter().any(|r| r == target)),
            Some(V::Text(t)) => DOTTED.find_iter(t).any(|m| m.as_str() == target),
            _ => false,
        };
        if reads {
            return true;
        }
    }
    m.values()
        .any(|v| matches!(v, V::Text(t) if REFS.captures_iter(t).any(|c| &c[1] == target)))
}

/// Everything that rests on `subject`, directly or through what rests on it.
fn dependents(
    place: &BTreeMap<String, (String, V)>,
    deps: &str,
    predicate: &str,
    subject: &str,
) -> BTreeSet<String> {
    let mut reach = BTreeSet::new();
    let mut frontier = vec![subject.to_owned()];
    while let Some(target) = frontier.pop() {
        for (id, (_, body)) in place {
            if id != subject && !reach.contains(id) && mentions(body, deps, predicate, &target) {
                reach.insert(id.clone());
                frontier.push(id.clone());
            }
        }
    }
    reach
}

fn git(dir: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

/// Ids of the record already in the committed tree, or `None` when no work tree holds it.
fn committed(held: &Record, record: &BTreeSet<String>) -> Result<Option<BTreeSet<String>>> {
    let entry = &held.entry;
    let dir = entry.parent().ok_or_else(|| error("invalid_path"))?;
    let Some(top) = git(dir, &["rev-parse", "--show-toplevel"]) else {
        return Ok(None);
    };
    let top = PathBuf::from(String::from_utf8_lossy(&top).trim()).canonicalize()?;
    if entry.canonicalize()?.strip_prefix(&top).is_err() {
        return Ok(None);
    }
    let mut found = BTreeSet::new();
    let view;
    let files: Vec<(&PathBuf, &Vec<u8>)> = match &held.capture {
        Some(capture) => capture.inventory().files.iter().collect(),
        None => {
            view = (entry.clone(), std::fs::read(entry)?);
            vec![(&view.0, &view.1)]
        }
    };
    for (path, raw) in files {
        let Ok(path) = path.canonicalize() else {
            continue;
        };
        let Ok(relative) = path.strip_prefix(&top) else {
            continue;
        };
        // A file of the record is one whose working copy holds ids of the record.
        let holds = crate::history_yaml::decode_ordinary_source_value(raw)
            .ok()
            .and_then(|v| entries(&v.projected()).ok())
            .is_some_and(|ids| ids.keys().any(|id| record.contains(id)));
        if !holds {
            continue;
        }
        let name = relative.to_string_lossy().replace('\\', "/");
        let Some(head) = git(&top, &["show", &format!("HEAD:{name}")]) else {
            continue;
        };
        if let Ok(value) = crate::history_yaml::decode_ordinary_source_value(&head)
            && let Ok(ids) = entries(&value.projected())
        {
            found.extend(ids.into_keys());
        }
    }
    Ok(Some(found))
}

fn session() -> Option<String> {
    std::env::var("KPOPPER_AGENT_SESSION")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::env::var("CODEX_THREAD_ID")
                .ok()
                .filter(|s| !s.is_empty())
        })
}

fn listed(ids: &[String]) -> String {
    match ids {
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
        [] => String::new(),
    }
}

/// A correction goes through without a person only while the entry, and everything
/// resting on it, is unlanded: absent from the committed tree, or outside git written
/// by this session. What rests on it is then flagged, never rewritten.
pub(crate) fn unlanded_or_refuse(paths: &[PathBuf], cwd: &Path, subject: &str) -> Result<()> {
    let held = captured(paths, cwd)?;
    let place = entries(&held.doc)?;
    require(
        place.contains_key(subject),
        &format!("refused - {subject} is not an entry of the record"),
    )?;
    let (deps, predicate, _) = roles(&held.doc)?;
    let mut ids = dependents(&place, &deps, &predicate, subject);
    ids.insert(subject.to_owned());
    let record = place.keys().cloned().collect::<BTreeSet<_>>();
    let route = format!(
        "write the corrected entry as a hypothesis (add {subject} ... --hypothesis NAME) and let a person take it at the fold"
    );
    let landed = match committed(&held, &record)? {
        Some(found) => ids.intersection(&found).cloned().collect::<Vec<_>>(),
        None => {
            let Some(sid) = session() else {
                return Err(error(&format!(
                    "refused - {subject} is outside git, where only the session that wrote it can correct it, and this command carries no session identity - {route}"
                )));
            };
            let tmp = crate::session_activity::temporary_directory();
            let owned = match &held.capture {
                Some(capture) => crate::session_activity::owned(&tmp, &sid, capture),
                None => crate::session_activity::owned_in_file(&tmp, &sid, &held.entry),
            };
            let foreign = ids
                .iter()
                .filter(|id| !owned.contains(*id))
                .cloned()
                .collect::<Vec<_>>();
            if !foreign.is_empty() {
                return Err(error(&format!(
                    "refused - {} {} not written by this session - a correction of work someone else may have read is a decision for a person: {route}",
                    listed(&foreign),
                    if foreign.len() == 1 { "was" } else { "were" }
                )));
            }
            vec![]
        }
    };
    if landed.iter().any(|id| id == subject) {
        return Err(error(&format!(
            "refused - {subject} is already in a commit - a correction of work others may have read is a decision for a person: {route}"
        )));
    }
    if !landed.is_empty() {
        return Err(error(&format!(
            "refused - {} rest{} on {subject} and {} already in a commit - a correction of work others may have read is a decision for a person: {route}",
            listed(&landed),
            if landed.len() == 1 { "s" } else { "" },
            if landed.len() == 1 { "is" } else { "are" },
        )));
    }
    Ok(())
}
