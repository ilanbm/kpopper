//! Oracle-only comparison of independently verified platform implementations.
//! Production assessments and retained replay audits keep their actual identities.
use crate::{
    Result,
    history_contract::{error, map, text},
    reasoning_runtime::Runtime,
    require,
    value::TypedValue as V,
};
use serde_json::{Value as J, json};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Mutex, OnceLock},
};

fn packaged(target: &str) -> Result<J> {
    static CACHE: OnceLock<Mutex<BTreeMap<String, J>>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap();
    if let Some(value) = cache.get(target) {
        return Ok(value.clone());
    }
    require(
        [
            "darwin-arm64",
            "darwin-x86_64",
            "linux-x86_64",
            "linux-aarch64",
            "windows-x86_64",
        ]
        .contains(&target),
        "unknown_oracle_platform",
    )?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/reasoning/native");
    let observation = crate::reasoning_runtime::inspect_archive(
        &root,
        &format!("{target}.kpopper-runtime"),
        target,
    )?;
    let manifest = &observation["manifest"];
    let files = manifest["files"]
        .as_object()
        .ok_or_else(|| error("invalid_oracle_runtime"))?;
    let libraries = manifest["libraries"]
        .as_array()
        .ok_or_else(|| error("invalid_oracle_runtime"))?
        .iter()
        .map(|name| {
            let name = name
                .as_str()
                .ok_or_else(|| error("invalid_oracle_runtime"))?;
            Ok((
                name.to_owned(),
                files
                    .get(name)
                    .cloned()
                    .ok_or_else(|| error("invalid_oracle_runtime"))?,
            ))
        })
        .collect::<Result<serde_json::Map<_, _>>>()?;
    let executable = manifest["executable"]
        .as_str()
        .ok_or_else(|| error("invalid_oracle_runtime"))?;
    let value = json!({
        "lean_version":manifest["lean_version"], "source_sha256":manifest["source_sha256"],
        "archive_sha256":observation["sha256"], "binary_sha256":files[executable],
        "target":target, "libraries":libraries, "modified_libraries":[],
    });
    cache.insert(target.into(), value.clone());
    Ok(value)
}

pub(crate) fn verify_pair(actual: &V, reference: &V, runtime: &Runtime) -> Result<()> {
    let actual_map = map(actual)?;
    let reference_map = map(reference)?;
    let protocol = text(&actual_map["protocol"])?;
    let request = match protocol {
        "KP2" => json!({}),
        "KP4" => json!({"version":4,"request":{"required_modules":["query/v1"]}}),
        value => json!({"protocol":value}),
    };
    require(
        *actual == V::from_json(&runtime.implementation_for(&request)?)?,
        "actual_oracle_runtime_mismatch",
    )?;
    let mut expected = packaged(text(&reference_map["target"])?)?;
    expected["protocol"] = reference_map["protocol"].to_json()?;
    expected["adapter_source_sha256"] = reference_map["adapter_source_sha256"].to_json()?;
    require(
        *reference == V::from_json(&expected)?,
        "reference_oracle_runtime_mismatch",
    )?;
    for field in ["protocol", "source_sha256", "lean_version"] {
        require(
            actual_map[field] == reference_map[field],
            "oracle_kernel_mismatch",
        )?;
    }
    Ok(())
}

#[test]
fn comparison_verifies_both_platforms_and_rejects_changed_provenance() {
    let cache = tempfile::tempdir().unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/reasoning/native");
    let target = crate::reasoning_runtime::target_name().unwrap();
    let runtime = Runtime::open(
        &root.join(format!("{target}.kpopper-runtime")),
        cache.path(),
        Default::default(),
    )
    .unwrap();
    let actual = V::from_json(&runtime.implementation_for(&json!({})).unwrap()).unwrap();
    for target in ["darwin-arm64", "linux-x86_64", "windows-x86_64"] {
        let mut reference = packaged(target).unwrap();
        reference["protocol"] = json!("KP2");
        reference["adapter_source_sha256"] = json!("0".repeat(64));
        verify_pair(&actual, &V::from_json(&reference).unwrap(), &runtime).unwrap();
        for field in ["archive_sha256", "binary_sha256", "source_sha256"] {
            let mut changed = reference.clone();
            changed[field] = json!("0".repeat(64));
            assert!(verify_pair(&actual, &V::from_json(&changed).unwrap(), &runtime).is_err());
        }
        let mut changed_actual = actual.to_json().unwrap();
        changed_actual["binary_sha256"] = json!("0".repeat(64));
        assert!(
            verify_pair(
                &V::from_json(&changed_actual).unwrap(),
                &V::from_json(&reference).unwrap(),
                &runtime
            )
            .is_err()
        );
    }
}
