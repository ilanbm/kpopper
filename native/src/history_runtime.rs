//! Bounded probes of explicitly selected managed Python launchers during cutover.
//! A declaration binds resolved sources. Excluding writers remains the caller's duty.
use crate::{Result, history_contract::error, identity::sha256, require};
use serde_json::{Value as J, json};
use std::{collections::BTreeSet, path::Path, process::Command, time::Duration};
const MAX_OUTPUT: usize = 2 * 1024 * 1024;
const INCLUSION: &[&str] = &["*.py", "reasoning/*.py", "session/*.py"];
const APPLICATION_INCLUSION: &[&str] = &[
    "*.py",
    "reasoning/*.py",
    "session/*.py",
    "applications/*.py",
];
const APPLICATION_REQUIRED: &[&str] = &[
    "applications/__init__.py",
    "applications/hub.py",
    "applications/annotated_doc.py",
];
const REQUIRED: &[&str] = &[
    "__init__.py",
    "cli.py",
    "history_cli.py",
    "history_runtime.py",
    "history_contract.py",
    "history_store.py",
    "history_adapter.py",
    "history_transaction.py",
    "history_paths.py",
    "history_authoring.py",
    "history_bootstrap.py",
    "history_identity.py",
    "history_edits.py",
    "history_direct.py",
    "history_bundle.py",
    "history_migration.py",
    "history_activation.py",
    "history_group_activation.py",
    "history_hypotheses.py",
    "history_hypothesis_import.py",
    "history_branch.py",
    "provenance.py",
    "pending_grounding.py",
    "knowledge_views.py",
    "reasoning/__init__.py",
    "reasoning/contract.py",
    "reasoning/snapshot.py",
    "reasoning/scenario.py",
    "reasoning/evaluate.py",
    "reasoning/runtime.py",
    "session/__init__.py",
];
fn required(applications: bool) -> Vec<&'static str> {
    let mut names = REQUIRED.to_vec();
    if applications {
        names.extend(APPLICATION_REQUIRED);
    }
    names.sort();
    names
}
fn exact<'a>(v: &'a J, keys: &[&str], code: &str) -> Result<&'a serde_json::Map<String, J>> {
    let m = v.as_object().ok_or_else(|| error(code))?;
    require(
        m.len() == keys.len() && keys.iter().all(|k| m.contains_key(*k)),
        code,
    )?;
    Ok(m)
}
fn text<'a>(v: &'a J, code: &str) -> Result<&'a str> {
    v.as_str().ok_or_else(|| error(code))
}
fn hex(v: &J) -> bool {
    v.as_str().is_some_and(|s| {
        s.len() == 64
            && s.bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    })
}
fn nonce_valid(nonce: &str) -> bool {
    (16..=128).contains(&nonce.len())
        && nonce
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
}
fn digest(value: &J) -> Result<String> {
    Ok(sha256(&serde_json::to_vec(value)?))
}
fn resolved(path: &J) -> Result<bool> {
    let path = text(path, "invalid_runtime_resolution")?;
    Ok(Path::new(path).is_absolute()
        && crate::project_modes::resolved(Path::new(path))? == Path::new(path))
}
fn history_schemas() -> J {
    json!({"authority":[1,2],"baseline":[1],"commit":[1],"typed_object":[2],"prepared_mutation":[1,2],"projection":[1],"import":[1,2],"authoring_receipt":[1,2,3,4,5,6,7,8,9],"identity_receipt":[1,2],"history_auxiliary":[1],"group_transition":[1],"named_hypotheses":[1],"physical_hypothesis_import":[1],"branch_capture":[1,2],"branch_adoption":[1,2],"bundle":[1,2,3],"contribution":[1,2,3],"retained_generations":[1],"cancellation":[1],"commit_capabilities":["explicit-root-disposition/v1","subject-paths/v2","temporal-applicability/v1"],"bundle_capabilities":["generation-cancellation/v1","history-closure/v1","history-generations/v1","history-subset/v1","subject-paths/v2"],"act_kinds":["accept","correct","propose","refute","retire","review"]})
}
/// Validate the retained v1 managed-Python contract; it does not attest bytecode
/// or claim the experimental native executable has this complete public surface.
pub fn validate_declaration(value: &J, nonce: &str) -> Result<()> {
    exact(
        value,
        &[
            "version",
            "endpoint",
            "nonce",
            "resolved",
            "sources",
            "schemas",
            "native",
            "assurance",
        ],
        "unsupported_runtime_endpoint",
    )?;
    require(
        value["version"] == json!(1) && value["endpoint"] == "history/capabilities",
        "unsupported_runtime_endpoint",
    )?;
    require(value["nonce"] == nonce, "runtime_nonce_mismatch")?;
    require(
        value["assurance"]
            == json!({"source":"resolved_files_only","native":"archive_validation_only","freshness":"nonce_correlation_only"}),
        "unsupported_runtime_assurance",
    )?;
    let resolution = &value["resolved"];
    exact(
        resolution,
        &[
            "executable",
            "cli",
            "package_root",
            "argv",
            "bytecode_write_disabled",
        ],
        "invalid_runtime_resolution",
    )?;
    require(
        resolution["bytecode_write_disabled"].is_boolean()
            && resolution["argv"]
                .as_array()
                .is_some_and(|a| a.iter().all(J::is_string)),
        "invalid_runtime_resolution",
    )?;
    for key in ["executable", "cli", "package_root"] {
        require(resolved(&resolution[key])?, "invalid_runtime_resolution")?;
    }
    for key in ["sources", "schemas", "native"] {
        let item = &value[key];
        require(
            item.is_object() && hex(&item["digest"]) && item["version"] == json!(1),
            "invalid_runtime_manifest",
        )?;
        let mut body = item.clone();
        body.as_object_mut().unwrap().remove("digest");
        require(item["digest"] == digest(&body)?, "runtime_digest_mismatch")?;
    }
    let source = &value["sources"];
    exact(
        source,
        &[
            "version",
            "scheme",
            "inclusion",
            "required",
            "files",
            "digest",
        ],
        "unsupported_source_manifest",
    )?;
    let applications = source["inclusion"] == json!(APPLICATION_INCLUSION);
    let required = required(applications);
    require(
        source["scheme"] == "product-python/v1"
            && (applications || source["inclusion"] == json!(INCLUSION))
            && source["required"] == json!(required)
            && source["files"].is_array(),
        "unsupported_source_manifest",
    )?;
    let mut paths = vec![];
    for item in source["files"].as_array().unwrap() {
        exact(item, &["path", "sha256"], "invalid_source_manifest")?;
        require(hex(&item["sha256"]), "invalid_source_manifest")?;
        let p = text(&item["path"], "invalid_source_manifest")?;
        crate::history_authority::relative_path(p).map_err(|_| error("invalid_source_manifest"))?;
        require(
            p.ends_with(".py") && !p.contains(':'),
            "invalid_source_manifest",
        )?;
        paths.push(p);
    }
    require(
        paths.len() <= 1024
            && paths.windows(2).all(|p| p[0] < p[1])
            && required.iter().all(|p| paths.contains(p)),
        "invalid_source_manifest",
    )?;
    let schemas = &value["schemas"];
    exact(
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
    let schema_paths = [
        "assessment.schema.json",
        "reasoning/assessment.schema.json",
        "reasoning/history_assessment.schema.json",
    ];
    require(
        schemas["history"] == history_schemas()
            && schemas["identity_schemes"] == json!(["prototype/v1", "typed-history/v2"])
            && schemas["resources"]
                .as_array()
                .is_some_and(|a| a.len() == 3),
        "unsupported_runtime_schema",
    )?;
    for (item, path) in schemas["resources"]
        .as_array()
        .unwrap()
        .iter()
        .zip(schema_paths)
    {
        exact(item, &["path", "sha256"], "unsupported_runtime_schema")?;
        require(
            item["path"] == path && hex(&item["sha256"]),
            "unsupported_runtime_schema",
        )?;
    }
    let native = &value["native"];
    require(
        native["readiness"] == "not_tested",
        "unsupported_native_assurance",
    )?;
    if native["status"] == "unavailable" {
        exact(
            native,
            &[
                "version",
                "status",
                "reason",
                "target",
                "readiness",
                "digest",
            ],
            "unsupported_native_manifest",
        )?;
        require(
            native["reason"] == "unsupported_target" || native["reason"] == "archive_missing",
            "unsupported_native_manifest",
        )?;
    } else {
        exact(
            native,
            &[
                "version",
                "status",
                "target",
                "archive",
                "archive_sha256",
                "manifest",
                "readiness",
                "digest",
            ],
            "unsupported_native_manifest",
        )?;
        let target = text(&native["target"], "unsupported_native_manifest")?;
        require(
            native["status"] == "archive_validated"
                && native["archive"] == format!("reasoning/native/{target}.zip")
                && hex(&native["archive_sha256"])
                && native["manifest"].is_object(),
            "unsupported_native_manifest",
        )?;
    }
    Ok(())
}
/// Inventory and expected digests must come from the deployment caller, not a
/// record or a prepared receipt. The absolute argv is executed without a shell.
pub fn probe_launchers(inventory: &J, expected: &J, nonce: &str, timeout: Duration) -> Result<J> {
    let launchers = inventory
        .as_array()
        .ok_or_else(|| error("invalid_launcher_inventory"))?;
    require(
        !launchers.is_empty() && launchers.len() <= 16,
        "invalid_launcher_inventory",
    )?;
    let expected = expected
        .as_object()
        .ok_or_else(|| error("invalid_expected_digests"))?;
    require(
        timeout > Duration::ZERO && timeout <= Duration::from_secs(30),
        "invalid_probe_timeout",
    )?;
    require(nonce_valid(nonce), "invalid_runtime_nonce")?;
    let mut ids = BTreeSet::new();
    for item in launchers {
        exact(
            item,
            &["id", "argv", "package_root", "executable"],
            "invalid_launcher_inventory",
        )?;
        let id = text(&item["id"], "invalid_launcher_inventory")?;
        require(
            (1..=80).contains(&id.len())
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
                && ids.insert(id),
            "invalid_launcher_inventory",
        )?;
        let argv = item["argv"]
            .as_array()
            .ok_or_else(|| error("invalid_launcher_inventory"))?;
        require(
            !argv.is_empty()
                && argv.len() <= 64
                && argv.iter().all(|a| {
                    a.as_str()
                        .is_some_and(|a| !a.contains('\0') && a.chars().count() <= 8192)
                }),
            "invalid_launcher_inventory",
        )?;
        let executable = Path::new(argv[0].as_str().unwrap());
        require(
            executable.is_absolute() && executable.is_file(),
            "absolute_launcher_required",
        )?;
        for key in ["package_root", "executable"] {
            require(resolved(&item[key])?, "invalid_launcher_inventory")?;
        }
        require(
            Path::new(item["package_root"].as_str().unwrap()).is_dir()
                && Path::new(item["executable"].as_str().unwrap()).is_file(),
            "invalid_launcher_inventory",
        )?;
        let expected = expected
            .get(id)
            .ok_or_else(|| error("invalid_expected_digests"))?;
        exact(
            expected,
            &["sources", "schemas", "native"],
            "invalid_expected_digests",
        )?;
        require(
            expected.as_object().unwrap().values().all(hex),
            "invalid_expected_digests",
        )?;
    }
    require(
        ids.iter().copied().eq(expected.keys().map(String::as_str)),
        "invalid_launcher_inventory",
    )?;
    let mut proof = vec![];
    for item in launchers {
        let id = item["id"].as_str().unwrap();
        let mut argv = item["argv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        argv.extend(["history", "capabilities", "--nonce", nonce, "--json"].map(str::to_string));
        let mut command = Command::new(&argv[0]);
        command.args(&argv[1..]);
        let raw = crate::reasoning_runtime::run_command_bounded(
            &mut command,
            vec![],
            timeout,
            MAX_OUTPUT,
        )
        .map_err(|_| error("launcher_probe_failed"))?;
        serde_json::from_slice::<crate::store::Unique>(&raw)
            .map_err(|_| error("invalid_runtime_json"))?;
        let value: J = serde_json::from_slice(&raw).map_err(|_| error("invalid_runtime_json"))?;
        validate_declaration(&value, nonce)?;
        let resolution = &value["resolved"];
        require(
            resolution["package_root"] == item["package_root"]
                && resolution["executable"] == item["executable"]
                && resolution["cli"]
                    == Path::new(item["package_root"].as_str().unwrap())
                        .join("cli.py")
                        .to_string_lossy()
                        .as_ref(),
            "runtime_root_mismatch",
        )?;
        let digests = json!({"sources":value["sources"]["digest"],"schemas":value["schemas"]["digest"],"native":value["native"]["digest"]});
        require(digests == expected[id], "runtime_digest_mismatch")?;
        proof.push(
            json!({"id":id,"argv":argv,"declaration_digest":digest(&value)?,"declaration":value}),
        );
    }
    Ok(
        json!({"version":1,"kind":"managed-launcher-probe/v1","nonce":nonce,"scope":"selected_managed_launchers","complete":true,"launchers":proof,"assurance":"fresh_nonce_correlation_not_attestation","deployment_stability":"caller_responsibility","unlisted_launchers":"not_covered"}),
    )
}
