//! Re-evaluated checked-reader/v1 sessions over an ordinary captured graph.

use crate::{
    Error, Result,
    history_contract::map,
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
type NavigationIndex = (BTreeMap<String, BTreeSet<String>>, BTreeMap<String, String>);

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
    leaves: BTreeMap<String, String>,
    proposals: BTreeMap<String, J>,
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
    if let Some(expression) = value.as_str() {
        return expression.into();
    }
    if let Some(expr) = value.get("expr").and_then(J::as_str) {
        return expr.into();
    }
    serde_json::to_string(value).unwrap_or_else(|_| "UNKNOWN".into())
}

impl OrdinarySession {
    pub fn capture(
        capture: &CapturedSource,
        runtime: &Runtime,
        project: &str,
        navigation: Option<&V>,
    ) -> Result<Self> {
        let context = capture.ordinary_context();
        let projection = Projection::new(
            capture.ordinary_document(),
            map(capture.hypotheses())?,
            map(&map(&context)?["conflicts"])?,
            capture.reader_lines()?,
            Some(runtime),
        )?;
        let data = projection.session_data()?;
        let program = runtime
            .ordinary_program()
            .ok_or_else(|| Error("ordinary expression program is not configured".into()))?;
        let mut graph = graph(capture, data, project, program.provenance()?)?;
        apply_profile(&mut graph, navigation)?;
        let scan = program.scan(&graph, &OperationalBounds::default())?;
        let snapshot = sha256(canonical(&graph)?.as_bytes());
        let revision = sha256(canonical(&json!({"project":project,"graph":snapshot}))?.as_bytes());
        let (groups, leaves) = navigation_index(&graph)?;
        Ok(Self {
            project: project.into(),
            revision,
            graph,
            scan,
            groups,
            leaves,
            proposals: BTreeMap::new(),
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
        }
        self.proposals = proposals;
        Ok(self)
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
        let packet = self.packet()?;
        let text = self.render_open(&packet, true)?;
        require(
            count(&text) <= tokens,
            &format!(
                "minimum complete opening needs {} reference tokens",
                count(&text)
            ),
        )?;
        Ok(Opening {
            tokens: count(&text),
            text,
            packet,
        })
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
            let text = self.branch_text(reference)?;
            require(
                count(&text) <= tokens,
                "budget cannot carry the complete branch root",
            )?;
            return Ok(text);
        }
        if reference.starts_with("links:") && !reference.contains('#') {
            require(offset.is_none(), "offset applies only to exact text fields")?;
            let text = self.link_text(&reference[6..])?;
            require(count(&text) <= tokens, "budget cannot carry link metadata")?;
            return Ok(text);
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
        let children = value
            .as_object()
            .map(|map| {
                map.keys()
                    .map(|key| {
                        format!("{reference}#/{}", key.replace('~', "~0").replace('/', "~1"))
                    })
                    .collect::<Vec<_>>()
            })
            .or_else(|| {
                value
                    .as_array()
                    .map(|a| (0..a.len()).map(|i| format!("{reference}#/{i}")).collect())
            })
            .unwrap_or_default();
        let diagnostic = canonical(
            &json!({"complete":false,"required_tokens":count(&response),"ref":reference,"children":children,"next":if value.is_string(){"read this text with offset=0 for labeled fragments"}else{"read a field or raise tokens"}}),
        )?;
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
            None,
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
        let mut value = if let Some(id) = base.strip_prefix("checked:") {
            self.assessment(id)
                .cloned()
                .ok_or_else(|| Error("not_a_judgment".into()))?
        } else if let Some(id) = base.strip_prefix("node:") {
            let node = nodes
                .get(id)
                .ok_or_else(|| Error("unlisted operation or identifier".into()))?;
            if node["kind"] == "judgment"
                && (pointer_path.is_empty()
                    || [
                        "/epistemic_card",
                        "/checked_bundle_ref",
                        "/raw_ref",
                        "/scope",
                    ]
                    .iter()
                    .any(|field| pointer_path.starts_with(field)))
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
            let source = self.graph["sources"]
                .get(handle)
                .cloned()
                .ok_or_else(|| Error("unlisted operation or identifier".into()))?;
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
                                ["contested", "broken", "unchecked", "falsified"]
                                    .contains(&state.as_str().unwrap_or(""))
                            })
                            .cloned()
                            .collect::<Vec<_>>();
                        (!alerts.is_empty()).then(|| (id.clone(), J::Array(alerts)))
                    })
                    .collect(),
            )
        } else if base == "assessment" {
            json!({"counts":self.scan["counts"],"events":self.scan["events"].as_object().map(|m|m.keys().collect::<Vec<_>>()).unwrap_or_default(),"errors":self.scan["errors"]})
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
        } else if let Some(key) = base.strip_prefix("links:") {
            J::Array(self.links(key)?)
        } else {
            return Err(Error("unknown reference".into()));
        };
        if !pointer_path.is_empty() {
            value = pointer(&value, pointer_path)?.clone();
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
    fn branch_text(&self, path: &str) -> Result<String> {
        let ids = self.groups.get(path).ok_or_else(|| {
            Error("unknown branch; read / with this revision to list valid branches".into())
        })?;
        let mut lines = vec![format!("revision={}", self.revision), format!("MAP {path}")];
        let mut context = String::new();
        for id in ids {
            let cell = self.cell(id)?;
            let topic = cell["topic"].as_str().unwrap_or("/");
            if topic != context && topic != path {
                lines.push(format!("@ {topic}"));
                context = topic.into();
            }
            lines.push(format!("- node:{id}"));
        }
        lines.push(
            "Counts describe links, not IDs. Read node:ID, /topic or links:ID. Names are locators."
                .into(),
        );
        Ok(lines.join("\n") + "\n")
    }

    fn link_text(&self, key: &str) -> Result<String> {
        let edges = self.links(key)?;
        let mut lines = vec![
            format!("revision={}", self.revision),
            format!("LINKS {key} — source relation target; rests_on does not mean implies."),
        ];
        lines.extend(edges.iter().map(|edge| {
            format!(
                "{} {} {}",
                edge["from"].as_str().unwrap_or(""),
                edge["rel"].as_str().unwrap_or(""),
                edge["to"].as_str().unwrap_or("")
            )
        }));
        Ok(lines.join("\n") + "\n")
    }

    fn packet(&self) -> Result<J> {
        let nodes = self.graph["nodes"].as_object().unwrap();
        let events = self.scan["events"].as_object().unwrap();
        let cells = nodes
            .keys()
            .map(|id| self.cell(id))
            .collect::<Result<Vec<_>>>()?;
        let stale = self
            .proposals
            .values()
            .filter(|proposal| proposal["base_revision"] != self.revision)
            .count();
        Ok(
            json!({"schema":"kpopper.epistemic-view.v3","project":self.project,"revision":self.revision,"counts":self.scan["counts"],"cells":cells,"conditions_ref":"conditions:/","events":self.scan["events"],"orientation":self.graph.get("orientation").cloned().unwrap_or_else(||json!({})),"rules":RULES,"links":self.graph["edges"],"pending":self.proposals.len(),"stale_pending":stale,"native_hypotheses":self.graph["native_hypotheses"].as_object().map(|m|m.values().filter(|h|h["kind"]!="contribution").count()).unwrap_or(0),"contributions":self.graph["contributions"],"read_mode":self.graph["read_mode"],"events_expanded":!events.is_empty()}),
        )
    }
    fn cell(&self, id: &str) -> Result<J> {
        let node = &self.graph["nodes"][id];
        let topic = self.leaves.get(id).cloned().unwrap_or_else(|| "/".into());
        let edges = self.graph["edges"].as_array().unwrap();
        let indices = edges
            .iter()
            .enumerate()
            .filter(|(_, edge)| edge["from"] == id)
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        let mut counts = BTreeMap::<String, usize>::new();
        for index in &indices {
            *counts
                .entry(edges[*index]["rel"].as_str().unwrap_or("").into())
                .or_default() += 1;
        }
        Ok(
            json!({"kind":"node","key":id,"topic":topic,"members":[id],"conflicts":if node["states"].as_array().is_some_and(|s|s.contains(&json!("contested"))){vec![id]}else{vec![]},"questions":if node["states"].as_array().is_some_and(|s|s.contains(&json!("question"))){vec![id]}else{vec![]},"attention":self.scan["events"].as_object().unwrap().values().any(|event|event["affected"].as_array().is_some_and(|a|a.contains(&json!(id)))).then_some(vec![id]).unwrap_or_default(),"edge_indices":indices,"relation_counts":counts,"links_ref":format!("links:{id}")}),
        )
    }
    fn render_open(&self, packet: &J, expanded: bool) -> Result<String> {
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
        if !self.proposals.is_empty() {
            lines.insert(
                lines.len() - 1,
                format!(
                    "pending={} stale={}; native hypotheses={}",
                    packet["pending"], packet["stale_pending"], packet["native_hypotheses"]
                ),
            );
        }
        if expanded {
            for (key, event) in self.scan["events"].as_object().unwrap() {
                lines.push(format!(
                    "@event:{key} {} {}",
                    event["id"].as_str().unwrap_or(""),
                    event["kind"].as_str().unwrap_or("").to_uppercase()
                ));
            }
        }
        lines.push("MAP / — declared navigation; names do not establish claims:".into());
        let mut context = "";
        for cell in packet["cells"].as_array().unwrap() {
            let topic = cell["topic"].as_str().unwrap_or("/");
            if topic != context && topic != "/" {
                lines.push(format!("@ {topic}"));
                context = topic;
            }
            let id = cell["key"].as_str().unwrap();
            let mut line = format!("- node:{id}");
            for (relation, count) in cell["relation_counts"].as_object().unwrap() {
                line.push_str(&format!(
                    " {}_count={count}",
                    if relation == "rests_on" {
                        "dependency"
                    } else if relation == "from" {
                        "source"
                    } else {
                        relation
                    }
                ));
            }
            lines.push(line);
        }
        let mut folded = BTreeMap::<(String, String, String), usize>::new();
        for edge in self.graph["edges"].as_array().unwrap() {
            *folded
                .entry((
                    edge["from"].as_str().unwrap_or("").into(),
                    edge["rel"].as_str().unwrap_or("").into(),
                    edge["to"].as_str().unwrap_or("").into(),
                ))
                .or_default() += 1;
        }
        if !folded.is_empty() {
            lines.push(
                "LINK MAP — folded endpoints; all links counted; exact links at links:/".into(),
            );
            lines.extend(
                folded.into_iter().map(|((from, relation, to), count)| {
                    format!("{from} {relation} {to} [{count}]")
                }),
            );
        }
        lines.push("Counts describe links: dependency_count=rests_on, source_count=from; not proof. Read node:ID, /topic, links:ID or @ref without @; pass revision.".into());
        Ok(lines.join("\n") + "\n")
    }
}

fn graph(
    capture: &CapturedSource,
    data: OrdinarySessionData,
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
    let mut nodes = Map::from_iter(data.nodes);
    for (id, node) in &mut nodes {
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
        json!({"nodes":nodes,"edges":data.edges,"topics":data.topics,"scope":data.scope,"sources":sources,"native_hypotheses":data.native_hypotheses,"contributions":data.contributions,"knowledge_conflicts":data.knowledge_conflicts,"read_mode":map(&capture.ordinary_context())?["read_mode"].to_json()?,"origin":provenance,"project_context":project}),
    )
}

fn apply_profile(graph: &mut J, profile: Option<&V>) -> Result<()> {
    let Some(profile) = profile else {
        return Ok(());
    };
    let profile = ordinary_reader::json_value(profile, 0)?;
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
    for (path, ids) in groups {
        require(
            !path.is_empty() && !path.split('/').any(str::is_empty),
            "invalid profile path",
        )?;
        for id in ids
            .as_array()
            .ok_or_else(|| Error("profile group must list IDs".into()))?
        {
            let id = id
                .as_str()
                .ok_or_else(|| Error("profile group must list IDs".into()))?;
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
    graph["topics"] = J::Object(topics);
    graph["orientation"] = profile
        .get("orientation")
        .cloned()
        .unwrap_or_else(|| json!({}));
    graph["navigation_profile"] = json!({"description":profile["description"].as_str().unwrap_or("Declared navigation."),"sha256":sha256(canonical(&profile)?.as_bytes()),"unmatched_ids":assigned.keys().filter(|id|!node_ids.contains(*id)).collect::<Vec<_>>(),"opening_depth":profile["opening_depth"].as_u64().unwrap_or(1)});
    Ok(())
}

fn navigation_index(graph: &J) -> Result<NavigationIndex> {
    let topics = graph["topics"]
        .as_object()
        .ok_or_else(|| Error("invalid ordinary topics".into()))?;
    let mut groups = BTreeMap::<String, BTreeSet<String>>::new();
    let mut leaves = BTreeMap::new();
    for (id, parts) in topics {
        let mut path = "/".to_owned();
        groups.entry(path.clone()).or_default().insert(id.clone());
        for part in parts
            .as_array()
            .ok_or_else(|| Error("invalid topic path".into()))?
        {
            let part = part
                .as_str()
                .ok_or_else(|| Error("invalid topic path".into()))?;
            path.push_str(part);
            groups.entry(path.clone()).or_default().insert(id.clone());
            path.push('/');
        }
        leaves.insert(id.clone(), path.trim_end_matches('/').into());
    }
    Ok((groups, leaves))
}
