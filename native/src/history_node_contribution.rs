//! Project contributions from compact history as selected semantic closures.
//! A closure carries typed-history/v2 objects, their source orders and the source-clock
//! commits its reduction reads - never compact storage and never the rest of the record.
//! Adoption and materialization import the same immutable objects into compact history.
use crate::{
    Result,
    history_authoring::{self as A, empty, n, obj, s, strings},
    history_authority::Files,
    history_contract::*,
    history_node_capture::Capture,
    history_node_observation::ObservationNode,
    history_node_publication as P,
    history_node_semantics::History,
    history_source_ancestry as Clock,
    history_view::list,
    history_yaml as Y,
    identity::sha256,
    require,
    value::TypedValue as V,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub(crate) const FORMAT: &str = "node-contribution/v1";
pub(crate) const IMPORT: &str = "contribution-import";
pub(crate) const EVIDENCE: &str = "evidence/contributions/";
const OBJECTS: &str = "node-closure/objects/";
const CLOCKS: &str = "node-closure/clocks/";
const RESERVED: &str = "node-closure/";
const ENTRY: &str = "GROUNDING.yaml";

fn requires() -> V {
    strings([FORMAT.to_owned(), "typed-history/v2".to_owned()])
}
fn hex64(value: &V) -> Result<()> {
    require(
        text(value).is_ok_and(|v| v.len() == 64 && crate::history_paths::object_id(v)),
        "invalid_node_contribution",
    )
}
fn entries(document: &V) -> Result<BTreeMap<String, (String, V)>> {
    crate::reasoning_snapshot::entries(document)
}
fn reasoning(document: &V) -> V {
    map(document)
        .ok()
        .and_then(|m| m.get("meta"))
        .and_then(|v| map(v).ok())
        .and_then(|m| m.get("reasoning"))
        .cloned()
        .unwrap_or(V::Null)
}

/// Selected, immutable semantic content of one contribution (v3 or v4).
pub(crate) struct Closure {
    pub(crate) revision: String,
    /// Full typed objects, `saw` included.
    pub(crate) objects: Map,
    pub(crate) orders: Map,
    pub(crate) clocks: Files,
    pub(crate) template: V,
    pub(crate) rules: V,
    pub(crate) document: V,
    pub(crate) roots: V,
    /// When the source observed this closure; materialization records it unchanged.
    pub(crate) recorded_at: String,
}
impl Closure {
    fn subjects(&self) -> Result<BTreeSet<String>> {
        self.objects
            .values()
            .map(|o| Ok(text(field(map(o)?, "subject")?)?.to_owned()))
            .collect()
    }
    fn graph(&self) -> Result<Clock::Graph> {
        let mut graph = Clock::Graph::default();
        for (path, raw) in &self.clocks {
            graph.insert(path, raw)?;
        }
        Ok(graph)
    }
}

fn compact(objects: &Map, orders: &Map) -> Result<History> {
    History::from_ordered(
        objects
            .iter()
            .map(|(id, object)| {
                let mut value = map(object)?.clone();
                let saw = list(&value.remove("saw").ok_or_else(|| error("invalid_schema"))?)?
                    .iter()
                    .map(|v| text(v).map(str::to_owned))
                    .collect::<Result<BTreeSet<_>>>()?;
                Ok((V::Map(value), ObservationNode::root(id.clone(), &saw)?))
            })
            .collect::<Result<Vec<_>>>()?,
        orders.clone(),
    )
}
fn reduce(history: &History, rules: &V, clocks: &Clock::Graph) -> Result<V> {
    clocks.with_ancestry(|ancestry| history.reduce(Some(map(rules)?), Some(ancestry)))
}
fn render(template: &V, history: &History, state: &V) -> Result<V> {
    A::destination(&crate::history_node_capture::render(
        template,
        history.objects(),
        state,
        true,
    )?)
}
fn inventory(objects: &Map, subject: &str) -> Result<V> {
    Ok(V::Map(
        objects
            .iter()
            .filter(|(_, o)| map(o).is_ok_and(|m| string_is(&m["subject"], subject)))
            .map(|(id, o)| {
                Ok((
                    id.clone(),
                    s(&sha256(&crate::history_emit::encode_document(o)?)),
                ))
            })
            .collect::<Result<Map>>()?,
    ))
}

/// A prepared compact contribution before its evidence files are attached.
pub(crate) struct Plan {
    source: V,
    closure: Closure,
    subjects: Map,
}
impl Plan {
    pub(crate) fn document(&self) -> &V {
        &self.closure.document
    }
    /// Objects whose `file` references must be supplied as portable evidence.
    pub(crate) fn objects(&self) -> V {
        V::Map(self.closure.objects.clone())
    }
}

// Domain-profile support is a separate capability; a scoped import must not
// discard a declared contract when its package cannot be interpreted here.
pub(crate) fn has_domain_profile(document: &V) -> Result<bool> {
    let Some(meta) = map(document)?.get("meta") else { return Ok(false); };
    Ok(map(meta)?.get("domain_profile").is_some_and(|v| *v != V::Null))
}

/// Select the dependency closure of `roots` from the compact candidate that includes the
/// prepared claims. Local state is not written; the candidate is an in-memory reduction.
pub(crate) fn plan(
    source: &Capture,
    new: &[V],
    roots: &[String],
    operation: &str,
    recorded_at: &str,
    disclosed: &V,
) -> Result<Plan> {
    require(
        !has_domain_profile(source.document())?,
        "domain_profile_subset_unsupported: retain the complete history and package closure",
    )?;
    token(&s(operation))?;
    require(!recorded_at.is_empty(), "invalid_recorded_time")?;
    let candidate = source.candidate(new)?;
    let full = candidate
        .history
        .objects()
        .keys()
        .map(|id| Ok((id.clone(), candidate.history.object(id)?)))
        .collect::<Result<Map>>()?;
    let root_value = strings(roots.iter().cloned().collect::<BTreeSet<_>>());
    let subjects =
        crate::history_bundle::subset_subjects_in(&full, candidate.document(), &root_value)?;
    let objects = full
        .into_iter()
        .filter(|(_, o)| {
            map(o).is_ok_and(|m| text(&m["subject"]).is_ok_and(|s| subjects.contains(s)))
        })
        .collect::<Map>();
    let orders = candidate
        .history
        .source_orders()
        .iter()
        .filter(|(id, _)| objects.contains_key(*id))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect::<Map>();
    // Only the commit ancestry reachable from values in the closure is carried.
    let mut clocks = Files::new();
    let mut todo = Vec::new();
    let mut walk = objects.values().collect::<Vec<_>>();
    while let Some(value) = walk.pop() {
        match value {
            V::Text(t) if Clock::oid(t) => todo.push(t.clone()),
            V::Map(m) => walk.extend(m.values()),
            V::List(v) => walk.extend(v),
            _ => {}
        }
    }
    while let Some(id) = todo.pop() {
        let path = format!("{}{id}.commit", Clock::PREFIX);
        if clocks.contains_key(&path) {
            continue;
        }
        let Some(parents) = candidate.clocks.parents_of(&id) else {
            continue;
        };
        let raw = source
            .snapshot
            .raw_evidence
            .get(&path)
            .ok_or_else(|| error("source_ancestry_evidence_missing"))?;
        clocks.insert(path, raw.as_ref().clone());
        todo.extend(parents.iter().cloned());
    }
    let rules = map(candidate.state())?["rules"].clone();
    let mut template = Map::new();
    if let Some(schema) = map(candidate.document())?.get("schema") {
        template.insert("schema".into(), schema.clone());
    }
    let mut meta = Map::new();
    if reasoning(candidate.document()) != V::Null {
        meta.insert("reasoning".into(), reasoning(candidate.document()));
    }
    template.insert("meta".into(), V::Map(meta));
    let template = V::Map(template);
    let history = compact(&objects, &orders)?;
    let graph = {
        let mut g = Clock::Graph::default();
        for (path, raw) in &clocks {
            g.insert(path, raw)?;
        }
        g
    };
    let state = reduce(&history, &rules, &graph)?;
    let full_state = map(&map(candidate.state())?["subjects"])?;
    let mut evidence = Map::new();
    for subject in &subjects {
        let reduced = map(&map(&state)?["subjects"])?
            .get(subject)
            .ok_or_else(|| error("subset_reduction_mismatch"))?;
        require(
            full_state.get(subject).map(V::digest).transpose()? == Some(reduced.digest()?),
            "subset_reduction_mismatch",
        )?;
        let prepared = objects.iter().any(|(id, o)| {
            map(o).is_ok_and(|m| string_is(&m["subject"], subject))
                && !source.history.objects().contains_key(id)
        });
        evidence.insert(
            subject.clone(),
            obj([
                (
                    "source_state",
                    s(if prepared {
                        "prepared_candidate"
                    } else {
                        "committed"
                    }),
                ),
                (
                    "objects_digest",
                    s(&inventory(&objects, subject)?.digest()?),
                ),
                ("reduction_digest", s(&reduced.digest()?)),
            ]),
        );
    }
    let document = render(&template, &history, &state)?;
    let prepared_digest = if new.is_empty() {
        V::Null
    } else {
        s(&V::List(new.to_vec()).digest()?)
    };
    let source_value = obj([
        ("kind", s(crate::history_node_codec::FORMAT)),
        ("authority", source.snapshot.authority.clone()),
        ("revision", s(source.revision())),
        ("entry", s(ENTRY)),
        ("operation", s(operation)),
        ("recorded_at", s(recorded_at)),
        ("prepared_digest", prepared_digest),
        ("disclosed_locators", disclosed.clone()),
    ]);
    Ok(Plan {
        source: source_value,
        closure: Closure {
            revision: String::new(),
            objects,
            orders,
            clocks,
            template,
            rules,
            document,
            roots: root_value,
            recorded_at: recorded_at.into(),
        },
        subjects: evidence,
    })
}

/// Attach exact evidence and seal the v4 bundle. The result is validated before return.
pub(crate) fn bundle(plan: &Plan, scope: &V, evidence: &Files) -> Result<(V, Files)> {
    require(
        !evidence.keys().any(|p| p.starts_with(RESERVED)),
        "evidence uses reserved node closure namespace",
    )?;
    let mut files = evidence.clone();
    for (id, object) in &plan.closure.objects {
        files.insert(
            format!("{OBJECTS}{id}.yaml"),
            crate::history_emit::encode_document(object)?,
        );
    }
    for (path, raw) in &plan.closure.clocks {
        let name = path
            .strip_prefix(Clock::PREFIX)
            .ok_or_else(|| error("source_ancestry_path"))?;
        files.insert(format!("{CLOCKS}{name}"), raw.clone());
    }
    let manifest = obj([
        ("version", n("4")),
        ("format", s(FORMAT)),
        ("requires", requires()),
        ("roots", plan.closure.roots.clone()),
        ("scope", scope.clone()),
        ("shareability", s("project")),
        ("document", plan.closure.document.clone()),
        (
            "reasoning",
            crate::reasoning_capabilities::document_capabilities(&plan.closure.document)?,
        ),
        ("source", plan.source.clone()),
        (
            "closure",
            obj([
                ("template", plan.closure.template.clone()),
                ("rules", plan.closure.rules.clone()),
                ("subjects", V::Map(plan.subjects.clone())),
                ("source_orders", V::Map(plan.closure.orders.clone())),
            ]),
        ),
        (
            "evidence",
            V::Map(
                files
                    .iter()
                    .map(|(p, b)| (p.clone(), s(&sha256(b))))
                    .collect(),
            ),
        ),
    ]);
    let bundle = obj([("revision", s(&manifest.digest()?)), ("manifest", manifest)]);
    validate(&bundle, &files)?;
    Ok((bundle, files))
}

/// Replay every v4 invariant from the bundle and its files alone.
pub(crate) fn validate(bundle: &V, files: &Files) -> Result<Closure> {
    Y::validate_value(bundle, 16 * 1024 * 1024)?;
    let b = schema(bundle, &["revision", "manifest"], &[])?;
    let m = schema(
        &b["manifest"],
        &[
            "version",
            "format",
            "requires",
            "roots",
            "scope",
            "shareability",
            "document",
            "reasoning",
            "source",
            "closure",
            "evidence",
        ],
        &[],
    )?;
    require(
        is_int(&m["version"], "4")
            && string_is(&m["format"], FORMAT)
            && m["requires"] == requires()
            && b["revision"] == s(&b["manifest"].digest()?),
        "invalid_contribution_identity",
    )?;
    crate::history_bundle::files_valid(files)?;
    let hashes = map(&m["evidence"])?;
    require(
        hashes.len() == files.len()
            && files
                .iter()
                .all(|(p, raw)| hashes.get(p) == Some(&s(&sha256(raw)))),
        "contribution_evidence_mismatch",
    )?;
    require(
        string_is(&m["shareability"], "project"),
        "private_contribution_requires_draft",
    )?;
    let scope = schema(&m["scope"], &["kind", "environment"], &["commit"])?;
    require(
        text(&scope["kind"]).is_ok_and(|k| ["project", "external", "code"].contains(&k))
            && text(&scope["environment"]).is_ok_and(|e| !e.trim().is_empty()),
        "invalid_contribution_scope",
    )?;
    crate::history_bundle::privacy(&m["scope"])?;
    let roots = list(&m["roots"])?
        .iter()
        .map(|v| {
            subject(v)?;
            Ok(text(v)?.to_owned())
        })
        .collect::<Result<Vec<_>>>()?;
    require(
        !roots.is_empty() && roots.windows(2).all(|w| w[0] < w[1]),
        "invalid_contribution_roots",
    )?;
    let source = schema(
        &m["source"],
        &[
            "kind",
            "authority",
            "revision",
            "entry",
            "operation",
            "recorded_at",
            "prepared_digest",
            "disclosed_locators",
        ],
        &[],
    )?;
    require(
        string_is(&source["kind"], crate::history_node_codec::FORMAT)
            && string_is(&source["entry"], ENTRY)
            && text(&source["recorded_at"]).is_ok_and(|t| !t.is_empty()),
        "invalid_node_contribution_source",
    )?;
    P::validate_authority(&source["authority"])?;
    hex64(&source["revision"])?;
    token(&source["operation"])?;
    let closure = schema(
        &m["closure"],
        &["template", "rules", "subjects", "source_orders"],
        &[],
    )?;
    let template = schema(&closure["template"], &["meta"], &["schema"])?;
    schema(&template["meta"], &[], &["reasoning"])?;
    let mut objects = Map::new();
    let mut clocks = Files::new();
    let mut raws = BTreeMap::new();
    for (path, raw) in files {
        if let Some(name) = path.strip_prefix(OBJECTS) {
            let id = name
                .strip_suffix(".yaml")
                .ok_or_else(|| error("invalid_node_closure_path"))?;
            let object = Y::decode_document(raw)?;
            validate_object(&object)?;
            let o = map(&object)?;
            require(
                string_is(field(o, "id")?, id)
                    && o.get("id_scheme")
                        .is_some_and(|v| string_is(v, "typed-history/v2"))
                    && crate::history_emit::encode_document(&object)? == *raw,
                "invalid_node_closure_object",
            )?;
            raws.insert(id.to_owned(), raw);
            objects.insert(id.to_owned(), object);
        } else if let Some(name) = path.strip_prefix(CLOCKS) {
            clocks.insert(format!("{}{name}", Clock::PREFIX), raw.clone());
        } else {
            require(!path.starts_with(RESERVED), "invalid_node_closure_path")?;
        }
    }
    let result = Closure {
        revision: text(&b["revision"])?.to_owned(),
        objects,
        orders: map(&closure["source_orders"])?.clone(),
        clocks,
        template: closure["template"].clone(),
        rules: closure["rules"].clone(),
        document: m["document"].clone(),
        roots: m["roots"].clone(),
        recorded_at: text(&source["recorded_at"])?.to_owned(),
    };
    let graph = result.graph()?;
    let history = compact(&result.objects, &result.orders)?;
    let state = reduce(&history, &result.rules, &graph)?;
    let subjects = map(&closure["subjects"])?;
    require(
        result.subjects()?.into_iter().eq(subjects.keys().cloned()),
        "invalid_subset_coverage",
    )?;
    let mut prepared = false;
    for (name, evidence) in subjects {
        let e = schema(
            evidence,
            &["source_state", "objects_digest", "reduction_digest"],
            &[],
        )?;
        require(
            text(&e["source_state"])
                .is_ok_and(|v| ["committed", "prepared_candidate"].contains(&v)),
            "invalid_source_state",
        )?;
        prepared |= string_is(&e["source_state"], "prepared_candidate");
        require(
            e["objects_digest"] == s(&inventory(&result.objects, name)?.digest()?),
            "invalid_subset_receipt",
        )?;
        require(
            e["reduction_digest"]
                == s(&map(&map(&state)?["subjects"])?
                    .get(name)
                    .ok_or_else(|| error("subset_reduction_mismatch"))?
                    .digest()?),
            "subset_reduction_mismatch",
        )?;
    }
    require(
        !prepared || source["prepared_digest"] != V::Null,
        "invalid_source_state",
    )?;
    if source["prepared_digest"] != V::Null {
        hex64(&source["prepared_digest"])?;
    }
    require(
        render(&result.template, &history, &state)? == result.document,
        "invalid_node_contribution_document",
    )?;
    require(
        crate::reasoning_capabilities::document_capabilities(&result.document)? == m["reasoning"],
        "contribution_capability_mismatch",
    )?;
    require(
        !has_domain_profile(&result.document)?,
        "domain_profile_subset_unsupported: retain the complete history and package closure",
    )?;
    require(
        crate::history_bundle::subset_subjects_in(
            &result.objects,
            &result.document,
            &result.roots,
        )?
        .into_iter()
        .eq(subjects.keys().cloned()),
        "invalid_subset_coverage",
    )?;
    let present = entries(&result.document)?;
    for root in &roots {
        let (_, body) = present
            .get(root)
            .ok_or_else(|| error("unavailable_subset_root"))?;
        require(
            map(body)
                .ok()
                .and_then(|b| b.get("scope"))
                .map(V::digest)
                .transpose()?
                == Some(m["scope"].digest()?),
            "contribution_root_scope_mismatch",
        )?;
    }
    crate::history_bundle::privacy(&result.document)?;
    // Every retained version leaves with the closure, so each one must be shareable.
    crate::history_bundle::privacy(&V::Map(result.objects.clone()))?;
    crate::history_bundle::locator_disclosures_in(
        &result.objects,
        ENTRY,
        &source["disclosed_locators"],
    )?;
    require(
        crate::pending_bundle::required_files(&V::Map(result.objects.clone()))?
            .into_iter()
            .eq(files.keys().filter(|p| !p.starts_with(RESERVED)).cloned()),
        "contribution_evidence_allowlist",
    )?;
    Ok(result)
}

/// The closure of a validated legacy v3 subset. Legacy reduction read no source clocks.
fn legacy(bundle: &V, files: &Files) -> Result<Closure> {
    let captured = crate::pending_bundle::contribution_history(bundle, files)?;
    let manifest = map(field(map(bundle)?, "manifest")?)?;
    let adapted = crate::history_adapter::from_store_capture(&captured)?;
    let mut orders = Map::new();
    for ((_, id), raw) in &captured.object_bytes {
        let order = crate::history_node_source_order::encode(
            &crate::history_node_source_order::without_saw(Y::decode_source_document(raw)?)?,
        );
        if order != V::Null {
            orders.insert(id.clone(), order);
        }
    }
    let mut template = Map::new();
    if let Some(schema) = map(adapted.document())?.get("schema") {
        template.insert("schema".into(), schema.clone());
    }
    let mut meta = Map::new();
    if reasoning(adapted.document()) != V::Null {
        meta.insert("reasoning".into(), reasoning(adapted.document()));
    }
    template.insert("meta".into(), V::Map(meta));
    Ok(Closure {
        revision: text(field(map(bundle)?, "revision")?)?.to_owned(),
        objects: captured.objects.clone(),
        orders,
        clocks: Files::new(),
        template: V::Map(template),
        rules: map(&captured.state)?["rules"].clone(),
        document: adapted.document().clone(),
        roots: field(manifest, "roots")?.clone(),
        recorded_at: text(field(
            map(field(
                map(field(map(&captured.document)?, "meta")?)?,
                "history_subset",
            )?)?,
            "recorded_at",
        )?)?
        .to_owned(),
    })
}

/// A closure from either versioned history contribution; plain bundles have none.
pub(crate) fn closure(bundle: &V, files: &Files) -> Result<Closure> {
    let version = field(map(field(map(bundle)?, "manifest")?)?, "version")?;
    if is_int(version, "4") {
        validate(bundle, files)
    } else if is_int(version, "3") {
        legacy(bundle, files)
    } else {
        Err(error("history_contribution_required"))
    }
}

/// The retained evidence file for one contribution, hash-bound by the importing transaction.
pub(crate) fn encode_evidence(bundle: &V, files: &Files) -> Result<(String, Vec<u8>)> {
    let revision = text(field(map(bundle)?, "revision")?)?;
    token(&s(revision))?;
    let value = serde_json::json!({
        "bundle": bundle.to_tagged()?,
        "files": files.iter().map(|(p, b)| (p.clone(), serde_json::Value::String(STANDARD.encode(b)))).collect::<serde_json::Map<_, _>>(),
    });
    Ok((
        format!("{EVIDENCE}{revision}.json"),
        serde_json::to_vec(&value)?,
    ))
}
fn decode_evidence(path: &str, raw: &[u8]) -> Result<(V, Files)> {
    let value: serde_json::Value = serde_json::from_slice(raw)?;
    let o = value
        .as_object()
        .filter(|o| o.len() == 2)
        .ok_or_else(|| error("invalid_contribution_evidence"))?;
    let bundle = V::from_tagged(
        o.get("bundle")
            .ok_or_else(|| error("invalid_contribution_evidence"))?,
    )?;
    let files = o
        .get("files")
        .and_then(|f| f.as_object())
        .ok_or_else(|| error("invalid_contribution_evidence"))?
        .iter()
        .map(|(p, b)| {
            Ok((
                p.clone(),
                STANDARD
                    .decode(
                        b.as_str()
                            .ok_or_else(|| error("invalid_contribution_evidence"))?,
                    )
                    .map_err(|_| error("invalid_contribution_evidence"))?,
            ))
        })
        .collect::<Result<Files>>()?;
    let (expected, bytes) = encode_evidence(&bundle, &files)?;
    require(
        expected == path && bytes == raw,
        "invalid_contribution_evidence",
    )?;
    Ok((bundle, files))
}

/// The exact evidence of one import: the retained bundle and every carried commit the
/// target has not admitted. Clocks and objects are admitted in the same transaction.
fn import_evidence(
    capture: &Capture,
    bundle: &V,
    files: &Files,
    closure: &Closure,
) -> Result<Files> {
    let (path, raw) = encode_evidence(bundle, files)?;
    let mut evidence = Files::from([(path, raw)]);
    for (path, raw) in &closure.clocks {
        let id = Clock::path_id(path).ok_or_else(|| error("source_ancestry_path"))?;
        if capture.clocks.parents_of(id).is_none() {
            evidence.insert(path.clone(), raw.clone());
        }
    }
    Ok(evidence)
}

struct Incoming {
    closure: Closure,
    /// Closure objects the target does not hold yet, in causal order.
    new: Vec<V>,
}
fn incoming(capture: &Capture, closure: Closure) -> Result<Incoming> {
    require(
        closure.rules == map(capture.state())?["rules"],
        "rules_mismatch",
    )?;
    // A marker-only record takes the incoming verified template; an established record
    // keeps its own and must already read the closure under the same profile and roles.
    if !capture.is_unborn() {
        require(
            crate::reasoning_fields::capabilities(capture.document(), None)?
                == crate::reasoning_fields::capabilities(&closure.document, None)?,
            "adoption_profile_mismatch",
        )?;
        let empty_roles = Map::new();
        let held = map(capture.document())?
            .get("schema")
            .map(map)
            .transpose()?
            .unwrap_or(&empty_roles);
        if let Some(roles) = map(&closure.template)?.get("schema") {
            for (role, field) in map(roles)? {
                require(
                    held.get(role).is_none_or(|v| v == field),
                    "adoption_profile_mismatch",
                )?;
            }
        }
    }
    require(
        !has_domain_profile(capture.document())?
            && !has_domain_profile(&closure.document)?,
        "incompatible_domain_profiles",
    )?;
    let graph = closure.graph()?;
    for (path, _) in &closure.clocks {
        let id = Clock::path_id(path).ok_or_else(|| error("source_ancestry_path"))?;
        let held = capture.clocks.parents_of(id);
        require(
            held.is_none() || held == graph.parents_of(id),
            "source_ancestry_collision",
        )?;
    }
    let mut new = Vec::new();
    for (id, object) in &closure.objects {
        if capture.history.objects().contains_key(id) {
            require(capture.history.object(id)? == *object, "identity_mismatch")?;
        } else {
            new.push(object.clone());
        }
    }
    new.sort_by_key(|v| {
        let m = map(v).unwrap();
        (
            text(&m["subject"]).unwrap().to_owned(),
            list(&m["saw"]).unwrap().len(),
            text(&m["id"]).unwrap().to_owned(),
        )
    });
    Ok(Incoming { closure, new })
}
fn profile_of(reasoning: &V) -> String {
    map(reasoning)
        .ok()
        .and_then(|m| m.get("profile"))
        .and_then(|v| text(v).ok())
        .unwrap_or("ordinary-reader/v1")
        .to_owned()
}
fn reasoning_of(meta: &V) -> V {
    map(meta)
        .ok()
        .and_then(|m| m.get("reasoning"))
        .cloned()
        .unwrap_or(V::Null)
}
fn operations(new: &[V]) -> Result<V> {
    Ok(strings(
        new.iter()
            .map(|o| Ok(text(field(map(o)?, "op")?)?.to_owned()))
            .collect::<Result<BTreeSet<_>>>()?,
    ))
}
fn requires_choice(capture: &Capture, incoming: &Incoming) -> Result<BTreeSet<String>> {
    let present = map(&map(capture.state())?["subjects"])?;
    incoming
        .new
        .iter()
        .map(|o| Ok(text(field(map(o)?, "subject")?)?.to_owned()))
        .filter(|s| s.as_ref().is_ok_and(|s| present.contains_key(s)))
        .collect()
}

/// Heads and disputed acts other than the choice, from one subject's reduction.
fn competing(state: &Map, chosen: &V) -> Result<V> {
    Ok(strings(
        list(&state["heads"])?
            .iter()
            .chain(list(&state["disputed_acts"])?)
            .filter(|v| *v != chosen)
            .map(|v| text(v).map(str::to_owned))
            .collect::<Result<BTreeSet<_>>>()?,
    ))
}
/// The one accept act an explicit choice records. Writer and reader both build it here.
fn adoption_act(
    subject: &str,
    chosen: &V,
    revision: &str,
    over: V,
    saw: V,
    options: &A::Options,
) -> Result<V> {
    A::make_object(
        subject,
        "act",
        obj([
            ("act", s("accept")),
            ("of", chosen.clone()),
            ("over", over),
            ("because", s(&format!("explicit adoption of {revision}"))),
        ]),
        saw,
        None,
        empty(),
        empty(),
        options,
    )
}
/// A chosen head must be accepted with every head sharing its claim meaning.
fn resolved(
    state: &V,
    subject: &str,
    chosen: &V,
    object: &dyn Fn(&str) -> Result<V>,
) -> Result<()> {
    let state = map(&map(&map(state)?["subjects"])?[subject])?;
    let heads = list(&state["heads"])?;
    let meaning = crate::history_reduce::claim_meaning(map(&object(text(chosen)?)?)?)?;
    require(
        string_is(&state["acceptance"], "accepted") && heads.contains(chosen),
        "unresolved_adoption_choice",
    )?;
    for head in heads {
        require(
            crate::history_reduce::claim_meaning(map(&object(text(head)?)?)?)? == meaning,
            "unresolved_adoption_choice",
        )?;
    }
    Ok(())
}

/// Deterministic import request; replay derives the same objects from retained evidence.
pub(crate) fn import_action(
    capture: &Capture,
    bundle: &V,
    files: &Files,
    choices: &V,
) -> Result<V> {
    let incoming = incoming(capture, closure(bundle, files)?)?;
    Ok(obj([
        ("kind", s(IMPORT)),
        ("revision", s(&incoming.closure.revision)),
        ("choices", choices.clone()),
        ("operations", operations(&incoming.new)?),
    ]))
}

/// Writer plan for `contribution-import`: imported objects keep their semantic
/// identities; explicit choices add accept acts by the recorded adopter.
pub(crate) fn plan_import(
    capture: &Capture,
    action: &V,
    options: &A::Options,
    evidence: &BTreeMap<String, Vec<u8>>,
) -> Result<(Vec<V>, V, V)> {
    let a = schema(action, &["kind", "revision", "choices", "operations"], &[])?;
    require(string_is(&a["kind"], IMPORT), "invalid_contribution_import")?;
    let revision = text(&a["revision"])?;
    let key = format!("{EVIDENCE}{revision}.json");
    let raw = evidence
        .get(&key)
        .ok_or_else(|| error("invalid_contribution_evidence"))?;
    let (bundle, files) = decode_evidence(&key, raw)?;
    let incoming = incoming(capture, closure(&bundle, &files)?)?;
    require(
        import_evidence(capture, &bundle, &files, &incoming.closure)? == *evidence,
        "invalid_contribution_evidence",
    )?;
    // Carried commits join the target's ancestry only through this transaction.
    let mut clocks = capture.clocks.clone();
    clocks.merge(&incoming.closure.graph()?)?;
    let base = capture.with_clocks(clocks)?;
    require(
        incoming.closure.revision == revision,
        "invalid_contribution_import",
    )?;
    require(
        operations(&incoming.new)? == a["operations"]
            && !list(&a["operations"])?
                .iter()
                .any(|op| string_is(op, &options.operation)),
        "invalid_contribution_import",
    )?;
    let choices = map(&a["choices"]).map_err(|_| error("invalid_adoption_choices"))?;
    let required = requires_choice(capture, &incoming)?;
    let closure_subjects = incoming.closure.subjects()?;
    require(
        required.iter().all(|s| choices.contains_key(s))
            && choices.keys().all(|s| closure_subjects.contains(s)),
        "adoption_choice_required",
    )?;
    require(
        choices.is_empty() || matches!(&options.by, V::Text(by) if !by.trim().is_empty()),
        "adoption requires an explicit --by actor",
    )?;
    let template = capture
        .is_unborn()
        .then(|| incoming.closure.template.clone());
    let candidate_of = |new: &[V]| match &template {
        Some(template) => base.candidate_with_template(new, template),
        None => base.candidate(new),
    };
    let mut new = incoming.new.clone();
    let mut candidate = candidate_of(&new)?;
    for (subject, chosen) in choices {
        let claim = candidate.history.objects().get(text(chosen)?);
        require(
            claim.is_some_and(|c| {
                map(c).is_ok_and(|m| {
                    string_is(&m["subject"], subject) && !string_is(&m["kind"], "act")
                })
            }),
            "invalid_adoption_choice",
        )?;
        let state = map(&map(&map(candidate.state())?["subjects"])?[subject])?;
        #[allow(unused_mut)]
        let mut over = competing(state, chosen)?;
        #[allow(unused_mut)]
        let mut saw = strings(
            candidate
                .history
                .objects()
                .iter()
                .filter(|(_, v)| map(v).is_ok_and(|m| string_is(&m["subject"], subject)))
                .map(|(id, _)| id.clone()),
        );
        #[allow(unused_mut)]
        let mut act_options = options.clone();
        #[cfg(all(test, unix))]
        tests::forge(&mut over, &mut saw, &mut act_options.by);
        new.push(adoption_act(
            subject,
            chosen,
            revision,
            over,
            saw,
            &act_options,
        )?);
        candidate = candidate_of(&new)?;
    }
    for (subject, chosen) in choices {
        resolved(candidate.state(), subject, chosen, &|id| {
            candidate.history.object(id)
        })?;
    }
    let before = A::destination(capture.document())?;
    let after = A::destination(candidate.document())?;
    let cap = crate::reasoning_fields::capabilities(&after, None)?;
    let receipt = crate::history_transaction::semantic_receipt(
        text(&map(&cap)?["profile"])?,
        &cap,
        &obj([("document", before)]),
        &obj([("document", after.clone())]),
    )?;
    Ok((new, after, receipt))
}

/// Whether a ledger transaction context is a contribution import.
pub(crate) fn is_import(context: &V) -> bool {
    crate::history_node_transaction::is_context(context)
        && crate::history_node_transaction::validate(context)
            .ok()
            .and_then(|c| map(&c["action"]).ok())
            .and_then(|a| a.get("kind"))
            .is_some_and(|k| string_is(k, IMPORT))
}

/// Whether semantic op `semantic` belongs to an import transaction's declared operations.
pub(crate) fn member(action: &Map, semantic: &str) -> Result<bool> {
    Ok(list(field(action, "operations")?)?
        .iter()
        .any(|op| string_is(op, semantic)))
}

/// Accepted on a compact target: an import of exactly this revision is recorded, every
/// closure object is present unchanged, and non-closure evidence matches the target files.
pub(crate) fn accepted(
    target: &Capture,
    bundle: &V,
    files: &Files,
    evidence: &Files,
) -> Result<bool> {
    let closure = closure(bundle, files)?;
    // The capture already bound each import to its retained bundle; acceptance also
    // requires that retained bundle to be byte-identical to the one being observed.
    let (key, raw) = encode_evidence(bundle, files)?;
    let imported = target.snapshot.transactions.values().any(|tx| {
        tx.context.as_ref().is_some_and(is_import)
            && tx.evidence.get(&key) == Some(&sha256(&raw))
            && target
                .snapshot
                .raw_evidence
                .get(&key)
                .is_some_and(|retained| retained.as_slice() == raw.as_slice())
    });
    if !imported {
        return Ok(false);
    }
    for (id, object) in &closure.objects {
        if !target.history.objects().contains_key(id) || target.history.object(id)? != *object {
            return Ok(false);
        }
    }
    let manifest = map(field(map(bundle)?, "manifest")?)?;
    Ok(map(field(manifest, "evidence")?)?
        .iter()
        .all(|(path, digest)| {
            path.starts_with(RESERVED)
                || path.starts_with("history-closure/")
                || evidence
                    .get(path)
                    .is_some_and(|raw| *digest == s(&sha256(raw)))
        }))
}

thread_local! {
    /// Validated retained closures by exact retained-byte digest. Bounded; a closure is
    /// immutable, so reuse never changes a reading.
    static RETAINED: std::cell::RefCell<BTreeMap<String, std::sync::Arc<Closure>>> =
        const { std::cell::RefCell::new(BTreeMap::new()) };
}
fn retained(key: &str, raw: &[u8]) -> Result<std::sync::Arc<Closure>> {
    let digest = format!("{key}:{}", sha256(raw));
    if let Some(found) = RETAINED.with(|c| c.borrow().get(&digest).cloned()) {
        return Ok(found);
    }
    let (bundle, files) = decode_evidence(key, raw)?;
    let value = std::sync::Arc::new(closure(&bundle, &files)?);
    RETAINED.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() >= 8 {
            c.clear();
        }
        c.insert(digest, value.clone());
    });
    Ok(value)
}
fn ancestors(snapshot: &P::Snapshot, op: &str) -> Result<BTreeSet<String>> {
    let mut found = BTreeSet::new();
    let mut todo = snapshot
        .transactions
        .get(op)
        .ok_or_else(|| error("node_transaction_missing_parent"))?
        .parents
        .clone();
    while let Some(parent) = todo.pop() {
        if found.insert(parent.clone()) {
            todo.extend(
                snapshot
                    .transactions
                    .get(&parent)
                    .ok_or_else(|| error("node_transaction_missing_parent"))?
                    .parents
                    .iter()
                    .cloned(),
            );
            require(
                found.len() <= crate::history_node_codec::MAX_EVENTS,
                "node_transaction_limit",
            )?;
        }
    }
    Ok(found)
}

/// Read-time binding of every contribution import to its retained, validated closure.
/// Checks what the transaction introduced against its causal before-state (ancestors):
/// the exact imported objects, their operations and source orders, the carried commits
/// it admitted, and explicit choices with their accept acts and recorded actor. It does
/// not replay reductions; publication already replayed the whole plan under its guard.
pub(crate) fn validate_imports(
    snapshot: &P::Snapshot,
    history: &History,
    events: &BTreeMap<String, String>,
) -> Result<()> {
    for (op, tx) in &snapshot.transactions {
        let Some(context) = tx.context.as_ref().filter(|c| is_import(c)) else {
            continue;
        };
        bind_import(snapshot, history, events, op, tx, context)
            .map_err(|e| error(&format!("contribution_import_binding: {}", e.0)))?;
    }
    Ok(())
}
fn bind_import(
    snapshot: &P::Snapshot,
    history: &History,
    events: &BTreeMap<String, String>,
    op: &str,
    tx: &P::Transaction,
    context: &V,
) -> Result<()> {
    let c = crate::history_node_transaction::validate(context)?;
    let a = schema(
        &c["action"],
        &["kind", "revision", "choices", "operations"],
        &[],
    )?;
    let revision = text(&a["revision"])?;
    let key = format!("{EVIDENCE}{revision}.json");
    require(tx.evidence.contains_key(&key), "retained_bundle_missing")?;
    let raw = snapshot
        .raw_evidence
        .get(&key)
        .ok_or_else(|| error("retained_bundle_missing"))?;
    let closure = retained(&key, raw)?;
    require(closure.revision == revision, "retained_bundle_revision")?;
    let before = ancestors(snapshot, op)?;
    // Carried commits: exactly those the causal before-state had not admitted.
    let admitted = before
        .iter()
        .flat_map(|o| snapshot.transactions[o].evidence.keys())
        .filter(|p| p.starts_with(Clock::PREFIX))
        .collect::<BTreeSet<_>>();
    let carried = tx
        .evidence
        .keys()
        .filter(|p| p.starts_with(Clock::PREFIX))
        .collect::<BTreeSet<_>>();
    let expected = closure
        .clocks
        .keys()
        .filter(|p| !admitted.contains(p))
        .collect::<BTreeSet<_>>();
    require(carried == expected, "imported_clock_mismatch")?;
    for path in carried {
        require(
            snapshot.raw_evidence.get(path).map(|r| r.as_slice())
                == closure.clocks.get(path).map(Vec::as_slice),
            "imported_clock_mismatch",
        )?;
    }
    require(
        tx.evidence
            .keys()
            .all(|p| *p == key || p.starts_with(Clock::PREFIX)),
        "import_evidence_unexpected",
    )?;
    // Objects this transaction introduced, against those its ancestors introduced.
    let introduced_by = |ops: &dyn Fn(&str) -> bool| {
        events
            .iter()
            .filter(|(_, event)| snapshot.operations.get(*event).is_some_and(|o| ops(o)))
            .map(|(id, _)| id.clone())
            .collect::<BTreeSet<_>>()
    };
    let prior = introduced_by(&|o| before.contains(o));
    let mut imported = BTreeSet::new();
    let mut operations = BTreeSet::new();
    let mut resolutions = Vec::new();
    for id in introduced_by(&|o| o == op) {
        let object = history.object(&id)?;
        let o = map(&object)?;
        if string_is(&o["op"], op) {
            resolutions.push(object);
            continue;
        }
        require(
            closure.objects.get(&id) == Some(&object),
            "imported_object_unwitnessed",
        )?;
        require(
            history.source_orders().get(&id) == closure.orders.get(&id),
            "imported_source_order_mismatch",
        )?;
        operations.insert(text(&o["op"])?.to_owned());
        imported.insert(id);
    }
    let expected = closure
        .objects
        .keys()
        .filter(|id| !prior.contains(*id))
        .cloned()
        .collect::<BTreeSet<_>>();
    require(imported == expected, "imported_object_set_mismatch")?;
    require(
        a["operations"] == strings(operations),
        "imported_operation_unwitnessed",
    )?;
    // Explicit choices: every subject the before-state held and this import extended.
    let subject_of = |id: &String| -> Result<String> {
        let object = history
            .objects()
            .get(id)
            .ok_or_else(|| error("missing_object"))?;
        Ok(text(field(map(object)?, "subject")?)?.to_owned())
    };
    let held = prior
        .iter()
        .map(subject_of)
        .collect::<Result<BTreeSet<_>>>()?;
    let extended = imported
        .iter()
        .map(subject_of)
        .collect::<Result<BTreeSet<_>>>()?;
    let choices = map(&a["choices"])?;
    let subjects = closure.subjects()?;
    require(
        extended
            .intersection(&held)
            .all(|subject| choices.contains_key(subject))
            && choices.keys().all(|subject| subjects.contains(subject)),
        "adoption_choice_binding",
    )?;
    let options = crate::history_node_writer::decode_options(op, &c["options"])?;
    require(
        choices.is_empty() || matches!(&options.by, V::Text(t) if !t.trim().is_empty()),
        "adoption_actor_binding",
    )?;
    require(resolutions.len() == choices.len(), "adoption_act_binding")?;
    let mut acts = BTreeMap::new();
    for act in resolutions {
        let subject = text(&map(&act)?["subject"])?.to_owned();
        require(
            choices.contains_key(&subject) && acts.insert(subject, act).is_none(),
            "adoption_act_binding",
        )?;
    }
    // Ancestry the plan read: commits its ancestors admitted plus those this import carried.
    let mut clocks = Clock::Graph::default();
    for path in before
        .iter()
        .flat_map(|o| snapshot.transactions[o].evidence.keys())
        .chain(tx.evidence.keys())
        .filter(|p| p.starts_with(Clock::PREFIX))
    {
        clocks.insert(
            path,
            snapshot
                .raw_evidence
                .get(path)
                .ok_or_else(|| error("imported_clock_mismatch"))?,
        )?;
    }
    // Reduction is per subject, so each chosen subject is reduced over exactly its causal
    // before-state plus the imported objects; the recorded act must be the one derived.
    for (subject, chosen) in choices {
        let ids = prior
            .iter()
            .chain(&imported)
            .filter(|id| subject_of(id).is_ok_and(|s| s == *subject))
            .cloned()
            .collect::<BTreeSet<_>>();
        require(ids.contains(text(chosen)?), "adoption_act_binding")?;
        let claim = history.object(text(chosen)?)?;
        require(
            !string_is(&map(&claim)?["kind"], "act"),
            "adoption_act_binding",
        )?;
        let mut scoped = ids
            .iter()
            .map(|id| Ok((id.clone(), history.objects()[id].clone())))
            .collect::<Result<Map>>()?;
        let reduce = |scoped: &Map| {
            clocks.with_ancestry(|ancestry| history.reduce_subset(scoped, None, Some(ancestry)))
        };
        let before_state = reduce(&scoped)?;
        let entry = map(&map(&map(&before_state)?["subjects"])?[subject])?;
        let expected = adoption_act(
            subject,
            chosen,
            revision,
            competing(entry, chosen)?,
            strings(ids.iter().cloned()),
            &options,
        )?;
        let act = &acts[subject];
        require(*act == expected, "adoption_act_binding")?;
        let act_id = text(&map(act)?["id"])?.to_owned();
        scoped.insert(act_id.clone(), history.objects()[&act_id].clone());
        resolved(&reduce(&scoped)?, subject, chosen, &|id| history.object(id))?;
    }
    Ok(())
}

fn options(operation: String, recorded_at: &str, by: V) -> A::Options {
    A::Options {
        operation,
        recorded_at: recorded_at.into(),
        recording_day: recorded_at.get(..10).unwrap_or(recorded_at).into(),
        by,
        strict: true,
        paths: crate::history_paths::Scheme::Hashed,
        receipt_version: None,
    }
}
/// One transaction admits the carried commits and imports the closure. Choices, clocks,
/// membership and the actor are checked while planning, before anything is written; the
/// caller's guard runs under the publication lock before the journal and at each phase.
fn import(
    root: &Path,
    bundle: &V,
    files: &Files,
    choices: &V,
    options: &A::Options,
    guard: &mut dyn FnMut() -> Result<()>,
) -> Result<()> {
    let capture = Capture::read(root)?;
    let closure = closure(bundle, files)?;
    let evidence = import_evidence(&capture, bundle, files, &closure)?;
    let action = import_action(&capture, bundle, files, choices)?;
    let prepared =
        crate::history_node_writer::prepare_evidence(root, &action, options, None, evidence)?;
    guard()?;
    let guard = std::cell::RefCell::new(guard);
    P::publish(
        root,
        &prepared,
        |p| {
            (guard.borrow_mut())()?;
            crate::history_node_writer::verify(root, p, None)?;
            (guard.borrow_mut())()
        },
        |_| {
            crate::history_node_writer::verify_sources(root, &prepared)?;
            (guard.borrow_mut())()
        },
    )
}

/// Verified compact materialization of a v3 or v4 contribution into an empty directory.
/// The record identity is the contribution's own; it never claims the source record.
pub(crate) fn materialize(root: &Path, bundle: &V, files: &Files) -> Result<V> {
    let closure = closure(bundle, files)?;
    let recorded_at = closure.recorded_at.clone();
    let marker = Y::encode_document(&obj([
        ("version", n("3")),
        ("profile", s(crate::history_node_codec::FORMAT)),
        ("authority", s("history")),
        (
            "record_id",
            s(&format!("contribution-{}", closure.revision)),
        ),
        ("generation", n("1")),
        (
            "requires",
            strings([crate::history_node_codec::FORMAT.to_owned()]),
        ),
    ]))?;
    std::fs::create_dir_all(root.join(".kpopper"))?;
    crate::history_transaction_fs::publish_immutable(root, ".kpopper/history.yaml", &marker)?;
    import(
        root,
        bundle,
        files,
        &empty(),
        &options(
            format!("materialize-{}", &closure.revision[..32]),
            &recorded_at,
            V::Null,
        ),
        &mut || Ok(()),
    )?;
    let captured = Capture::read(root)?;
    let incoming = map(&closure.template)?
        .get("meta")
        .map(reasoning_of)
        .unwrap_or(V::Null);
    let roles_kept = match map(&closure.template)?.get("schema") {
        Some(roles) => map(roles)?.iter().all(|(role, field)| {
            map(captured.document())
                .ok()
                .and_then(|d| d.get("schema"))
                .and_then(|v| map(v).ok())
                .and_then(|m| m.get(role))
                == Some(field)
        }),
        None => true,
    };
    require(
        entries(captured.document())?.digest_eq(&entries(&closure.document)?)?
            && profile_of(&reasoning(captured.document())) == profile_of(&incoming)
            && roles_kept,
        "materialized history does not reproduce captured contribution",
    )?;
    Ok(captured.document().clone())
}
trait DigestEq {
    fn digest_eq(&self, other: &Self) -> Result<bool>;
}
impl DigestEq for BTreeMap<String, (String, V)> {
    fn digest_eq(&self, other: &Self) -> Result<bool> {
        let value = |m: &Self| {
            V::Map(
                m.iter()
                    .map(|(k, (c, b))| (k.clone(), V::List(vec![s(c), b.clone()])))
                    .collect(),
            )
        };
        Ok(value(self).digest()? == value(other).digest()?)
    }
}

/// Public adoption into a compact target. Preview reports required choices only.
pub(crate) fn adopt(
    root: &Path,
    bundle: &V,
    files: &Files,
    choices: &V,
    by: Option<&str>,
    preview: bool,
    guard: &mut dyn FnMut() -> Result<()>,
) -> Result<V> {
    let capture = Capture::read(root)?;
    let incoming = incoming(&capture, closure(bundle, files)?)?;
    if preview {
        let required = requires_choice(&capture, &incoming)?;
        let targets = map(&map(capture.state())?["subjects"])?;
        let graph = incoming.closure.graph()?;
        let history = compact(&incoming.closure.objects, &incoming.closure.orders)?;
        let state = reduce(&history, &incoming.closure.rules, &graph)?;
        let mut subjects = Map::new();
        for subject in incoming.closure.subjects()? {
            subjects.insert(
                subject.clone(),
                obj([
                    ("requires_choice", V::Bool(required.contains(&subject))),
                    (
                        "target_heads",
                        targets
                            .get(&subject)
                            .map(map)
                            .transpose()?
                            .and_then(|m| m.get("heads"))
                            .cloned()
                            .unwrap_or(V::List(vec![])),
                    ),
                    (
                        "incoming_heads",
                        map(&map(&map(&state)?["subjects"])?[&subject])?["heads"].clone(),
                    ),
                    (
                        "claims",
                        strings(
                            incoming
                                .closure
                                .objects
                                .iter()
                                .filter(|(_, v)| {
                                    map(v).is_ok_and(|m| {
                                        string_is(&m["subject"], &subject)
                                            && !string_is(&m["kind"], "act")
                                    })
                                })
                                .map(|(id, _)| id.clone()),
                        ),
                    ),
                ]),
            );
        }
        return Ok(obj([
            ("artifact_revision", s(&incoming.closure.revision)),
            ("target_authority", capture.snapshot.authority.clone()),
            ("subjects", V::Map(subjects)),
        ]));
    }
    let by = by
        .filter(|b| !b.trim().is_empty())
        .ok_or_else(|| error("adoption requires an explicit --by actor"))?;
    let operation = format!("adopt-{}", uuid::Uuid::new_v4().simple());
    let recorded_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, false);
    import(
        root,
        bundle,
        files,
        choices,
        &options(operation.clone(), &recorded_at, s(by)),
        guard,
    )?;
    Ok(obj([
        ("state", s("adopted")),
        ("revision", s(&incoming.closure.revision)),
        ("operation", s(&operation)),
    ]))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::{
        history_node_codec as C, history_node_writer as W, history_source_ancestry::Proof,
    };
    use serde_json::json;

    fn value(v: serde_json::Value) -> V {
        V::from_json(&v).unwrap()
    }
    fn setup() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".kpopper")).unwrap();
        std::fs::write(root.path().join(".kpopper/history.yaml"), "version: 3\nprofile: node-history/v1\nauthority: history\nrecord_id: fixture\ngeneration: 1\nrequires: [node-history/v1]\n").unwrap();
        std::fs::write(
            root.path().join("GROUNDING.yaml"),
            "meta:\n  purpose: Fixture\nschema:\n  deps: rests_on\n  snapshot: seen\n  predicate: wrong_if\nknown: {}\n",
        )
        .unwrap();
        root
    }
    fn git(root: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .current_dir(root)
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.test",
                "-c",
                "commit.gpgSign=false",
            ])
            .args(args)
            .env("GIT_AUTHOR_DATE", "2026-09-24T12:00:00+00:00")
            .env("GIT_COMMITTER_DATE", "2026-09-24T12:00:00+00:00")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().into()
    }
    /// A scoped reading whose acceptance depends on source-clock ancestry.
    fn seed(root: &Path, operation: &str, commit: &str, n: i32) {
        let scope = json!({"kind":"project","environment":"workspace"});
        let mut object = json!({"id_scheme":"typed-history/v2","schema_version":2,"subject":"p.clock",
            "kind":"reading","by":"fixture","on":"2026-09-24","op":operation,"at":{"commit":commit},
            "body":{"v":n,"from":"source.repository","scope":scope},"saw":[],"pins":{},
            "authored":{"collection":"known","profile":"ordinary-reader/v1","fields":{"deps":"rests_on","snapshot":"seen","predicate":"wrong_if"}}});
        let id = crate::identity::typed_object_identity(&value(object.clone())).unwrap();
        object["id"] = json!(id);
        let object = value(object);
        let observation = ObservationNode::root(&id, &Default::default()).unwrap();
        let payload = crate::history_node_capture::payload(&object, &observation).unwrap();
        let event = C::Event::create("p.clock", operation, vec![], None, Some(payload)).unwrap();
        let doc = value(
            json!({"meta":{"purpose":"Fixture"},"schema":{"deps":"rests_on","snapshot":"seen","predicate":"wrong_if"},
            "known":{"p.clock":{"v":n,"from":"source.repository","scope":scope}}}),
        );
        let p = P::prepare(
            root,
            operation,
            P::bind_view(&Y::encode_document(&doc).unwrap(), operation).unwrap(),
            BTreeMap::from([("p.clock".into(), event.encode().unwrap())]),
        )
        .unwrap();
        P::publish(root, &p, |_| Ok(()), |_| Ok(())).unwrap();
    }
    fn acceptance(root: &Path) -> V {
        let c = Capture::read(root).unwrap();
        map(&map(&map(c.state()).unwrap()["subjects"]).unwrap()["p.clock"]).unwrap()["acceptance"]
            .clone()
    }
    fn reseal(bundle: &mut V, files: &Files) {
        let V::Map(b) = bundle else { panic!() };
        let V::Map(m) = b.get_mut("manifest").unwrap() else {
            panic!()
        };
        m.insert(
            "evidence".into(),
            V::Map(
                files
                    .iter()
                    .map(|(p, raw)| (p.clone(), s(&sha256(raw))))
                    .collect(),
            ),
        );
        let revision = V::Map(m.clone()).digest().unwrap();
        b.insert("revision".into(), s(&revision));
    }

    /// A source whose `p.clock` acceptance depends on admitted commit ancestry.
    fn clock_source() -> (tempfile::TempDir, tempfile::TempDir) {
        let repo = tempfile::tempdir().unwrap();
        git(repo.path(), &["init", "--object-format=sha1"]);
        git(repo.path(), &["commit", "--allow-empty", "-m", "older"]);
        let a = git(repo.path(), &["rev-parse", "HEAD"]);
        git(repo.path(), &["commit", "--allow-empty", "-m", "newer"]);
        let b = git(repo.path(), &["rev-parse", "HEAD"]);
        let proof = Proof::capture(repo.path(), &a, &b).unwrap();

        let source = setup();
        let sibling = setup();
        seed(source.path(), "left-clock", &a, 1);
        seed(sibling.path(), "right-clock", &b, 2);
        let authored = |op: &str| A::Options {
            operation: op.into(),
            recorded_at: "2026-09-24T12:00:00+00:00".into(),
            recording_day: "2026-09-24".into(),
            by: s("writer"),
            strict: false,
            paths: crate::history_paths::Scheme::Hashed,
            receipt_version: None,
        };
        for (root, op, id) in [
            (source.path(), "left-seed", "p.a"),
            (sibling.path(), "right-seed", "p.b"),
        ] {
            let p = W::prepare(
                root,
                &value(json!({"kind":"add","id":id,"body":{"v":1}})),
                &authored(op),
                None,
            )
            .unwrap();
            W::publish(root, &p, None, |_| Ok(())).unwrap();
        }
        let union = crate::history_node_branch::prepare(
            source.path(),
            &[P::export(sibling.path()).unwrap()],
            "merge",
        )
        .unwrap();
        W::publish(source.path(), &union, None, |_| Ok(())).unwrap();
        assert_eq!(acceptance(source.path()), s("contested"));
        let p = crate::history_node_clocks::prepare(source.path(), "proof", &proof).unwrap();
        W::publish(source.path(), &p, None, |_| Ok(())).unwrap();
        assert_eq!(acceptance(source.path()), s("accepted"));

        (repo, source)
    }
    fn clock_bundle(source: &Path) -> (V, Files) {
        let capture = Capture::read(source).unwrap();
        let plan = plan(
            &capture,
            &[],
            &["p.clock".into()],
            "contribution-clock",
            "2026-09-24T00:00:00Z",
            &V::List(vec![]),
        )
        .unwrap();
        assert!(!plan.closure.clocks.is_empty());
        let scope = obj([("kind", s("project")), ("environment", s("workspace"))]);
        super::bundle(&plan, &scope, &Files::new()).unwrap()
    }
    /// Every directory and file below `root`, with exact bytes.
    fn tree(root: &Path) -> Vec<(String, Option<Vec<u8>>)> {
        let mut out = vec![];
        let mut todo = vec![root.to_owned()];
        while let Some(dir) = todo.pop() {
            for item in std::fs::read_dir(&dir).unwrap() {
                let path = item.unwrap().path();
                let name = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                if path.is_dir() {
                    out.push((name, None));
                    todo.push(path);
                } else {
                    out.push((name, Some(std::fs::read(&path).unwrap())));
                }
            }
        }
        out.sort();
        out
    }

    /// A second, unrelated valid v4 bundle (ordinary-reader `p.a`).
    fn other_bundle() -> (tempfile::TempDir, V, Files) {
        let source = setup();
        let p = W::prepare(
            source.path(),
            &value(json!({"kind":"add","id":"p.a","body":{"v":1,"scope":{"kind":"project","environment":"workspace"}}})),
            &A::Options {
                operation: "seed".into(),
                recorded_at: "2026-09-24T12:00:00+00:00".into(),
                recording_day: "2026-09-24".into(),
                by: s("writer"),
                strict: false,
                paths: crate::history_paths::Scheme::Hashed,
                receipt_version: None,
            },
            None,
        )
        .unwrap();
        W::publish(source.path(), &p, None, |_| Ok(())).unwrap();
        let capture = Capture::read(source.path()).unwrap();
        let plan = plan(
            &capture,
            &[],
            &["p.a".into()],
            "c",
            "2026-09-24T00:00:00Z",
            &V::List(vec![]),
        )
        .unwrap();
        let scope = obj([("kind", s("project")), ("environment", s("workspace"))]);
        let (bundle, files) = super::bundle(&plan, &scope, &Files::new()).unwrap();
        (source, bundle, files)
    }
    fn set_path(value: &mut V, path: &[&str], new: V) {
        let (last, parents) = path.split_last().unwrap();
        let mut current = value;
        for key in parents {
            let V::Map(m) = current else { panic!("{key}") };
            current = m.get_mut(*key).unwrap();
        }
        let V::Map(m) = current else { panic!("{last}") };
        m.insert((*last).into(), new);
    }

    #[test]
    fn read_time_binding_refuses_rehashed_import_envelopes() {
        let (_repo, source) = clock_source();
        let (bundle, files) = clock_bundle(source.path());
        let target = setup();
        seed(target.path(), "target-clock", &"f".repeat(40), 9);
        let closure = validate(&bundle, &files).unwrap();
        let reading = |v: &str| {
            closure
                .objects
                .iter()
                .find(|(_, o)| {
                    map(o).is_ok_and(|m| {
                        string_is(&m["kind"], "reading")
                            && map(&m["body"]).is_ok_and(|b| b.get("v") == Some(&n(v)))
                    })
                })
                .map(|(id, _)| id.clone())
                .unwrap()
        };
        let chosen = obj([("p.clock", s(&reading("2")))]);
        adopt(
            target.path(),
            &bundle,
            &files,
            &chosen,
            Some("reviewer"),
            false,
            &mut || Ok(()),
        )
        .unwrap();
        let snapshot = P::capture_snapshot(target.path()).unwrap();
        let (op, _) = snapshot
            .transactions
            .iter()
            .find(|(_, tx)| tx.context.as_ref().is_some_and(is_import))
            .map(|(op, tx)| (op.clone(), tx.clone()))
            .unwrap();
        let key = format!("{EVIDENCE}{}.json", closure.revision);
        let accepted_capture = Capture::from_snapshot(snapshot.clone()).unwrap();
        assert!(accepted(&accepted_capture, &bundle, &files, &Files::new()).unwrap());
        let refuse = |snapshot: P::Snapshot, code: &str| {
            let error = Capture::from_snapshot(snapshot)
                .err()
                .expect("tampered import accepted")
                .0;
            assert!(
                error.starts_with("contribution_import_binding") && error.ends_with(code),
                "{code}: {error}"
            );
        };
        fn context<'a>(snapshot: &'a mut P::Snapshot, op: &str) -> &'a mut V {
            snapshot
                .transactions
                .get_mut(op)
                .unwrap()
                .context
                .as_mut()
                .unwrap()
        }

        // An operation the retained closure never introduced.
        let mut tampered = snapshot.clone();
        let V::Map(action) = field(map(context(&mut tampered, &op)).unwrap(), "action")
            .unwrap()
            .clone()
        else {
            panic!()
        };
        let mut ops = list(&action["operations"]).unwrap().to_vec();
        ops.push(s("zzz-forged-op"));
        set_path(
            context(&mut tampered, &op),
            &["action", "operations"],
            V::List(ops),
        );
        refuse(tampered, "imported_operation_unwitnessed");

        // The retained bundle is missing.
        let mut tampered = snapshot.clone();
        tampered.raw_evidence.remove(&key);
        refuse(tampered, "retained_bundle_missing");

        // A different valid bundle, with revision, evidence map and bytes rehashed consistently.
        let (_other, other, other_files) = other_bundle();
        let (other_key, other_raw) = encode_evidence(&other, &other_files).unwrap();
        let mut tampered = snapshot.clone();
        {
            let tx = tampered.transactions.get_mut(&op).unwrap();
            tx.evidence.remove(&key);
            tx.evidence.insert(other_key.clone(), sha256(&other_raw));
            let evidence = V::Map(tx.evidence.iter().map(|(p, h)| (p.clone(), s(h))).collect());
            let c = tx.context.as_mut().unwrap();
            set_path(c, &["evidence"], evidence);
            set_path(
                c,
                &["action", "revision"],
                s(text(field(map(&other).unwrap(), "revision").unwrap()).unwrap()),
            );
        }
        tampered.raw_evidence.remove(&key);
        tampered
            .raw_evidence
            .insert(other_key.clone(), std::sync::Arc::new(other_raw.clone()));
        // The substitute carries no commits, so the admitted clocks no longer match it.
        refuse(tampered.clone(), "imported_clock_mismatch");
        // Rehash the envelope without those clocks too: the imported objects stay unwitnessed.
        {
            let tx = tampered.transactions.get_mut(&op).unwrap();
            tx.evidence.retain(|p, _| !p.starts_with(Clock::PREFIX));
            let evidence = V::Map(tx.evidence.iter().map(|(p, h)| (p.clone(), s(h))).collect());
            set_path(tx.context.as_mut().unwrap(), &["evidence"], evidence);
        }
        refuse(tampered, "imported_object_unwitnessed");

        // A changed choice no longer matches the recorded accept act.
        let mut tampered = snapshot.clone();
        set_path(
            context(&mut tampered, &op),
            &["action", "choices", "p.clock"],
            s(&reading("1")),
        );
        refuse(tampered, "adoption_act_binding");

        // Dropping the choice leaves an extended, previously held subject unchosen.
        let mut tampered = snapshot.clone();
        set_path(context(&mut tampered, &op), &["action", "choices"], empty());
        refuse(tampered, "adoption_choice_binding");

        // A changed actor no longer matches the act's recorded author.
        let mut tampered = snapshot.clone();
        let mut options = field(map(context(&mut tampered, &op)).unwrap(), "options")
            .unwrap()
            .clone();
        set_path(&mut options, &["by"], s("mallory"));
        set_path(context(&mut tampered, &op), &["options"], options);
        refuse(tampered, "adoption_act_binding");

        // Acceptance is bound to the exact retained bundle, not a revision string.
        assert!(!accepted(&accepted_capture, &other, &other_files, &Files::new()).unwrap());
    }

    thread_local! {
        static FORGE: std::cell::RefCell<Option<Box<dyn Fn(&mut V, &mut V, &mut V)>>> =
            std::cell::RefCell::new(None);
    }
    /// Test-only writer seam: a foreign writer may record any well-formed act.
    pub(super) fn forge(over: &mut V, saw: &mut V, by: &mut V) {
        FORGE.with(|f| {
            if let Some(f) = f.borrow().as_ref() {
                f(over, saw, by)
            }
        })
    }
    fn setup_schema(deps: &str) -> tempfile::TempDir {
        let root = setup();
        std::fs::write(
            root.path().join("GROUNDING.yaml"),
            format!("meta:\n  purpose: Fixture\nschema:\n  deps: {deps}\n  snapshot: seen\n  predicate: wrong_if\nknown: {{}}\n"),
        )
        .unwrap();
        root
    }
    fn ordinary(deps: &str) -> (tempfile::TempDir, V, Files) {
        let source = setup_schema(deps);
        let p = W::prepare(
            source.path(),
            &value(json!({"kind":"add","id":"p.a","body":{"v":1,"scope":{"kind":"project","environment":"workspace"}}})),
            &A::Options {
                operation: "seed".into(),
                recorded_at: "2026-09-24T12:00:00+00:00".into(),
                recording_day: "2026-09-24".into(),
                by: s("writer"),
                strict: false,
                paths: crate::history_paths::Scheme::Hashed,
                receipt_version: None,
            },
            None,
        )
        .unwrap();
        W::publish(source.path(), &p, None, |_| Ok(())).unwrap();
        let capture = Capture::read(source.path()).unwrap();
        let plan = plan(
            &capture,
            &[],
            &["p.a".into()],
            "c",
            "2026-09-24T00:00:00Z",
            &V::List(vec![]),
        )
        .unwrap();
        let scope = obj([("kind", s("project")), ("environment", s("workspace"))]);
        let (bundle, files) = super::bundle(&plan, &scope, &Files::new()).unwrap();
        (source, bundle, files)
    }
    fn roles(document: &V) -> V {
        map(document)
            .unwrap()
            .get("schema")
            .cloned()
            .unwrap_or(V::Null)
    }
    fn oracle(name: &str) -> (V, Files) {
        let cases = crate::history_contribution_adoption::tests::cases();
        let case = cases["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .unwrap();
        (
            V::from_tagged(&case["bundle"]).unwrap(),
            crate::history_contribution_adoption::tests::files(&case["bundle_files"]),
        )
    }
    fn commits(root: &Path) -> usize {
        std::fs::read_dir(root.join(".kpopper/history-commits"))
            .map(|d| d.count())
            .unwrap_or(0)
    }

    #[test]
    fn marker_only_materialization_keeps_incoming_profile_roles_and_meaning() {
        // Ordinary v4, with the default roles and with a changed dependency role.
        for deps in ["rests_on", "because_of"] {
            let (source, bundle, files) = ordinary(deps);
            let staging = tempfile::tempdir().unwrap();
            materialize(staging.path(), &bundle, &files).unwrap();
            let captured = Capture::read(staging.path()).unwrap();
            let original = Capture::read(source.path()).unwrap();
            assert_eq!(reasoning(captured.document()), V::Null);
            assert_eq!(
                roles(captured.document()),
                roles(original.document()),
                "{deps}"
            );
            assert_eq!(
                entries(captured.document())
                    .unwrap()
                    .digest_eq(&entries(original.document()).unwrap())
                    .unwrap(),
                true
            );
        }
        // Legacy v3 oracle bundles in both profiles.
        for name in ["ordinary-reader/v1/incoming", "core/v1/incoming"] {
            let (bundle, files) = oracle(name);
            crate::pending_bundle::validate(&bundle, &files).unwrap();
            let closure = closure(&bundle, &files).unwrap();
            let staging = tempfile::tempdir().unwrap();
            materialize(staging.path(), &bundle, &files).unwrap_or_else(|e| panic!("{name}: {e}"));
            let captured = Capture::read(staging.path()).unwrap();
            let expected = map(&closure.template)
                .unwrap()
                .get("meta")
                .map(reasoning_of)
                .unwrap_or(V::Null);
            assert_eq!(
                profile_of(&reasoning(captured.document())),
                profile_of(&expected),
                "{name}"
            );
            assert!(name.starts_with(&profile_of(&expected)), "{name}");
            assert!(
                entries(captured.document())
                    .unwrap()
                    .digest_eq(&entries(&closure.document).unwrap())
                    .unwrap()
            );
        }
    }

    #[test]
    fn rules_that_compact_history_cannot_hold_are_refused_not_reinterpreted() {
        let (_source, mut bundle, files) = ordinary("rests_on");
        let rules = value(json!({"version": 2, "self_review_counts": true}));
        {
            let V::Map(b) = &mut bundle else { panic!() };
            let V::Map(m) = b.get_mut("manifest").unwrap() else {
                panic!()
            };
            let V::Map(c) = m.get_mut("closure").unwrap() else {
                panic!()
            };
            c.insert("rules".into(), rules);
            let revision = V::Map(m.clone()).digest().unwrap();
            b.insert("revision".into(), s(&revision));
        }
        // The rules variant is itself a valid closure...
        validate(&bundle, &files).unwrap();
        // ...but compact capture reduces with default rules, so it is refused unchanged.
        let staging = tempfile::tempdir().unwrap();
        assert_eq!(
            materialize(staging.path(), &bundle, &files)
                .err()
                .unwrap()
                .0,
            "rules_mismatch"
        );
        assert_eq!(commits(staging.path()), 0);
    }

    #[test]
    fn established_targets_are_never_reinterpreted() {
        let (core_bundle, core_files) = oracle("core/v1/incoming");
        let core = tempfile::tempdir().unwrap();
        materialize(core.path(), &core_bundle, &core_files).unwrap();
        let (_ordinary, ordinary_bundle, ordinary_files) = ordinary("rests_on");
        let (_variant, variant_bundle, variant_files) = ordinary("because_of");
        let refuse = |target: &Path, bundle: &V, files: &Files| {
            let before = tree(target);
            let error = adopt(
                target,
                bundle,
                files,
                &empty(),
                Some("reviewer"),
                false,
                &mut || Ok(()),
            )
            .err()
            .unwrap()
            .0;
            assert_eq!(tree(target), before);
            error
        };
        assert_eq!(
            refuse(core.path(), &ordinary_bundle, &ordinary_files),
            "adoption_profile_mismatch"
        );
        let ordinary_target = setup();
        seed(ordinary_target.path(), "target-clock", &"f".repeat(40), 9);
        assert_eq!(
            refuse(ordinary_target.path(), &core_bundle, &core_files),
            "adoption_profile_mismatch"
        );
        // Same profile, different field role for dependencies.
        assert_eq!(
            refuse(ordinary_target.path(), &variant_bundle, &variant_files),
            "adoption_profile_mismatch"
        );
        // The same profile name does not license changing its version or required
        // capabilities. The import boundary compares the complete declaration.
        let mut changed = closure(&core_bundle, &core_files).unwrap();
        let declaration = value(json!({"version":2, "profile":"core/v1",
            "requires":["arithmetic/v1", "composition/v1", "query/v1"]}));
        crate::history_view::map_mut(
            crate::history_view::map_mut(&mut changed.document)
                .unwrap()
                .get_mut("meta")
                .unwrap(),
        )
        .unwrap()
        .insert("reasoning".into(), declaration.clone());
        crate::history_view::map_mut(
            crate::history_view::map_mut(&mut changed.template)
                .unwrap()
                .get_mut("meta")
                .unwrap(),
        )
        .unwrap()
        .insert("reasoning".into(), declaration);
        let capture = Capture::read(core.path()).unwrap();
        assert_ne!(
            crate::reasoning_fields::capabilities(capture.document(), None).unwrap(),
            crate::reasoning_fields::capabilities(&changed.document, None).unwrap()
        );
        assert_eq!(
            incoming(&capture, changed).err().unwrap().0,
            "adoption_profile_mismatch"
        );
    }

    #[test]
    fn forged_adoption_acts_published_byte_validly_are_refused_on_read() {
        let (_repo, source) = clock_source();
        let (bundle, files) = clock_bundle(source.path());
        let closure = validate(&bundle, &files).unwrap();
        let reading = |v: &str| {
            closure
                .objects
                .iter()
                .find(|(_, o)| {
                    map(o).is_ok_and(|m| {
                        string_is(&m["kind"], "reading")
                            && map(&m["body"]).is_ok_and(|b| b.get("v") == Some(&n(v)))
                    })
                })
                .map(|(id, _)| id.clone())
                .unwrap()
        };
        let superseded = reading("1");
        let chosen = obj([("p.clock", s(&reading("2")))]);
        type Forge = Box<dyn Fn(&mut V, &mut V, &mut V)>;
        let over_extra = superseded.clone();
        let saw_drop = superseded.clone();
        let forgeries: Vec<(&str, Option<Forge>)> = vec![
            ("honest", None),
            (
                "over",
                Some(Box::new(move |over: &mut V, _: &mut V, _: &mut V| {
                    let mut ids = list(over).unwrap().to_vec();
                    ids.push(s(&over_extra));
                    ids.sort_by(|a, b| text(a).unwrap().cmp(text(b).unwrap()));
                    *over = V::List(ids);
                })),
            ),
            (
                "saw",
                Some(Box::new(move |_: &mut V, saw: &mut V, _: &mut V| {
                    *saw = V::List(
                        list(saw)
                            .unwrap()
                            .iter()
                            .filter(|v| !string_is(v, &saw_drop))
                            .cloned()
                            .collect(),
                    );
                })),
            ),
            (
                "actor",
                Some(Box::new(|_: &mut V, _: &mut V, by: &mut V| {
                    *by = s("mallory")
                })),
            ),
        ];
        for (name, forgery) in forgeries {
            let target = setup();
            seed(target.path(), "target-clock", &"f".repeat(40), 9);
            let capture = Capture::read(target.path()).unwrap();
            let action = import_action(&capture, &bundle, &files, &chosen).unwrap();
            let evidence = import_evidence(&capture, &bundle, &files, &closure).unwrap();
            let honest = forgery.is_none();
            FORGE.with(|f| *f.borrow_mut() = forgery);
            let prepared = crate::history_node_writer::prepare_evidence(
                target.path(),
                &action,
                &options(
                    format!("adopt-{name}"),
                    "2026-09-25T00:00:00Z",
                    s("reviewer"),
                ),
                None,
                evidence,
            );
            FORGE.with(|f| *f.borrow_mut() = None);
            let prepared = prepared.unwrap_or_else(|e| panic!("{name}: {e}"));
            // A foreign writer that skips semantic verification; every byte hash is valid.
            P::publish(target.path(), &prepared, |_| Ok(()), |_| Ok(())).unwrap();
            P::capture_snapshot(target.path()).unwrap();
            let read = Capture::read(target.path());
            if honest {
                read.unwrap();
            } else {
                let error = read.err().unwrap_or_else(|| panic!("{name} accepted")).0;
                assert_eq!(
                    error, "contribution_import_binding: adoption_act_binding",
                    "{name}"
                );
                assert!(
                    crate::history_node_writer::verify(target.path(), &prepared, None).is_err()
                );
            }
        }
    }

    #[test]
    fn refused_adoption_leaves_no_clock_evidence_or_journal() {
        let (_repo, source) = clock_source();
        let (bundle, files) = clock_bundle(source.path());
        // The target already holds the subject, so an explicit choice is required,
        // and it has not admitted the carried commits.
        let target = setup();
        seed(target.path(), "target-clock", &"f".repeat(40), 9);
        let before = tree(target.path());
        let reading = Capture::read(target.path()).unwrap().state().clone();
        let adopt = |choices: &V, guard: &mut dyn FnMut() -> Result<()>| {
            adopt(
                target.path(),
                &bundle,
                &files,
                choices,
                Some("reviewer"),
                false,
                guard,
            )
            .err()
            .map(|e| e.0)
            .unwrap_or_default()
        };
        assert_eq!(adopt(&empty(), &mut || Ok(())), "adoption_choice_required");
        assert_eq!(tree(target.path()), before);
        let invalid = obj([("p.clock", s(&"0".repeat(64)))]);
        assert_eq!(adopt(&invalid, &mut || Ok(())), "invalid_adoption_choice");
        assert_eq!(tree(target.path()), before);
        let closure = validate(&bundle, &files).unwrap();
        let incoming = closure
            .objects
            .iter()
            .find(|(_, o)| {
                map(o).is_ok_and(|m| {
                    string_is(&m["subject"], "p.clock")
                        && string_is(&m["kind"], "reading")
                        && map(&m["body"]).is_ok_and(|b| b.get("v") == Some(&n("2")))
                })
            })
            .map(|(id, _)| id.clone())
            .unwrap();
        let chosen = obj([("p.clock", s(&incoming))]);
        // A stale caller guard refuses before any durable write.
        assert_eq!(
            adopt(&chosen, &mut || Err(error("pending_changed"))),
            "pending_changed"
        );
        assert_eq!(tree(target.path()), before);
        assert_eq!(Capture::read(target.path()).unwrap().state(), &reading);
        // Import evidence is exact: the carried commits cannot be dropped or padded.
        let capture = Capture::read(target.path()).unwrap();
        let action = import_action(&capture, &bundle, &files, &chosen).unwrap();
        let exact = import_evidence(&capture, &bundle, &files, &closure).unwrap();
        assert!(exact.keys().any(|p| p.starts_with(Clock::PREFIX)));
        let opts = options("adopt-probe".into(), "2026-09-25T00:00:00Z", s("reviewer"));
        let without = exact
            .iter()
            .filter(|(p, _)| !p.starts_with(Clock::PREFIX))
            .map(|(p, b)| (p.clone(), b.clone()))
            .collect::<Files>();
        let mut padded = exact.clone();
        padded.insert(
            format!("{}{}.commit", Clock::PREFIX, "e".repeat(40)),
            b"x".to_vec(),
        );
        for evidence in [without, padded] {
            assert_eq!(
                plan_import(&capture, &action, &opts, &evidence)
                    .err()
                    .unwrap()
                    .0,
                "invalid_contribution_evidence"
            );
        }
        plan_import(&capture, &action, &opts, &exact).unwrap();
        assert_eq!(tree(target.path()), before);
        // The accepted adoption admits the commits and imports the closure in one transaction.
        let transactions = |root: &Path| P::capture_snapshot(root).unwrap().transactions.len();
        let count = transactions(target.path());
        assert_eq!(adopt(&chosen, &mut || Ok(())), "");
        assert_eq!(transactions(target.path()), count + 1);
        let adopted = Capture::read(target.path()).unwrap();
        let heads = &map(&map(&map(adopted.state()).unwrap()["subjects"]).unwrap()["p.clock"])
            .unwrap()["heads"];
        assert_eq!(heads, &V::List(vec![s(&incoming)]));
        assert!(accepted(&adopted, &bundle, &files, &Files::new()).unwrap());
    }

    #[test]
    fn source_clock_reduction_travels_is_admitted_and_acceptance_is_observed() {
        let (_repo, source) = clock_source();
        let capture = Capture::read(source.path()).unwrap();
        let plan = plan(
            &capture,
            &[],
            &["p.clock".into()],
            "contribution-clock",
            "2026-09-24T00:00:00Z",
            &V::List(vec![]),
        )
        .unwrap();
        assert!(!plan.closure.clocks.is_empty());
        let scope = obj([("kind", s("project")), ("environment", s("workspace"))]);
        let (bundle, files) = super::bundle(&plan, &scope, &Files::new()).unwrap();
        let closure = validate(&bundle, &files).unwrap();
        assert_eq!(
            map(&map(&closure.document).unwrap()["known"]).unwrap()["p.clock"],
            value(
                json!({"v":2,"from":"source.repository","scope":{"kind":"project","environment":"workspace"}})
            )
        );

        // Without the retained commits the same objects reduce differently.
        let stripped = files
            .iter()
            .filter(|(p, _)| !p.starts_with(CLOCKS))
            .map(|(p, raw)| (p.clone(), raw.clone()))
            .collect::<Files>();
        let mut tampered = bundle.clone();
        reseal(&mut tampered, &stripped);
        assert_eq!(
            validate(&tampered, &stripped).err().unwrap().0,
            "subset_reduction_mismatch"
        );

        // Adoption admits the carried commits first, then imports the exact objects.
        let target = setup();
        let before = accepted(
            &Capture::read(target.path()).unwrap(),
            &bundle,
            &files,
            &Files::new(),
        )
        .unwrap();
        assert!(!before);
        adopt(
            target.path(),
            &bundle,
            &files,
            &empty(),
            Some("reviewer"),
            false,
            &mut || Ok(()),
        )
        .unwrap();
        assert_eq!(acceptance(target.path()), s("accepted"));
        let adopted = Capture::read(target.path()).unwrap();
        for (id, object) in &closure.objects {
            assert_eq!(&adopted.history.object(id).unwrap(), object);
        }
        assert!(accepted(&adopted, &bundle, &files, &Files::new()).unwrap());
        // The source holds the objects but never imported this revision.
        assert!(!accepted(&capture, &bundle, &files, &Files::new()).unwrap());
        // Replay under the writer's own verification succeeds after reopening.
        let copy = P::export(target.path()).unwrap().reconstruct().unwrap();
        assert_eq!(acceptance(copy.path()), s("accepted"));
    }
}
