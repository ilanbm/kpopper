//! Live contributions remain explicit alternatives with scope and disposition.
//! Neither sequence nor cached acceptance chooses an effective winner.
use crate::{
    Result,
    history_contract::{self as C, error},
    ordinary_document::Document,
    ordinary_value::{Map, Value as V, is_int, map, map_mut, string_is, text, truth},
    pending_state::Observation,
    require,
    source_overlay::{Comparison, Overlay},
    value::TypedValue as CV,
};
use std::collections::{BTreeMap, BTreeSet};
/// How an ordinary reader leaves a target kept by its history unverified.
const HISTORY_CONSUMER: &str =
    "unsupported_history_consumer: history target needs captured consumer";
fn s(v: &str) -> V {
    V::Text(v.into())
}
fn empty() -> V {
    V::Map(Map::new())
}
fn obj(items: impl IntoIterator<Item = (&'static str, V)>) -> V {
    V::Map(items.into_iter().map(|(k, v)| (k.into(), v)).collect())
}

// The pending ledger has its own canonical identity boundary. Ordinary reading
// does not widen values that may be shared or compared as contributions.
fn shared_identity(value: &V) -> Result<String> {
    value
        .try_typed()
        .map_err(|error| match error.0.as_str() {
            "nonfinite_value_not_canonical" => {
                crate::Error("nonfinite values cannot be shared".into())
            }
            "invalid_yaml_key" => crate::Error("record mappings need text keys".into()),
            _ => error,
        })?
        .digest()
}

struct Meaning {
    schema: V,
    capability: V,
    authority: Option<V>,
    roles: Option<(BTreeSet<String>, Map)>,
}
impl Meaning {
    fn new(document: &V, history: Option<&CV>) -> Result<Self> {
        let mut plain = document.clone();
        let mut authority = None;
        if map(document)?
            .get("meta")
            .and_then(|v| map(v).ok())
            .is_some_and(|m| m.contains_key("history"))
        {
            let history = history.ok_or_else(|| error("missing_history_context"))?;
            crate::history_projection::CapturedHistory::new(
                document.try_typed()?,
                history.clone(),
            )?;
            map_mut(map_mut(&mut plain)?.get_mut("meta").unwrap())?.remove("history");
            authority = Some(V::from_typed(&C::map(history)?["authority"]));
        }
        let cap = crate::ordinary_fields::capabilities(&plain, None)?;
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
            roles: crate::ordinary_fields::semantic_roles(document)?,
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
        shared_identity(&value)
    }
}
fn layered(base: &V, hypothesis: &V) -> Result<V> {
    let mut out = base.clone();
    for (collection, members) in crate::ordinary_fields::collections(hypothesis)? {
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
/// Who holds each id, and with which comparison meanings, in one group of readings.
#[derive(Default)]
struct Held {
    holders: BTreeMap<String, Vec<(String, String, V)>>,
    meanings: BTreeMap<String, BTreeSet<String>>,
}
impl Held {
    fn observe(
        &mut self,
        label: &str,
        document: &V,
        comparison: &V,
        history: Option<&CV>,
    ) -> Result<()> {
        let mut context = None;
        for (id, (collection, body)) in entries(document)? {
            // Retain the first holder even if its comparison context is unreadable.
            self.holders
                .entry(id.clone())
                .or_default()
                .push((label.into(), collection, body));
            if context.is_none() {
                context = Some(Meaning::new(comparison, history)?);
            }
            self.meanings
                .entry(id.clone())
                .or_default()
                .insert(context.as_ref().unwrap().identity(&id)?);
        }
        Ok(())
    }
}
/// The active ids that the groups hold in more than one body or meaning, each with every
/// holder in the order the groups were read.
fn conflicts(active: &BTreeSet<String>, groups: &[&Held]) -> Result<CV> {
    let mut conflicts = Map::new();
    for id in active {
        let variants = groups
            .iter()
            .flat_map(|group| group.holders.get(id).into_iter().flatten())
            .collect::<Vec<_>>();
        let bodies = variants
            .iter()
            .map(|(_, collection, body)| {
                shared_identity(&V::List(vec![s(collection), body.clone()]))
            })
            .collect::<Result<BTreeSet<_>>>()?;
        let meanings = groups
            .iter()
            .flat_map(|group| group.meanings.get(id).into_iter().flatten())
            .collect::<BTreeSet<_>>();
        if bodies.len() > 1 || meanings.len() > 1 {
            conflicts.insert(
                id.clone(),
                V::List(
                    variants
                        .iter()
                        .map(|(name, _, body)| V::List(vec![s(name), body.clone()]))
                        .collect(),
                ),
            );
        }
    }
    V::Map(conflicts).try_typed()
}
/// The core/v1 consumer's boundary over the target's documents: a declaration the contract
/// refuses stops it.
fn core_reads(documents: &[&V]) -> Result<()> {
    for document in documents {
        crate::ordinary_fields::capabilities(document, None)?;
    }
    Ok(())
}
/// The ordinary reader's boundary over the same documents, in order: a declaration the
/// contract refuses, said in the contract's words, or a core/v1 declaration stops it.
fn ordinary_reads(documents: &[&V]) -> Result<()> {
    for document in documents {
        let capabilities = crate::ordinary_fields::explained_capabilities(document, None)?;
        require(
            !string_is(&map(&capabilities)?["profile"], "core/v1"),
            crate::source_capture::CORE_CONSUMER,
        )?;
    }
    Ok(())
}
fn reason(error: &crate::Error) -> String {
    error
        .0
        .strip_prefix("invalid_history_value: ")
        .unwrap_or(&error.0)
        .to_owned()
}
pub(crate) fn apply(
    document: &mut Document,
    observation: &Observation,
    root: &std::path::Path,
    record: &str,
    runtime: Option<&crate::reasoning_runtime::Runtime>,
) -> Result<Overlay<V>> {
    let base = document.source.projected();
    // The checkout and its hypotheses, the configured target, and the pending ledger, read
    // in that order: an ordinary reader leaves out the target's group when it may not read it.
    let mut local = Held::default();
    let mut target_held = Held::default();
    let mut pending = Held::default();
    local.observe(
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
        local.observe(
            &format!("hypothesis:{name}"),
            body,
            &layered(&base, body)?,
            document.history_projection.as_ref(),
        )?;
    }
    let mut unavailable = None;
    // Why an ordinary reader leaves the target unverified where a core/v1 consumer compares
    // it: the target is kept by its history, or declares core/v1 in its record or in one of
    // its hypotheses. None when both readers see the same comparison.
    let mut ordinary_refusal = None;
    let mut target_snapshot = None;
    let target_observation = observation.target.as_ref().map(V::from_typed);
    if let Some(target) = &target_observation {
        if map(target)?.get("revision").is_some_and(truth) {
            let revision = text(&map(target)?["revision"])?;
            let captured = (|| -> Result<()> {
                let target_doc =
                    match crate::source_target::records(root, record, revision, runtime) {
                        Ok(target_doc) => V::from_typed(&target_doc),
                        Err(e) => {
                            // An ordinary reader stops at a history target's authority marker,
                            // before anything later in the reading could fail, and reads the
                            // other targets' declarations without computing them, so a target
                            // it may not read is never replayed for it.
                            ordinary_refusal =
                                if crate::source_target::kept_by_history(root, record, revision)
                                    .unwrap_or(false)
                                {
                                    Some(error(HISTORY_CONSUMER))
                                } else {
                                    crate::source_target::records_ordinary(
                                        root, record, revision, runtime,
                                    )
                                    .ok()
                                    .and_then(|declared| {
                                        let mut documents = vec![&declared.document];
                                        documents.extend(declared.hypotheses.values().filter_map(
                                            |hypothesis| map(hypothesis).ok()?.get("doc"),
                                        ));
                                        ordinary_reads(&documents).err()
                                    })
                                };
                            return Err(e);
                        }
                    };
                let target_map = map(&target_doc)?;
                let mut documents = vec![&target_map["doc"]];
                for hyp in crate::ordinary_value::list(&target_map["hypotheses"])? {
                    documents.push(&map(hyp)?["doc"]);
                }
                ordinary_refusal = if target_map.contains_key("history") {
                    Some(error(HISTORY_CONSUMER))
                } else {
                    ordinary_reads(&documents).err()
                };
                core_reads(&documents)?;
                target_snapshot = Some(target_doc.clone());
                let history = target_map
                    .get("history")
                    .and_then(|h| map(h).ok())
                    .and_then(|m| m.get("projection"))
                    .map(|v| v.try_typed())
                    .transpose()?;
                let label = format!("target:{}", text(&map(target)?["ref"])?);
                target_held.observe(
                    &label,
                    &target_map["doc"],
                    &target_map["doc"],
                    history.as_ref(),
                )?;
                for hyp in crate::ordinary_value::list(&target_map["hypotheses"])? {
                    let hyp = map(hyp)?;
                    let mut layered = target_map["doc"].clone();
                    for (collection, members) in crate::ordinary_fields::collections(&hyp["doc"])? {
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
                    target_held.observe(
                        &format!("{label}:hypothesis:{}", text(&hyp["name"])?),
                        &layered,
                        &layered,
                        history.as_ref(),
                    )?;
                }
                Ok(())
            })();
            if let Err(e) = captured {
                unavailable = Some(reason(&e));
            }
        } else {
            unavailable = Some("configured target has no locally available observation".into());
        }
    }
    let publication = V::from_typed(&observation.publication);
    let cache = map(&publication)?;
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
        let revision = C::text(&C::map(event)?["revision"])?;
        if !done.insert(revision.to_owned()) {
            continue;
        }
        let bundle = &observation.ledger.bundles[revision];
        let bundle_value = V::from_typed(&bundle.value);
        let manifest = map(&map(&bundle_value)?["manifest"])?;
        let mut body = manifest["document"].clone();
        let mut history = None;
        if is_int(&manifest["version"], "3") {
            let adapted =
                crate::history_bundle::validate_contribution(&bundle.value, &bundle.files)?;
            body = V::from_typed(adapted.document());
            history = Some(adapted.projection().clone());
            history_contributions.insert(
                revision.into(),
                obj([
                    (
                        "artifact_revision",
                        map(&manifest["history"])?["revision"].clone(),
                    ),
                    ("projection", V::from_typed(history.as_ref().unwrap())),
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
                        .filter(|e| C::string_is(&C::map(e).unwrap()["revision"], revision))
                        .map(V::from_typed)
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
        pending.observe(&name, &body, &body, history.as_ref())?;
    }
    let core = Comparison {
        conflicts: conflicts(&active, &[&local, &target_held, &pending])?,
        unavailable,
    };
    let ordinary = match ordinary_refusal {
        Some(refusal) => Comparison {
            conflicts: conflicts(&active, &[&local, &pending])?,
            unavailable: Some(reason(&refusal)),
        },
        None => core.clone(),
    };
    Ok(Overlay {
        pending: observation.ledger.portable(),
        publication: observation.publication.clone(),
        contributions: contributions
            .iter()
            .map(|v| v.try_typed())
            .collect::<Result<_>>()?,
        core,
        ordinary,
        target: observation.target.clone(),
        target_snapshot,
        history_contributions: V::Map(history_contributions).try_typed()?,
    })
}

fn entries(document: &V) -> Result<BTreeMap<String, (String, V)>> {
    Ok(crate::ordinary_fields::collections(document)?
        .into_iter()
        .flat_map(|(section, members)| {
            members
                .into_iter()
                .map(move |(id, body)| (id, (section.clone(), body)))
        })
        .collect())
}
