//! Live contributions remain explicit alternatives with scope and disposition.
//! Neither sequence nor cached acceptance chooses an effective winner.
use crate::{
    Result,
    history_contract::*,
    history_view::{map_mut, truth},
    pending_state::Observation,
    reasoning_snapshot::entries,
    source_document::Document,
    value::TypedValue as V,
};
use std::collections::{BTreeMap, BTreeSet};
fn s(v: &str) -> V {
    V::Text(v.into())
}
fn empty() -> V {
    V::Map(Map::new())
}
fn obj(items: impl IntoIterator<Item = (&'static str, V)>) -> V {
    V::Map(items.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
pub(crate) struct Overlay {
    pub pending: V,
    pub publication: V,
    pub contributions: Vec<V>,
    pub conflicts: V,
    pub target: Option<V>,
    pub target_snapshot: Option<V>,
    pub unavailable: Option<String>,
    pub history_contributions: V,
}
struct Meaning {
    schema: V,
    capability: V,
    authority: Option<V>,
    roles: Option<(BTreeSet<String>, Map)>,
}
impl Meaning {
    fn new(document: &V, history: Option<&V>) -> Result<Self> {
        let mut plain = document.clone();
        let mut authority = None;
        if map(document)?
            .get("meta")
            .and_then(|v| map(v).ok())
            .is_some_and(|m| m.contains_key("history"))
        {
            let history = history.ok_or_else(|| error("missing_history_context"))?;
            crate::history_projection::CapturedHistory::new(document.clone(), history.clone())?;
            map_mut(map_mut(&mut plain)?.get_mut("meta").unwrap())?.remove("history");
            authority = Some(map(history)?["authority"].clone());
        }
        let cap = crate::reasoning_capabilities::document_capabilities(&plain)?;
        let cap = map(&cap)?;
        Ok(Self {
            schema: V::Map(
                map(document)?
                    .iter()
                    .filter(|(k, _)| *k == "schema")
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
            capability: obj([
                ("profile", cap["profile"].clone()),
                ("requires", cap["requires"].clone()),
            ]),
            authority,
            roles: crate::reasoning_fields::semantic_roles(document)?,
        })
    }
    fn identity(&self, id: &str) -> Result<String> {
        let roles = match &self.roles {
            Some((judgments, fields)) => obj([
                ("judgment", V::Bool(judgments.contains(id))),
                (
                    "fields",
                    if judgments.contains(id) {
                        V::Map(fields.clone())
                    } else {
                        empty()
                    },
                ),
            ]),
            None => obj([("unreadable", V::Bool(true))]),
        };
        let mut value = obj([
            ("schema", self.schema.clone()),
            ("roles", roles),
            ("reasoning", self.capability.clone()),
        ]);
        if let Some(authority) = &self.authority {
            map_mut(&mut value)?.insert("history_authority".into(), authority.clone());
        }
        value.digest()
    }
}
fn layered(base: &V, hypothesis: &V) -> Result<V> {
    let mut out = base.clone();
    for (collection, members) in crate::reasoning_fields::collections(hypothesis)? {
        for (name, value) in map_mut(&mut out)?.iter_mut() {
            if *name != collection
                && let V::Map(m) = value
            {
                for id in members.keys() {
                    m.remove(id);
                }
            }
        }
        let current = map_mut(&mut out)?.entry(collection).or_insert_with(empty);
        if let V::Map(m) = current {
            m.extend(members);
        } else {
            *current = V::Map(members);
        }
    }
    Ok(out)
}
type Holders = BTreeMap<String, Vec<(String, String, V)>>;
fn observe(
    holders: &mut Holders,
    meanings: &mut BTreeMap<String, BTreeSet<String>>,
    label: &str,
    document: &V,
    comparison: &V,
    history: Option<&V>,
) -> Result<()> {
    let mut context = None;
    for (id, (collection, body)) in entries(document)? {
        // Retain the first holder even if its comparison context is unreadable.
        holders
            .entry(id.clone())
            .or_default()
            .push((label.into(), collection, body));
        if context.is_none() {
            context = Some(Meaning::new(comparison, history)?);
        }
        meanings
            .entry(id.clone())
            .or_default()
            .insert(context.as_ref().unwrap().identity(&id)?);
    }
    Ok(())
}
pub(crate) fn apply(
    document: &mut Document,
    observation: &Observation,
    root: &std::path::Path,
    record: &str,
    runtime: Option<&crate::reasoning_runtime::Runtime>,
) -> Result<Overlay> {
    let base = document.source.typed();
    let mut holders = Holders::new();
    let mut meanings = BTreeMap::new();
    observe(
        &mut holders,
        &mut meanings,
        "checkout",
        &base,
        &base,
        document.history_projection.as_ref(),
    )?;
    for (name, hypothesis) in map(&document.hypotheses)? {
        let hypothesis = map(hypothesis)?;
        if hypothesis.get("error").is_some_and(truth) {
            continue;
        }
        let body = hypothesis
            .get("doc")
            .or_else(|| hypothesis.get("document"))
            .ok_or_else(|| error("invalid_snapshot"))?;
        observe(
            &mut holders,
            &mut meanings,
            &format!("hypothesis:{name}"),
            body,
            &layered(&base, body)?,
            document.history_projection.as_ref(),
        )?;
    }
    let mut unavailable = None;
    let mut target_snapshot = None;
    if let Some(target) = &observation.target {
        if map(target)?.get("revision").is_some_and(truth) {
            let captured = (|| -> Result<()> {
                let target_doc = crate::source_target::records(
                    root,
                    record,
                    text(&map(target)?["revision"])?,
                    runtime,
                )?;
                let target_map = map(&target_doc)?;
                crate::reasoning_fields::capabilities(&target_map["doc"], None)?;
                for hyp in crate::history_view::list(&target_map["hypotheses"])? {
                    crate::reasoning_fields::capabilities(&map(hyp)?["doc"], None)?;
                }
                target_snapshot = Some(target_doc.clone());
                let history = target_map
                    .get("history")
                    .and_then(|h| map(h).ok())
                    .and_then(|m| m.get("projection"));
                let label = format!("target:{}", text(&map(target)?["ref"])?);
                observe(
                    &mut holders,
                    &mut meanings,
                    &label,
                    &target_map["doc"],
                    &target_map["doc"],
                    history,
                )?;
                for hyp in crate::history_view::list(&target_map["hypotheses"])? {
                    let hyp = map(hyp)?;
                    let mut layered = target_map["doc"].clone();
                    for (collection, members) in crate::reasoning_fields::collections(&hyp["doc"])?
                    {
                        let current = map_mut(&mut layered)?
                            .entry(collection)
                            .or_insert_with(empty);
                        map_mut(current)?.extend(members);
                    }
                    if let Some(schema) = map(&hyp["doc"])?.get("schema") {
                        map_mut(&mut layered)?.insert("schema".into(), schema.clone());
                    }
                    if let Some(reasoning) = map(&hyp["doc"])?
                        .get("meta")
                        .and_then(|v| map(v).ok())
                        .and_then(|m| m.get("reasoning"))
                    {
                        let meta = map_mut(&mut layered)?
                            .entry("meta".into())
                            .or_insert_with(empty);
                        map_mut(meta)?.insert("reasoning".into(), reasoning.clone());
                    }
                    observe(
                        &mut holders,
                        &mut meanings,
                        &format!("{label}:hypothesis:{}", text(&hyp["name"])?),
                        &layered,
                        &layered,
                        history,
                    )?;
                }
                Ok(())
            })();
            if let Err(e) = captured {
                unavailable = Some(e.0);
            }
        } else {
            unavailable = Some("configured target has no locally available observation".into());
        }
    }
    let cache = map(&observation.publication)?;
    let observed = cache
        .get("last_verified")
        .filter(|v| truth(v))
        .cloned()
        .unwrap_or(V::Null);
    let mut active = BTreeSet::new();
    let mut done = BTreeSet::new();
    let mut contributions = vec![];
    let mut history_contributions = Map::new();
    for event in &observation.ledger.events {
        let revision = text(&map(event)?["revision"])?;
        if !done.insert(revision.to_owned()) {
            continue;
        }
        let bundle = &observation.ledger.bundles[revision];
        let manifest = map(&map(&bundle.value)?["manifest"])?;
        let mut body = manifest["document"].clone();
        let mut history = None;
        if is_int(&manifest["version"], "3") {
            let adapted =
                crate::history_bundle::validate_contribution(&bundle.value, &bundle.files)?;
            body = adapted.document().clone();
            history = Some(adapted.projection().clone());
            history_contributions.insert(
                revision.into(),
                obj([
                    (
                        "artifact_revision",
                        map(&manifest["history"])?["revision"].clone(),
                    ),
                    ("projection", history.clone().unwrap()),
                    ("scope", manifest["scope"].clone()),
                    ("roots", manifest["roots"].clone()),
                    ("status", s("active")),
                ]),
            );
        }
        let state = map(&cache["states"])?
            .get(revision)
            .cloned()
            .unwrap_or_else(|| s("captured"));
        let state_name = text(&state)?;
        let status_name = if state_name == "captured" {
            "captured locally".into()
        } else if ["accepted", "proposed", "closed"].contains(&state_name) {
            format!("{state_name} (last observed)")
        } else {
            state_name.into()
        };
        let status = obj([
            ("revision", s(revision)),
            ("state", s(&status_name)),
            ("scope", manifest["scope"].clone()),
            ("roots", manifest["roots"].clone()),
            (
                "events",
                V::List(
                    observation
                        .ledger
                        .events
                        .iter()
                        .filter(|e| map(e).unwrap()["revision"] == s(revision))
                        .cloned()
                        .collect(),
                ),
            ),
            (
                "ledger_ref",
                observation
                    .ledger
                    .head
                    .as_ref()
                    .map(|h| s(h))
                    .unwrap_or(V::Null),
            ),
            ("publication_state", state),
            ("verified", V::Bool(false)),
            ("last_verified", observed.clone()),
            ("pr", cache["pr"].clone()),
        ]);
        contributions.push(status.clone());
        let retired = map(&cache["decisions"])?
            .get(revision)
            .and_then(|v| map(v).ok())
            .and_then(|m| m.get("state"))
            .is_some_and(|v| {
                ["withdrawn", "rejected", "superseded"]
                    .iter()
                    .any(|name| string_is(v, name))
            });
        if retired {
            if let Some(h) = history_contributions.get_mut(revision) {
                map_mut(h)?.insert("status".into(), s("retired"));
            }
            continue;
        }
        crate::pending_bundle::validate(&bundle.value, &bundle.files)?;
        active.extend(entries(&body)?.into_keys());
        let name = format!("pending-{revision}");
        let hyp = obj([
            ("kind", s("contribution")),
            ("name", s(&name)),
            (
                "path",
                s(&format!(
                    "git:{}:{revision}",
                    observation.ledger.head.as_ref().unwrap()
                )),
            ),
            (
                "head",
                obj([
                    (
                        "claim",
                        s(&format!(
                            "Project contribution: {status_name}; current remote acceptance is unverified"
                        )),
                    ),
                    ("folds", s("never")),
                    ("scope", manifest["scope"].clone()),
                    ("publication", status),
                ]),
            ),
            ("doc", body.clone()),
            ("error", V::Null),
        ]);
        map_mut(&mut document.hypotheses)?.insert(name.clone(), hyp);
        observe(
            &mut holders,
            &mut meanings,
            &name,
            &body,
            &body,
            history.as_ref(),
        )?;
    }
    let mut conflicts = Map::new();
    for id in active {
        let variants = holders.get(&id).map(Vec::as_slice).unwrap_or(&[]);
        let bodies = variants
            .iter()
            .map(|(_, collection, body)| V::List(vec![s(collection), body.clone()]).digest())
            .collect::<Result<BTreeSet<_>>>()?;
        if bodies.len() > 1 || meanings.get(&id).is_some_and(|m| m.len() > 1) {
            conflicts.insert(
                id,
                V::List(
                    variants
                        .iter()
                        .map(|(name, _, body)| V::List(vec![s(name), body.clone()]))
                        .collect(),
                ),
            );
        }
    }
    Ok(Overlay {
        pending: observation.ledger.portable(),
        publication: observation.publication.clone(),
        contributions,
        conflicts: V::Map(conflicts),
        target: observation.target.clone(),
        target_snapshot,
        unavailable,
        history_contributions: V::Map(history_contributions),
    })
}
