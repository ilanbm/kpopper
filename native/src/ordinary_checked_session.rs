//! Re-evaluated checked-reader/v1 sessions over an ordinary captured graph.

use crate::{
    Error, Result,
    history_contract::{field, map, string_is, text},
    history_view::map_mut,
    identity::sha256,
    ordinary_reader,
    public_ordinary_readers::{OrdinarySessionData, Projection},
    reasoning_runtime::{OperationalBounds, Runtime},
    require,
    source_capture::CapturedSource,
    value::TypedValue as V,
};
use serde_json::{Map, Value as J, json};
use std::collections::{BTreeMap, BTreeSet};

const RULES: &str = include_str!("../../scripts/session/rules.txt");
#[derive(Clone, Debug, PartialEq, Eq)]
enum Entry {
    Group(String),
    Node(String),
}
impl Entry {
    fn key(&self) -> &str {
        match self {
            Self::Group(key) | Self::Node(key) => key,
        }
    }
}
#[derive(Clone, Debug, Default)]
struct Navigation {
    groups: BTreeMap<String, BTreeSet<String>>,
    leaves: BTreeMap<String, String>,
    children: BTreeMap<String, BTreeSet<String>>,
    direct: BTreeMap<String, Vec<String>>,
}
impl Navigation {
    fn successors(&self, path: &str) -> Vec<Entry> {
        self.children
            .get(path)
            .into_iter()
            .flatten()
            .cloned()
            .map(Entry::Group)
            .chain(
                self.direct
                    .get(path)
                    .into_iter()
                    .flatten()
                    .cloned()
                    .map(Entry::Node),
            )
            .collect()
    }
    fn members(&self, entry: &Entry) -> BTreeSet<String> {
        match entry {
            Entry::Group(key) => self.groups.get(key).cloned().unwrap_or_default(),
            Entry::Node(key) => BTreeSet::from([key.clone()]),
        }
    }
    fn balance(&mut self, path: &str) {
        let entries = self.successors(path);
        if entries.len() > 64 {
            let width = 16.max(entries.len().div_ceil(16));
            self.children.remove(path);
            self.direct.remove(path);
            for (index, chunk) in entries.chunks(width).enumerate() {
                let bucket = format!("{}/@ids{}", path.trim_end_matches('/'), index + 1);
                self.children
                    .entry(path.into())
                    .or_default()
                    .insert(bucket.clone());
                for entry in chunk {
                    let members = self.members(entry);
                    self.groups
                        .entry(bucket.clone())
                        .or_default()
                        .extend(members);
                    match entry {
                        Entry::Group(key) => {
                            self.children
                                .entry(bucket.clone())
                                .or_default()
                                .insert(key.clone());
                        }
                        Entry::Node(key) => self
                            .direct
                            .entry(bucket.clone())
                            .or_default()
                            .push(key.clone()),
                    }
                }
            }
        }
        for child in self.children.get(path).cloned().unwrap_or_default() {
            self.balance(&child);
        }
    }
    fn layers(&self, path: &str) -> Vec<Vec<Entry>> {
        let mut levels = vec![vec![Entry::Group(path.into())]];
        loop {
            let next = levels
                .last()
                .unwrap()
                .iter()
                .flat_map(|entry| {
                    let children = match entry {
                        Entry::Group(key) => self.successors(key),
                        Entry::Node(_) => vec![],
                    };
                    if children.is_empty() {
                        vec![entry.clone()]
                    } else {
                        children
                    }
                })
                .collect::<Vec<_>>();
            if next == *levels.last().unwrap() {
                return levels;
            }
            levels.push(next);
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Opening {
    pub text: String,
    pub packet: J,
    pub tokens: usize,
}

#[derive(Clone, Debug)]
pub struct OrdinarySession {
    project: String,
    revision: String,
    graph: J,
    scan: J,
    groups: BTreeMap<String, BTreeSet<String>>,
    navigation: Navigation,
    proposals: BTreeMap<String, J>,
    directory_keys: BTreeMap<String, Vec<String>>,
}

fn canonical(value: &J) -> Result<String> {
    Ok(serde_json::to_string(value)?)
}

fn pointer<'a>(mut value: &'a J, path: &str) -> Result<&'a J> {
    require(
        path.starts_with('/'),
        "field selector must be a JSON pointer",
    )?;
    for part in path[1..].split('/') {
        let key = part.replace("~1", "/").replace("~0", "~");
        value = match value {
            J::Object(map) => map.get(&key),
            J::Array(values) => key
                .parse::<usize>()
                .ok()
                .and_then(|index| values.get(index)),
            _ => None,
        }
        .ok_or_else(|| Error("unknown field".into()))?;
    }
    Ok(value)
}

fn card(bundle: &J) -> Result<String> {
    let mut lines = vec![format!(
        "RECORDED CLAIM {}: {}",
        bundle["id"]
            .as_str()
            .ok_or_else(|| Error("invalid ordinary assessment".into()))?,
        bundle["recorded_claim"]
            .as_str()
            .ok_or_else(|| Error("invalid ordinary assessment".into()))?
    )];
    if !bundle["recorded_status"].is_null() {
        lines.push(format!(
            "RECORDED STATUS (not evaluated): {}",
            canonical(&bundle["recorded_status"])?
        ));
    }
    lines.push("PREMISES — current recorded value / value at review:".into());
    for premise in bundle["premises"]
        .as_array()
        .ok_or_else(|| Error("invalid ordinary assessment".into()))?
    {
        let changed = match premise["changed"].as_bool() {
            Some(true) => "changed",
            Some(false) => "unchanged",
            None => "unknown",
        };
        let current = if premise["current_recorded_value"].is_null() {
            "UNKNOWN".into()
        } else {
            canonical(&premise["current_recorded_value"])?
        };
        let old = if premise["changed"] == false {
            "same".into()
        } else if premise["value_at_review"].is_null() {
            "UNKNOWN".into()
        } else {
            canonical(&premise["value_at_review"])?
        };
        let mut line = format!(
            "{}: {current} / {old} [{changed}]",
            premise["id"].as_str().unwrap_or("")
        );
        if premise["rule_changed"] == true {
            line.push_str("; formula changed");
        }
        if premise["role"] == "prior_premise" {
            line.push_str("; confidence belongs to this premise only");
        }
        lines.push(line);
    }
    let falsifier = &bundle["falsifier"];
    let outcome = match falsifier["holds_on_current_values"].as_bool() {
        Some(true) => "TRIGGERED",
        Some(false) => "NOT TRIGGERED",
        None => "UNKNOWN",
    };
    lines.push(format!(
        "EXECUTABLE FALSIFIER FOR {}: {} => {outcome} ({})",
        bundle["id"].as_str().unwrap_or(""),
        expression_text(&falsifier["expression"]),
        falsifier["reason"].as_str().unwrap_or("")
    ));
    let trigger = match bundle["mechanical_review_trigger"].as_bool() {
        Some(true) => "YES",
        Some(false) => "NO",
        None => "UNKNOWN",
    };
    lines.push(format!(
        "MECHANICAL REVIEW TRIGGER: {trigger}; human conditions are not evaluated"
    ));
    if !bundle["human_reopener"]["declaration"].is_null() {
        lines.push(format!(
            "HUMAN RE-OPENER (not evaluated): {}",
            canonical(&bundle["human_reopener"]["declaration"])?
        ));
    }
    if !bundle["declared_gap"]["declaration"].is_null() {
        lines.push(format!(
            "RECORDED blocked_on DECLARATION (not evaluated): {}",
            canonical(&bundle["declared_gap"]["declaration"])?
        ));
    }
    lines.push(format!(
        "JUDGMENT CONFIDENCE: {}",
        if bundle["declared_judgment_confidence"].is_null() {
            "not recorded; do not inherit premise confidence".into()
        } else {
            canonical(&bundle["declared_judgment_confidence"])?
        }
    ));
    lines.push(
        "Scope: this record snapshot; source truth and action authority are not certified.".into(),
    );
    Ok(lines.join("\n"))
}

fn expression_text(value: &J) -> String {
    if !value.is_object() {
        return V::from_json(value)
            .map(|value| {
                if crate::history_view::truth(&value) {
                    crate::source_text::ordinary_python_str(&value)
                } else {
                    String::new()
                }
            })
            .unwrap_or_default();
    }
    let value_typed = V::from_json(value).unwrap();
    let predicate = matches!(
        value["op"].as_str(),
        Some("eq" | "ne" | "lt" | "le" | "gt" | "ge")
    ) || crate::reasoning_language::legacy_expression_detailed(&value_typed, true)
        .is_ok();
    if let Err(error) =
        crate::reasoning_language::legacy_expression_detailed(&value_typed, predicate)
    {
        return format!("<invalid expression: {}>", error.0);
    }
    if let Some(expr) = value.get("expr").and_then(J::as_str) {
        return expr.into();
    }
    fn render(value: &J) -> String {
        if let Some(text) = value
            .get("ref")
            .or_else(|| value.get("num"))
            .and_then(J::as_str)
        {
            return text.into();
        }
        if let Some(text) = value.get("text") {
            return serde_json::to_string(text).unwrap();
        }
        if let Some(boolean) = value.get("bool") {
            return boolean.to_string();
        }
        let symbol = match value["op"].as_str().unwrap() {
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
            _ => unreachable!(),
        };
        format!(
            "({} {symbol} {})",
            render(&value["args"][0]),
            render(&value["args"][1])
        )
    }
    render(value)
}

impl OrdinarySession {
    pub fn capture(
        capture: &CapturedSource,
        runtime: &Runtime,
        project: &str,
        navigation: Option<&V>,
    ) -> Result<Self> {
        capture.require_ordinary_reader()?;
        let context = capture.ordinary_context();
        let (document, contributed) = with_contributions(capture)?;
        let projection = Projection::new(
            document.as_ref().unwrap_or(capture.ordinary_document()),
            map(capture.hypotheses())?,
            map(&map(&context)?["conflicts"])?,
            capture.reader_lines()?,
            Some(runtime),
        )?;
        let data = projection.session_data(&contributed.keys().cloned().collect())?;
        let mut directory_keys = source_directory_keys(capture.source(), &data.sections);
        let program = runtime
            .ordinary_program()
            .ok_or_else(|| Error("ordinary expression program is not configured".into()))?;
        let mut graph = graph(capture, data, &contributed, project, program.provenance()?)?;
        apply_profile(&mut graph, navigation)?;
        let navigation = navigation_index(&graph)?;
        graph["navigation_routes"] = json!(navigation.groups);
        graph["navigation_leaf_routes"] = json!(navigation.leaves);
        add_protocol_directory_keys(&mut directory_keys, &graph);
        add_hypothesis_directory_keys(&mut directory_keys, &graph, capture);
        let scan = program.scan(&graph, &OperationalBounds::default())?;
        let snapshot = sha256(canonical(&graph)?.as_bytes());
        let revision = sha256(canonical(&json!({"project":project,"graph":snapshot}))?.as_bytes());
        let groups = navigation.groups.clone();
        Ok(Self {
            project: project.into(),
            revision,
            graph,
            scan,
            groups,
            navigation,
            proposals: BTreeMap::new(),
            directory_keys,
        })
    }

    pub fn revision(&self) -> &str {
        &self.revision
    }
    pub fn expect(&self, revision: &str) -> Result<()> {
        require(
            revision == self.revision,
            "project record changed or revision belongs elsewhere; reopen",
        )
    }
    pub fn with_proposals(mut self, proposals: BTreeMap<String, J>) -> Result<Self> {
        for (id, proposal) in &proposals {
            crate::checked_session_store::validate_stored_proposal(
                &format!("proposal-{id}.json"),
                &self.project,
                proposal,
            )?;
            let mut keys = proposal
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            keys.push("stale_base".into());
            self.directory_keys
                .insert(format!("proposal:{id}"), keys.clone());
            self.directory_keys.insert(format!("pending#/{id}"), keys);
        }
        self.directory_keys
            .insert("pending".into(), proposals.keys().cloned().collect());
        self.proposals = proposals;
        Ok(self)
    }

    pub(crate) fn with_navigation_order(
        mut self,
        profile: Option<&crate::history_yaml::OrdinaryValue>,
    ) -> Self {
        if let Some(orientation @ crate::history_yaml::OrdinaryValue::Map(fields)) =
            profile.and_then(|profile| profile.get("orientation"))
            && !fields.is_empty()
        {
            index_directory_keys(orientation, "orientation", &mut self.directory_keys);
        }
        self
    }

    fn pending(&self) -> J {
        J::Object(
            self.proposals
                .iter()
                .map(|(id, proposal)| {
                    let mut value = proposal.clone();
                    value["stale_base"] = json!(value["base_revision"] != self.revision);
                    (id.clone(), value)
                })
                .collect(),
        )
    }

    pub fn opening<F>(&self, tokens: usize, count: F) -> Result<Opening>
    where
        F: Fn(&str) -> usize,
    {
        require((64..=65_536).contains(&tokens), "tokens must be 64..65536")?;
        let levels = self.navigation.layers("/");
        let mut frontier = levels[0].clone();
        let mut expanded = false;
        let mut packet = self.packet(&frontier)?;
        let mut text = self.render_open(&packet, expanded, &count)?;
        require(
            count(&text) <= tokens,
            &format!(
                "minimum complete opening needs {} reference tokens",
                count(&text)
            ),
        )?;
        let preferred = self.graph["navigation_profile"]["opening_depth"]
            .as_u64()
            .unwrap_or(1) as usize;
        for level in levels.iter().skip(1).take(preferred) {
            let candidate = self.packet(level)?;
            let rendered = self.render_open(&candidate, false, &count)?;
            if count(&rendered) > tokens {
                break;
            }
            frontier = level.clone();
            packet = candidate;
        }
        let rendered = self.render_open(&packet, true, &count)?;
        if count(&rendered) <= tokens {
            expanded = true;
        }
        for level in &levels {
            if level.len() < frontier.len() {
                continue;
            }
            let rendered = self.render_open(&self.packet(level)?, expanded, &count)?;
            if count(&rendered) > tokens {
                break;
            }
            frontier = level.clone();
        }
        (frontier, text) = self.refine(
            frontier,
            |cells| self.render_open(&self.packet(cells)?, expanded, &count),
            tokens,
            &count,
        )?;
        packet = self.packet(&frontier)?;
        for level in levels.iter().skip(1) {
            let mut candidate = packet.clone();
            candidate["link_map"] = json!(self.link_map(level));
            candidate["link_cells"] = json!(
                level
                    .iter()
                    .map(|entry| self.cell(entry))
                    .collect::<Result<Vec<_>>>()?
            );
            let rendered = self.render_open(&candidate, expanded, &count)?;
            if count(&rendered) > tokens {
                break;
            }
            packet = candidate;
            text = rendered;
        }
        packet["events_expanded"] = json!(expanded);
        Ok(Opening {
            tokens: count(&text),
            text,
            packet,
        })
    }

    /// Guard the exact opening packet against this immutable ordinary graph.
    pub fn guard_opening(
        &self,
        opening: &Opening,
        program: &crate::ordinary_runtime::Program,
    ) -> Result<J> {
        program.guard_view(&self.graph, &opening.packet, &OperationalBounds::default())
    }

    pub fn read<F>(
        &self,
        reference: &str,
        revision: &str,
        tokens: usize,
        offset: Option<usize>,
        count: F,
    ) -> Result<String>
    where
        F: Fn(&str) -> usize,
    {
        require(
            revision == self.revision,
            "project record changed or revision belongs elsewhere; reopen",
        )?;
        require((64..=65_536).contains(&tokens), "tokens must be 64..65536")?;
        if reference.starts_with('/') {
            require(offset.is_none(), "offset applies only to exact text fields")?;
            return self.branch_text(reference, tokens, &count);
        }
        if reference.starts_with("links:") && !reference.contains('#') {
            require(offset.is_none(), "offset applies only to exact text fields")?;
            return self.link_text(&reference[6..], tokens, &count);
        }
        let value = self.read_value(reference)?;
        if let Some(offset) = offset {
            let text = value
                .as_str()
                .ok_or_else(|| Error("offset must address this exact text field".into()))?;
            let chars = text.chars().collect::<Vec<_>>();
            require(
                offset <= chars.len(),
                "offset must address this exact text field",
            )?;
            let fragment = |end| {
                canonical(
                    &json!({"ref":reference,"revision":revision,"complete":offset==0&&end==chars.len(),"text_fragment":chars[offset..end].iter().collect::<String>(),"offset":offset,"next_offset":if end<chars.len(){Some(end)}else{None},"total_characters":chars.len(),"sha256":sha256(text.as_bytes())}),
                )
            };
            let mut low = offset;
            let mut high = chars.len();
            require(
                count(&fragment(low)?) <= tokens,
                "budget cannot carry fragment metadata",
            )?;
            while low < high {
                let middle = (low + high).div_ceil(2);
                if count(&fragment(middle)?) <= tokens {
                    low = middle
                } else {
                    high = middle - 1
                }
            }
            require(
                low != offset || offset == chars.len(),
                "budget cannot carry any text",
            )?;
            return fragment(low);
        }
        let response = canonical(
            &json!({"project":self.project,"revision":revision,"ref":reference,"complete":true,"value":value}),
        )?;
        if count(&response) <= tokens {
            return Ok(response);
        }
        let child_prefix = format!(
            "{reference}{}/",
            if reference.contains('#') { "" } else { "#" }
        );
        let base = reference
            .split_once('#')
            .map_or(reference, |(base, _)| base);
        let normalized = if self.graph["nodes"].get(base).is_some()
            && !["orientation", "assessment", "pending", "native"].contains(&base)
        {
            format!("node:{reference}")
        } else {
            reference.to_owned()
        };
        let children = value
            .as_object()
            .map(|map| {
                let mut keys = self
                    .directory_keys
                    .get(&normalized)
                    .cloned()
                    .unwrap_or_else(|| map.keys().cloned().collect());
                if base.starts_with("events:") && !reference.contains('#') {
                    keys.sort_by_key(|key| key[1..].parse::<usize>().unwrap());
                }
                keys.iter()
                    .map(|key| {
                        format!(
                            "{child_prefix}{}",
                            key.replace('~', "~0").replace('/', "~1")
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .or_else(|| {
                value
                    .as_array()
                    .map(|a| (0..a.len()).map(|i| format!("{child_prefix}{i}")).collect())
            })
            .unwrap_or_default();
        let mut value = json!({"complete":false,"required_tokens":count(&response),"ref":reference,"children":children,"next":if value.is_string(){"read this text with offset=0 for labeled fragments"}else{"read a field or raise tokens"}});
        let mut diagnostic = canonical(&value)?;
        if count(&diagnostic) > tokens {
            value.as_object_mut().unwrap().remove("children");
            diagnostic = canonical(&value)?;
        }
        require(
            count(&diagnostic) <= tokens,
            "budget cannot carry a complete read response",
        )?;
        Ok(diagnostic)
    }

    pub fn verify_claims(
        &self,
        judgment: &str,
        revision: &str,
        assertions: &[J],
        program: &crate::ordinary_runtime::Program,
    ) -> Result<String> {
        require(
            revision == self.revision,
            "project record changed or revision belongs elsewhere; reopen",
        )?;
        require(
            !assertions.is_empty(),
            "no assertions supplied; nothing was checked",
        )?;
        let result = program.assess(
            &self.graph,
            judgment,
            assertions,
            &OperationalBounds::default(),
        )?;
        if result["assertions_accepted"] != true {
            return Err(Error(canonical(
                &json!({"accepted":false,"checks":result["assertion_checks"]}),
            )?));
        }
        canonical(
            &json!({"accepted":true,"checks":result["assertion_checks"],"scope":"Only these assertions against this record, not prose or source-world truth."}),
        )
    }

    pub fn search<F>(
        &self,
        revision: &str,
        request: &crate::session_search::SearchRequest,
        count: F,
    ) -> Result<String>
    where
        F: Fn(&str) -> usize,
    {
        self.search_with_semantic(revision, request, count, None)
    }

    pub fn search_with_semantic<F>(
        &self,
        revision: &str,
        request: &crate::session_search::SearchRequest,
        count: F,
        semantic: Option<&dyn crate::session_search::SemanticProvider>,
    ) -> Result<String>
    where
        F: Fn(&str) -> usize,
    {
        require(
            revision == self.revision,
            "project record changed or revision belongs elsewhere; reopen",
        )?;
        let nodes = self.graph["nodes"]
            .as_object()
            .ok_or_else(|| Error("invalid ordinary session graph".into()))?
            .iter()
            .map(|(id, node)| (id.clone(), node.clone()))
            .collect::<BTreeMap<_, _>>();
        crate::session_search::search_checked_session(
            &self.project,
            &self.revision,
            &nodes,
            &self.groups,
            request,
            count,
            semantic,
        )
        .map(|response| response.text)
        .map_err(|error| Error(error.0))
    }

    pub fn contextualize<F>(
        &self,
        ids: &[String],
        revision: &str,
        options: &crate::checked_session::ContextOptions,
        count: F,
    ) -> Result<String>
    where
        F: Fn(&str) -> usize,
    {
        let node_ids = self.graph["nodes"]
            .as_object()
            .ok_or_else(|| Error("invalid ordinary session graph".into()))?
            .keys()
            .cloned()
            .collect();
        let edges = self.graph["edges"]
            .as_array()
            .ok_or_else(|| Error("invalid ordinary session graph".into()))?;
        crate::checked_session::contextualize_graph(
            crate::checked_session::ContextGraph {
                project: &self.project,
                revision: &self.revision,
                node_ids: &node_ids,
                edges,
                revision_error: "project record changed or revision belongs elsewhere; reopen",
            },
            ids,
            revision,
            options,
            |id| self.read_value(&format!("node:{id}")),
            count,
        )
    }

    pub fn validate_reference(&self, reference: &str) -> Result<()> {
        self.read_value(reference).map(|_| ())
    }

    fn assessment(&self, id: &str) -> Option<&J> {
        self.scan["assessments"]
            .as_array()?
            .iter()
            .find(|bundle| bundle["id"] == id)
    }

    fn read_value(&self, reference: &str) -> Result<J> {
        let (base, pointer_path) = reference.split_once('#').unwrap_or((reference, ""));
        let nodes = self.graph["nodes"]
            .as_object()
            .ok_or_else(|| Error("invalid ordinary session graph".into()))?;
        if nodes.contains_key(base)
            && !["orientation", "assessment", "pending", "native"].contains(&base)
        {
            return self.read_value(&format!("node:{reference}"));
        }
        let selected = pointer_path
            .strip_prefix('/')
            .unwrap_or("")
            .split('/')
            .next()
            .unwrap_or("");
        let mut value = if base == "orientation" {
            self.graph
                .get("orientation")
                .filter(|value| value.as_object().is_some_and(|value| !value.is_empty()))
                .cloned()
                .unwrap_or_else(|| json!({"text":self.graph["scope"],"basis":["record scope"]}))
        } else if let Some(id) = base.strip_prefix("checked:") {
            self.assessment(id)
                .cloned()
                .ok_or_else(|| Error("not_a_judgment".into()))?
        } else if let Some(id) = base.strip_prefix("node:") {
            let node = nodes
                .get(id)
                .ok_or_else(|| Error("unlisted operation or identifier; read / with this revision to list valid handles".into()))?;
            if node["kind"] == "judgment"
                && (!reference.contains('#')
                    || [
                        "epistemic_card",
                        "checked_bundle_ref",
                        "raw_ref",
                        "scope",
                        "state_tags_ref",
                        "state_tags_scope",
                    ]
                    .contains(&selected))
            {
                let bundle = self
                    .assessment(id)
                    .ok_or_else(|| Error("not_a_judgment".into()))?;
                let mut card = card(bundle)?;
                if self.scan["events"].as_object().is_some_and(|events| {
                    events
                        .values()
                        .any(|event| event["kind"] == "contested" && event["id"] == id)
                }) {
                    card.push_str("\nRECORDED CONFLICT: YES; this flag does not establish or refute the claim.");
                }
                json!({"body":node["body"],"epistemic_card":card,"checked_bundle_ref":format!("checked:{id}"),"raw_ref":format!("node:{id}#/body"),"scope":"Checks and source fields belong to this ID. Read a premise itself to establish its fields or their absence. World truth and action authority are not verified.","state_tags_ref":format!("node:{id}#/states"),"state_tags_scope":"Derived annotations, not the source status; read body for the recorded status and revision history."})
            } else {
                node.clone()
            }
        } else if let Some(handle) = base.strip_prefix("source:") {
            require(
                !nodes.contains_key(handle) || self.graph["sources"].get(handle).is_some(),
                &format!("this is a record entry; read node:{handle} with this revision"),
            )?;
            let source = self.graph["sources"]
                .get(handle)
                .cloned()
                .ok_or_else(|| Error("unlisted operation or identifier; read / with this revision to list valid handles".into()))?;
            let text = source["text"]
                .as_str()
                .ok_or_else(|| Error("invalid source".into()))?;
            require(
                source["sha256"] == sha256(text.as_bytes()),
                "source checksum mismatch",
            )?;
            source
        } else if base == "native" {
            self.graph["native_hypotheses"].clone()
        } else if base == "pending" {
            self.pending()
        } else if let Some(id) = base.strip_prefix("proposal:") {
            self.pending()
                .get(id)
                .cloned()
                .ok_or_else(|| Error("unknown proposal".into()))?
        } else if base == "alerts" {
            J::Object(
                nodes
                    .iter()
                    .filter_map(|(id, node)| {
                        let states = node["states"].as_array()?;
                        let alerts = states
                            .iter()
                            .filter(|state| {
                                [
                                    "moved",
                                    "falsified",
                                    "broken",
                                    "unchecked",
                                    "no_predicate",
                                    "contested",
                                ]
                                .contains(&state.as_str().unwrap_or(""))
                            })
                            .cloned()
                            .collect::<Vec<_>>();
                        (!alerts.is_empty()).then(|| (id.clone(), J::Array(alerts)))
                    })
                    .collect(),
            )
        } else if base == "assessment" {
            let mut events = self.scan["events"]
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>();
            events.sort_by_key(|key| key[1..].parse::<usize>().unwrap());
            json!({"counts":self.scan["counts"],"events":events,"errors":self.scan["errors"]})
        } else if let Some(key) = base.strip_prefix("event:") {
            self.scan["events"]
                .get(key)
                .cloned()
                .ok_or_else(|| Error("unknown event".into()))?
        } else if let Some(key) = base.strip_prefix("events:") {
            let ids = self.members(key)?;
            J::Object(
                self.scan["events"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .filter(|(_, event)| {
                        event["affected"].as_array().is_some_and(|affected| {
                            affected
                                .iter()
                                .any(|id| id.as_str().is_some_and(|id| ids.contains(id)))
                        })
                    })
                    .map(|(id, event)| (id.clone(), event.clone()))
                    .collect(),
            )
        } else if let Some(key) = base.strip_prefix("conditions:") {
            let ids = self.members(key)?;
            J::Object(
                self.scan["conditions"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .filter(|(id, _)| ids.contains(*id))
                    .map(|(id, v)| (id.clone(), v.clone()))
                    .collect(),
            )
        } else if let Some(key) = base.strip_prefix("edges:") {
            let members = self.members(key)?;
            J::Array(
                self.graph["edges"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|edge| {
                        members.contains(edge["from"].as_str().unwrap())
                            || members.contains(edge["to"].as_str().unwrap())
                    })
                    .cloned()
                    .collect(),
            )
        } else if let Some(key) = base.strip_prefix("links:") {
            J::Array(self.links(key)?)
        } else {
            return Err(Error(
                "unknown reference; read / with this revision to list valid handles".into(),
            ));
        };
        if !pointer_path.is_empty() {
            let checked = base == "orientation"
                || base == "assessment"
                || ["checked:", "event:", "events:", "links:", "conditions:"]
                    .iter()
                    .any(|prefix| base.starts_with(prefix))
                || (base.strip_prefix("node:").is_some_and(|id| {
                    nodes.get(id).is_some_and(|node| node["kind"] == "judgment")
                }) && [
                    "epistemic_card",
                    "checked_bundle_ref",
                    "raw_ref",
                    "scope",
                    "state_tags_ref",
                    "state_tags_scope",
                ]
                .contains(&selected));
            value = pointer(&value, pointer_path)
                .map_err(|error| {
                    if checked && error.0 == "unknown field" {
                        Error("unknown checked field".into())
                    } else {
                        error
                    }
                })?
                .clone();
        }
        Ok(value)
    }

    fn members(&self, key: &str) -> Result<BTreeSet<String>> {
        if let Some(group) = self.groups.get(key) {
            Ok(group.clone())
        } else if self.graph["nodes"].get(key).is_some() {
            Ok(BTreeSet::from([key.into()]))
        } else {
            Err(Error(
                "unknown node or topic; read / with this revision to list valid handles".into(),
            ))
        }
    }
    fn links(&self, key: &str) -> Result<Vec<J>> {
        let members = self.members(key)?;
        Ok(self.graph["edges"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|edge| edge["from"].as_str().is_some_and(|id| members.contains(id)))
            .cloned()
            .collect())
    }
    fn refine<F, C>(
        &self,
        mut frontier: Vec<Entry>,
        render: F,
        tokens: usize,
        count: &C,
    ) -> Result<(Vec<Entry>, String)>
    where
        F: Fn(&[Entry]) -> Result<String>,
        C: Fn(&str) -> usize,
    {
        let mut text = render(&frontier)?;
        let mut current_tokens = count(&text);
        loop {
            let mut best = None;
            for (index, entry) in frontier.iter().enumerate() {
                let Entry::Group(key) = entry else {
                    continue;
                };
                let children = self.navigation.successors(key);
                if children.is_empty() || children == [entry.clone()] {
                    continue;
                }
                let mut candidate = frontier[..index].to_vec();
                candidate.extend(children.iter().cloned());
                candidate.extend_from_slice(&frontier[index + 1..]);
                let rendered = render(&candidate)?;
                let size = count(&rendered);
                if size > tokens {
                    continue;
                }
                let added = size.saturating_sub(current_tokens).max(1) as f64;
                let score = (
                    children
                        .iter()
                        .filter(|entry| matches!(entry, Entry::Node(_)))
                        .count() as f64
                        / added,
                    (children.len() as f64 - 1.0) / added,
                    -(size as i64),
                    -(index as i64),
                );
                if best
                    .as_ref()
                    .is_none_or(|(previous, _, _, _)| score > *previous)
                {
                    best = Some((score, candidate, rendered, size));
                }
            }
            let Some((_, next, rendered, size)) = best else {
                return Ok((frontier, text));
            };
            frontier = next;
            text = rendered;
            current_tokens = size;
        }
    }

    fn branch_text<C: Fn(&str) -> usize>(
        &self,
        path: &str,
        tokens: usize,
        count: &C,
    ) -> Result<String> {
        require(
            self.groups.contains_key(path),
            "unknown branch; read / with this revision to list valid branches",
        )?;
        let render = |level: &[Entry]| -> Result<String> {
            let mut lines = vec![format!("revision={}", self.revision), format!("MAP {path}")];
            lines.extend(
                self.map_lines(
                    &level
                        .iter()
                        .map(|entry| self.cell(entry))
                        .collect::<Result<Vec<_>>>()?,
                    path,
                )?,
            );
            lines.push("Counts describe links, not IDs. Read node:ID, /topic or links:ID. Names are locators.".into());
            Ok(lines.join("\n") + "\n")
        };
        let mut chosen = None;
        for level in self.navigation.layers(path) {
            if count(&render(&level)?) > tokens {
                break;
            }
            chosen = Some(level);
        }
        let frontier =
            chosen.ok_or_else(|| Error("budget cannot carry the complete branch root".into()))?;
        Ok(self.refine(frontier, render, tokens, count)?.1)
    }

    fn link_text<C: Fn(&str) -> usize>(
        &self,
        key: &str,
        tokens: usize,
        count: &C,
    ) -> Result<String> {
        let edges = self.links(key)?;
        let mut lines = vec![
            format!("revision={}", self.revision),
            format!("LINKS {key} — source relation target; rests_on does not mean implies."),
        ];
        lines.extend(edges.iter().map(|edge| {
            format!(
                "{} {} {}",
                edge["from"].as_str().unwrap(),
                edge["rel"].as_str().unwrap(),
                edge["to"].as_str().unwrap()
            )
        }));
        let text = lines.join("\n") + "\n";
        if count(&text) <= tokens {
            return Ok(text);
        }
        if !self.groups.contains_key(key) {
            let diagnostic = format!(
                "revision={}\n{} links folded; read links:{key}#/INDEX (0..{}) or raise tokens.\n",
                self.revision,
                edges.len(),
                edges.len() as i64 - 1
            );
            require(
                count(&diagnostic) <= tokens,
                "budget cannot carry link metadata",
            )?;
            return Ok(diagnostic);
        }
        let mut chosen = None;
        for level in self.navigation.layers(key) {
            let mut lines = vec![
                format!("revision={}", self.revision),
                format!("LINKS {key} — grouped by source; expand a links: reference."),
            ];
            for entry in level {
                let cell = self.cell(&entry)?;
                lines.push(format!(
                    "+ {} [{}] {}",
                    cell["links_ref"].as_str().unwrap(),
                    cell["edge_indices"].as_array().unwrap().len(),
                    relation_counts(&cell["relation_counts"])
                ));
            }
            let text = lines.join("\n") + "\n";
            if count(&text) > tokens {
                break;
            }
            chosen = Some(text);
        }
        chosen.ok_or_else(|| Error("budget cannot carry link directory".into()))
    }

    fn link_map(&self, frontier: &[Entry]) -> Vec<J> {
        let owner = frontier
            .iter()
            .flat_map(|entry| {
                self.navigation
                    .members(entry)
                    .into_iter()
                    .map(move |id| (id, entry.key()))
            })
            .collect::<BTreeMap<_, _>>();
        let mut rows = BTreeMap::<(String, String, String), Vec<usize>>::new();
        for (index, edge) in self.graph["edges"].as_array().unwrap().iter().enumerate() {
            let target = edge["to"].as_str().unwrap();
            let key = (
                owner[edge["from"].as_str().unwrap()].to_owned(),
                edge["rel"].as_str().unwrap().to_owned(),
                owner
                    .get(target)
                    .map(|key| (*key).to_owned())
                    .unwrap_or_else(|| format!("unresolved:{target}")),
            );
            rows.entry(key).or_default().push(index);
        }
        rows.into_iter().map(|((source, relation, target), indices)| json!({"from":source,"rel":relation,"to":target,"edge_indices":indices})).collect()
    }

    fn packet(&self, frontier: &[Entry]) -> Result<J> {
        let cells = frontier
            .iter()
            .map(|entry| self.cell(entry))
            .collect::<Result<Vec<_>>>()?;
        let stale = self
            .proposals
            .values()
            .filter(|proposal| proposal["base_revision"] != self.revision)
            .count();
        Ok(
            json!({"schema":"kpopper.epistemic-view.v3","project":self.project,"revision":self.revision,"counts":self.scan["counts"],"cells":cells,"conditions_ref":"conditions:/","events":self.scan["events"],"orientation":self.graph.get("orientation").cloned().unwrap_or_else(||json!({})),"rules":RULES.trim(),"links":self.graph["edges"],"pending":self.proposals.len(),"stale_pending":stale,"native_hypotheses":self.graph["native_hypotheses"].as_object().map(|m|m.values().filter(|h|h["kind"]!="contribution").count()).unwrap_or(0),"contributions":self.graph["contributions"],"read_mode":self.graph["read_mode"]}),
        )
    }
    fn cell(&self, entry: &Entry) -> Result<J> {
        let key = entry.key();
        let members = self.navigation.members(entry);
        let edges = self.graph["edges"].as_array().unwrap();
        let indices = edges
            .iter()
            .enumerate()
            .filter(|(_, edge)| members.contains(edge["from"].as_str().unwrap()))
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        let mut counts = BTreeMap::<String, usize>::new();
        for index in &indices {
            *counts
                .entry(edges[*index]["rel"].as_str().unwrap().into())
                .or_default() += 1;
        }
        let has_state = |id: &str, state: &str| {
            self.graph["nodes"][id]["states"]
                .as_array()
                .unwrap()
                .contains(&json!(state))
        };
        Ok(
            json!({"kind":if matches!(entry, Entry::Node(_)){"node"}else{"group"},"key":key,
            "topic":if matches!(entry, Entry::Node(_)){self.navigation.leaves[key].as_str()}else{key},
            "members":members,"conflicts":members.iter().filter(|id|has_state(id,"contested")).collect::<Vec<_>>(),
            "questions":members.iter().filter(|id|has_state(id,"question")).collect::<Vec<_>>(),
            "attention":members.iter().filter(|id|self.scan["events"].as_object().unwrap().values().any(|event|event["affected"].as_array().unwrap().contains(&json!(id)))).collect::<Vec<_>>(),
            "edge_indices":indices,"relation_counts":counts,"links_ref":format!("links:{key}")}),
        )
    }
    fn cell_line(&self, cell: &J) -> Result<String> {
        let key = cell["key"].as_str().unwrap();
        let mut line = if cell["kind"] == "group" {
            format!("+ {key} [{}]", cell["members"].as_array().unwrap().len())
        } else {
            let reference = format!("node:{key}");
            format!(
                "- {}",
                if reference.chars().any(char::is_whitespace) {
                    canonical(&json!(reference))?
                } else {
                    reference
                }
            )
        };
        for (field, label) in [
            ("conflicts", "CONTESTED"),
            ("questions", "questions"),
            ("attention", "review"),
        ] {
            let n = cell[field].as_array().unwrap().len();
            if n != 0 {
                line += &format!(" {label}={n}");
            }
        }
        if !cell["relation_counts"].as_object().unwrap().is_empty() {
            line += &format!(" {}", relation_counts(&cell["relation_counts"]));
        }
        Ok(line)
    }
    fn map_lines(&self, cells: &[J], path: &str) -> Result<Vec<String>> {
        let mut lines = vec![];
        let mut context = None;
        for cell in cells {
            if cell["kind"] == "node" {
                let topic = cell["topic"].as_str().unwrap();
                if Some(topic) != context && topic != path {
                    lines.push(format!("@ {topic}"));
                }
                context = Some(topic);
            } else {
                context = None;
            }
            lines.push(self.cell_line(cell)?);
        }
        Ok(lines)
    }
    fn event_line<C: Fn(&str) -> usize>(&self, key: &str, event: &J, count: &C) -> Result<String> {
        let id = event["id"].as_str().unwrap();
        let kind = event["kind"].as_str().unwrap();
        let detail = if kind == "premise_change" {
            let mut values = format!(
                "seen={} current={}",
                canonical(&event["value_at_review"])?,
                canonical(&event["current_recorded_value"])?
            );
            if event["value_at_review"] == event["current_recorded_value"] {
                values = "formula changed".into();
            }
            if count(&values) > 32 {
                values = if event["role"] == "recorded_judgment_claim" {
                    "text changed"
                } else {
                    "recorded value changed"
                }
                .into();
            }
            format!("{id} {values}")
        } else if ["contested", "unreadable"].contains(&kind) {
            format!("{id} {}", kind.to_uppercase())
        } else {
            let predicate = &event["falsifier"];
            let state = match predicate["holds_on_current_values"].as_bool() {
                Some(true) => "TRIGGERED",
                Some(false) => "NOT_TRIGGERED",
                None => "UNKNOWN",
            };
            let detail = format!("{id} {} {state}", expression_text(&predicate["expression"]));
            if count(&detail) <= 48 {
                detail
            } else {
                format!("{id} {}", kind.to_uppercase())
            }
        };
        Ok(format!("@event:{key} {detail}"))
    }
    fn render_open<C: Fn(&str) -> usize>(
        &self,
        packet: &J,
        expanded: bool,
        count: &C,
    ) -> Result<String> {
        let c = &packet["counts"];
        let purpose = packet["orientation"]["text"]
            .as_str()
            .unwrap_or_else(|| self.graph["scope"].as_str().unwrap_or(""));
        let mut lines = vec![
            RULES.trim_end().into(),
            String::new(),
            format!("project={} revision={}", self.project, self.revision),
            format!("purpose={} @orientation", canonical(&json!(purpose))?),
            format!(
                "record: {} ids; {} judgments; contested={}; changed={}; unreadable={}",
                c["nodes"], c["judgments"], c["contested"], c["premise_changed"], c["errors"]
            ),
            format!(
                "executable falsifiers: triggered={} not_triggered={} unknown={} absent={}",
                c["falsifier_triggered"],
                c["falsifier_not_triggered"],
                c["falsifier_unknown"],
                c["falsifier_not_declared"]
            ),
            format!(
                "prose declarations (not evaluated): blocked_on={} reopened_by={} @conditions:/",
                c["gap_declared"], c["human_reopener_declared"]
            ),
            "current=recorded; seen=review snapshot. Not triggered does not mean verified.".into(),
            format!(
                "events={} @events:/",
                self.scan["events"].as_object().unwrap().len()
            ),
        ];
        if packet["pending"] != 0
            || packet["stale_pending"] != 0
            || packet["native_hypotheses"] != 0
        {
            lines.insert(
                lines.len() - 1,
                format!(
                    "pending={} stale={}; native hypotheses={}",
                    packet["pending"], packet["stale_pending"], packet["native_hypotheses"]
                ),
            );
        }
        for contribution in packet["contributions"].as_array().into_iter().flatten() {
            lines.push(format!(
                "PENDING {} {} {} @native",
                contribution["revision"]
                    .as_str()
                    .unwrap()
                    .chars()
                    .take(12)
                    .collect::<String>(),
                contribution["state"].as_str().unwrap(),
                canonical(&contribution["scope"])?
            ));
        }
        if expanded {
            let mut events = self.scan["events"]
                .as_object()
                .unwrap()
                .iter()
                .collect::<Vec<_>>();
            events.sort_by_key(|(key, _)| key[1..].parse::<usize>().unwrap());
            for (key, event) in events {
                lines.push(self.event_line(key, event, count)?);
            }
        }
        lines.push("MAP / — declared navigation; names do not establish claims:".into());
        lines.extend(self.map_lines(packet["cells"].as_array().unwrap(), "/")?);
        if let Some(rows) = packet
            .get("link_map")
            .and_then(J::as_array)
            .filter(|rows| !rows.is_empty())
        {
            lines.push(
                "LINK MAP — folded endpoints; all links counted; exact links at links:/".into(),
            );
            lines.extend(rows.iter().map(|row| {
                format!(
                    "{} {} {} [{}]",
                    row["from"].as_str().unwrap(),
                    row["rel"].as_str().unwrap(),
                    row["to"].as_str().unwrap(),
                    row["edge_indices"].as_array().unwrap().len()
                )
            }));
        }
        lines.push("Counts describe links: dependency_count=rests_on, source_count=from; not proof. Read node:ID, /topic, links:ID or @ref without @; pass revision.".into());
        Ok(lines.join("\n") + "\n")
    }
}

/// The source handle of a pending contribution, which the entries it adds name
/// as their `record_source`.
fn contribution_handle(name: &str) -> String {
    format!(
        "contribution.{}",
        name.strip_prefix("pending-").unwrap_or(name)
    )
}

/// The record as the session reads it, with each pending contribution's new
/// entries added. Contributions apply in ledger order: the first to add an id
/// supplies its body, and an id the record already holds keeps the record's
/// body. Returns the document when anything was added, and the source handle
/// of each added id.
fn with_contributions(capture: &CapturedSource) -> Result<(Option<V>, BTreeMap<String, String>)> {
    let hypotheses = map(capture.hypotheses())?;
    let mut document = capture.ordinary_document().clone();
    let held = map(&document)?
        .iter()
        .filter(|(section, _)| !["meta", "schema", "record", "also"].contains(&section.as_str()))
        .filter_map(|(_, members)| map(members).ok())
        .flat_map(|members| members.keys().cloned())
        .collect::<BTreeSet<_>>();
    let mut added = BTreeMap::new();
    for contribution in capture.contributions() {
        let revision = text(field(map(contribution)?, "revision")?)?;
        let name = format!("pending-{revision}");
        // A retired contribution keeps its status but is no longer layered.
        let Some(hypothesis) = hypotheses.get(&name).map(map).transpose()? else {
            continue;
        };
        if !hypothesis
            .get("kind")
            .is_some_and(|kind| string_is(kind, "contribution"))
        {
            continue;
        }
        for (collection, members) in
            crate::reasoning_fields::collections(field(hypothesis, "doc")?)?
        {
            for (id, body) in members {
                if held.contains(&id) || added.contains_key(&id) {
                    continue;
                }
                let section = map_mut(&mut document)?
                    .entry(collection.clone())
                    .or_insert(V::Null);
                if !matches!(section, V::Map(_)) {
                    *section = V::Map(Default::default());
                }
                map_mut(section)?.insert(id.clone(), body);
                added.insert(id, contribution_handle(&name));
            }
        }
    }
    Ok(((!added.is_empty()).then_some(document), added))
}

fn graph(
    capture: &CapturedSource,
    data: OrdinarySessionData,
    contributed: &BTreeMap<String, String>,
    project: &str,
    provenance: J,
) -> Result<J> {
    let input = capture.members().first().cloned();
    let mut sources = Map::new();
    let mut handles = BTreeMap::new();
    for path in capture.members() {
        if let Some(bytes) = capture.files().get(path) {
            let handle = if Some(path) == input.as_ref() {
                "record".into()
            } else {
                format!(
                    "record.{}",
                    &sha256(path.to_string_lossy().as_bytes())[..16]
                )
            };
            let text = String::from_utf8(bytes.clone())
                .map_err(|_| Error("record source is not UTF-8".into()))?;
            sources.insert(
                handle.clone(),
                json!({"text":text,"sha256":sha256(bytes),"location":path}),
            );
            handles.insert(path.clone(), handle);
        }
    }
    // Every layered contribution is a source, whether or not it added an entry.
    for (name, hypothesis) in map(capture.hypotheses())? {
        let hypothesis = map(hypothesis)?;
        if !hypothesis
            .get("kind")
            .is_some_and(|kind| string_is(kind, "contribution"))
        {
            continue;
        }
        let dumped = crate::public_ordinary_readers::python_safe_dump_unicode(
            &crate::history_yaml::OrdinaryValue::from_typed(field(hypothesis, "doc")?),
        )?;
        let digest = sha256(&dumped);
        let dumped = String::from_utf8(dumped)
            .map_err(|_| Error("contribution source is not UTF-8".into()))?;
        sources.insert(
            contribution_handle(name),
            json!({"text":dumped,"sha256":digest,"location":text(field(hypothesis, "path")?)?}),
        );
    }
    let native_hypotheses = session_hypotheses(capture, data.native_hypotheses)?;
    // Every observed contribution status, not only those still layered as
    // hypotheses: a retired contribution stays visible with its state.
    let contributions = capture
        .contributions()
        .iter()
        .map(|contribution| ordinary_reader::json_value(contribution, 0))
        .collect::<Result<Vec<_>>>()?;
    let mut nodes = Map::from_iter(data.nodes);
    for (id, node) in &mut nodes {
        if let Some(handle) = contributed.get(id) {
            node["record_source"] = json!(handle);
            continue;
        }
        let section = data.sections.get(id);
        let origin = section
            .and_then(|section| capture.origins().get(section))
            .and_then(|origins| origins.get(id))
            .and_then(|path| handles.get(path));
        if let Some(handle) = origin {
            node["record_source"] = json!(handle)
        } else {
            node["record_sources"] = json!(sources.keys().collect::<Vec<_>>())
        }
    }
    Ok(
        json!({"nodes":nodes,"edges":data.edges,"topics":data.topics,"scope":data.scope,"sources":sources,"native_hypotheses":native_hypotheses,"contributions":contributions,"knowledge_conflicts":data.knowledge_conflicts,"read_mode":map(&capture.ordinary_context())?["read_mode"].to_json()?,"origin":provenance,"project_context":project}),
    )
}

fn hypothesis_document_source(
    capture: &CapturedSource,
    hypothesis: &J,
) -> Option<crate::history_yaml::OrdinaryValue> {
    use crate::history_yaml::OrdinaryValue as S;
    let path = std::path::Path::new(hypothesis["path"].as_str()?);
    let S::Map(mut fields) =
        crate::history_yaml::decode_ordinary_source_value(capture.files().get(path)?).ok()?
    else {
        return None;
    };
    fields.retain(|(key, _)| key.text() != Some("hypothesis"));
    let source = S::Map(fields);
    (ordinary_reader::json_value(&source.projected(), 0)
        .ok()
        .as_ref()
        == Some(&hypothesis["doc"]))
    .then_some(source)
}

// provenance.bodies intentionally includes every mapping collection, even when
// collections_of excludes one because its members have incompatible shapes.
fn hypothesis_raw(
    source: &crate::history_yaml::OrdinaryValue,
) -> crate::history_yaml::OrdinaryValue {
    use crate::history_yaml::{OrdinaryKey, OrdinaryValue as S};
    let mut raw: Vec<(OrdinaryKey, S)> = vec![];
    if let S::Map(collections) = source {
        for (collection, members) in collections {
            if collection
                .text()
                .is_some_and(|name| ["meta", "schema", "record", "also"].contains(&name))
            {
                continue;
            }
            if let S::Map(members) = members {
                for (key, body) in members {
                    if let Some((_, previous)) = raw
                        .iter_mut()
                        .find(|(candidate, _)| candidate.python_eq(key))
                    {
                        *previous = body.clone();
                    } else {
                        raw.push((key.clone(), body.clone()));
                    }
                }
            }
        }
    }
    S::Map(raw)
}

fn session_hypotheses(capture: &CapturedSource, mut hypotheses: J) -> Result<J> {
    use crate::history_yaml::OrdinaryValue as S;
    for hypothesis in hypotheses
        .as_object_mut()
        .into_iter()
        .flat_map(|values| values.values_mut())
    {
        let document = V::from_json(&hypothesis["doc"])?;
        let source = hypothesis_document_source(capture, hypothesis)
            .unwrap_or_else(|| S::from_typed(&document));
        hypothesis.as_object_mut().unwrap().remove("document");
        hypothesis["raw"] = ordinary_reader::json_value(&hypothesis_raw(&source).projected(), 0)?;
        let ids = crate::reasoning_fields::collections(&document)?
            .into_values()
            .flat_map(|members| members.into_keys())
            .collect::<BTreeSet<_>>();
        hypothesis["ids"] = json!(ids);
    }
    Ok(hypotheses)
}

fn apply_profile(graph: &mut J, profile: Option<&V>) -> Result<()> {
    let Some(profile) = profile else {
        return Ok(());
    };
    let profile = ordinary_reader::json_value(profile, 0)?;
    if profile
        .as_object()
        .is_some_and(|profile| profile.is_empty())
    {
        return Ok(());
    }
    let groups = profile["groups"]
        .as_object()
        .ok_or_else(|| Error("profile must contain declared groups".into()))?;
    let node_ids = graph["nodes"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut assigned = BTreeMap::new();
    let mut assigned_values = Vec::new();
    let mut unmatched_ids = Vec::new();
    for (path, ids) in groups {
        require(
            !path.is_empty() && !path.split('/').any(str::is_empty),
            "invalid profile path",
        )?;
        for id in ids
            .as_array()
            .ok_or_else(|| Error("profile group must list IDs".into()))?
        {
            if id.is_array() || id.is_object() {
                return Err(Error(format!(
                    "unhashable type: '{}'",
                    if id.is_array() { "list" } else { "dict" }
                )));
            }
            if assigned_values.iter().any(|assigned| {
                let left = V::from_json(assigned).unwrap();
                let right = V::from_json(id).unwrap();
                crate::source_clock::python_equal(&left, &right)
            }) {
                return Err(Error("profile assigns an ID more than once".into()));
            }
            assigned_values.push(id.clone());
            let Some(id) = id.as_str() else {
                unmatched_ids.push(id.clone());
                continue;
            };
            require(
                assigned
                    .insert(
                        id.to_owned(),
                        path.split('/').map(str::to_owned).collect::<Vec<_>>(),
                    )
                    .is_none(),
                "profile assigns an ID more than once",
            )?;
        }
    }
    let mut topics = Map::new();
    for id in &node_ids {
        topics.insert(
            id.clone(),
            json!(assigned.get(id).cloned().unwrap_or_else(|| {
                std::iter::once("catalog".into())
                    .chain(
                        id.split('.')
                            .take(id.split('.').count().saturating_sub(1))
                            .map(str::to_owned),
                    )
                    .collect()
            })),
        );
    }
    let orientation = profile
        .get("orientation")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if orientation.as_object().is_none_or(|o| !o.is_empty()) {
        require(
            orientation["text"]
                .as_str()
                .is_some_and(|text| !text.is_empty())
                && orientation["basis"]
                    .as_array()
                    .is_some_and(|ids| !ids.is_empty()),
            "orientation needs text and basis",
        )?;
        for id in orientation["basis"].as_array().unwrap() {
            let id = id
                .as_str()
                .ok_or_else(|| Error("orientation basis must list IDs".into()))?;
            require(
                node_ids.contains(id),
                &format!("orientation basis missing: {id}"),
            )?;
        }
    }
    let depth = profile.get("opening_depth").cloned().unwrap_or(json!(1));
    require(
        depth.as_u64().is_some_and(|depth| (1..=8).contains(&depth)),
        "opening_depth must be 1..8",
    )?;
    graph["topics"] = J::Object(topics);
    graph["orientation"] = profile
        .get("orientation")
        .cloned()
        .unwrap_or_else(|| json!({}));
    unmatched_ids.extend(
        assigned
            .keys()
            .filter(|id| !node_ids.contains(*id))
            .map(|id| J::String(id.clone())),
    );
    let mixed_unmatched = unmatched_ids.iter().any(|value| {
        let string = value.is_string();
        unmatched_ids
            .iter()
            .any(|other| other.is_string() != string)
    });
    require(
        !mixed_unmatched,
        "'<' not supported between instances of 'str' and 'int'",
    )?;
    let type_name = |value: &J| {
        if value.is_string() {
            "str"
        } else if value.is_number() {
            "int"
        } else if value.is_boolean() {
            "bool"
        } else {
            "NoneType"
        }
    };
    let kind = unmatched_ids.first().map(|value| {
        if value.is_string() {
            0
        } else if value.is_number() || value.is_boolean() {
            1
        } else {
            2
        }
    });
    if unmatched_ids.iter().any(|value| {
        kind != Some(if value.is_string() {
            0
        } else if value.is_number() || value.is_boolean() {
            1
        } else {
            2
        })
    }) {
        let left = unmatched_ids.first().unwrap();
        let right = unmatched_ids
            .iter()
            .find(|value| type_name(value) != type_name(left))
            .unwrap();
        return Err(Error(format!(
            "'<' not supported between instances of '{}' and '{}'",
            type_name(right),
            type_name(left)
        )));
    }
    // Compare the binary float exactly against arbitrary-size integers, like
    // Python; converting every key to f64 loses ordering above 2**53.
    fn number(value: &J) -> (num_bigint::BigInt, num_bigint::BigInt) {
        use num_bigint::BigInt;
        match V::from_json(value).expect("profile keys came from typed values") {
            V::Integer(n) => (n.as_str().parse().unwrap(), BigInt::from(1)),
            V::Bool(b) => (BigInt::from(u8::from(b)), BigInt::from(1)),
            V::Float(f) => {
                let bits = f.get().to_bits();
                let exponent = ((bits >> 52) & 0x7ff) as i32;
                let fraction = bits & ((1u64 << 52) - 1);
                let (mantissa, shift) = if exponent == 0 {
                    (fraction, -1074)
                } else {
                    (fraction | (1u64 << 52), exponent - 1023 - 52)
                };
                let mut numerator = BigInt::from(mantissa);
                if bits >> 63 != 0 {
                    numerator = -numerator;
                }
                if shift >= 0 {
                    (numerator << shift as usize, BigInt::from(1))
                } else {
                    (numerator, BigInt::from(1) << (-shift) as usize)
                }
            }
            _ => unreachable!("only numeric profile keys are compared"),
        }
    }
    unmatched_ids.sort_by(|left, right| {
        if kind == Some(0) {
            left.as_str().cmp(&right.as_str())
        } else if kind == Some(1) {
            let (a, ad) = number(left);
            let (b, bd) = number(right);
            (a * bd).cmp(&(b * ad))
        } else {
            std::cmp::Ordering::Equal
        }
    });
    graph["navigation_profile"] = json!({"description":profile["description"].as_str().unwrap_or("Declared navigation."),"sha256":sha256(canonical(&profile)?.as_bytes()),"unmatched_ids":unmatched_ids,"opening_depth":depth});
    Ok(())
}

// Directory ordering is presentation metadata, kept outside the canonical graph
// and revision. Ordinary scalar keys retain Python's spelling and first-key
// identity after YAML numeric-key collisions.
fn index_directory_keys(
    value: &crate::history_yaml::OrdinaryValue,
    reference: &str,
    out: &mut BTreeMap<String, Vec<String>>,
) {
    use crate::history_yaml::OrdinaryValue as S;
    match value {
        S::Map(values) => {
            let keys = values
                .iter()
                .map(|(key, _)| crate::source_text::ordinary_python_str(key.scalar()))
                .collect::<Vec<_>>();
            out.insert(reference.into(), keys.clone());
            for ((_, value), key) in values.iter().zip(keys) {
                index_directory_keys(
                    value,
                    &format!(
                        "{reference}{}/{}",
                        if reference.contains('#') { "" } else { "#" },
                        key.replace('~', "~0").replace('/', "~1")
                    ),
                    out,
                );
            }
        }
        S::List(values) => {
            for (i, value) in values.iter().enumerate() {
                index_directory_keys(
                    value,
                    &format!(
                        "{reference}{}/{i}",
                        if reference.contains('#') { "" } else { "#" }
                    ),
                    out,
                );
            }
        }
        S::Scalar(_) => (),
    }
}

fn source_directory_keys(
    source: &crate::history_yaml::OrdinaryValue,
    sections: &BTreeMap<String, String>,
) -> BTreeMap<String, Vec<String>> {
    use crate::history_yaml::OrdinaryValue as S;
    let mut out = BTreeMap::new();
    for (id, section) in sections {
        if let Some(body) = source.get(section).and_then(|members| members.get(id)) {
            if matches!(body, S::Map(_)) {
                index_directory_keys(body, &format!("node:{id}#/body"), &mut out);
                index_directory_keys(body, &format!("node:{id}#/assessment_body"), &mut out);
            } else {
                out.insert(format!("node:{id}#/body"), vec!["v".into()]);
                index_directory_keys(body, &format!("node:{id}#/body/v"), &mut out);
                index_directory_keys(body, &format!("node:{id}#/source_body"), &mut out);
            }
        }
    }
    out
}
fn add_protocol_directory_keys(out: &mut BTreeMap<String, Vec<String>>, graph: &J) {
    out.insert(
        "orientation".into(),
        if graph
            .get("orientation")
            .and_then(J::as_object)
            .is_none_or(|value| value.is_empty())
        {
            vec!["text".into(), "basis".into()]
        } else {
            graph["orientation"]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect()
        },
    );
    out.insert(
        "assessment".into(),
        vec!["counts".into(), "events".into(), "errors".into()],
    );
    for (id, node) in graph["nodes"].as_object().unwrap() {
        let raw = [
            "kind",
            "states",
            "body",
            "source_body",
            "assessment_body",
            "assessment_fields",
            "record_source",
            "record_sources",
        ]
        .into_iter()
        .filter(|key| node.get(*key).is_some())
        .map(str::to_owned)
        .collect::<Vec<_>>();
        out.insert(format!("node:{id}#"), raw.clone());
        out.insert(
            format!("node:{id}"),
            if node["kind"] == "judgment" {
                [
                    "body",
                    "epistemic_card",
                    "checked_bundle_ref",
                    "raw_ref",
                    "scope",
                    "state_tags_ref",
                    "state_tags_scope",
                ]
                .map(str::to_owned)
                .to_vec()
            } else {
                raw
            },
        );
        if node["kind"] == "judgment" {
            out.insert(
                format!("node:{id}#/assessment_fields"),
                ["deps", "snapshot", "predicate"]
                    .map(str::to_owned)
                    .to_vec(),
            );
            let reference = format!("node:{id}#/assessment_body");
            if let Some(keys) = out.get_mut(&reference) {
                for (role, canonical) in [
                    ("deps", "rests_on"),
                    ("snapshot", "seen"),
                    ("predicate", "wrong_if"),
                ] {
                    if node["assessment_fields"][role].is_string()
                        && !keys.iter().any(|key| key == canonical)
                    {
                        keys.push(canonical.into());
                    }
                }
            }
            for (role, canonical) in [
                ("deps", "rests_on"),
                ("snapshot", "seen"),
                ("predicate", "wrong_if"),
            ] {
                if let Some(source) = node["assessment_fields"][role].as_str() {
                    let source_prefix = format!(
                        "node:{id}#/body/{}",
                        source.replace('~', "~0").replace('/', "~1")
                    );
                    let target_prefix = format!("{reference}/{canonical}");
                    let aliases = out
                        .iter()
                        .filter_map(|(path, keys)| {
                            path.strip_prefix(&source_prefix)
                                .filter(|tail| tail.is_empty() || tail.starts_with('/'))
                                .map(|tail| (format!("{target_prefix}{tail}"), keys.clone()))
                        })
                        .collect::<Vec<_>>();
                    out.extend(aliases);
                }
            }
        }
        if crate::reasoning_fields::BUILTINS.contains(&id.as_str()) {
            out.insert(
                format!("node:{id}#/body"),
                ["name", "v", "from"]
                    .into_iter()
                    .filter(|key| node["body"].get(*key).is_some())
                    .map(str::to_owned)
                    .collect(),
            );
        }
    }
    for handle in graph["sources"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(handle, _)| handle)
    {
        out.insert(
            format!("source:{handle}"),
            ["text", "sha256", "location"].map(str::to_owned).to_vec(),
        );
    }
}

fn add_hypothesis_directory_keys(
    out: &mut BTreeMap<String, Vec<String>>,
    graph: &J,
    capture: &CapturedSource,
) {
    use crate::history_yaml::OrdinaryValue as S;
    for (name, hypothesis) in graph["native_hypotheses"].as_object().into_iter().flatten() {
        if hypothesis["kind"] == "contribution" {
            continue;
        }
        let Some(path) = hypothesis["path"].as_str() else {
            continue;
        };
        let Some(bytes) = capture.files().get(std::path::Path::new(path)) else {
            continue;
        };
        let Ok(S::Map(mut fields)) = crate::history_yaml::decode_ordinary_source_value(bytes)
        else {
            continue;
        };
        let reference = format!("native#/{}", name.replace('~', "~0").replace('/', "~1"));
        out.insert(
            reference.clone(),
            ["name", "path", "head", "doc", "ids", "raw", "error"]
                .map(str::to_owned)
                .to_vec(),
        );
        if let Some((_, head)) = fields
            .iter()
            .find(|(key, _)| key.text() == Some("hypothesis"))
            && ordinary_reader::json_value(&head.projected(), 0)
                .ok()
                .as_ref()
                == Some(&hypothesis["head"])
        {
            index_directory_keys(head, &format!("{reference}/head"), out);
        }
        fields.retain(|(key, _)| key.text() != Some("hypothesis"));
        let document = S::Map(fields);
        if ordinary_reader::json_value(&document.projected(), 0)
            .ok()
            .as_ref()
            == Some(&hypothesis["doc"])
        {
            index_directory_keys(&document, &format!("{reference}/doc"), out);
            index_directory_keys(&hypothesis_raw(&document), &format!("{reference}/raw"), out);
        }
    }
}

fn relation_counts(counts: &J) -> String {
    counts
        .as_object()
        .unwrap()
        .iter()
        .map(|(relation, count)| {
            let label = match relation.as_str() {
                "rests_on" => "dependency_count".into(),
                "from" => "source_count".into(),
                _ => format!("{relation}_count"),
            };
            format!("{label}={count}")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn navigation_index(graph: &J) -> Result<Navigation> {
    let topics = graph["topics"]
        .as_object()
        .ok_or_else(|| Error("invalid ordinary topics".into()))?;
    let mut tree = Navigation::default();
    tree.groups.entry("/".into()).or_default();
    for (id, parts) in topics {
        let mut path = "/".to_owned();
        tree.groups
            .entry(path.clone())
            .or_default()
            .insert(id.clone());
        for part in parts
            .as_array()
            .ok_or_else(|| Error("invalid topic path".into()))?
        {
            let part = part
                .as_str()
                .filter(|part| !part.is_empty())
                .ok_or_else(|| Error("invalid topic path".into()))?;
            let quoted = part
                .as_bytes()
                .iter()
                .map(|byte| {
                    if byte.is_ascii_alphanumeric() || b"-._~".contains(byte) {
                        (*byte as char).to_string()
                    } else {
                        format!("%{byte:02X}")
                    }
                })
                .collect::<String>();
            let child = format!("{}/{quoted}", path.trim_end_matches('/'));
            tree.children.entry(path).or_default().insert(child.clone());
            tree.groups
                .entry(child.clone())
                .or_default()
                .insert(id.clone());
            path = child;
        }
        tree.leaves.insert(id.clone(), path.clone());
        tree.direct.entry(path).or_default().push(id.clone());
    }
    if let Some(order) = graph.get("node_order").and_then(J::as_array) {
        let rank = order
            .iter()
            .enumerate()
            .map(|(index, id)| (id.as_str().unwrap_or(""), index))
            .collect::<BTreeMap<_, _>>();
        require(
            order.len() == topics.len()
                && rank.len() == topics.len()
                && topics.keys().all(|id| rank.contains_key(id.as_str())),
            "node_order must be a permutation of node IDs",
        )?;
        for ids in tree.direct.values_mut() {
            ids.sort_by_key(|id| rank[id.as_str()]);
        }
    }
    tree.balance("/");
    Ok(tree)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokenizer::Encoding;
    fn fixtures() -> J {
        serde_json::from_str(include_str!(
            "../tests/fixtures/ordinary-session-view-oracle.json"
        ))
        .unwrap()
    }
    fn from_fixture(case: &J) -> OrdinarySession {
        let graph = case["graph"].clone();
        let navigation = navigation_index(&graph).unwrap();
        let revision = case["revision"].as_str().unwrap().to_owned();
        let pending = case["pending"].as_u64().unwrap();
        let stale = case["stale"].as_u64().unwrap();
        OrdinarySession {
            project: "fixture".into(),
            revision: revision.clone(),
            graph,
            scan: case["scan"].clone(),
            groups: navigation.groups.clone(),
            navigation,
            directory_keys: serde_json::from_value(case["directory_keys"].clone()).unwrap(),
            proposals: (0..pending)
                .map(|i| {
                    (
                        format!("proposal{i}"),
                        json!({"base_revision":if i < stale { "old" } else { &revision }}),
                    )
                })
                .collect(),
        }
    }
    fn program() -> crate::ordinary_runtime::Program {
        let root = std::env::var_os("KPOP_TEST_ORDINARY_PROGRAM")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::PathBuf::from(std::env::var_os("HOME").unwrap())
                    .join(".cache/kpopper/lean")
                    .join(crate::reasoning_runtime::target_name().unwrap())
                    .join(env!("KPOP_ORDINARY_SOURCE_SHA256"))
            });
        crate::ordinary_runtime::Program::open(&root).unwrap()
    }
    #[test]
    fn python_oracle_complete_packets_and_budgeted_navigation() {
        let fixtures = fixtures();
        for case in fixtures["cases"].as_array().unwrap() {
            let session = from_fixture(case);
            let name = case["name"].as_str().unwrap();
            assert_eq!(
                json!(session.navigation.groups),
                case["graph"]["navigation_routes"],
                "{name}: complete routes"
            );
            assert_eq!(
                json!(session.navigation.leaves),
                case["graph"]["navigation_leaf_routes"],
                "{name}: leaf routes"
            );
            for row in case["openings"].as_array().unwrap() {
                let budget = row["budget"].as_u64().unwrap() as usize;
                let actual = session.opening(budget, |text| Encoding::O200kBase.count(text));
                if let Some(error) = row.get("error") {
                    assert_eq!(
                        actual.unwrap_err().0,
                        error.as_str().unwrap(),
                        "{name} budget={budget}"
                    );
                } else {
                    let actual = actual.unwrap();
                    assert_eq!(
                        actual.text,
                        row["opening"]["text"].as_str().unwrap(),
                        "{name} budget={budget} text"
                    );
                    assert_eq!(
                        actual.packet, row["opening"]["packet"],
                        "{name} budget={budget} packet"
                    );
                    assert_eq!(
                        actual.tokens,
                        row["opening"]["tokens"].as_u64().unwrap() as usize,
                        "{name} budget={budget} tokens"
                    );
                    assert!(actual.tokens <= budget);
                }
            }
            for row in case["reads"].as_array().unwrap() {
                let reference = row["ref"].as_str().unwrap();
                let budget = row["budget"].as_u64().unwrap() as usize;
                let actual = session.read(reference, session.revision(), budget, None, |text| {
                    Encoding::O200kBase.count(text)
                });
                if let Some(error) = row.get("error") {
                    assert_eq!(
                        actual.unwrap_err().0,
                        error.as_str().unwrap(),
                        "{name} {reference} budget={budget}"
                    );
                } else {
                    assert_eq!(
                        actual.unwrap(),
                        row["text"].as_str().unwrap(),
                        "{name} {reference} budget={budget}"
                    );
                }
            }
            for row in case["field_reads"].as_array().unwrap() {
                let reference = row["ref"].as_str().unwrap();
                let budget = row["budget"].as_u64().unwrap() as usize;
                let offset = row["offset"].as_u64().map(|offset| offset as usize);
                let actual = session.read(reference, session.revision(), budget, offset, |text| {
                    Encoding::O200kBase.count(text)
                });
                if let Some(error) = row.get("error") {
                    assert_eq!(
                        actual.unwrap_err().0,
                        error.as_str().unwrap(),
                        "{name} {reference} budget={budget}"
                    );
                } else {
                    assert_eq!(
                        actual.unwrap(),
                        row["text"].as_str().unwrap(),
                        "{name} {reference} budget={budget}"
                    );
                }
            }
            for row in case["value_reads"].as_array().unwrap() {
                let reference = row["ref"].as_str().unwrap();
                let actual = session.read_value(reference);
                if let Some(error) = row.get("error") {
                    assert_eq!(
                        actual.unwrap_err().0,
                        error.as_str().unwrap(),
                        "{name} {reference}"
                    );
                } else {
                    assert_eq!(actual.unwrap(), row["value"], "{name} {reference}");
                }
            }
        }
    }
    #[test]
    fn python_oracle_navigation_profiles_and_validation() {
        let fixtures = fixtures();
        for case in fixtures["cases"].as_array().unwrap() {
            let mut graph = case["authored_graph"].clone();
            let profile =
                (!case["profile"].is_null()).then(|| V::from_json(&case["profile"]).unwrap());
            apply_profile(&mut graph, profile.as_ref()).unwrap();
            let navigation = navigation_index(&graph).unwrap();
            graph["navigation_routes"] = json!(navigation.groups);
            graph["navigation_leaf_routes"] = json!(navigation.leaves);
            assert_eq!(graph, case["graph"], "{}", case["name"]);
        }
        let numeric_cases: J = serde_json::from_str(include_str!(
            "../tests/fixtures/session-profile-numeric.json"
        ))
        .unwrap();
        for case in numeric_cases["cases"].as_array().unwrap() {
            let mut graph = fixtures["cases"][0]["authored_graph"].clone();
            let profile = V::from_json(&case["profile"]).unwrap();
            let result = apply_profile(&mut graph, Some(&profile));
            if let Some(error) = case.get("error") {
                assert_eq!(
                    result.unwrap_err().0,
                    error.as_str().unwrap(),
                    "{}",
                    case["name"]
                );
            } else {
                assert_eq!(
                    graph["navigation_profile"]["unmatched_ids"], case["unmatched_ids"],
                    "{}",
                    case["name"]
                );
            }
        }
        for case in fixtures["invalid_profiles"].as_array().unwrap() {
            let mut graph = fixtures["cases"][0]["authored_graph"].clone();
            let profile = V::from_json(&case["profile"]).unwrap();
            assert_eq!(
                apply_profile(&mut graph, Some(&profile)).unwrap_err().0,
                case["error"].as_str().unwrap()
            );
        }
    }
    #[test]
    fn verified_lean_guard_accepts_python_packets_and_refuses_corruptions() {
        let program = program();
        let fixtures = fixtures();
        for case in fixtures["cases"].as_array().unwrap() {
            let session = from_fixture(case);
            assert_eq!(
                program
                    .scan(&session.graph, &OperationalBounds::default())
                    .unwrap(),
                session.scan
            );
            for budget in [700, 1200] {
                let opening = session
                    .opening(budget, |text| Encoding::O200kBase.count(text))
                    .unwrap();
                assert_eq!(
                    session.guard_opening(&opening, &program).unwrap()["accepted"],
                    true,
                    "{}",
                    case["name"]
                );
            }
        }
        let session = from_fixture(&fixtures["cases"][0]);
        let opening = session
            .opening(700, |text| Encoding::O200kBase.count(text))
            .unwrap();
        for (field, value) in [
            ("cells", json!([])),
            ("events", json!({})),
            ("links", json!([])),
            ("counts", json!({})),
        ] {
            let mut corrupt = opening.clone();
            corrupt.packet[field] = value;
            assert!(
                session
                    .guard_opening(&corrupt, &program)
                    .unwrap_err()
                    .0
                    .contains("invalid projection"),
                "{field}"
            );
        }
        for field in ["project", "revision"] {
            let mut corrupt = opening.clone();
            corrupt.packet[field] = json!("wrong");
            assert_eq!(
                session.guard_opening(&corrupt, &program).unwrap_err().0,
                "view context does not match this record snapshot"
            );
        }
        for field in ["orientation", "rules"] {
            let mut corrupt = opening.clone();
            corrupt.packet[field] = json!("wrong");
            assert_eq!(
                session.guard_opening(&corrupt, &program).unwrap_err().0,
                "view orientation or rules changed"
            );
        }
    }
}
