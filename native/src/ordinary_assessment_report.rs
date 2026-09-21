//! Finite report facade; all ordinary findings and revisions share one evaluator.
use crate::ordinary_report as O;
use crate::{
    Result,
    history_contract::*,
    ordinary_reader::{ordinary, ordinary_map},
    reasoning_runtime::Runtime,
    source_capture::{CapturedSource, ReadMode},
    value::TypedValue as V,
};
use std::path::{Path, PathBuf};
pub fn legacy_json(value: &V) -> Result<String> {
    O::legacy_json(&ordinary(value))
}
pub fn compact_json(value: &V) -> Result<String> {
    O::compact_json(&ordinary(value))
}
pub fn legacy_digest(value: &V) -> Result<String> {
    O::legacy_digest(&ordinary(value))
}
pub fn assess(
    document: &V,
    hypotheses: &Map,
    context: &V,
    runtime: Option<&Runtime>,
    policy: &str,
) -> Result<V> {
    O::assess(
        &ordinary(document),
        &ordinary_map(hypotheses),
        &ordinary(context),
        runtime,
        policy,
    )?
    .finite_projection()
}
pub fn from_capture(
    capture: &CapturedSource,
    runtime: Option<&Runtime>,
    policy: &str,
) -> Result<V> {
    let report = assess(
        capture.ordinary_document(),
        map(capture.hypotheses())?,
        &capture.ordinary_context(),
        runtime,
        policy,
    )?;
    capture.verify()?;
    Ok(report)
}
pub fn load(
    paths: &[PathBuf],
    cwd: &Path,
    mode: ReadMode,
    runtime: Option<&Runtime>,
    policy: &str,
) -> Result<V> {
    let capture =
        crate::source_capture::capture_source_with_runtime(paths, cwd, mode, None, runtime)?;
    from_capture(&capture, runtime, policy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ordinary_assessment as A;
    fn differences(a: &V, b: &V, path: &str, out: &mut Vec<String>) {
        if a == b {
            return;
        }
        match (a, b) {
            (V::Map(a), V::Map(b)) if a.keys().eq(b.keys()) => {
                for k in a.keys() {
                    differences(&a[k], &b[k], &format!("{path}.{k}"), out)
                }
            }
            (V::List(a), V::List(b)) if a.len() == b.len() => {
                for (i, (a, b)) in a.iter().zip(b).enumerate() {
                    differences(a, b, &format!("{path}[{i}]"), out)
                }
            }
            _ => out.push(format!("{path}: {a:?} != {b:?}")),
        }
    }
    #[test]
    fn complete_ordinary_assessments_match_final_python() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/ordinary-assessment.json"))
                .unwrap();
        let cache = tempfile::tempdir().unwrap();
        let runtime = crate::history_authoring::tests::runtime(cache.path())
            .with_ordinary_program(crate::ordinary_reader::tests::program());
        let mut failures = vec![];
        for c in fixture["cases"].as_array().unwrap() {
            let doc = V::from_tagged(&c["document"]).unwrap();
            let layers = V::from_tagged(&c["layers"]).unwrap();
            let context = V::from_tagged(&c["context"]).unwrap();
            for policy in [A::POLICY, "falsifiers-only/v1"] {
                let actual = assess(
                    &doc,
                    map(&layers).unwrap(),
                    &context,
                    Some(&runtime),
                    policy,
                )
                .unwrap();
                let expected = V::from_tagged(&c["reports"][policy]).unwrap();
                differences(
                    &actual,
                    &expected,
                    &format!("{} {policy}", c["name"]),
                    &mut failures,
                );
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}
