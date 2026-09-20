//! A native deployment declaration covers resolved file bytes and embedded
//! contracts. It neither attests loaded code nor tests computation readiness.
use crate::{
    Result,
    history_contract::error,
    history_runtime as H,
    identity::sha256,
    public_workspace::{self as W, ResourceSelection},
    reasoning_runtime as R, require,
};
use serde_json::{Value as J, json};
use std::path::{Path, PathBuf};
pub const KIND: &str = "native-rust/v1";
const MAX_EXECUTABLE: usize = 256 * 1024 * 1024;
const SCHEMAS: [(&str, &[u8]); 3] = [
    (
        "assessment.schema.json",
        include_bytes!("../../scripts/assessment.schema.json"),
    ),
    (
        "reasoning/assessment.schema.json",
        include_bytes!("../../scripts/reasoning/assessment.schema.json"),
    ),
    (
        "reasoning/history_assessment.schema.json",
        include_bytes!("../../scripts/reasoning/history_assessment.schema.json"),
    ),
];
fn seal(mut value: J) -> Result<J> {
    let digest = H::digest(&value)?;
    value
        .as_object_mut()
        .ok_or_else(|| error("invalid_runtime_manifest"))?
        .insert("digest".into(), json!(digest));
    Ok(value)
}
fn schemas() -> Result<J> {
    let mut resources = Vec::new();
    for (name, raw) in SCHEMAS {
        crate::json_ingress::parse_slice(raw, crate::json_ingress::DuplicateKeys::Reject)?;
        resources.push(json!({"name":name,"storage":"embedded","sha256":sha256(raw)}));
    }
    seal(
        json!({"version":2,"history":H::history_schemas(),"identity_schemes":["prototype/v1","typed-history/v2"],"resources":resources}),
    )
}
fn resources(selection: &ResourceSelection) -> Result<J> {
    let (core, ordinary) = if let Some(root) = &selection.root {
        let core = if let Some(path) = &selection.core {
            R::inspect_archive(root, path, &selection.target)?
        } else {
            json!({"status":"unavailable","reason":"archive_missing"})
        };
        let ordinary = if let Some(directory) = &selection.ordinary {
            crate::ordinary_runtime::Program::inspect(root, directory)?
        } else {
            json!({"status":"unavailable","reason":"ordinary_missing"})
        };
        (core, ordinary)
    } else {
        let missing = json!({"status":"unavailable","reason":"resource_root_missing"});
        (missing.clone(), missing)
    };
    seal(
        json!({"version":1,"scheme":"native-resources/v1","target":selection.target,
        "core":core,"ordinary":ordinary,"readiness":"not_tested"}),
    )
}
fn executable_hash(path: &Path) -> Result<String> {
    let parent = path
        .parent()
        .ok_or_else(|| error("invalid_runtime_resolution"))?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| error("invalid_runtime_resolution"))?;
    R::resource_hash(parent, name, MAX_EXECUTABLE)
}
/// This endpoint performs no record lookup, process launch, extraction or write.
pub fn describe(nonce: &str) -> Result<J> {
    describe_with_probe(nonce, &mut |_| Ok(()))
}
fn describe_with_probe(nonce: &str, probe: &mut dyn FnMut(&str) -> Result<()>) -> Result<J> {
    require(H::nonce_valid(nonce), "invalid_runtime_nonce")?;
    let executable = std::env::current_exe()?.canonicalize()?;
    let configured = std::env::var_os("KPOPPER_NATIVE_RESOURCES").map(PathBuf::from);
    let argv = std::env::args_os()
        .map(|a| {
            a.into_string()
                .map_err(|_| error("invalid_runtime_resolution"))
        })
        .collect::<Result<Vec<_>>>()?;
    let result = describe_context(
        nonce,
        &executable,
        configured.as_deref(),
        &argv,
        &mut |stage| {
            probe(stage)?;
            let now = std::env::var_os("KPOPPER_NATIVE_RESOURCES").map(PathBuf::from);
            require(
                now == configured && std::env::current_exe()?.canonicalize()? == executable,
                "runtime_sources_changed",
            )
        },
    )?;
    require(
        std::env::var_os("KPOPPER_NATIVE_RESOURCES").map(PathBuf::from) == configured
            && std::env::current_exe()?.canonicalize()? == executable,
        "runtime_sources_changed",
    )?;
    Ok(result)
}
fn describe_context(
    nonce: &str,
    executable: &Path,
    configured: Option<&Path>,
    argv: &[String],
    probe: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<J> {
    let selection = W::select_resources(executable, configured, true)?;
    let result = describe_selected(nonce, executable, &selection, argv, &mut |stage| {
        probe(stage)?;
        require(
            W::select_resources(executable, configured, true)? == selection,
            "runtime_sources_changed",
        )
    })?;
    require(
        W::select_resources(executable, configured, true)? == selection,
        "runtime_sources_changed",
    )?;
    Ok(result)
}
fn describe_selected(
    nonce: &str,
    executable: &Path,
    selection: &ResourceSelection,
    argv: &[String],
    probe: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<J> {
    require(H::nonce_valid(nonce), "invalid_runtime_nonce")?;
    let hash = executable_hash(executable)?;
    let artifact = seal(
        json!({"version":1,"kind":KIND,"target":selection.target,"executable_sha256":hash,
        "adapter_source_sha256":env!("KPOP_REASONING_ADAPTER_SHA256")}),
    )?;
    let resources = resources(selection)?;
    let value = json!({"version":2,"endpoint":"history/capabilities","nonce":nonce,
        "resolved":{"executable":executable,"resource_root":selection.root,"argv":argv},
        "artifact":artifact,"schemas":schemas()?,"resources":resources,
        "assurance":{"executable":"resolved_file_hash_only","resources":"file_and_manifest_validation_only","freshness":"nonce_correlation_only"}});
    probe("captured")?;
    require(
        executable_hash(executable)? == hash && self::resources(selection)? == resources,
        "runtime_sources_changed",
    )?;
    validate(&value, nonce)?;
    require(
        serde_json::to_vec(&value)?.len() <= 2 * 1024 * 1024,
        "runtime_size_limit",
    )?;
    Ok(value)
}
fn component(value: &J, version: u64) -> Result<()> {
    require(
        value.is_object() && value["version"] == version && H::hex(&value["digest"]),
        "invalid_runtime_manifest",
    )?;
    let mut body = value.clone();
    body.as_object_mut().unwrap().remove("digest");
    require(
        value["digest"] == H::digest(&body)?,
        "runtime_digest_mismatch",
    )
}
fn missing(value: &J, reasons: &[&str]) -> Result<()> {
    H::exact(value, &["status", "reason"], "unsupported_native_manifest")?;
    require(
        value["status"] == "unavailable"
            && value["reason"]
                .as_str()
                .is_some_and(|s| reasons.contains(&s)),
        "unsupported_native_manifest",
    )
}
/// Strict shape/integrity validation. Expected deployment hashes remain owned by
/// the caller and are checked by probe_launchers, not inferred from this packet.
pub(crate) fn validate(value: &J, nonce: &str) -> Result<()> {
    H::exact(
        value,
        &[
            "version",
            "endpoint",
            "nonce",
            "resolved",
            "artifact",
            "schemas",
            "resources",
            "assurance",
        ],
        "unsupported_runtime_endpoint",
    )?;
    require(
        value["version"] == 2 && value["endpoint"] == "history/capabilities",
        "unsupported_runtime_endpoint",
    )?;
    require(value["nonce"] == nonce, "runtime_nonce_mismatch")?;
    require(
        value["assurance"]
            == json!({"executable":"resolved_file_hash_only","resources":"file_and_manifest_validation_only","freshness":"nonce_correlation_only"}),
        "unsupported_runtime_assurance",
    )?;
    let resolved = &value["resolved"];
    H::exact(
        resolved,
        &["executable", "resource_root", "argv"],
        "invalid_runtime_resolution",
    )?;
    require(
        H::resolved(&resolved["executable"])?
            && (resolved["resource_root"].is_null() || H::resolved(&resolved["resource_root"])?)
            && resolved["argv"].as_array().is_some_and(|a| {
                a.len() <= 128
                    && a.iter().all(|v| {
                        v.as_str()
                            .is_some_and(|s| !s.contains('\0') && s.len() <= 8192)
                    })
            }),
        "invalid_runtime_resolution",
    )?;
    let artifact = &value["artifact"];
    H::exact(
        artifact,
        &[
            "version",
            "kind",
            "target",
            "executable_sha256",
            "adapter_source_sha256",
            "digest",
        ],
        "unsupported_native_manifest",
    )?;
    component(artifact, 1)?;
    let target = R::target_name()?;
    require(
        artifact["kind"] == KIND
            && artifact["target"] == target
            && H::hex(&artifact["executable_sha256"])
            && H::hex(&artifact["adapter_source_sha256"]),
        "unsupported_native_manifest",
    )?;
    let schemas = &value["schemas"];
    H::exact(
        schemas,
        &[
            "version",
            "history",
            "identity_schemes",
            "resources",
            "digest",
        ],
        "unsupported_runtime_schema",
    )?;
    component(schemas, 2)?;
    require(
        schemas["history"] == H::history_schemas()
            && schemas["identity_schemes"] == json!(["prototype/v1", "typed-history/v2"])
            && schemas["resources"]
                .as_array()
                .is_some_and(|a| a.len() == SCHEMAS.len()),
        "unsupported_runtime_schema",
    )?;
    for (item, (name, _)) in schemas["resources"].as_array().unwrap().iter().zip(SCHEMAS) {
        H::exact(
            item,
            &["name", "storage", "sha256"],
            "unsupported_runtime_schema",
        )?;
        require(
            item["name"] == name && item["storage"] == "embedded" && H::hex(&item["sha256"]),
            "unsupported_runtime_schema",
        )?;
    }
    let resources = &value["resources"];
    H::exact(
        resources,
        &[
            "version",
            "scheme",
            "target",
            "core",
            "ordinary",
            "readiness",
            "digest",
        ],
        "unsupported_native_manifest",
    )?;
    component(resources, 1)?;
    require(
        resources["scheme"] == "native-resources/v1"
            && resources["target"] == target
            && resources["readiness"] == "not_tested",
        "unsupported_native_manifest",
    )?;
    if resolved["resource_root"].is_null() {
        missing(&resources["core"], &["resource_root_missing"])?;
        missing(&resources["ordinary"], &["resource_root_missing"])?;
    } else {
        let core = &resources["core"];
        if core["status"] == "unavailable" {
            missing(core, &["archive_missing"])?;
        } else {
            H::exact(
                core,
                &["status", "path", "sha256", "manifest"],
                "unsupported_native_manifest",
            )?;
            require(
                core["status"] == "archive_validated"
                    && [
                        format!("reasoning/{target}.zip"),
                        format!("reasoning/{target}.kpopper-runtime"),
                    ]
                    .iter()
                    .any(|p| core["path"] == *p)
                    && H::hex(&core["sha256"]),
                "unsupported_native_manifest",
            )?;
            R::Runtime::validate_manifest(&core["manifest"], &target)?;
        }
        let ordinary = &resources["ordinary"];
        if ordinary["status"] == "unavailable" {
            missing(ordinary, &["ordinary_missing"])?;
        } else {
            H::exact(
                ordinary,
                &["status", "directory", "manifest", "files"],
                "unsupported_native_manifest",
            )?;
            let name = if cfg!(windows) {
                "epistemic-core.exe"
            } else {
                "epistemic-core"
            };
            H::exact(
                &ordinary["files"],
                &["build.json", name],
                "unsupported_native_manifest",
            )?;
            require(
                ordinary["status"] == "files_validated"
                    && ordinary["directory"] == format!("ordinary/{target}")
                    && ordinary["manifest"].is_object()
                    && ordinary["manifest"]["source_sha256"] == env!("KPOP_ORDINARY_SOURCE_SHA256")
                    && ordinary["manifest"]["binary_sha256"] == ordinary["files"][name]
                    && ordinary["files"].as_object().unwrap().values().all(H::hex),
                "unsupported_native_manifest",
            )?;
        }
    }
    Ok(())
}
/// Integrity required by native transition consumers. This does not execute the
/// programs or promote the declaration's `not_tested` readiness.
pub(crate) fn require_complete(value: &J) -> Result<()> {
    require(
        value["resources"]["core"]["status"] == "archive_validated"
            && value["resources"]["ordinary"]["status"] == "files_validated",
        "history_transition_runtime_unsupported",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    fn setup(root: &Path) -> (PathBuf, ResourceSelection) {
        let binary = root.join("fixture-native");
        fs::write(&binary, b"native fixture, never executed").unwrap();
        let resources = root.join("resources");
        let target = R::target_name().unwrap();
        fs::create_dir_all(resources.join("reasoning")).unwrap();
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../scripts/reasoning/native")
                .join(format!("{target}.zip")),
            resources
                .join("reasoning")
                .join(format!("{target}.kpopper-runtime")),
        )
        .unwrap();
        let ordinary = resources.join("ordinary").join(&target);
        fs::create_dir_all(&ordinary).unwrap();
        let name = if cfg!(windows) {
            "epistemic-core.exe"
        } else {
            "epistemic-core"
        };
        let bytes = b"not executed";
        fs::write(ordinary.join(name), bytes).unwrap();
        fs::write(ordinary.join("build.json"),serde_json::to_vec(&json!({"source_sha256":env!("KPOP_ORDINARY_SOURCE_SHA256"),"binary_sha256":sha256(bytes),"lean_version":"fixture","platform":"fixture"})).unwrap()).unwrap();
        let selection = W::select_resources(&binary, None, true).unwrap();
        (binary, selection)
    }
    fn observe(
        binary: &Path,
        selection: &ResourceSelection,
        probe: &mut dyn FnMut(&str) -> Result<()>,
    ) -> Result<J> {
        assert_eq!(&W::select_resources(binary, None, true).unwrap(), selection);
        describe_context(
            "0123456789abcdef",
            binary,
            None,
            &[binary.to_str().unwrap().into()],
            probe,
        )
    }
    #[test]
    fn exact_native_contract_uses_real_embedded_bytes_and_diagnostic_missing_resources() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let (binary, selection) = setup(&root);
        let declaration = observe(&binary, &selection, &mut |_| Ok(())).unwrap();
        validate(&declaration, "0123456789abcdef").unwrap();
        require_complete(&declaration).unwrap();
        assert_eq!(
            declaration["artifact"]["executable_sha256"],
            sha256(&fs::read(&binary).unwrap())
        );
        for (row, (_, raw)) in declaration["schemas"]["resources"]
            .as_array()
            .unwrap()
            .iter()
            .zip(SCHEMAS)
        {
            assert_eq!(row["sha256"], sha256(raw));
        }
        assert!(declaration.get("sources").is_none());
        assert!(declaration["resolved"].get("cli").is_none());
        assert_eq!(declaration["resources"]["readiness"], "not_tested");
        fs::remove_dir_all(root.join("resources")).unwrap();
        let absent = W::select_resources(&binary, None, true).unwrap();
        let v = observe(&binary, &absent, &mut |_| Ok(())).unwrap();
        validate(&v, "0123456789abcdef").unwrap();
        assert!(require_complete(&v).is_err());
        assert!(W::select_resources(&binary, Some(&root.join("missing")), true).is_err());
        assert!(observe(&binary, &absent, &mut |_| Ok(())).is_ok());
        for nonce in ["", "short", "0123456789abcde."] {
            assert!(describe_selected(nonce, &binary, &absent, &[], &mut |_| Ok(())).is_err());
        }
    }
    #[test]
    fn captured_executable_resources_and_root_changes_refuse() {
        for changed in [
            "executable",
            "core",
            "ordinary",
            "manifest",
            "root",
            "suffix",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            let (binary, selection) = setup(&root);
            let result = observe(&binary, &selection, &mut |_| {
                match changed {
                    "executable" => fs::write(&binary, b"changed")?,
                    "core" => fs::write(
                        root.join("resources")
                            .join(selection.core.as_ref().unwrap()),
                        b"changed",
                    )?,
                    "ordinary" => fs::write(
                        root.join("resources")
                            .join(selection.ordinary.as_ref().unwrap())
                            .join(if cfg!(windows) {
                                "epistemic-core.exe"
                            } else {
                                "epistemic-core"
                            }),
                        b"changed",
                    )?,
                    "manifest" => fs::write(
                        root.join("resources")
                            .join(selection.ordinary.as_ref().unwrap())
                            .join("build.json"),
                        b"{}",
                    )?,
                    "root" => fs::rename(root.join("resources"), root.join("moved"))?,
                    "suffix" => {
                        fs::copy(
                            root.join("resources")
                                .join(selection.core.as_ref().unwrap()),
                            root.join("resources/reasoning")
                                .join(format!("{}.zip", selection.target)),
                        )?;
                    }
                    _ => unreachable!(),
                };
                Ok(())
            });
            assert!(result.is_err(), "{changed}");
        }
    }
    #[test]
    fn strict_v2_validation_rejects_resealed_schema_manifest_and_assurance_forgery() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let (binary, selection) = setup(&root);
        let original = observe(&binary, &selection, &mut |_| Ok(())).unwrap();
        for change in [
            "schema",
            "target",
            "source",
            "path",
            "ordinary",
            "readiness",
            "assurance",
            "extra",
            "scheme",
        ] {
            let mut v = original.clone();
            match change {
                "schema" => v["schemas"]["history"]["prepared_mutation"] = json!([1]),
                "target" => v["artifact"]["target"] = json!("unknown"),
                "source" => {
                    v["resources"]["core"]["manifest"]["source_sha256"] = json!("0".repeat(64))
                }
                "path" => v["resources"]["core"]["path"] = json!("../other.zip"),
                "ordinary" => v["resources"]["ordinary"]["files"]["extra"] = json!("0".repeat(64)),
                "readiness" => v["resources"]["readiness"] = json!("ready"),
                "assurance" => v["assurance"]["executable"] = json!("attested"),
                "extra" => v["extra"] = json!(true),
                "scheme" => v["artifact"]["kind"] = json!("product-python/v1"),
                _ => unreachable!(),
            }
            for part in ["artifact", "schemas", "resources"] {
                v[part].as_object_mut().unwrap().remove("digest");
                v[part] = seal(v[part].clone()).unwrap();
            }
            assert!(validate(&v, "0123456789abcdef").is_err(), "{change}");
        }
        assert!(validate(&original, "differentnonce123456").is_err());
    }
    #[test]
    fn resource_selection_handles_exact_suffixes_without_ordinary_core_coupling() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let (binary, selection) = setup(&root);
        let resources = root.join("resources");
        let modern = resources.join(selection.core.unwrap());
        let old = resources
            .join("reasoning")
            .join(format!("{}.zip", selection.target));
        fs::rename(&modern, &old).unwrap();
        let legacy = W::select_resources(&binary, None, true).unwrap();
        assert!(legacy.core.unwrap().ends_with(".zip"));
        fs::copy(&old, &modern).unwrap();
        assert_eq!(
            W::select_resources(&binary, None, true).unwrap_err().0,
            "ambiguous_runtime_archive"
        );
        fs::remove_file(&old).unwrap();
        let dir = resources.join("ordinary").join(&selection.target);
        fs::remove_dir_all(&dir).unwrap();
        fs::write(&dir, b"invalid ordinary directory").unwrap();
        assert!(W::select_resources(&binary, None, true).is_err());
        assert!(W::select_resources(&binary, None, false).is_ok());
    }
    #[cfg(unix)]
    #[test]
    fn symlinks_and_bounded_resource_reads_refuse() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let (binary, selection) = setup(&root);
        assert!(R::resource_read(&root, "fixture-native", 2).is_err());
        assert!(R::resource_read(&root, "../outside", 1024).is_err());
        let path = root.join("resources").join(selection.core.unwrap());
        fs::remove_file(&path).unwrap();
        symlink(&binary, &path).unwrap();
        assert_eq!(
            W::select_resources(&binary, None, true).unwrap_err().0,
            "runtime_symlink"
        );
    }
    #[test]
    fn archive_members_checksums_and_strict_manifest_are_inspected_without_extraction() {
        use std::io::{Read, Write};
        use zip::write::SimpleFileOptions;
        for changed in ["member", "manifest", "path", "duplicate_json"] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            let (binary, selection) = setup(&root);
            let path = root
                .join("resources")
                .join(selection.core.as_ref().unwrap());
            let raw = fs::read(&path).unwrap();
            let mut source = zip::ZipArchive::new(std::io::Cursor::new(raw)).unwrap();
            let mut output = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
            let mut changed_member = false;
            for index in 0..source.len() {
                let mut file = source.by_index(index).unwrap();
                let name = file.name().to_owned();
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes).unwrap();
                if name == "manifest.json" && changed == "manifest" {
                    let mut value: J = serde_json::from_slice(&bytes).unwrap();
                    value["target"] = json!("wrong-target");
                    bytes = serde_json::to_vec(&value).unwrap();
                }
                if name == "manifest.json" && changed == "duplicate_json" {
                    bytes = b"{\"version\":3,\"version\":3}".to_vec();
                }
                if name != "manifest.json" && changed == "member" && !changed_member {
                    bytes.push(0);
                    changed_member = true;
                }
                output
                    .start_file(&name, SimpleFileOptions::default())
                    .unwrap();
                output.write_all(&bytes).unwrap();
            }
            if changed == "path" {
                output
                    .start_file("../escaped", SimpleFileOptions::default())
                    .unwrap();
                output.write_all(b"bad").unwrap();
            }
            fs::write(&path, output.finish().unwrap().into_inner()).unwrap();
            assert!(
                observe(&binary, &selection, &mut |_| Ok(())).is_err(),
                "{changed}"
            );
            assert!(!root.join("escaped").exists());
        }
    }
}
