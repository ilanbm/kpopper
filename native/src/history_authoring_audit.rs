//! Historical adapter fingerprints are admissible only with causal-parent evidence.
//! Audit restoration is private to semantic replay; it never supplies computations.
use crate::{
    Result, history_authority,
    history_contract::*,
    history_transaction as T,
    history_view::map_mut,
    history_yaml,
    reasoning_authoring::World,
    reasoning_snapshot::digest,
    require,
    value::{Integer, TypedValue as V},
};
use std::collections::BTreeSet;

fn hash_without(value: &V, field: &str) -> Result<String> {
    digest(&V::Map(
        map(value)?
            .iter()
            .filter(|(k, _)| *k != field)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    ))
}
fn computation(result: &mut V, visit: &mut impl FnMut(&mut Map) -> Result<()>) -> Result<()> {
    if *result != V::Null {
        visit(map_mut(result)?)?;
    }
    Ok(())
}
fn computations(report: &mut V, mut visit: impl FnMut(&mut Map) -> Result<()>) -> Result<()> {
    if let Some(nodes) = map_mut(report)?.get_mut("nodes") {
        for node in map_mut(nodes)?.values_mut() {
            let node = map_mut(node)?;
            if let Some(result) = node.get_mut("computation") {
                computation(result, &mut visit)?;
            }
            if let Some(state) = node.get_mut("state") {
                let state = map_mut(state)?;
                if let Some(falsifier) = state.get_mut("falsifier")
                    && let Some(result) = map_mut(falsifier)?.get_mut("computation")
                {
                    computation(result, &mut visit)?;
                }
                if let Some(basis) = state.get_mut("basis")
                    && let Some(deps) = map_mut(basis)?.get_mut("dependencies")
                {
                    for dep in map_mut(deps)?.values_mut() {
                        if let Some(result) = map_mut(dep)?.get_mut("computation") {
                            computation(result, &mut visit)?;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn recorded(receipt: &V) -> Result<Map> {
    let mut audits = Map::new();
    let mut pending = vec![receipt];
    while let Some(current) = pending.pop() {
        T::validate_receipt(current)?;
        let current = map(current)?;
        for role in ["before", "after"] {
            let evidence = map(&current[role])?;
            let proposal = evidence.get("proposal").map(map).transpose()?;
            for report in [
                evidence.get("assessment"),
                proposal.and_then(|p| p.get("assessment")),
            ]
            .into_iter()
            .flatten()
            {
                if *report == V::Null {
                    continue;
                }
                let valid = map(report).ok().is_some_and(|r| {
                    r.get("schema_version").is_some_and(|v| {
                        crate::reasoning_authoring::same(v, &V::Integer(Integer::new("2").unwrap()))
                            .unwrap_or(false)
                    }) && r.get("assessment_revision").is_some_and(|v| {
                        hash_without(report, "assessment_revision").is_ok_and(|h| string_is(v, &h))
                    })
                });
                require(valid, "invalid_retained_assessment")?;
                computations(&mut report.clone(), |result| {
                    let Some(implementation) =
                        result.get("implementation").filter(|v| **v != V::Null)
                    else {
                        return Ok(());
                    };
                    let valid = map(implementation).ok().is_some_and(|m| {
                        m.get("adapter_source_sha256")
                            .and_then(|v| text(v).ok())
                            .is_some_and(|s| s.len() == 64 && crate::history_paths::object_id(s))
                            && result
                                .get("assurance")
                                .and_then(|v| map(v).ok())
                                .and_then(|a| a.get("implementation"))
                                .is_some_and(|v| {
                                    digest(implementation).is_ok_and(|h| string_is(v, &h))
                                })
                    });
                    require(valid, "invalid_retained_implementation")?;
                    let key = hash_without(implementation, "adapter_source_sha256")?;
                    require(
                        audits.get(&key).is_none_or(|a| a == implementation),
                        "ambiguous_retained_adapter_audit",
                    )?;
                    audits.insert(key, implementation.clone());
                    Ok(())
                })?;
            }
        }
        if let Some(authoring) = map(&current["after"])?.get("authoring")
            && let Some(steps) = map(authoring)?.get("steps")
        {
            for step in crate::history_view::list(steps)? {
                if let Ok(step) = map(step)
                    && let Some(receipt) = step.get("receipt")
                {
                    pending.push(receipt);
                }
            }
        }
    }
    Ok(audits)
}

fn compact_map(value: &V) -> Result<Map> {
    let audits = map(value)?;
    for (key, audit) in audits {
        let implementation = map(audit)?;
        require(
            implementation
                .get("adapter_source_sha256")
                .and_then(|v| text(v).ok())
                .is_some_and(|hash| hash.len() == 64 && crate::history_paths::object_id(hash))
                && hash_without(audit, "adapter_source_sha256")? == key.as_str(),
            "invalid_retained_implementation",
        )?;
    }
    Ok(audits.clone())
}

fn witnessed(receipt: &V, parents: &history_authority::Files, actual: &str) -> Result<Map> {
    let mut witnesses = BTreeSet::new();
    for raw in parents.values() {
        let parent = history_yaml::decode_document(raw)?;
        history_authority::validate_commit(&parent)?;
        let prior = &map(&parent)?["receipt"];
        if !["before", "after", "capabilities", "profile", "digest"]
            .iter()
            .all(|k| map(prior).is_ok_and(|p| p.contains_key(*k)))
        {
            continue;
        }
        for audit in recorded(prior)?.values() {
            witnesses.insert(digest(audit)?);
        }
    }
    let audits = recorded(receipt)?;
    for audit in audits.values() {
        require(
            string_is(&map(audit)?["adapter_source_sha256"], actual)
                || witnesses.contains(&digest(audit)?),
            "unknown_retained_adapter_audit",
        )?;
    }
    Ok(audits)
}

fn retain(report: &V, recorded: Option<&Map>) -> Result<V> {
    let Some(recorded) = recorded else {
        return Ok(report.clone());
    };
    let mut report = report.clone();
    computations(&mut report, |result| {
        let Some(implementation) = result.get("implementation").filter(|v| **v != V::Null) else {
            return Ok(());
        };
        let key = hash_without(implementation, "adapter_source_sha256")?;
        let old = recorded
            .get(&key)
            .ok_or_else(|| error("authoring_native_audit_mismatch"))?;
        let hash = digest(old)?;
        result.insert("implementation".into(), old.clone());
        map_mut(
            result
                .get_mut("assurance")
                .ok_or_else(|| error("invalid_retained_implementation"))?,
        )?
        .insert("implementation".into(), V::Text(hash));
        Ok(())
    })?;
    let hash = hash_without(&report, "assessment_revision")?;
    map_mut(&mut report)?.insert("assessment_revision".into(), V::Text(hash));
    Ok(report)
}

pub(crate) struct ReplayAudit {
    recorded: Map,
    #[cfg(test)]
    oracle: bool,
}
impl ReplayAudit {
    pub(crate) fn validate_compact(value: &V) -> Result<()> {
        compact_map(value).map(|_| ())
    }
    /// Persist only validated adapter implementations, never assessment reports.
    pub(crate) fn compact(receipt: &V) -> Result<V> {
        Ok(V::Map(recorded(receipt)?))
    }

    /// A compact parent map witnesses an old adapter only by its exact digest.
    pub(crate) fn from_compact(
        value: &V,
        parents: impl FnOnce() -> Result<Vec<V>>,
    ) -> Result<Self> {
        let audits = compact_map(value)?;
        let current = env!("KPOP_REASONING_ADAPTER_SHA256");
        if audits
            .values()
            .all(|audit| string_is(&map(audit).unwrap()["adapter_source_sha256"], current))
        {
            return Ok(Self {
                recorded: audits,
                #[cfg(test)]
                oracle: false,
            });
        }
        let mut witnesses = BTreeSet::new();
        for parent in parents()? {
            for audit in compact_map(&parent)?.values() {
                witnesses.insert(digest(audit)?);
            }
        }
        for audit in audits.values() {
            require(
                string_is(&map(audit)?["adapter_source_sha256"], current)
                    || witnesses.contains(&digest(audit)?),
                "unknown_retained_adapter_audit",
            )?;
        }
        Ok(Self {
            recorded: audits,
            #[cfg(test)]
            oracle: false,
        })
    }
    /// Only receipts from verified causal ancestors may witness a retained adapter.
    pub(crate) fn from_receipts(receipt: &V, parents: &[V]) -> Result<Self> {
        let mut witnesses = BTreeSet::new();
        for parent in parents {
            crate::history_transaction::validate_receipt(parent)?;
            for audit in recorded(parent)?.values() {
                witnesses.insert(digest(audit)?);
            }
        }
        let audits = recorded(receipt)?;
        for audit in audits.values() {
            require(
                string_is(
                    &map(audit)?["adapter_source_sha256"],
                    env!("KPOP_REASONING_ADAPTER_SHA256"),
                ) || witnesses.contains(&digest(audit)?),
                "unknown_retained_adapter_audit",
            )?;
        }
        Ok(Self {
            recorded: audits,
            #[cfg(test)]
            oracle: false,
        })
    }
    /// Load causal-parent witnesses only when the receipt retains a foreign adapter audit.
    /// The receipt is fully validated before the callback can perform expensive reconstruction.
    pub(crate) fn from_receipts_lazy(
        receipt: &V,
        parents: impl FnOnce() -> Result<Vec<V>>,
    ) -> Result<Self> {
        let audits = recorded(receipt)?;
        let current = env!("KPOP_REASONING_ADAPTER_SHA256");
        let mut needs_witness = false;
        for audit in audits.values() {
            needs_witness |= !string_is(&map(audit)?["adapter_source_sha256"], current);
        }
        if !needs_witness {
            return Ok(Self {
                recorded: audits,
                #[cfg(test)]
                oracle: false,
            });
        }

        let mut witnesses = BTreeSet::new();
        for parent in parents()? {
            crate::history_transaction::validate_receipt(&parent)?;
            for audit in recorded(&parent)?.values() {
                witnesses.insert(digest(audit)?);
            }
        }
        for audit in audits.values() {
            require(
                string_is(&map(audit)?["adapter_source_sha256"], current)
                    || witnesses.contains(&digest(audit)?),
                "unknown_retained_adapter_audit",
            )?;
        }
        Ok(Self {
            recorded: audits,
            #[cfg(test)]
            oracle: false,
        })
    }
    #[cfg(test)]
    pub(crate) fn oracle(receipt: &V) -> Result<Self> {
        Ok(Self {
            recorded: recorded(receipt)?,
            oracle: true,
        })
    }
    pub(crate) fn from_parents(receipt: &V, parents: &history_authority::Files) -> Result<Self> {
        Ok(Self {
            recorded: witnessed(receipt, parents, env!("KPOP_REASONING_ADAPTER_SHA256"))?,
            #[cfg(test)]
            oracle: false,
        })
    }
    pub(crate) fn assessment(&self, world: &mut World<'_>) -> Result<V> {
        let report = world.assessment()?;
        #[cfg(test)]
        if self.oracle {
            let mut audits = self.recorded.clone();
            computations(&mut report.clone(), |result| {
                if let Some(actual) = result.get("implementation").filter(|v| **v != V::Null) {
                    let runtime = world
                        .runtime
                        .ok_or_else(|| error("missing_oracle_runtime"))?;
                    let reference = self
                        .recorded
                        .values()
                        .find(|reference| {
                            crate::test_runtime_provenance::verify_pair(actual, reference, runtime)
                                .is_ok()
                        })
                        .ok_or_else(|| error("oracle_platform_audit_mismatch"))?;
                    audits.insert(
                        hash_without(actual, "adapter_source_sha256")?,
                        reference.clone(),
                    );
                }
                Ok(())
            })?;
            return retain(&report, Some(&audits));
        }
        retain(&report, Some(&self.recorded))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value as J;
    #[test]
    fn compact_audits_reject_wrong_key_before_parent_loading() {
        let wrong = V::Map(Map::from([(
            "wrong".into(),
            V::Map(Map::from([(
                "adapter_source_sha256".into(),
                V::Text("0".repeat(64)),
            )])),
        )]));
        let mut loaded = false;
        assert!(
            ReplayAudit::from_compact(&wrong, || {
                loaded = true;
                Ok(Vec::new())
            })
            .is_err()
        );
        assert!(!loaded);
    }

    fn fixture_receipt(group: &str, name: &str) -> V {
        let data: J = serde_json::from_str(include_str!(
            "../tests/fixtures/history-authoring-audit.json"
        ))
        .unwrap();
        let case = data[group]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"] == name)
            .unwrap();
        let input = if group == "witnessed" {
            &case["input"]["receipt"]
        } else {
            &case["input"]
        };
        V::from_tagged(input).unwrap()
    }

    #[test]
    fn lazy_receipt_audit_skips_parent_loading_without_foreign_audits() {
        // The fixture is a Python audit; rebind its validated report to this native adapter.
        let source = fixture_receipt("witnessed", "current");
        let mut report = map(&map(&source).unwrap()["after"]).unwrap()["assessment"].clone();
        computations(&mut report, |result| {
            let Some(implementation) = result.get_mut("implementation").filter(|v| **v != V::Null)
            else {
                return Ok(());
            };
            map_mut(implementation)?.insert(
                "adapter_source_sha256".into(),
                V::Text(env!("KPOP_REASONING_ADAPTER_SHA256").into()),
            );
            let hash = digest(implementation)?;
            map_mut(
                result
                    .get_mut("assurance")
                    .ok_or_else(|| error("invalid_retained_implementation"))?,
            )?
            .insert("implementation".into(), V::Text(hash));
            Ok(())
        })
        .unwrap();
        let revision = hash_without(&report, "assessment_revision").unwrap();
        map_mut(&mut report)
            .unwrap()
            .insert("assessment_revision".into(), V::Text(revision));
        let side = V::Map(Map::from([("assessment".into(), report)]));
        let current = T::semantic_receipt("core/v1", &V::Map(Map::new()), &side, &side).unwrap();
        let mut called = false;
        ReplayAudit::from_receipts_lazy(&current, || {
            called = true;
            Err(error("unexpected_parent_load"))
        })
        .unwrap();
        assert!(!called);

        let empty = V::Map(Map::from([("assessment".into(), V::Null)]));
        let no_audit = T::semantic_receipt("core/v1", &V::Map(Map::new()), &empty, &empty).unwrap();
        ReplayAudit::from_receipts_lazy(&no_audit, || {
            called = true;
            Err(error("unexpected_parent_load"))
        })
        .unwrap();
        assert!(!called);

        let invalid = fixture_receipt("recorded", "bad-assessment_revision");
        let invalid_result = ReplayAudit::from_receipts_lazy(&invalid, || {
            called = true;
            Err(error("unexpected_parent_load"))
        });
        assert!(matches!(
            invalid_result,
            Err(refusal) if refusal.0 == "invalid_retained_assessment"
        ));
        assert!(!called);
    }

    #[test]
    fn lazy_receipt_audit_loads_parent_witness_only_for_foreign_audits() {
        let old = fixture_receipt("witnessed", "unwitnessed-old");
        let mut called = false;
        let refused = ReplayAudit::from_receipts_lazy(&old, || {
            called = true;
            Ok(Vec::new())
        });
        assert!(called);
        assert!(matches!(
            refused,
            Err(refusal) if refusal.0 == "unknown_retained_adapter_audit"
        ));

        let mut called = false;
        ReplayAudit::from_receipts_lazy(&old, || {
            called = true;
            Ok(vec![old.clone()])
        })
        .unwrap();
        assert!(called);
    }

    #[test]
    fn retained_adapter_audits_match_python_and_require_parent_witnesses() {
        let data: J = serde_json::from_str(include_str!(
            "../tests/fixtures/history-authoring-audit.json"
        ))
        .unwrap();
        for group in ["recorded", "witnessed", "retain"] {
            for case in data[group].as_array().unwrap() {
                let input = &case["input"];
                let result = match group {
                    "recorded" => recorded(&V::from_tagged(input).unwrap()).map(V::Map),
                    "witnessed" => {
                        let parents = input["parents"]
                            .as_object()
                            .unwrap()
                            .iter()
                            .map(|(k, v)| (k.clone(), v.as_str().unwrap().as_bytes().to_vec()))
                            .collect();
                        witnessed(
                            &V::from_tagged(&input["receipt"]).unwrap(),
                            &parents,
                            data["python_adapter"].as_str().unwrap(),
                        )
                        .map(V::Map)
                    }
                    _ => {
                        let audits = V::from_tagged(&input["audits"]).unwrap();
                        retain(
                            &V::from_tagged(&input["report"]).unwrap(),
                            if audits == V::Null {
                                None
                            } else {
                                Some(map(&audits).unwrap())
                            },
                        )
                    }
                };
                if let Some(expected) = case.get("output") {
                    assert_eq!(
                        result.unwrap(),
                        V::from_tagged(expected).unwrap(),
                        "{group} {}",
                        case["name"]
                    );
                } else {
                    let err = result.unwrap_err().0;
                    assert!(
                        case["refused"].as_str().unwrap().contains(&err),
                        "{group} {}: {err}",
                        case["name"]
                    );
                }
            }
        }
    }
}
