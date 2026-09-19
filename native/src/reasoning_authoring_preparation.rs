//! Writer profile selection over a frozen source observation and effective overlay.
use crate::{
    Result,
    history_authority::Files,
    history_contract::*,
    history_view::truth,
    pending_bundle,
    reasoning_authoring::{self as A, World},
    reasoning_fields as F, reasoning_language as L,
    reasoning_runtime::{OperationalBounds, Runtime},
    reasoning_snapshot::{Snapshot, entries},
    require,
    value::TypedValue as V,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::LazyLock,
};
static EXPR: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"[<>=!+\-*/()]|\b(?:or|and|not)\b").unwrap());
static ID: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+").unwrap());
fn core(document: &V) -> Result<bool> {
    Ok(string_is(
        &map(&F::capabilities(document, None)?)?["profile"],
        "core/v1",
    ))
}
/// The returned triples retain the exact entry, field and authored blocker value.
pub fn promotion_blockers(document: &V, context: Option<&V>) -> Result<Vec<(String, String, V)>> {
    let known = entries(document)?;
    let mut ids = known.keys().cloned().collect::<BTreeSet<_>>();
    if let Some(context) = context {
        ids.extend(entries(context)?.into_keys());
    }
    let fields = F::snapshot_fields(document)?;
    let mut blockers = vec![];
    for (id, (_, body)) in &known {
        if let V::Map(body) = body {
            for field in ["rule", text(&fields["predicate"])?] {
                if let Some(value) = body.get(field) {
                    let executable = match value {
                        V::Map(_) => true,
                        V::Text(v) => L::legacy_expression(
                            &crate::history_authoring::obj([(
                                "expr",
                                crate::history_authoring::s(v),
                            )]),
                            false,
                        )
                        .is_ok(),
                        _ => false,
                    };
                    if executable {
                        blockers.push((id.clone(), field.into(), value.clone()));
                    }
                }
            }
        }
    }
    for (id, (_, body)) in known {
        let values = if let V::Map(body) = body {
            ["v", "quoted"]
                .iter()
                .filter_map(|k| body.get(*k).map(|v| ((*k).into(), v.clone())))
                .collect::<Vec<_>>()
        } else {
            vec![("value".into(), body)]
        };
        for (field, value) in values {
            let blocked = match &value {
                V::Map(_) | V::List(_) => L::literal(&value).is_err(),
                V::Date(_) | V::DateTime(_) => true,
                V::Text(v) => EXPR.is_match(v) && ID.find_iter(v).any(|m| ids.contains(m.as_str())),
                _ => false,
            };
            if blocked {
                blockers.push((id.clone(), field, value));
            }
        }
    }
    Ok(blockers)
}
/// This overlay is captured under the project policy lock by the caller. Passing
/// None means the source is outside the advanced-mode canonical record route.
pub struct PendingOverlay<'a> {
    pub bundles: &'a Map,
    pub files: &'a BTreeMap<String, Files>,
    pub decisions: &'a V,
    pub resume: &'a [String],
}
pub fn prepare<'a>(
    original: &Snapshot,
    action: &V,
    pending: Option<&PendingOverlay<'_>>,
    runtime: Option<&'a Runtime>,
) -> Result<(V, Option<World<'a>>)> {
    let source = map(original.data())?;
    let document = &source["document"];
    let hypotheses = map(&source["hypotheses"])?;
    let action = map(action)?;
    let requested = action
        .get("profile")
        .filter(|v| **v != V::Null)
        .map(text)
        .transpose()?;
    let mut active = A::selected(document, requested)?;
    A::validate_declared(document)?;
    if let Some(name) = action.get("hypothesis").and_then(|v| text(v).ok())
        && let Some(hyp) = hypotheses.get(name)
        && core(&map(hyp)?["document"])?
    {
        active = true;
    }
    if !active {
        for hyp in hypotheses.values() {
            require(
                !core(&map(hyp)?["document"])?,
                "unsupported_capability: use core/v1 consumer",
            )?;
        }
        return Ok((document.clone(), None));
    }
    require(
        map(document)?
            .get("meta")
            .is_none_or(|v| matches!(v, V::Map(_))),
        "metadata_requires_explicit_migration",
    )?;
    if !core(document)? {
        require(
            !hypotheses
                .values()
                .any(|h| map(h).is_ok_and(|m| m.get("error").is_some_and(truth))),
            "unreadable_hypothesis_requires_migration",
        )?;
        require(
            promotion_blockers(document, Some(document))?.is_empty(),
            "legacy_executables_require_migration",
        )?;
        for hyp in hypotheses.values() {
            let doc = &map(hyp)?["document"];
            if !core(doc)? {
                require(
                    promotion_blockers(doc, Some(document))?.is_empty(),
                    "legacy_executables_require_migration",
                )?;
            }
        }
        let destination = A::declare_document(document)?;
        if let Some(pending) = pending {
            pending_bundle::compatible(
                &destination,
                pending.bundles,
                pending.files,
                pending.decisions,
                pending.resume,
            )?;
        }
    }
    for hyp in hypotheses.values() {
        let doc = &map(hyp)?["document"];
        if !core(doc)? {
            require(
                promotion_blockers(doc, Some(document))?.is_empty(),
                "legacy_hypothesis_requires_migration",
            )?;
        }
    }
    let document = A::declare_document(document)?;
    let world = World::new(
        &document,
        Some(original),
        runtime,
        OperationalBounds::default(),
    )?;
    Ok((document, Some(world)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profile_preparation_matches_python_frozen_snapshots() {
        let corpus: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/reasoning-promotion.json"))
                .unwrap();
        let mut failures = vec![];
        for case in corpus["cases"].as_array().unwrap() {
            let original = if let Some(raw) = case.get("original") {
                Snapshot::from_json(raw.as_str().unwrap().as_bytes())
            } else {
                Snapshot::from_data(
                    &V::from_tagged(&case["document"]).unwrap(),
                    crate::reasoning_snapshot::CaptureOptions {
                        hypotheses: Some(V::from_tagged(&case["hypotheses"]).unwrap()),
                        ..Default::default()
                    },
                )
            };
            let result = original.and_then(|snapshot| {
                prepare(
                    &snapshot,
                    &V::from_tagged(&case["action"]).unwrap(),
                    None,
                    None,
                )
            });
            match (case.get("output"), result) {
                (Some(expected), Ok((document, world))) => {
                    if document != V::from_tagged(expected).unwrap()
                        || world.is_some() != case["world"].as_bool().unwrap()
                    {
                        failures.push(format!("{}: document/world differs", case["name"]));
                    }
                    if let Some(world) = world
                        && world.snapshot().to_json().unwrap() != case["snapshot"].as_str().unwrap()
                    {
                        failures.push(format!("{}: snapshot differs", case["name"]));
                    }
                }
                (None, Err(_)) => {}
                (Some(_), Err(e)) => failures.push(format!("{}: refused {e}", case["name"])),
                (None, Ok(_)) => failures.push(format!("{}: accepted", case["name"])),
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn profile_promotion_reconciles_the_supplied_effective_overlay() {
        use crate::history_authoring::{empty, n, obj, s};
        let scope = obj([("kind", s("project")), ("environment", s("test"))]);
        let document = obj([(
            "known",
            obj([("p.x", obj([("v", n("1")), ("scope", scope.clone())]))]),
        )]);
        let bundle =
            pending_bundle::prepare(&document, &["p.x".into()], &scope, "project", &Files::new())
                .unwrap();
        let revision = text(&map(&bundle).unwrap()["revision"]).unwrap().to_owned();
        let bundles = Map::from([(revision.clone(), bundle)]);
        let files = BTreeMap::new();
        let snapshot = Snapshot::from_data(&document, Default::default()).unwrap();
        let before = snapshot.to_json().unwrap();
        let action = obj([
            ("profile", s("core/v1")),
            ("kind", s("add")),
            ("id", s("p.next")),
            ("body", obj([("v", n("2"))])),
        ]);
        for state in ["pending", "accepted", "withdrawn"] {
            let decisions = V::Map(Map::from([(revision.clone(), obj([("state", s(state))]))]));
            let overlay = PendingOverlay {
                bundles: &bundles,
                files: &files,
                decisions: &decisions,
                resume: &[],
            };
            let result = prepare(&snapshot, &action, Some(&overlay), None);
            if state == "withdrawn" {
                assert!(result.unwrap().1.is_some());
            } else {
                assert!(
                    result
                        .err()
                        .unwrap()
                        .0
                        .starts_with("pending_profile_reconciliation_required")
                );
            }
        }
        assert_eq!(snapshot.to_json().unwrap(), before);
        let overlay = PendingOverlay {
            bundles: &Map::new(),
            files: &files,
            decisions: &empty(),
            resume: &[],
        };
        let (promoted, world) = prepare(&snapshot, &action, Some(&overlay), None).unwrap();
        assert!(core(&promoted).unwrap());
        assert!(world.is_some());
    }
}
