//! Exact, read-only core/v1 graph export over one captured assessment.
//!
//! Projection and rendering consume only immutable captured data. They never
//! load a source record, fetch evidence, or invoke an evaluator.
use crate::{
    Error, Result, public_core_readers::json_value, reasoning_context::CapturedAssessment,
    reasoning_projection as projection, source_capture::CapturedSource, value::TypedValue as V,
};
use serde_json::{Map, Value as J, json};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

pub const MAX_EDGES: usize = 96;
const FIELD_CHARS: usize = 600;
const LABEL_CHARS: usize = 80;
const PREMISE_ROWS: usize = 12;
const CELL_CHARS: usize = 120;

pub fn command_error(error: &str) -> String {
    format!(
        "usage: kpop export [-h] [--record RECORD]\n                   [--format {{markdown,markdown-mermaid,mermaid}}]\n                   [--direction {{support,impact}}] [--depth DEPTH]\n                   [--max-nodes MAX_NODES] [--details] [--profile {{core/v1}}]\n                   ids [ids ...]\nkpop export: error: {error}\n"
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    Markdown,
    MarkdownMermaid,
    Mermaid,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Direction {
    Support,
    Impact,
}
#[derive(Clone, Debug)]
pub struct Options {
    pub direction: Direction,
    pub depth: usize,
    pub max_nodes: usize,
    pub details: bool,
    pub format: Format,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            direction: Direction::Support,
            depth: 1,
            max_nodes: 12,
            details: false,
            format: Format::Markdown,
        }
    }
}

#[derive(Clone, Debug, clap::Args)]
pub struct CommandOptions {
    #[arg(required = true, num_args = 1..)]
    pub ids: Vec<String>,
    #[arg(long = "record")]
    pub records: Vec<std::path::PathBuf>,
    #[arg(long, value_enum, default_value = "markdown")]
    pub format: Format,
    #[arg(long, value_enum, default_value = "support")]
    pub direction: Direction,
    #[arg(long, default_value_t = 1)]
    pub depth: usize,
    #[arg(long, default_value_t = 12)]
    pub max_nodes: usize,
    #[arg(long)]
    pub details: bool,
    #[arg(long, value_parser = ["core/v1"])]
    pub profile: Option<String>,
}

pub fn run(
    options: &CommandOptions,
    cwd: &std::path::Path,
    mode: crate::source_capture::ReadMode,
) -> Result<String> {
    run_inner(options, cwd, mode).map_err(|error| {
        if error.0 == "invalid_ordinary_structural_key" {
            Error(
                "export requires string entry and dependency IDs; quote numeric IDs in YAML".into(),
            )
        } else {
            let named = options.records.iter().filter_map(|p| p.to_str());
            crate::public_readers::explain_missing(
                error,
                named,
                cwd,
                mode == crate::source_capture::ReadMode::Live,
            )
        }
    })
}

fn run_inner(
    options: &CommandOptions,
    cwd: &std::path::Path,
    mode: crate::source_capture::ReadMode,
) -> Result<String> {
    let paths = if options.records.is_empty() {
        crate::public_workspace::records(cwd)?
    } else {
        options.records.clone()
    };
    let runtime =
        crate::public_workspace::runtime_for_paths(&paths, cwd, options.profile.as_deref())?;
    let capture = crate::source_capture::capture_source_with_runtime(
        &paths,
        cwd,
        mode,
        None,
        runtime.as_ref(),
    )?;
    let capabilities = crate::reasoning_fields::capabilities(
        capture.ordinary_document(),
        options.profile.as_deref(),
    )?;
    let profile =
        crate::history_contract::text(&crate::history_contract::map(&capabilities)?["profile"])?;
    let render_options = Options {
        direction: options.direction,
        depth: options.depth,
        max_nodes: options.max_nodes,
        details: options.details,
        format: options.format,
    };
    let packet = if profile == "core/v1" {
        let context = CapturedAssessment::from_snapshot(
            capture.snapshot()?.clone(),
            None,
            "focused-review/v1",
            runtime.as_ref(),
            crate::reasoning_runtime::OperationalBounds::default(),
            None,
        )?;
        core_render_packet(&capture, &project(&context, &options.ids, &render_options)?)
    } else {
        capture.require_ordinary_reader()?;
        crate::require(
            profile == "ordinary-reader/v1",
            "unsupported export profile",
        )?;
        let packet = project_ordinary(&capture, runtime.as_ref(), &options.ids, &render_options)?;
        ordinary_render_packet(&capture, &packet)?
    };
    let output = render_packet(&packet, &render_options)?;
    capture.verify()?;
    Ok(output)
}

fn text<'a>(value: &'a J, message: &str) -> Result<&'a str> {
    value.as_str().ok_or_else(|| Error(message.into()))
}
fn object<'a>(value: &'a J, message: &str) -> Result<&'a Map<String, J>> {
    value.as_object().ok_or_else(|| Error(message.into()))
}
fn render_expression(value: &J) -> Result<String> {
    projection::render_expression(&V::from_json(value)?)
}
fn render_value(value: &J) -> Result<String> {
    projection::render_value(&V::from_json(value)?)
}

fn core_readings(finding: &J, selected: &HashSet<String>) -> Result<(Vec<J>, usize)> {
    let dependencies = object(
        &finding["state"]["basis"]["dependencies"],
        "invalid captured assessment",
    )?;
    // The assessment is read in the judgment's own dependency order; its
    // captured maps are name-ordered, so take that order from the body.
    let declared = finding["fields"]["deps"]
        .as_str()
        .and_then(|field| finding["body"].get(field))
        .and_then(J::as_array)
        .into_iter()
        .flatten()
        .filter_map(J::as_str);
    let mut order = Vec::<String>::new();
    for dep in declared.chain(dependencies.keys().map(String::as_str)) {
        if dependencies.contains_key(dep) && !order.iter().any(|seen| seen == dep) {
            order.push(dep.to_owned());
        }
    }
    let mut rows = Vec::new();
    for dep in order.iter().take(PREMISE_ROWS) {
        let item = object(&dependencies[dep], "invalid captured assessment")?;
        let prior = object(&item["at_review"], "invalid captured assessment")?;
        let current = object(&item["current"], "invalid captured assessment")?;
        let prior_status = text(&prior["status"], "invalid captured assessment")?;
        let status = text(&current["status"], "invalid captured assessment")?;
        let comparison = item
            .get("comparison")
            .and_then(J::as_str)
            .filter(|v| matches!(*v, "same" | "changed" | "unknown"))
            .unwrap_or("unknown");
        let mut row = Map::from_iter([
            ("id".into(), json!(dep)),
            ("has_review".into(), json!(prior_status == "recorded")),
            (
                "at_review".into(),
                prior.get("value").cloned().unwrap_or(J::Null),
            ),
            ("current_status".into(), json!(status)),
            (
                "comparison".into(),
                json!(if comparison == "unknown" {
                    "not_compared"
                } else {
                    comparison
                }),
            ),
            (
                "formula_changed".into(),
                item.get("rule_changed").cloned().unwrap_or(J::Null),
            ),
        ]);
        if status == "ok" && current.get("value").is_some_and(|v| !v.is_null()) {
            row.insert("current_status".into(), json!("shown"));
            row.insert("current".into(), json!(render_value(&current["value"])?));
            row.insert("current_calculated".into(), json!(true));
        } else if matches!(
            status,
            "unknown" | "error" | "limit" | "operational_error" | "unsupported_capability"
        ) {
            row.insert("current_status".into(), json!("unavailable"));
        }
        if !selected.contains(dep) {
            row.insert("current_status".into(), json!("omitted"));
            row.remove("current");
            row.remove("current_calculated");
        }
        rows.push(J::Object(row));
    }
    Ok((rows, dependencies.len().saturating_sub(PREMISE_ROWS)))
}

fn ordinary_json(value: &V) -> Result<J> {
    Ok(serde_json::from_str(
        &crate::ordinary_assessment_report::legacy_json(value)?,
    )?)
}

fn public_key(key: &str) -> Result<String> {
    match crate::history_yaml::projected_ordinary_key(key) {
        None => Ok(key.into()),
        Some(V::Text(value)) => Ok(value),
        Some(_) => Err(Error(
            "export requires string entry and dependency IDs; quote numeric IDs in YAML".into(),
        )),
    }
}

fn validate_collection_ids(document: &V) -> Result<()> {
    for members in crate::reasoning_fields::collections(document)?.values() {
        for id in members.keys() {
            public_key(id)?;
        }
    }
    Ok(())
}

fn ordinary_readings(
    finding: &V,
    nodes: &crate::history_contract::Map,
    ids: &BTreeSet<String>,
    selected: &HashSet<String>,
    dependency_field: &str,
) -> Result<(Vec<J>, usize)> {
    use crate::history_contract::{map, text};
    let finding = map(finding)?;
    let state = map(&finding["state"])?;
    let falsifier = map(&state["falsifier"])?;
    let covered = falsifier
        .get("reads")
        .and_then(|value| crate::history_view::list(value).ok())
        .into_iter()
        .flatten()
        .filter_map(|value| text(value).ok())
        .collect::<BTreeSet<_>>();
    let dependencies = map(&map(&state["basis"])?["dependencies"])?;
    let mut order = Vec::new();
    if let Some(values) = map(&finding["body"])
        .ok()
        .and_then(|body| body.get(dependency_field))
        .and_then(|value| crate::history_view::list(value).ok())
    {
        for value in values {
            if let V::Text(id) = value
                && dependencies.contains_key(id)
                && !order.contains(id)
            {
                order.push(id.clone());
            }
        }
    }
    for id in dependencies.keys() {
        if !order.contains(id) {
            order.push(id.clone());
        }
    }
    let mut rows = Vec::new();
    for internal_dep in order.iter().take(PREMISE_ROWS) {
        let dep = public_key(internal_dep)?;
        let item = map(&dependencies[internal_dep])?;
        let prior = map(&item["at_review"])?;
        let current = map(&item["current"])?;
        let prior_status = text(&prior["status"])?;
        let status = text(&current["status"])?;
        let value = current.get("value");
        let mut row = Map::from_iter([
            ("id".into(), json!(dep)),
            ("has_review".into(), json!(prior_status == "recorded")),
            (
                "at_review".into(),
                prior
                    .get("value")
                    .map(ordinary_json)
                    .transpose()?
                    .unwrap_or(J::Null),
            ),
        ]);
        row.insert(
            "current_status".into(),
            json!(if status == "recorded" && value == Some(&V::Null) {
                "null"
            } else if status == "recorded" {
                "shown"
            } else {
                status
            }),
        );
        if status == "recorded" {
            row.insert(
                "current".into(),
                value.map(ordinary_json).transpose()?.unwrap_or(J::Null),
            );
        } else if status == "unavailable"
            && let Some(reason) = current.get("detail").or_else(|| current.get("reason"))
        {
            row.insert("unavailable_reason".into(), ordinary_json(reason)?);
        }
        let calculated = nodes.get(internal_dep).is_some_and(|node| {
            map(node)
                .ok()
                .and_then(|node| node.get("body"))
                .and_then(|body| map(body).ok())
                .is_some_and(|body| matches!(body.get("rule"), Some(V::Map(_))))
        });
        row.insert(
            "current_calculated".into(),
            json!(calculated && status == "recorded"),
        );
        if let Some(value) = item.get("historical_calculation") {
            row.insert("review_calculated".into(), ordinary_json(value)?);
        }
        row.insert(
            "formula_changed".into(),
            item.get("rule_changed")
                .map(ordinary_json)
                .transpose()?
                .unwrap_or(J::Null),
        );
        if crate::history_view::truth(item.get("historical_formula_only").unwrap_or(&V::Null)) {
            row.insert("historical_formula_only".into(), json!(true));
        }
        if let Some(unit) = nodes.get(internal_dep).and_then(|node| {
            map(node)
                .ok()?
                .get("body")
                .and_then(|body| map(body).ok())?
                .get("unit")
        }) && crate::history_view::truth(unit)
        {
            row.insert("unit".into(), ordinary_json(unit)?);
        }
        let comparison = if status == "missing" {
            "unavailable"
        } else if prior_status != "recorded" {
            "unreviewed"
        } else if item.get("rule_changed") == Some(&V::Bool(true))
            || item["comparison"] == V::Text("changed".into())
        {
            if covered.contains(internal_dep.as_str())
                && falsifier["status"] == V::Text("holds".into())
            {
                "crossed"
            } else if covered.contains(internal_dep.as_str())
                && falsifier["status"] == V::Text("does_not_hold".into())
                && item.get("rule_changed") != Some(&V::Bool(true))
            {
                "muted"
            } else {
                "moved"
            }
        } else if status == "unavailable" {
            "unavailable"
        } else if crate::history_view::truth(
            item.get("historical_formula_only").unwrap_or(&V::Null),
        ) {
            "formula_only"
        } else if item["comparison"] == V::Text("unknown".into()) {
            "not_compared"
        } else {
            "same"
        };
        row.insert("comparison".into(), json!(comparison));
        if ids.contains(internal_dep) && !selected.contains(&dep) {
            row.insert("current_status".into(), json!("omitted"));
            for field in [
                "current",
                "current_calculated",
                "unit",
                "unavailable_reason",
            ] {
                row.remove(field);
            }
        }
        rows.push(J::Object(row));
    }
    Ok((rows, dependencies.len().saturating_sub(PREMISE_ROWS)))
}

/// Project one already-captured and fully assessed ordinary record.
fn project_ordinary_assessed(
    capture: &CapturedSource,
    assessment: &V,
    runtime: Option<&crate::reasoning_runtime::Runtime>,
    seeds: &[String],
    options: &Options,
) -> Result<J> {
    use crate::history_contract::{map, text};
    if options.depth > 4 {
        return Err(Error("depth must be 0..4".into()));
    }
    if !(1..=32).contains(&options.max_nodes) {
        return Err(Error("max-nodes must be 1..32".into()));
    }
    let mut seen = HashSet::new();
    let seeds = seeds
        .iter()
        .filter(|seed| seen.insert((*seed).clone()))
        .cloned()
        .collect::<Vec<_>>();
    if seeds.is_empty() || seeds.len() > 8 || seeds.len() > options.max_nodes {
        return Err(Error(
            "supply 1..8 exact IDs; max-nodes must include every seed".into(),
        ));
    }
    validate_collection_ids(capture.ordinary_document())?;
    let hypotheses = map(capture.hypotheses())?;
    for hypothesis in hypotheses.values() {
        let hypothesis = map(hypothesis)?;
        if let Some(document) = hypothesis.get("document").or_else(|| hypothesis.get("doc")) {
            validate_collection_ids(document)?;
        }
    }
    let report = map(assessment)?;
    crate::require(
        report["assessment_profile"] == V::Text("ordinary-reader/v1".into()),
        "invalid ordinary assessment",
    )?;
    let assessed_nodes = map(&report["nodes"])?;
    let mut public_to_internal = BTreeMap::new();
    for internal in assessed_nodes.keys() {
        let public = public_key(internal)?;
        crate::require(
            public_to_internal
                .insert(public, internal.clone())
                .is_none(),
            "duplicate ordinary entry ID",
        )?;
    }
    let unknown = seeds
        .iter()
        .filter(|id| !public_to_internal.contains_key(*id))
        .cloned()
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        let mut pending = BTreeSet::new();
        for hypothesis in hypotheses.values() {
            let hypothesis = map(hypothesis)?;
            if !crate::history_contract::string_is(
                hypothesis.get("kind").unwrap_or(&V::Null),
                "contribution",
            ) {
                continue;
            }
            if let Some(document) = hypothesis.get("document").or_else(|| hypothesis.get("doc")) {
                for members in crate::reasoning_fields::collections(document)?.values() {
                    for id in members.keys() {
                        pending.insert(public_key(id)?);
                    }
                }
            }
        }
        if unknown.iter().any(|id| pending.contains(id)) {
            return Err(Error("pending contribution IDs are not expanded by export; use open, pull or knowledge snapshot".into()));
        }
        return Err(Error(format!(
            "unknown exact ID(s): {}; use kpop open or pull",
            unknown.join(", ")
        )));
    }
    let mut sections = BTreeMap::new();
    for (section, members) in crate::reasoning_fields::collections(capture.ordinary_document())? {
        for internal in members.keys() {
            sections.insert(internal.clone(), section.clone());
        }
    }
    let context = capture.ordinary_context();
    let conflicts = map(&context)?
        .get("conflicts")
        .and_then(|value| map(value).ok())
        .cloned()
        .unwrap_or_default();
    let normalized_hypotheses = hypotheses
        .iter()
        .map(|(name, value)| {
            let mut value = map(value)?.clone();
            if !value.contains_key("document") {
                value.insert(
                    "document".into(),
                    value.get("doc").cloned().unwrap_or(V::Null),
                );
            }
            Ok((name.clone(), V::Map(value)))
        })
        .collect::<Result<crate::history_contract::Map>>()?;
    let reader = crate::ordinary_reader::Reader::new(capture.ordinary_document(), runtime)?
        .with_layers(normalized_hypotheses, conflicts.keys().cloned().collect())?;
    let fields = reader.fields().clone();
    let dep_field = text(&fields["deps"])?.to_owned();
    let ids = assessed_nodes.keys().cloned().collect::<BTreeSet<_>>();
    let computations = crate::ordinary_reader::compute(reader.raw(), &ids, None, reader.program());
    let mut edges = BTreeSet::<(String, String, String)>::new();
    let mut states = BTreeMap::<String, Vec<String>>::new();
    for (internal, finding) in assessed_nodes {
        let id = public_key(internal)?;
        let finding_map = map(finding)?;
        let body = finding_map.get("body").unwrap_or(&V::Null);
        let state = map(&finding_map["state"])?;
        let judgment = state["basis"] != V::Null
            && !crate::history_contract::string_is(
                &map(&state["basis"])?["status"],
                "not_applicable",
            );
        let mut flags = if judgment {
            crate::ordinary_counts::reader_flags(&reader, body, false)?
                .into_iter()
                .map(str::to_owned)
                .collect::<BTreeSet<_>>()
        } else {
            BTreeSet::new()
        };
        if crate::history_contract::string_is(&map(&state["contention"])?["status"], "detected") {
            flags.insert("contested".into());
        }
        if sections
            .get(internal)
            .is_some_and(|s| matches!(s.as_str(), "open" | "questions"))
            && !crate::public_amend::settled(body)
        {
            flags.insert("question".into());
        }
        states.insert(internal.clone(), flags.into_iter().collect());
        let Ok(body_map) = map(body) else { continue };
        if judgment {
            let deps = body_map.get(&dep_field).unwrap_or(&V::Null);
            let list = crate::history_view::list(deps)
                .map_err(|_| Error(format!("{id}: {dep_field} must be a list of entry IDs")))?;
            for dep in list {
                let dep = match dep { V::Text(value) => value.clone(), _ => return Err(Error("export requires string entry and dependency IDs; quote numeric IDs in YAML".into())) };
                edges.insert((id.clone(), "rests_on".into(), dep));
            }
        }
        if let Some(V::Text(source)) = body_map.get("from")
            && public_to_internal.contains_key(source)
        {
            edges.insert((id.clone(), "from".into(), source.clone()));
        }
        if !judgment {
            let refs = if matches!(body_map.get("rule"), Some(V::Map(_))) {
                crate::reasoning_language::legacy_references(&body_map["rule"])
            } else {
                ["rule", "v"].iter().filter_map(|field| body_map.get(*field))
                    .filter(|value| matches!(value,V::Text(text) if crate::ordinary_reader::EXPR.is_match(text)))
                    .flat_map(crate::ordinary_reader::predicate_refs)
                    .filter(|dep| public_to_internal.contains_key(dep)).collect()
            };
            for dep in refs {
                if dep != id {
                    edges.insert((id.clone(), "rule_reads".into(), dep));
                }
            }
        }
        if id.starts_with("hyp.")
            && body_map.get("v") == Some(&V::Text("refuted".into()))
            && let Some(V::List(values)) = body_map.get("refutes")
        {
            for value in values {
                if let V::Text(dep) = value {
                    edges.insert((id.clone(), "refutes".into(), dep.clone()));
                }
            }
        }
    }
    let mut adjacent = BTreeMap::<String, BTreeSet<String>>::new();
    for (a, _, b) in &edges {
        let (s, t) = match options.direction {
            Direction::Support => (a, b),
            Direction::Impact => (b, a),
        };
        adjacent.entry(s.clone()).or_default().insert(t.clone());
    }
    let mut distances = HashMap::new();
    let mut order = Vec::new();
    let mut queue = VecDeque::new();
    for seed in &seeds {
        distances.insert(seed.clone(), 0usize);
        order.push(seed.clone());
        queue.push_back(seed.clone());
    }
    while let Some(id) = queue.pop_front() {
        let depth = distances[&id];
        if depth >= options.depth {
            continue;
        }
        for next in adjacent.get(&id).into_iter().flatten() {
            if !distances.contains_key(next) && distances.len() < options.max_nodes {
                distances.insert(next.clone(), depth + 1);
                order.push(next.clone());
                queue.push_back(next.clone());
            }
        }
    }
    let selected = distances.keys().cloned().collect::<HashSet<_>>();
    let mut nodes = Map::new();
    for id in &order {
        let Some(internal) = public_to_internal.get(id) else {
            nodes.insert(
                id.clone(),
                json!({"body":null,"missing":true,"states":[],"kind":"missing"}),
            );
            continue;
        };
        let finding = &assessed_nodes[internal];
        let finding_map = map(finding)?;
        let body = finding_map.get("body").unwrap_or(&V::Null);
        let state = map(&finding_map["state"])?;
        let judgment =
            !crate::history_contract::string_is(&map(&state["basis"])?["status"], "not_applicable");
        let kind = if judgment {
            "judgment".into()
        } else if crate::reasoning_fields::BUILTINS.contains(&id.as_str()) {
            "computed".into()
        } else {
            sections
                .get(internal)
                .cloned()
                .unwrap_or_else(|| "entry".into())
        };
        let mut node = Map::from_iter([
            ("body".into(), ordinary_json(body)?),
            ("missing".into(), json!(false)),
            ("states".into(), json!(states[internal])),
            ("kind".into(), json!(kind)),
        ]);
        if judgment {
            let falsifier = map(&state["falsifier"])?;
            let issues = crate::history_view::list(&map(&state["integrity"])?["issues"])?;
            let issue_ids = |code: &str, only_known: bool| -> Result<Vec<String>> {
                let mut out = vec![];
                for issue in issues {
                    let issue = map(issue)?;
                    if crate::history_contract::string_is(&issue["code"], code) {
                        for value in crate::history_view::list(&issue["related_ids"])? {
                            let id = text(value)?.to_owned();
                            if !only_known || public_to_internal.contains_key(&id) {
                                out.push(id)
                            }
                        }
                    }
                }
                Ok(out)
            };
            let reads = falsifier
                .get("reads")
                .and_then(|v| crate::history_view::list(v).ok())
                .into_iter()
                .flatten()
                .filter_map(|v| text(v).ok())
                .filter(|id| !public_to_internal.contains_key(*id))
                .map(str::to_owned)
                .collect::<Vec<_>>();
            node.insert("condition".into(),json!({"expression":ordinary_json(&falsifier["expression"])? ,"result":match text(&falsifier["status"]).unwrap_or(""){"holds"=>J::Bool(true),"does_not_hold"=>J::Bool(false),_=>J::Null},"undeclared_reads":issue_ids("undeclared_predicate_dependencies",true)?,"missing_reads":reads,"unparsed_mentions":issue_ids("unsupported_predicate",false)?}));
            let (readings, omitted) =
                ordinary_readings(finding, assessed_nodes, &ids, &selected, &dep_field)?;
            node.insert("readings".into(), J::Array(readings));
            node.insert("omitted_readings".into(), json!(omitted));
        } else if matches!(
            map(body).ok().and_then(|body| body.get("rule")),
            Some(V::Map(_))
        ) {
            let value=computations.get("values").and_then(|values|values.get(internal)).cloned().unwrap_or_else(||json!({"value":null,"reason":computations.get("error").and_then(J::as_str).unwrap_or("unavailable")}));
            node.insert("calculation".into(), value);
        }
        nodes.insert(id.clone(), J::Object(node));
    }
    let internal = edges
        .iter()
        .filter(|(a, _, b)| nodes.contains_key(a) && nodes.contains_key(b))
        .collect::<Vec<_>>();
    let frontier = edges
        .iter()
        .filter(|(a, _, b)| nodes.contains_key(a) != nodes.contains_key(b))
        .count();
    let hypothesis_errors = hypotheses
        .iter()
        .filter_map(|(name, h)| {
            let h = map(h).ok()?;
            let error = h.get("error")?;
            crate::history_view::truth(error)
                .then(|| ordinary_json(error).ok().map(|error| (name.clone(), error)))
                .flatten()
        })
        .collect::<Map<_, _>>();
    let hypothesis_count = hypotheses
        .values()
        .filter(|h| {
            map(h).ok().is_none_or(|h| {
                !crate::history_contract::string_is(
                    h.get("kind").unwrap_or(&V::Null),
                    "contribution",
                )
            })
        })
        .count();
    let knowledge = capture.knowledge_status_context();
    let contributions = map(&knowledge)?
        .get("contributions")
        .and_then(|v| crate::history_view::list(v).ok())
        .map_or(0, |v| v.len());
    let mut assessment_without_nodes = report.clone();
    assessment_without_nodes.remove("nodes");
    Ok(
        json!({"nodes":nodes,"edges":internal.iter().take(MAX_EDGES).map(|(a,r,b)|json!([a,r,b])).collect::<Vec<_>>(),"seeds":seeds,"direction":match options.direction{Direction::Support=>"support",Direction::Impact=>"impact"},"depth":options.depth,"max_nodes":options.max_nodes,"snapshot":text(&report["record_revision"])? .chars().take(16).collect::<String>(),"outside_nodes":public_to_internal.keys().filter(|id|!nodes.contains_key(*id)).count(),"frontier_edges":frontier,"omitted_edges":internal.len().saturating_sub(MAX_EDGES),"hypotheses":hypothesis_count,"contributions":contributions,"hypothesis_errors":hypothesis_errors,"fields":ordinary_json(&V::Map(fields))?,"assessment":ordinary_json(&V::Map(assessment_without_nodes))?}),
    )
}

/// Capture-compatible ordinary route. The full record is assessed once before
/// any bounded selection is projected.
pub fn project_ordinary(
    capture: &CapturedSource,
    runtime: Option<&crate::reasoning_runtime::Runtime>,
    seeds: &[String],
    options: &Options,
) -> Result<J> {
    let assessment =
        crate::ordinary_assessment_report::from_capture(capture, runtime, "focused-review/v1")?;
    project_ordinary_assessed(capture, &assessment, runtime, seeds, options)
}

pub fn render_ordinary(
    capture: &CapturedSource,
    runtime: Option<&crate::reasoning_runtime::Runtime>,
    seeds: &[String],
    options: &Options,
) -> Result<String> {
    let packet = project_ordinary(capture, runtime, seeds, options)?;
    render_packet(&ordinary_render_packet(capture, &packet)?, options)
}

fn source_body<'a>(
    source: &'a crate::history_yaml::OrdinaryValue,
    id: &str,
) -> Option<&'a crate::history_yaml::OrdinaryValue> {
    let crate::history_yaml::OrdinaryValue::Map(collections) = source else {
        return None;
    };
    collections
        .iter()
        .filter_map(|(_, members)| {
            let crate::history_yaml::OrdinaryValue::Map(members) = members else {
                return None;
            };
            members
                .iter()
                .find(|(key, _)| key.text() == Some(id))
                .map(|(_, body)| body)
        })
        .next_back()
}

fn ordinary_render_packet(capture: &CapturedSource, packet: &J) -> Result<J> {
    use crate::history_yaml::OrdinaryValue;
    let mut rendered = packet.clone();
    let nodes = rendered["nodes"].as_object_mut().unwrap();
    let mut orders = Map::new();
    for (id, node) in nodes.iter_mut().filter(|(_, node)| node["missing"] != true) {
        if crate::reasoning_fields::BUILTINS.contains(&id.as_str()) {
            // A computed name reads as its name, its count when the record
            // has one, then where the count comes from.
            let body = node["body"].as_object().cloned().unwrap_or_default();
            orders.insert(id.clone(), in_order(&body, &["name", "v", "from"]));
            continue;
        }
        let Some(OrdinaryValue::Map(fields)) = source_body(capture.source(), id) else {
            continue;
        };
        let mut ordered = Vec::new();
        for (key, value) in fields {
            let key = ordinary_json(key.scalar())?;
            let is_date = matches!(value, OrdinaryValue::Scalar(V::Date(_) | V::DateTime(_)));
            let mut value = ordinary_json(&value.projected())?;
            if is_date {
                value = json!({"__kpopper_ordinary_date":value});
                if let (Some(field), Some(body)) = (key.as_str(), node["body"].as_object_mut()) {
                    body.insert(field.into(), value.clone());
                }
            }
            ordered.push(json!([key, value]));
        }
        orders.insert(id.clone(), J::Array(ordered));
    }
    rendered
        .as_object_mut()
        .unwrap()
        .insert("__ordinary_body_fields".into(), J::Object(orders));
    Ok(rendered)
}

/// Body fields as `[field, value]` pairs: the named fields first, in that
/// order, then the rest by name.
fn in_order(body: &Map<String, J>, first: &[&str]) -> J {
    let known = first
        .iter()
        .filter_map(|field| body.get(*field).map(|value| json!([field, value])));
    let rest = body
        .iter()
        .filter(|(field, _)| !first.contains(&field.as_str()))
        .map(|(field, value)| json!([field, value]));
    J::Array(known.chain(rest).collect())
}

/// Show each core body's fields in the record's own order; the shared
/// assessment holds them name-ordered.
fn core_render_packet(capture: &CapturedSource, packet: &J) -> J {
    let mut rendered = packet.clone();
    let mut orders = Map::new();
    for (id, node) in packet["nodes"].as_object().into_iter().flatten() {
        let (Some(body), Some(crate::history_yaml::OrdinaryValue::Map(fields))) =
            (node["body"].as_object(), source_body(capture.source(), id))
        else {
            continue;
        };
        let first = fields
            .iter()
            .filter_map(|(key, _)| key.text())
            .collect::<Vec<_>>();
        orders.insert(id.clone(), in_order(body, &first));
    }
    rendered["__ordinary_body_fields"] = J::Object(orders);
    rendered
}

/// Project the core/v1 export packet defined by Python 1.8 `export_graph.py`.
pub fn project(context: &CapturedAssessment, seeds: &[String], options: &Options) -> Result<J> {
    if options.depth > 4 {
        return Err(Error("depth must be 0..4".into()));
    }
    if !(1..=32).contains(&options.max_nodes) {
        return Err(Error("max-nodes must be 1..32".into()));
    }
    let mut seen_seeds = HashSet::new();
    let seeds = seeds
        .iter()
        .filter(|seed| seen_seeds.insert((*seed).clone()))
        .cloned()
        .collect::<Vec<_>>();
    if seeds.is_empty() || seeds.len() > 8 || seeds.len() > options.max_nodes {
        return Err(Error(
            "supply 1..8 exact IDs; max-nodes must include every seed".into(),
        ));
    }
    let assessment = json_value(context.assessment())?;
    let view = json_value(context.view())?;
    let snapshot = json_value(&context.snapshot().to_data())?;
    let all_nodes = object(&assessment["nodes"], "invalid captured assessment")?;
    let unknown = seeds
        .iter()
        .filter(|id| !all_nodes.contains_key(*id))
        .cloned()
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        return Err(Error(format!(
            "unknown exact ID(s): {}; use kpop open or pull",
            unknown.join(", ")
        )));
    }
    let mut edges = BTreeSet::<(String, String, String)>::new();
    for edge in view["impacts"].as_array().into_iter().flatten() {
        let from = text(&edge["from"], "invalid captured assessment")?;
        let to = text(&edge["to"], "invalid captured assessment")?;
        let relation = if edge["classification"] == "executed" {
            "impact"
        } else {
            "potential_impact"
        };
        edges.insert((to.into(), relation.into(), from.into()));
    }
    for (id, finding) in all_nodes {
        if let Some(source) = finding["body"].get("from").and_then(J::as_str)
            && all_nodes.contains_key(source)
        {
            edges.insert((id.clone(), "from".into(), source.into()));
        }
    }
    let mut adjacent = BTreeMap::<String, BTreeSet<String>>::new();
    for (start, _, end) in &edges {
        let (source, target) = match options.direction {
            Direction::Support => (start, end),
            Direction::Impact => (end, start),
        };
        adjacent
            .entry(source.clone())
            .or_default()
            .insert(target.clone());
    }
    let mut distances = HashMap::<String, usize>::new();
    let mut order = Vec::new();
    let mut queue = VecDeque::new();
    for seed in &seeds {
        distances.insert(seed.clone(), 0);
        order.push(seed.clone());
        queue.push_back(seed.clone());
    }
    while let Some(id) = queue.pop_front() {
        let depth = distances[&id];
        if depth >= options.depth {
            continue;
        }
        for neighbor in adjacent.get(&id).into_iter().flatten() {
            if !distances.contains_key(neighbor) && distances.len() < options.max_nodes {
                distances.insert(neighbor.clone(), depth + 1);
                order.push(neighbor.clone());
                queue.push_back(neighbor.clone());
            }
        }
    }
    let selected = distances.keys().cloned().collect::<HashSet<_>>();
    let snapshot_nodes = object(&snapshot["nodes"], "invalid captured snapshot")?;
    let view_nodes = object(&view["nodes"], "invalid captured assessment")?;
    let mut nodes = Map::new();
    for id in &order {
        let Some(finding) = all_nodes.get(id) else {
            nodes.insert(
                id.clone(),
                json!({"body":null,"missing":true,"states":[],"kind":"missing"}),
            );
            continue;
        };
        let body = finding["body"].clone();
        let projected = &view_nodes[id];
        let status = projected["status"].clone();
        let mut states = Vec::new();
        if status["falsifier"]["holds"] == true {
            states.push("falsified");
        }
        if status["contention"] == "detected" {
            states.push("contested");
        }
        if matches!(
            status["falsifier"]["status"].as_str(),
            Some("unknown" | "error")
        ) || matches!(
            status["computation"]["status"].as_str(),
            Some("unknown" | "error" | "operational_error")
        ) {
            states.push("unknown");
        }
        states.sort_unstable();
        let kind = if finding["state"]["basis"]["status"] != "not_applicable" {
            "judgment"
        } else if body.as_object().is_some_and(|b| b.contains_key("rule")) {
            "computed"
        } else {
            text(
                &snapshot_nodes[id]["collection"],
                "invalid captured snapshot",
            )?
        };
        let mut node = Map::from_iter([
            ("body".into(), body),
            ("missing".into(), json!(false)),
            ("states".into(), json!(states)),
            ("kind".into(), json!(kind)),
            ("status".into(), status.clone()),
            ("status_text".into(), projected["status_text"].clone()),
        ]);
        if finding["computation"].is_object() {
            node.insert("calculation".into(), json!({"value":status["computation"]["value_text"],"reason":status["computation"]["status"]}));
        }
        if kind == "judgment" {
            let expression = finding["state"]["falsifier"]["expression"].clone();
            let issues = finding["state"]["integrity"]["issues"]
                .as_array()
                .ok_or_else(|| Error("invalid captured assessment".into()))?;
            let issue_ids = |code: &str| {
                issues
                    .iter()
                    .filter(|issue| issue["code"] == code)
                    .flat_map(|issue| issue["related_ids"].as_array().into_iter().flatten())
                    .cloned()
                    .collect::<Vec<_>>()
            };
            node.insert("condition".into(), json!({
                "expression":expression,
                "expression_text":if expression.is_object(){J::String(render_expression(&expression)?)}else{expression.clone()},
                "result":status["falsifier"]["holds"],
                "undeclared_reads":issue_ids("undeclared_predicate_dependencies"),
                "missing_reads":issue_ids("missing_dependency"),
                "unparsed_mentions":issue_ids("unsupported_predicate"),
            }));
            let (readings, omitted) = core_readings(finding, &selected)?;
            node.insert("readings".into(), J::Array(readings));
            node.insert("omitted_readings".into(), json!(omitted));
        }
        nodes.insert(id.clone(), J::Object(node));
    }
    let internal = edges
        .iter()
        .filter(|(a, _, b)| nodes.contains_key(a) && nodes.contains_key(b))
        .collect::<Vec<_>>();
    let frontier_edges = edges
        .iter()
        .filter(|(a, _, b)| nodes.contains_key(a) != nodes.contains_key(b))
        .count();
    let hypotheses = object(&snapshot["hypotheses"], "invalid captured snapshot")?;
    let hypothesis_errors = hypotheses
        .iter()
        .filter_map(|(name, hypothesis)| {
            let error = hypothesis.get("error")?;
            (!error.is_null() && error.as_str().is_none_or(|s| !s.is_empty()))
                .then(|| (name.clone(), error.clone()))
        })
        .collect::<Map<_, _>>();
    let fields = all_nodes
        .values()
        .next()
        .map(|node| node["fields"].clone())
        .unwrap_or_else(|| json!({"deps":"rests_on","snapshot":"seen","predicate":"wrong_if"}));
    let contributions = snapshot["context"]
        .get("pending")
        .and_then(|v| v.get("contributions"))
        .and_then(J::as_array)
        .map_or(0, Vec::len);
    let direction = match options.direction {
        Direction::Support => "support",
        Direction::Impact => "impact",
    };
    Ok(json!({
        "profile":"core/v1","nodes":nodes,
        "edges":internal.iter().take(MAX_EDGES).map(|(a,r,b)|json!([a,r,b])).collect::<Vec<_>>(),
        "seeds":seeds,"direction":direction,"depth":options.depth,"max_nodes":options.max_nodes,
        "snapshot":context.snapshot_id().chars().take(16).collect::<String>(),"snapshot_id":context.snapshot_id(),
        "findings_revision":context.findings_revision(),"outside_nodes":all_nodes.keys().filter(|id|!nodes.contains_key(*id)).count(),
        "frontier_edges":frontier_edges,"omitted_edges":internal.len().saturating_sub(MAX_EDGES),"hypotheses":hypotheses.len(),
        "contributions":contributions,"hypothesis_errors":hypothesis_errors,"fields":fields,
        "assessment":{"schema_version":assessment["schema_version"],"assessment_profile":assessment["assessment_profile"],
            "snapshot_id":assessment["snapshot_id"],"findings_revision":assessment["findings_revision"],
            "envelope_revision":assessment["envelope_revision"],"history":assessment["history"]},
    }))
}

fn python_json(value: &J) -> String {
    match value {
        J::Null => "null".into(),
        J::Bool(v) => v.to_string(),
        J::Number(v) => v.to_string(),
        J::String(v) => serde_json::to_string(v).unwrap(),
        J::Array(v) => format!(
            "[{}]",
            v.iter().map(python_json).collect::<Vec<_>>().join(", ")
        ),
        J::Object(v) => format!(
            "{{{}}}",
            v.iter()
                .map(|(k, v)| format!("{}: {}", serde_json::to_string(k).unwrap(), python_json(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}
fn plain(value: &J) -> String {
    let raw = if let Some(date) = value.get("__kpopper_ordinary_date") {
        serde_json::to_string(date.as_str().unwrap_or_default()).unwrap()
    } else {
        match value {
            J::String(v) => v.clone(),
            _ => python_json(value),
        }
    };
    raw.split(crate::python_text::space)
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .map(|c| {
            let n = c as u32;
            if n >= 32
                && n != 127
                && !(0x202a..=0x202e).contains(&n)
                && !(0x2066..=0x2069).contains(&n)
            {
                c.to_string()
            } else {
                format!("\\u{n:04x}")
            }
        })
        .collect()
}
fn markdown_text(value: &J) -> String {
    plain(value)
        .chars()
        .flat_map(|c| {
            if "\\`*_{}[]<>()!#|&".contains(c) {
                vec!['\\', c]
            } else {
                vec![c]
            }
        })
        .collect()
}
fn clipped(value: &J, limit: usize) -> String {
    let value = if value == "" {
        "\"\"".into()
    } else {
        plain(value)
    };
    let length = value.chars().count();
    if length <= limit {
        markdown_text(&json!(value))
    } else {
        format!(
            "{} … [{} characters omitted; read the record]",
            markdown_text(&json!(value.chars().take(limit).collect::<String>())),
            length - limit
        )
    }
}
fn boundaries(packet: &J) -> String {
    let identity = if packet["profile"] == "core/v1" {
        format!(
            "Shared assessment {}; findings {}",
            packet["snapshot_id"].as_str().unwrap_or_default(),
            packet["findings_revision"].as_str().unwrap_or_default()
        )
    } else {
        format!("Record {}", packet["snapshot"].as_str().unwrap_or_default())
    };
    format!(
        "{identity}; {} depth {}; {} nodes shown (limit {}), {} outside selection; {} boundary links; {} internal links omitted (cap {MAX_EDGES}).",
        packet["direction"].as_str().unwrap_or_default(),
        packet["depth"],
        packet["nodes"].as_object().map_or(0, Map::len),
        packet["max_nodes"],
        packet["outside_nodes"],
        packet["frontier_edges"],
        packet["omitted_edges"]
    )
}
fn state_meaning(state: &str) -> Option<&'static str> {
    Some(match state {
        "moved" => "changed premise needs review",
        "falsified" => "declared condition holds on recorded values",
        "blocked" => "referenced input is missing with a recorded explanation",
        "broken" => "referenced input is missing without a recorded explanation",
        "unchecked" => "a dependency has no historical snapshot",
        "no_predicate" => "no condition this reader can decide",
        "contested" => "readable hypotheses contain competing claims",
        "question" => "recorded open question",
        "unknown" => "condition cannot currently be evaluated",
        _ => return None,
    })
}
fn state_legend(packet: &J) -> Vec<String> {
    let mut states = BTreeSet::new();
    for node in packet["nodes"]
        .as_object()
        .into_iter()
        .flat_map(Map::values)
    {
        for state in node["states"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(J::as_str)
        {
            states.insert(state);
        }
    }
    states
        .into_iter()
        .filter_map(|s| state_meaning(s).map(|m| format!("{}: {m}", s.to_uppercase())))
        .collect()
}
fn node_order(packet: &J) -> Vec<String> {
    let nodes = packet["nodes"].as_object().unwrap();
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    let mut queue = VecDeque::new();
    for seed in packet["seeds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(J::as_str)
    {
        if nodes.contains_key(seed) && seen.insert(seed.to_owned()) {
            result.push(seed.to_owned());
            queue.push_back(seed.to_owned());
        }
    }
    let mut adjacent = BTreeMap::<String, BTreeSet<String>>::new();
    for edge in packet["edges"].as_array().into_iter().flatten() {
        let (Some(a), Some(b)) = (edge[0].as_str(), edge[2].as_str()) else {
            continue;
        };
        let (s, t) = if packet["direction"] == "impact" {
            (b, a)
        } else {
            (a, b)
        };
        adjacent.entry(s.into()).or_default().insert(t.into());
    }
    while let Some(id) = queue.pop_front() {
        for next in adjacent.get(&id).into_iter().flatten() {
            if nodes.contains_key(next) && seen.insert(next.clone()) {
                result.push(next.clone());
                queue.push_back(next.clone());
            }
        }
    }
    for id in nodes.keys() {
        if seen.insert(id.clone()) {
            result.push(id.clone());
        }
    }
    result
}
fn reading_value(value: &J) -> J {
    if let Some(m) = value.as_object()
        && m.len() == 1
        && let Some(p) = m.get("rational").and_then(J::as_array)
        && p.len() == 2
        && let (Some(n), Some(d)) = (p[0].as_str(), p[1].as_str())
    {
        return json!(if d == "1" {
            n.into()
        } else {
            format!("{n}/{d}")
        });
    }
    value.clone()
}
fn reading_table(node: &J) -> Vec<String> {
    let mut lines=vec!["| Dependency | `at_review` (historical) | `current` (recorded or calculated) | Comparison |".into(),"|---|---|---|---|".into()];
    for row in node["readings"].as_array().into_iter().flatten() {
        let mut old = if row["has_review"] == true {
            clipped(&row["at_review"], CELL_CHARS)
        } else {
            "not recorded".into()
        };
        if let Some(h) = row.get("review_calculated") {
            old = format!(
                "{} (calculated)",
                clipped(&reading_value(&h["value"]), CELL_CHARS)
            );
        }
        let status = row["current_status"].as_str().unwrap_or_default();
        let mut current = match status {
            "missing" => "missing from record".into(),
            "omitted" => "not included in this excerpt".into(),
            "null" => "null (recorded)".into(),
            "unavailable" => "not evaluated here".into(),
            _ => clipped(&reading_value(&row["current"]), CELL_CHARS),
        };
        if !matches!(status, "missing" | "omitted" | "null" | "unavailable") {
            if row["current_calculated"] == true {
                current.push_str(" (calculated)");
            }
            if let Some(unit) = row.get("unit") {
                current.push(' ');
                current.push_str(&clipped(unit, 40));
            }
        }
        let mut comparison = match row["comparison"].as_str().unwrap_or_default() {
            "same" => "unchanged",
            "moved" => "changed; review flag",
            "changed" => "changed",
            "muted" => "changed; within condition; no review flag",
            "crossed" => "changed; declared condition holds",
            "unreviewed" => "no historical reading",
            "not_compared" => "not compared by reader",
            "unavailable" => "not compared",
            "formula_only" => "historical formula only; no historical result",
            _ => "not compared by reader",
        }
        .to_owned();
        if row["formula_changed"] == true {
            comparison.push_str("; formula changed");
        }
        if row["historical_formula_only"] == true && row["comparison"] != "formula_only" {
            comparison.push_str("; historical formula only; no historical result");
        }
        lines.push(format!(
            "| {} | {old} | {current} | {comparison} |",
            markdown_text(&row["id"])
        ));
    }
    let omitted = node["omitted_readings"].as_u64().unwrap_or(0);
    if omitted > 0 {
        lines.push(String::new());
        lines.push(format!("{omitted} dependency rows omitted by the {PREMISE_ROWS}-row limit; read the record for the complete dependencies."));
    }
    lines
}

fn rendered_fields(packet: &J, id: &str, body: &Map<String, J>) -> Vec<(J, J)> {
    if let Some(fields) = packet["__ordinary_body_fields"]
        .get(id)
        .and_then(J::as_array)
    {
        return fields
            .iter()
            .map(|field| (field[0].clone(), field[1].clone()))
            .collect();
    }
    body.iter()
        .map(|(field, value)| (json!(field), value.clone()))
        .collect()
}

/// Render an already projected packet as the portable text export.
pub fn render_markdown(packet: &J, details: bool) -> String {
    let mut lines = vec![
        "## Knowledge excerpt".into(),
        String::new(),
        boundaries(packet),
        String::new(),
    ];
    let legend = state_legend(packet);
    if !legend.is_empty() {
        lines.push(format!("{}.", legend.join("; ")));
        lines.push(String::new());
    }
    let h = packet["hypotheses"].as_u64().unwrap_or(0);
    if h > 0 {
        lines.push(format!("Base record; {h} hypotheses are not expanded. Conflict flags compare readable hypotheses, not every possible alternative."));
        lines.push(String::new());
    }
    let c = packet["contributions"].as_u64().unwrap_or(0);
    if c > 0 {
        lines.push(format!(
            "{c} project contributions are not expanded; use open, pull or knowledge snapshot."
        ));
        lines.push(String::new());
    }
    for (name, error) in packet["hypothesis_errors"]
        .as_object()
        .into_iter()
        .flatten()
    {
        lines.push(format!(
            "Unreadable hypothesis {}: {}",
            markdown_text(&json!(name)),
            markdown_text(error)
        ));
        lines.push(String::new());
    }
    let nodes = packet["nodes"].as_object().unwrap();
    let roles = packet["fields"].as_object().unwrap();
    for id in node_order(packet) {
        let node = &nodes[&id];
        let status = node["states"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(J::as_str)
            .map(str::to_uppercase)
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!(
            "### {} — {}{}",
            markdown_text(&json!(id)),
            markdown_text(&node["kind"]),
            if status.is_empty() {
                String::new()
            } else {
                format!(" · {status}")
            }
        ));
        lines.push(String::new());
        if node["missing"] == true {
            lines.push("Referenced ID is absent from the base record.".into());
            lines.push(String::new());
            continue;
        }
        let fallback;
        let body = if let Some(v) = node["body"].as_object() {
            v
        } else {
            fallback = Map::from_iter([("v".into(), node["body"].clone())]);
            &fallback
        };
        let judgment = node["kind"] == "judgment";
        if packet["profile"] == "core/v1" {
            lines.push(format!(
                "- shared assessment: {}",
                clipped(&node["status_text"], FIELD_CHARS)
            ));
            lines.push(String::new());
        }
        let special = if judgment {
            ["deps", "snapshot", "predicate"]
                .iter()
                .filter_map(|r| roles.get(*r).and_then(J::as_str))
                .collect::<HashSet<_>>()
        } else {
            HashSet::new()
        };
        for (field_value, value) in rendered_fields(packet, &id, body) {
            let field = field_value.as_str().unwrap_or_default();
            if special.contains(field) {
                continue;
            }
            let mut label = if field == "v" {
                J::String("current".into())
            } else {
                field_value.clone()
            };
            if judgment && field == "reopened_by" {
                label = json!(format!(
                    "{} (human condition; not evaluated)",
                    plain(&label)
                ));
            } else if judgment && matches!(field, "blocked_on" | "unverified" | "status") {
                label = json!(format!("{} (recorded declaration)", plain(&label)));
            }
            let shown = if field == "rule" && value.is_object() {
                if packet["profile"] == "core/v1" {
                    json!(render_expression(&value).unwrap_or_else(|_| plain(&value)))
                } else {
                    json!(crate::public_ordinary_readers::predicate_text(
                        &V::from_json(&value).unwrap_or(V::Null),
                    ))
                }
            } else {
                value
            };
            lines.push(format!(
                "- {}: {}",
                markdown_text(&label),
                clipped(&shown, FIELD_CHARS)
            ));
        }
        if let Some(calc) = node.get("calculation") {
            lines.push(if calc["value"].is_null() {
                format!(
                    "- calculated current: unavailable — {}",
                    clipped(&calc["reason"], FIELD_CHARS)
                )
            } else {
                format!(
                    "- calculated current: {}",
                    clipped(&reading_value(&calc["value"]), FIELD_CHARS)
                )
            });
        }
        if judgment {
            let cond = &node["condition"];
            let declared = truthy(Some(&cond["expression"]));
            if declared {
                let outcome = match cond["result"].as_bool() {
                    Some(true) => "true on current recorded values",
                    Some(false) => "false on current recorded values",
                    None => "not evaluated: missing or unsupported reading/condition",
                };
                let shown = cond
                    .get("expression_text")
                    .filter(|v| !v.is_null())
                    .unwrap_or(&cond["expression"]);
                let predicate = roles
                    .get("predicate")
                    .and_then(J::as_str)
                    .unwrap_or("wrong_if");
                lines.push(format!(
                    "- {}: {} → {outcome}.",
                    markdown_text(&json!(predicate)),
                    clipped(shown, FIELD_CHARS)
                ));
            } else {
                lines.push("- No executable condition declared.".into());
            }
            for (key, prefix, suffix) in [
                (
                    "undeclared_reads",
                    "- Condition reads undeclared dependencies: ",
                    ". Changes to these inputs do not follow the declared dependency links.",
                ),
                (
                    "missing_reads",
                    "- Condition references entries missing from this record: ",
                    ".",
                ),
                (
                    "unparsed_mentions",
                    "- Unparsed condition mentions undeclared entries: ",
                    ". Executable reads could not be determined.",
                ),
            ] {
                let values = cond[key]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(J::as_str)
                    .collect::<Vec<_>>();
                if !values.is_empty() {
                    lines.push(format!(
                        "{prefix}{}{suffix}",
                        clipped(&json!(values.join(", ")), FIELD_CHARS)
                    ));
                }
            }
            lines.push(String::new());
            lines.extend(reading_table(node));
        }
        if details {
            lines.push(String::new());
            lines.push("Recorded fields:".into());
            for (field_value, value) in rendered_fields(packet, &id, body) {
                let field = field_value.as_str().unwrap_or_default();
                let label = if judgment && roles.get("snapshot").and_then(J::as_str) == Some(field)
                {
                    json!(format!("historical snapshot ({field})"))
                } else {
                    field_value
                };
                lines.push(format!(
                    "- {}: {}",
                    markdown_text(&label),
                    clipped(&value, FIELD_CHARS)
                ));
            }
        }
        lines.push(String::new());
    }
    lines.extend([
        "### Recorded links".into(),
        String::new(),
        "Left names right; impact traversal keeps this direction.".into(),
        String::new(),
    ]);
    let meanings = BTreeMap::from([
        ("rests_on", "declared premise"),
        ("from", "recorded source"),
        ("rule_reads", "input named by a rule"),
        ("impact", "executed read projected from shared findings"),
        (
            "potential_impact",
            "potential read projected from shared findings",
        ),
        (
            "refutes",
            "recorded refutation, not a dependency or a newly evaluated condition",
        ),
    ]);
    let edges = packet["edges"].as_array().unwrap();
    for e in edges {
        let relation = e[1].as_str().unwrap_or_default();
        lines.push(format!(
            "- {} → {relation} → {} ({}).",
            markdown_text(&e[0]),
            markdown_text(&e[2]),
            meanings[relation]
        ));
    }
    if edges.is_empty() {
        lines.push("No links shown within this selection.".into());
    }
    lines.push(String::new());
    lines.push("Conditions use the full base record; current readings outside this excerpt are marked. External source contents and page-only checks are not evaluated here. Read an entry with kpop pull ID; kpop export ID --details shows recorded fields. The source YAML remains authoritative.".into());
    lines.push(String::new());
    lines.join("\n")
}

fn mermaid_text(value: &J) -> String {
    plain(value)
        .chars()
        .map(|c| {
            if crate::python_text::alnum(c) || " .,_-:/".contains(c) {
                c.to_string()
            } else {
                format!("#{};", c as u32)
            }
        })
        .collect()
}
fn truthy(value: Option<&J>) -> bool {
    match value {
        None | Some(J::Null) | Some(J::Bool(false)) => false,
        Some(J::String(v)) => !v.is_empty(),
        Some(J::Array(v)) => !v.is_empty(),
        Some(J::Object(v)) => !v.is_empty(),
        Some(J::Number(v)) => v.as_f64() != Some(0.0),
        _ => true,
    }
}
fn named(body: &Map<String, J>) -> Option<String> {
    ["name", "title", "label", "what", "desc"]
        .iter()
        .find_map(|field| body.get(*field).filter(|value| truthy(Some(value))))
        .map(plain)
        .filter(|name| !name.is_empty())
}

/// Render an already projected packet as the optional Mermaid representation.
pub fn render_mermaid(packet: &J) -> String {
    let mut lines=vec!["flowchart TB".into(),"  accTitle: Knowledge excerpt".into(),"  accDescr: Declared links only. MOVED needs review; FALSIFIED means a declared condition is true.".into(),format!("  %% {}",boundaries(packet))];
    let order = node_order(packet);
    let aliases = order
        .iter()
        .enumerate()
        .map(|(i, id)| (id.clone(), format!("n{i}")))
        .collect::<HashMap<_, _>>();
    let nodes = packet["nodes"].as_object().unwrap();
    let mut clipped_count = 0;
    for id in &order {
        let node = &nodes[id];
        let fallback;
        let body = if let Some(v) = node["body"].as_object() {
            v
        } else {
            fallback = Map::from_iter([("v".into(), node["body"].clone())]);
            &fallback
        };
        let named_value = named(body);
        let verdict = body.get("verdict").filter(|v| truthy(Some(v)));
        let chosen = verdict
            .map(plain)
            .or_else(|| named_value.clone())
            .or_else(|| body.get("v").map(plain))
            .unwrap_or_else(|| plain(&node["kind"]));
        let mut label = if node["missing"] == true {
            "Referenced ID is absent from the base record".into()
        } else {
            chosen
        };
        if node["missing"] != true
            && body.contains_key("v")
            && named_value.is_none()
            && verdict.is_none()
            && truthy(body.get("unit"))
        {
            label.push(' ');
            label.push_str(&plain(&body["unit"]));
        }
        let mut shortened = label.chars().count() > LABEL_CHARS;
        if shortened {
            label = format!("{}…", label.chars().take(LABEL_CHARS).collect::<String>());
        }
        let status = if node["missing"] == true {
            "MISSING".into()
        } else {
            node["states"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(J::as_str)
                .map(str::to_uppercase)
                .collect::<Vec<_>>()
                .join(", ")
        };
        let mut parts = vec![
            id.clone(),
            format!(
                "{}{}",
                node["kind"].as_str().unwrap_or_default(),
                if status.is_empty() {
                    String::new()
                } else {
                    format!(": {status}")
                }
            ),
        ];
        if node["missing"] != true
            && body.contains_key("v")
            && (named_value.is_some() || verdict.is_some())
        {
            let mut value = plain(&body["v"]);
            if truthy(body.get("unit")) {
                value.push(' ');
                value.push_str(&plain(&body["unit"]));
            }
            shortened |= value.chars().count() > LABEL_CHARS;
            let short = value.chars().take(LABEL_CHARS).collect::<String>();
            parts.extend(crate::python_text::wrap(
                &format!(
                    "v: {short}{}",
                    if value.chars().count() > LABEL_CHARS {
                        "…"
                    } else {
                        ""
                    }
                ),
                36,
            ));
        }
        clipped_count += usize::from(shortened);
        parts.extend(crate::python_text::wrap(&label, 36));
        let rendered = parts
            .iter()
            .map(|p| mermaid_text(&json!(p)))
            .collect::<Vec<_>>()
            .join("<br/>");
        lines.push(format!("  {}[\"{rendered}\"]", aliases[id]));
    }
    for e in packet["edges"].as_array().into_iter().flatten() {
        let relation = e[1].as_str().unwrap_or_default();
        lines.push(format!(
            "  {} {}|{relation}| {}",
            aliases[e[0].as_str().unwrap_or_default()],
            if relation == "refutes" { "-.->" } else { "-->" },
            aliases[e[2].as_str().unwrap_or_default()]
        ));
    }
    let legend = state_legend(packet);
    if !legend.is_empty() {
        lines.push(format!(
            "  export_legend[\"{}\"]",
            legend
                .iter()
                .map(|v| mermaid_text(&json!(v)))
                .collect::<Vec<_>>()
                .join("<br/>")
        ));
    }
    let note = [
        "Scope (not a record)".into(),
        packet["snapshot"].as_str().unwrap_or_default().into(),
        format!("{} nodes outside selection", packet["outside_nodes"]),
        format!("{} boundary links", packet["frontier_edges"]),
        format!("{} internal links omitted", packet["omitted_edges"]),
        format!("{clipped_count} labels shortened ({LABEL_CHARS} chars)"),
        format!("{} hypotheses not expanded", packet["hypotheses"]),
        format!(
            "{} project contributions not expanded",
            packet["contributions"]
        ),
        format!(
            "{} unreadable hypotheses",
            packet["hypothesis_errors"].as_object().map_or(0, Map::len)
        ),
        "Details: read the text export".into(),
    ];
    let text = note
        .iter()
        .map(|v| mermaid_text(&json!(v)))
        .collect::<Vec<_>>()
        .join("<br/>");
    lines.push(format!("  export_note[\"{text}\"]"));
    lines.push("  style export_note stroke-dasharray: 4 4".into());
    lines.push(String::new());
    lines.join("\n")
}
pub fn render(context: &CapturedAssessment, seeds: &[String], options: &Options) -> Result<String> {
    render_packet(&project(context, seeds, options)?, options)
}

fn render_packet(packet: &J, options: &Options) -> Result<String> {
    if options.details && options.format == Format::Mermaid {
        return Err(Error(
            "--details needs a text format: markdown or markdown-mermaid".into(),
        ));
    }
    Ok(match options.format {
        Format::Markdown => render_markdown(packet, options.details),
        Format::Mermaid => render_mermaid(packet),
        Format::MarkdownMermaid => format!(
            "{}\n### Optional Mermaid diagram\n\nRequires a Mermaid renderer; otherwise this is a code block. The text above is the readable representation.\n\n```mermaid\n{}```\n",
            render_markdown(packet, options.details),
            render_mermaid(packet)
        ),
    })
}
