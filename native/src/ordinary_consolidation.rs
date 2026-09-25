//! Local hypothesis consolidation under the ordinary reader's interpretation.
//! Preview and publication share one captured candidate; no replacement bypasses admission.
use super::{CommandOutput, Options};
use crate::public_identity::ordinary_sameness::near;
use crate::reasoning_authoring::{named, same};
use crate::source_clock::python_equal;
use crate::source_text::ordinary_python_str as py;
use crate::{
    Result,
    history_authoring::{obj, s},
    history_contract::{Map, error, map, text},
    history_view::{map_mut, truth},
    history_yaml::{OrdinaryValue as O, SourceValue as Source},
    ordinary_reader::{self as R, Reader},
    project_modes::WriteRoute,
    public_ordinary_readers::{Projection, World, predicate_text, short},
    reasoning_authoring_guards as G,
    reasoning_runtime::Runtime,
    require,
    source_capture::{CapturedSource, ReadMode},
    source_inventory::Inventory,
    value::TypedValue as V,
};
use crate::{ordinary_counts as C, reasoning_fields as F, reasoning_language as L};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
#[path = "ordinary_consolidation_edit.rs"]
mod edit;
mod report {
    include!("ordinary_consolidation_report.rs");
    include!("ordinary_consolidation_fold_report.rs");
}

include!("ordinary_consolidation_engine.rs");
#[derive(Clone)]
struct Hypothesis {
    name: String,
    path: Option<PathBuf>,
    doc: V,
    head: V,
    raw: Map,
    ids: BTreeSet<String>,
    source: O,
    text: String,
    source_record: V,
    /// Another branch's whole record, when only what it holds differently is laid: the
    /// permissions its unchanged entries carry are still read from it.
    whole: Option<V>,
}
pub(crate) struct SuppliedHypothesis {
    pub name: String,
    pub document: V,
    /// `document` in the order its file holds it; a finite value keeps names in name order.
    pub ordered: crate::ordinary_value::Value,
    pub head: V,
    pub source: O,
    pub text: String,
    pub source_record: V,
    /// Whole committed record retained for reading permissions when the proposal is pre-diffed.
    pub whole_document: Option<V>,
    /// Git merge-base record for this committed branch, when it is available.
    pub comparison_base: Option<V>,
    /// Another branch's committed record, laid over the base as what it holds differently.
    pub differences_only: bool,
}
impl Hypothesis {
    fn path(&self) -> Result<&PathBuf> {
        self.path.as_ref().ok_or_else(|| {
            error("a supplied preview cannot be published without captured source files")
        })
    }
}
fn branch_judgment_body(body: &Map) -> bool {
    [
        "verdict",
        "title",
        "wrong_if",
        "reopened_by",
        "rests_on",
        "depends",
        "because",
        "request",
    ]
    .iter()
    .any(|key| body.contains_key(*key))
}
fn branch_role_evidence(document: &V) -> Result<V> {
    let mut evidence = document.clone();
    for (name, collection) in map_mut(&mut evidence)?.iter_mut() {
        if [
            "meta",
            "schema",
            "record",
            "also",
            "sources",
            "open",
            "questions",
        ]
        .contains(&name.as_str())
        {
            continue;
        }
        let Ok(bodies) = map_mut(collection) else {
            continue;
        };
        for body in bodies.values_mut() {
            let Ok(fields) = map_mut(body) else { continue };
            if !branch_judgment_body(fields) {
                fields.retain(|_, value| !matches!(value, V::List(_)));
            }
        }
    }
    Ok(evidence)
}
fn branch_role_order(document: &crate::ordinary_value::Value) -> crate::ordinary_value::Value {
    use crate::ordinary_value::{Value as OV, map_mut as omap_mut};
    let mut evidence = document.clone();
    let Ok(collections) = omap_mut(&mut evidence) else {
        return evidence;
    };
    for (name, collection) in collections.iter_mut() {
        if [
            "meta",
            "schema",
            "record",
            "also",
            "sources",
            "open",
            "questions",
        ]
        .contains(&name.as_str())
        {
            continue;
        }
        let Ok(bodies) = omap_mut(collection) else {
            continue;
        };
        for body in bodies.values_mut() {
            let Ok(fields) = omap_mut(body) else { continue };
            let judgment = [
                "verdict",
                "title",
                "wrong_if",
                "reopened_by",
                "rests_on",
                "depends",
                "because",
                "request",
            ]
            .iter()
            .any(|key| fields.contains_key(*key));
            if !judgment {
                fields.retain(|_, value| !matches!(value, OV::List(_)));
            }
        }
    }
    evidence
}

fn branch_duplicate_is_an_authored_kind_change(document: &V, basis: &V) -> Result<bool> {
    let mut source_ids = BTreeMap::<String, Vec<(String, V)>>::new();
    for (collection, members) in F::collections(document)? {
        for (id, body) in members {
            source_ids
                .entry(id)
                .or_default()
                .push((collection.clone(), body.clone()));
        }
    }
    let mut basis_ids = BTreeMap::<String, Vec<(String, V)>>::new();
    for (collection, members) in F::collections(basis)? {
        for (id, body) in members {
            basis_ids
                .entry(id)
                .or_default()
                .push((collection.clone(), body.clone()));
        }
    }
    for (id, bodies) in source_ids.iter().filter(|(_, bodies)| bodies.len() > 1) {
        let Some([(collection, original)]) = basis_ids.get(id).map(Vec::as_slice) else {
            return Ok(false);
        };
        if !bodies.iter().any(|(source_collection, body)| {
            source_collection == collection && python_equal(body, original)
        }) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn captured_projection<'a>(
    capture: &CapturedSource,
    runtime: Option<&'a Runtime>,
) -> Result<Projection<'a>> {
    let context = capture.ordinary_context();
    Projection::new(
        capture.ordinary_document(),
        map(capture.hypotheses())?,
        map(get(&context, "conflicts"))?,
        capture.reader_lines()?,
        runtime,
    )
}
fn read_hypotheses(
    capture: &CapturedSource,
    requested: &[String],
    supplied: &[SuppliedHypothesis],
    base: Option<&World<'_>>,
    runtime: Option<&Runtime>,
) -> Result<Vec<Hypothesis>> {
    let all = map(capture.hypotheses())?;
    let mut pool = BTreeMap::new();
    for (name, h) in all {
        if get(h, "kind") == &s("contribution") {
            continue;
        }
        require(
            !truth(get(h, "error")),
            &format!(
                "refused - hypothesis {name} could not be read: {}",
                py(get(h, "error"))
            ),
        )?;
        let doc = get(h, "doc");
        let merged = layer(capture.ordinary_document(), doc)?;
        Reader::new(&merged, runtime).map_err(|e| {
            error(&format!(
                "refused - hypothesis {name} cannot be read over the base: {}",
                crate::ordinary_semantics::layer_failure(&e)
            ))
        })?;
        let path = PathBuf::from(text(get(h, "path"))?);
        let raw = capture
            .files()
            .get(&path)
            .ok_or_else(|| error("hypothesis_source_unavailable"))?;
        let source = crate::history_yaml::decode_ordinary_source_value(raw)?;
        let body = entries(doc)?;
        let ids = body.keys().cloned().collect();
        pool.insert(
            name.clone(),
            Hypothesis {
                name: name.clone(),
                path: Some(path),
                doc: doc.clone(),
                head: get(h, "head").clone(),
                raw: body,
                ids,
                source,
                text: std::str::from_utf8(raw)
                    .map_err(|_| error("invalid_utf8"))?
                    .into(),
                source_record: V::Map(Map::new()),
                whole: None,
            },
        );
    }
    for h in supplied {
        require(
            !pool.contains_key(&h.name),
            &format!(
                "refused - {} names both a hypothesis beside the record and what {} holds; rename the file, or consolidate them one at a time",
                h.name,
                h.name.split(':').next().unwrap_or(&h.name)
            ),
        )?;
        if h.differences_only {
            // A source delta cannot stand in for validating the branch's whole record:
            // its schema and capability boundary remain authoritative.
            let basis = h
                .comparison_base
                .as_ref()
                .and_then(|value| map(value).ok().and_then(|m| m.get("doc")))
                .or(h.comparison_base.as_ref())
                .ok_or_else(|| error("branch_comparison_base_missing"))?;
            if let Err(failure) = crate::reasoning_snapshot::entries(&h.document) {
                if failure.0 != "duplicate_entry"
                    || !branch_duplicate_is_an_authored_kind_change(&h.document, basis)?
                {
                    return Err(failure);
                }
            }
            let source = crate::ordinary_value::Value::from_typed(&h.source_record);
            crate::source_capture::require_ordinary(
                &source,
                &crate::ordinary_value::Value::Map(crate::ordinary_value::Map::new()),
            )?;
        }
        let (doc, whole) = match base {
            Some(base) if h.differences_only => (
                branch_differences_from(&h.document, base, h.comparison_base.as_ref())?,
                Some(h.document.clone()),
            ),
            _ => (h.document.clone(), h.whole_document.clone()),
        };
        if h.differences_only {
            let delta_entries = entries(&doc)?;
            let (validation_doc, source_order) = if delta_entries.is_empty() {
                (&h.document, std::borrow::Cow::Borrowed(&h.ordered))
            } else {
                (
                    &doc,
                    std::borrow::Cow::Owned(kept_in_order(&h.ordered, &doc)),
                )
            };
            let role_evidence = branch_role_evidence(validation_doc)?;
            let merged = layer(capture.ordinary_document(), &role_evidence)?;
            let source_order = branch_role_order(&source_order);
            let source_layer = [std::borrow::Cow::Owned(source_order)];
            Reader::new(&merged, runtime)
                .map_err(|failure| in_union_order(capture, &source_layer, failure))?;
        }
        let mut head = h.head.clone();
        if let Some(comparison_base) = &h.comparison_base {
            map_mut(&mut head)?.insert("_comparison_base".into(), comparison_base.clone());
        }
        // An id the branch still holds in two collections has no one body to fold.
        crate::reasoning_snapshot::entries(&doc)?;
        let raw = entries(&doc)?;
        pool.insert(
            h.name.clone(),
            Hypothesis {
                name: h.name.clone(),
                path: None,
                doc,
                head,
                ids: raw.keys().cloned().collect(),
                raw,
                source: h.source.clone(),
                text: h.text.clone(),
                source_record: h.source_record.clone(),
                whole,
            },
        );
    }
    let missing = requested
        .iter()
        .filter(|n| !pool.contains_key(*n))
        .cloned()
        .collect::<Vec<_>>();
    require(
        missing.is_empty(),
        &format!(
            "refused - no hypothesis named {} beside the record (there: {})",
            missing.join(", "),
            if pool.is_empty() {
                "none".into()
            } else {
                pool.keys().cloned().collect::<Vec<_>>().join(", ")
            }
        ),
    )?;
    if requested.is_empty() {
        Ok(pool.into_values().collect())
    } else {
        Ok(requested
            .iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|n| pool.remove(n).unwrap())
            .collect())
    }
}

/// Each hypothesis's document in the order its file holds it, in the order the union lays
/// them.
fn in_file_order<'a>(
    hyps: &[Hypothesis],
    supplied: &'a [SuppliedHypothesis],
) -> Vec<std::borrow::Cow<'a, crate::ordinary_value::Value>> {
    hyps.iter()
        .map(|h| match supplied.iter().find(|s| s.name == h.name) {
            Some(s) if h.whole.is_some() => {
                std::borrow::Cow::Owned(kept_in_order(&s.ordered, &h.doc))
            }
            Some(s) => std::borrow::Cow::Borrowed(&s.ordered),
            None => {
                let mut doc = crate::ordinary_source::Source::from_finite(&h.source).projected();
                if let crate::ordinary_value::Value::Map(body) = &mut doc {
                    body.remove("hypothesis");
                }
                std::borrow::Cow::Owned(doc)
            }
        })
        .collect()
}
/// A branch's record in the order its file holds it, cut to what is laid from it: each
/// collection keeps the ids the laid document holds there.
fn kept_in_order(ordered: &crate::ordinary_value::Value, laid: &V) -> crate::ordinary_value::Value {
    use crate::ordinary_value::Value as Ordered;
    let mut kept = ordered.clone();
    if let (Ordered::Map(collections), Ok(laid)) = (&mut kept, map(laid)) {
        collections.retain(|name, members| match (members, laid.get(name)) {
            (Ordered::Map(members), Some(V::Map(held))) => {
                members.retain(|id, _| held.contains_key(id));
                !members.is_empty()
            }
            _ => false,
        });
    }
    kept
}
/// Why the union's field roles cannot be read, told over the union laid in the order its
/// records hold it - the base's own, then what each hypothesis adds - as the Python reader
/// lays and reads it; the finite union keeps ids and fields in name order.
fn in_union_order(
    capture: &CapturedSource,
    layers: &[std::borrow::Cow<'_, crate::ordinary_value::Value>],
    failure: crate::Error,
) -> crate::Error {
    if !crate::ordinary_fields::explains_unreadable(&failure) {
        return failure;
    }
    let mut union = crate::ordinary_source::Source::from_finite(capture.source()).projected();
    for layer in layers {
        match laid_in_order(&union, layer) {
            Ok(laid) => union = laid,
            Err(_) => return failure,
        }
    }
    crate::ordinary_fields::in_order_of(&union, failure)
}
/// `layer` over documents kept in their records' order: the hypothesis's collections are laid
/// in the order its file gives them, each id taking the place the base kept it in, and a new
/// one following what the collection already holds.
fn laid_in_order(
    base: &crate::ordinary_value::Value,
    hypothesis: &crate::ordinary_value::Value,
) -> Result<crate::ordinary_value::Value> {
    use crate::ordinary_value::{Map as Members, Value as Ordered, map as members, map_mut};
    let collections = crate::ordinary_fields::collections(hypothesis)?;
    let mut out = base.clone();
    for name in members(hypothesis)?.keys() {
        let Some(laid) = collections.get(name) else {
            continue;
        };
        for (other, held) in map_mut(&mut out)?.iter_mut() {
            if other != name
                && let Ordered::Map(held) = held
            {
                for id in laid.keys() {
                    held.remove(id);
                }
            }
        }
        let target = map_mut(&mut out)?
            .entry(name.clone())
            .or_insert_with(|| Ordered::Map(Members::new()));
        if !matches!(target, Ordered::Map(_)) {
            *target = Ordered::Map(Members::new());
        }
        let target = map_mut(target)?;
        for (id, body) in laid.iter() {
            target.insert(id.clone(), body.clone());
        }
    }
    Ok(out)
}

fn text_values<'a>(v: &'a V, out: &mut Vec<&'a str>) {
    match v {
        V::Text(s) => out.push(s),
        V::List(a) => {
            for v in a {
                text_values(v, out)
            }
        }
        V::Map(m) => {
            for v in m.values() {
                text_values(v, out)
            }
        }
        _ => {}
    }
}
fn metadata(document: &V) -> V {
    map(document)
        .ok()
        .and_then(|m| m.get("meta"))
        .cloned()
        .unwrap_or_else(|| V::Map(Map::new()))
}
fn privacy(
    capture: &CapturedSource,
    route: &WriteRoute,
    hyps: &[Hypothesis],
    action: &V,
    extra: &[String],
) -> Result<()> {
    let base = capture.ordinary_document();
    let mut combined = base.clone();
    let templates =
        regex::Regex::new(r"\{\{\s*([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+)\s*\}\}").unwrap();
    for h in hyps {
        combined = layer(&combined, h.whole.as_ref().unwrap_or(&h.doc))?;
    }
    let original_entries = entries(base)?;
    for h in hyps {
        let context = layer(&combined, h.whole.as_ref().unwrap_or(&h.doc))?;
        let record_metadata = if h.path.is_none() && h.source_record != V::Map(Map::new()) {
            metadata(&h.source_record)
        } else {
            metadata(&context)
        };
        let heads = obj([
            ("hypothesis", h.head.clone()),
            ("record", record_metadata.clone()),
            ("original_record", metadata(base)),
            ("source_record", h.source_record.clone()),
        ]);
        let present = entries(&context)?;
        let mut roots = h
            .ids
            .iter()
            .cloned()
            .chain(extra.iter().cloned())
            .collect::<BTreeSet<_>>();
        let mut texts = vec![];
        text_values(&heads, &mut texts);
        for value in texts {
            for capture in templates.captures_iter(value) {
                if !crate::reasoning_fields::BUILTINS.contains(&&capture[1]) {
                    roots.insert(capture[1].into());
                }
            }
            if present.contains_key(value) {
                roots.insert(value.into());
            }
            for id in R::ID.find_iter(value).map(|m| m.as_str()) {
                if present.contains_key(id) {
                    roots.insert(id.into());
                }
            }
        }
        let selected = if roots.is_empty() {
            V::Map(Map::new())
        } else {
            crate::pending_bundle::closure(&context,&roots.iter().cloned().collect::<Vec<_>>()).map_err(|_|error("refused - hypothesis source closure could not be checked; original retained"))?
        };
        let original_roots = roots
            .into_iter()
            .chain(entries(&selected)?.into_keys())
            .filter(|id| original_entries.contains_key(id))
            .collect::<BTreeSet<_>>();
        let original = if original_roots.is_empty() {
            V::Map(Map::new())
        } else {
            crate::pending_bundle::closure(base,&original_roots.into_iter().collect::<Vec<_>>()).map_err(|_|error("refused - hypothesis source closure could not be checked; original retained"))?
        };
        if [&heads, &h.doc, &selected, &original]
            .iter()
            .any(|v| crate::recording_privacy::private_marker(v))
        {
            let mut retained = map(&selected)?.clone();
            retained.extend(Map::from([
                ("hypothesis".into(), h.head.clone()),
                ("source_record_metadata".into(), h.source_record.clone()),
                ("record_metadata".into(), record_metadata),
                ("original_record_metadata".into(), metadata(base)),
                ("original_record_closure".into(), original),
            ]));
            let mut intent = map(action)?.clone();
            intent.insert("hypothesis".into(), s(&h.name));
            let receipt = crate::recording_privacy::draft(
                route.project(),
                &V::Map(intent),
                &V::Map(retained),
                "private hypothesis or source permission; original retained without publication",
            )?;
            return Err(error(&format!(
                "private draft retained at {}; original hypothesis retained",
                text(get(&receipt, "path"))?
            )));
        }
    }
    Ok(())
}
fn one_hypothesis(capture: &CapturedSource, name: &str) -> Result<Hypothesis> {
    let h = map(capture.hypotheses())?.get(name).ok_or_else(|| {
        error(&format!(
            "refused - no hypothesis named {name} beside the record"
        ))
    })?;
    require(
        !truth(get(h, "error")),
        &format!(
            "refused - hypothesis {name} could not be read: {}",
            py(get(h, "error"))
        ),
    )?;
    let path = PathBuf::from(text(get(h, "path"))?);
    let raw = capture
        .files()
        .get(&path)
        .ok_or_else(|| error("hypothesis_source_unavailable"))?;
    let doc = get(h, "doc").clone();
    let body = entries(&doc)?;
    Ok(Hypothesis {
        name: name.into(),
        path: Some(path),
        doc,
        head: get(h, "head").clone(),
        ids: body.keys().cloned().collect(),
        raw: body,
        source: crate::history_yaml::decode_ordinary_source_value(raw)?,
        text: std::str::from_utf8(raw)
            .map_err(|_| error("invalid_utf8"))?
            .into(),
        source_record: V::Map(Map::new()),
        whole: None,
    })
}
pub(super) fn run(
    options: &Options,
    route: &WriteRoute,
    runtime_override: Option<&Runtime>,
    probe: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<CommandOutput> {
    run_supplied(options, route, &[], runtime_override, probe)
}
fn require_committed_branch_base(
    capture: &CapturedSource,
    route: &WriteRoute,
    replaced: &Path,
) -> Result<()> {
    let root = &route.project().root;
    let mut relative = Vec::new();
    for path in capture
        .members()
        .iter()
        .map(PathBuf::as_path)
        .chain(std::iter::once(replaced))
    {
        if !path.exists() {
            continue;
        }
        let path = crate::project_modes::resolved(path)?;
        let item = path
            .strip_prefix(root)
            .map_err(|_| error("branch_target_outside_checkout"))?;
        relative.push(item.to_string_lossy().replace('\\', "/"));
    }
    // Ordinary folds follow Git's normal ignore rules, as the legacy reader does.
    // Query each captured member so rename records cannot become extra filenames
    // in the diagnostic. History adoption has a separate, stricter guard.
    let mut names = Vec::new();
    for path in &relative {
        let args = ["--literal-pathspecs", "status", "--porcelain", "--", path];
        let dirty = crate::public_readers::branch_read::git(root, &args, false)?
            .ok_or_else(|| error("branch_git_unavailable"))?;
        if !dirty.iter().all(u8::is_ascii_whitespace) {
            names.push(path.clone());
        }
    }
    if !names.is_empty() {
        let named = names.join(", ");
        return Err(error(&format!(
            "refused - {named} {} uncommitted changes: a branch's record folds onto a committed base, so the fold is a commit of its own - commit first, then consolidate again",
            if names.len() == 1 { "carries" } else { "carry" }
        )));
    }
    Ok(())
}
pub(crate) fn run_supplied(
    options: &Options,
    route: &WriteRoute,
    supplied: &[SuppliedHypothesis],
    runtime_override: Option<&Runtime>,
    probe: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<CommandOutput> {
    let entry = &route.paths()[0];
    let layout = crate::history_transaction::Layout::for_entry(
        entry
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| error("invalid_path"))?,
    )?;
    require(
        crate::history_transaction_fs::read(&entry.parent().unwrap().join(&layout.journal))?
            .is_none(),
        "recovery_required",
    )?;
    let loaded_runtime;
    let runtime = if runtime_override.is_some() {
        runtime_override
    } else {
        loaded_runtime = crate::public_workspace::runtime()?;
        loaded_runtime.as_ref()
    };
    let capture = crate::source_capture::capture_source_with_runtime(
        route.paths(),
        &route.project().root,
        ReadMode::Live,
        None,
        runtime,
    )?;
    if get(
        &crate::reasoning_fields::capabilities(capture.ordinary_document(), None)?,
        "profile",
    ) == &s("core/v1")
    {
        let hypotheses = crate::ordinary_value::Value::from_typed(capture.hypotheses());
        if let Some(name) = &options.refute {
            require(
                crate::ordinary_value::map(&hypotheses)?.contains_key(name),
                &format!("refused - no hypothesis named {name} beside the record"),
            )?;
        }
        require(
            options.refute.is_none() && supplied.is_empty(),
            crate::source_capture::CORE_CONSUMER,
        )?;
        let output = super::core_record(&hypotheses, &options.names)?;
        capture.verify()?;
        route.verify()?;
        return Ok(output);
    }
    capture.require_ordinary_reader()?;
    if !options.dry_run {
        capture.snapshot().map_err(|e| {
            if e.0 == "invalid_yaml_key" {
                error("invalid_history_value: snapshot mappings require string keys")
            } else {
                e
            }
        })?;
    }
    let stamp = options
        .as_of
        .clone()
        .unwrap_or_else(|| chrono::Local::now().date_naive().to_string());
    let mut inventory = Inventory::default();
    let side = edit::ancillary(entry, &mut inventory)?;
    if !options.dry_run && !supplied.is_empty() {
        require_committed_branch_base(
            &capture,
            route,
            &entry.parent().unwrap().join(&layout.replaced),
        )?;
    }
    if let Some(name) = &options.refute {
        let write = edit::WriteContext {
            capture: &capture,
            route,
            side: &side,
            inventory: &inventory,
            stamp: &stamp,
            runtime,
            page_capture: None,
        };

        let h = one_hypothesis(&capture, name)?;
        let out = edit::refute(
            &write,
            &h,
            options.why.as_deref().unwrap_or(""),
            options.source.as_deref(),
            probe,
        )?;
        return Ok(CommandOutput {
            stdout: out,
            stderr: String::new(),
            code: 0,
        });
    }
    // Another branch's record is laid as what it holds differently from this base - the
    // same base the union reads.
    let base = if supplied.iter().any(|h| h.differences_only) {
        Some(captured_projection(&capture, runtime)?)
    } else {
        None
    };
    let hyps = read_hypotheses(
        &capture,
        &options.names,
        supplied,
        base.as_ref().map(|base| &base.base),
        runtime,
    )?;
    if !options.dry_run {
        privacy(
            &capture,
            route,
            &hyps,
            &obj([("kind", s("consolidate"))]),
            &[],
        )?;
    }
    if hyps.is_empty() {
        capture.verify()?;
        route.verify()?;
        return Ok(CommandOutput {
            stdout: "no hypotheses beside the record - nothing to consolidate\n".into(),
            stderr: String::new(),
            code: 0,
        });
    }
    let drops = map(&super::drops(&options.drops)?)?.clone();
    let layers = in_file_order(&hyps, supplied);
    let page_capture = edit::page(&capture, &hyps, route, &inventory, runtime)
        .map_err(|e| in_union_order(&capture, &layers, e))?;
    let page = page_capture.as_ref().map(|capture| &capture.facts);
    let write = edit::WriteContext {
        capture: &capture,
        route,
        side: &side,
        inventory: &inventory,
        stamp: &stamp,
        runtime,
        page_capture: page_capture.as_ref(),
    };
    let c = union(UnionInput {
        doc: capture.ordinary_document(),
        all: map(capture.hypotheses())?,
        hyps,
        base: match base {
            Some(base) => base,
            None => captured_projection(&capture, runtime)?,
        },
        runtime,
        stamp: &stamp,
        take: &options.take,
        drops: &drops,
        brief: Some(&side.brief.projected()),
        page,
    })
    .map_err(|e| in_union_order(&capture, &layers, e))?;
    let mut output = report::lines(&c, chrono::Local::now().date_naive())?.join("\n") + "\n";
    capture.verify()?;
    inventory.verify()?;
    route.verify()?;
    if let Some(page) = &page_capture {
        page.verify()?;
    }
    if options.dry_run {
        let stray = report::stray(&c, &options.take);
        if let Some(why) = &stray {
            output.push('\n');
            output.push_str(why);
            output.push('\n');
        }
        return Ok(CommandOutput {
            stdout: output,
            stderr: String::new(),
            code: i32::from(stray.is_some() || c.red() || !c.drops_needed.is_empty()),
        });
    }
    output.push('\n');
    if let Some(why) = report::fold_refusal(&c, &options.take) {
        return Ok(CommandOutput {
            stdout: output,
            stderr: why + "\n",
            code: 1,
        });
    }
    match edit::fold(&c, &write, &drops, page, probe) {
        Ok(done) => {
            output.push_str(&done);
            Ok(CommandOutput {
                stdout: output,
                stderr: String::new(),
                code: 0,
            })
        }
        Err(e) => Ok(CommandOutput {
            stdout: output,
            stderr: e.0 + "\n",
            code: 1,
        }),
    }
}

pub(super) fn preview(
    request: &super::PreviewRequest<'_>,
    evidence: &super::PreviewEvidence<'_>,
) -> Result<super::Preview> {
    let empty = Map::new();
    let context = request.context.map(map).transpose()?;
    let conflicts = context
        .and_then(|m| m.get("conflicts"))
        .map(map)
        .transpose()?
        .unwrap_or(&empty);
    let base = Projection::new(
        request.document,
        request.hypotheses,
        conflicts,
        vec![],
        request.runtime,
    )?;
    let mut pool = BTreeMap::new();
    for (name, h) in request.hypotheses {
        if get(h, "kind") == &s("contribution") {
            continue;
        }
        require(
            !truth(get(h, "error")),
            &format!(
                "refused - hypothesis {name} could not be read: {}",
                py(get(h, "error"))
            ),
        )?;
        let doc = if get(h, "doc") != &V::Null {
            get(h, "doc")
        } else {
            get(h, "document")
        };
        let merged = layer(request.document, doc)?;
        Reader::new(&merged, request.runtime).map_err(|e| {
            error(&format!(
                "refused - hypothesis {name} cannot be read over the base: {}",
                crate::ordinary_semantics::layer_failure(&e)
            ))
        })?;
        let raw = entries(doc)?;
        pool.insert(
            name.clone(),
            Hypothesis {
                name: name.clone(),
                path: text(get(h, "path"))
                    .ok()
                    .filter(|p| !p.is_empty())
                    .map(PathBuf::from),
                doc: doc.clone(),
                head: get(h, "head").clone(),
                ids: raw.keys().cloned().collect(),
                raw,
                source: O::from_typed(doc),
                text: String::new(),
                source_record: V::Map(Map::new()),
                whole: None,
            },
        );
    }
    for proposal in request.proposals {
        require(
            !pool.contains_key(&proposal.name),
            &format!(
                "refused - {} names both an existing and a supplied hypothesis",
                proposal.name
            ),
        )?;
        let raw = entries(&proposal.document)?;
        pool.insert(
            proposal.name.clone(),
            Hypothesis {
                name: proposal.name.clone(),
                path: None,
                doc: proposal.document.clone(),
                head: proposal.head.clone(),
                ids: raw.keys().cloned().collect(),
                raw,
                source: O::from_typed(&proposal.document),
                text: String::new(),
                source_record: V::Map(Map::new()),
                whole: None,
            },
        );
    }
    let proposals = pool.into_values().collect::<Vec<_>>();
    let brief = evidence
        .brief
        .and_then(|raw| crate::history_yaml::decode_ordinary_source_value(raw).ok())
        .map(|v| v.projected());
    let page = if let Some(raw) = evidence.brief {
        if needs_page(request.document, &base, &proposals, request.runtime)? {
            use crate::ordinary_value::{Value as Full, map as full_map};
            let document = Full::from_finite_projection(request.document);
            let hypotheses = Full::from_finite_projection(&V::Map(request.hypotheses.clone()));
            let context = request.context.map(Full::from_finite_projection);
            Some(
                super::full::page_evidence(
                    &document,
                    full_map(&hypotheses)?,
                    context.as_ref(),
                    raw,
                    request.runtime,
                )
                .and_then(|facts| facts.try_typed())
                .map_err(|e| error(&format!("presentation contribution cannot be checked: {e}")))?,
            )
        } else {
            None
        }
    } else {
        None
    };
    let stamp = request
        .as_of
        .map(str::to_owned)
        .unwrap_or_else(crate::source_clock::latest_day);
    let c = union(UnionInput {
        doc: request.document,
        all: request.hypotheses,
        hyps: proposals,
        base,
        runtime: request.runtime,
        stamp: &stamp,
        take: &[],
        drops: &empty,
        brief: brief.as_ref(),
        page: page.as_ref(),
    })?;
    let report = report::lines(&c, chrono::Local::now().date_naive())?.join("\n") + "\n";
    Ok(super::Preview {
        report,
        exit_code: i32::from(c.red() || !c.drops_needed.is_empty()),
        blocked: c.blocked() || !c.drops_needed.is_empty(),
        candidate_document: c.view.as_ref().map(|_| c.doc.clone()),
        facts: c.facts(),
    })
}
