use chrono::{TimeZone, Utc};
use kpop_native::followup_store::Store;
use serde_json::{Value, json};
use std::{fs, path::{Path, PathBuf}, process::{Command, Output}};
use tempfile::TempDir;

struct Fixture {
    _temp: TempDir,
    workspace: PathBuf,
    state: PathBuf,
    _store: Store,
}

fn run(workspace: &Path, state: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_kpop"));
    command.args(args)
        .current_dir(workspace)
        .env("XDG_STATE_HOME", state)
        .env("TMPDIR", state)
        .env_remove("KPOPPER_RUNTIME")
        .env_remove("KPOPPER_NATIVE_RESOURCES")
        .env_remove("KPOPPER_SESSION_CONFIG");
    if workspace.join("resources").exists() {
        command.env("KPOPPER_NATIVE_RESOURCES", workspace.join("resources"))
            .env("KPOPPER_NATIVE_CACHE", state.join("cache"));
    }
    command.output().unwrap()
}

fn install_core_resources(workspace: &Path) {
    let resources = workspace.join("resources");
    let target = kpop_native::reasoning_runtime::target_name().unwrap();
    fs::create_dir_all(resources.join("reasoning")).unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(format!("{target}.kpopper-runtime")),
        resources.join("reasoning").join(format!("{target}.zip")),
    ).unwrap();
    let ordinary = resources.join("ordinary").join(&target);
    fs::create_dir_all(&ordinary).unwrap();
    let program = std::env::var_os("KPOP_TEST_ORDINARY_PROGRAM")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).unwrap())
                .join(".cache/kpopper/lean")
                .join(if cfg!(windows) { "windows-amd64" } else { &target })
                .join(env!("KPOP_ORDINARY_SOURCE_SHA256"))
        });
    for name in ["build.json", if cfg!(windows) { "epistemic-core.exe" } else { "epistemic-core" }] {
        fs::copy(program.join(name), ordinary.join(name)).unwrap();
    }
}

fn packet(workspace: &Path, state: &Path, args: &[&str]) -> Value {
    let output = run(workspace, state, args);
    assert!(output.status.success(), "{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn uncovered_pull_gets_conditional_discovery_without_inventing_a_policy() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("project");
    let state = temp.path().join("state");
    fs::create_dir(&workspace).unwrap();
    fs::create_dir(&state).unwrap();
    fs::write(workspace.join("PROVENANCE.yaml"),
        "meta: {name: Discovery fixture}\nknown:\n  fact.answer: {v: 1}\n").unwrap();
    let first = packet(&workspace, &state, &["--json", "pull", "fact.answer"]);
    assert!(first["output"].as_str().unwrap().starts_with("fact.answer: 1"));
    let discovery = &first["maintenance_discovery"];
    assert_eq!(discovery["subjects"], json!(["fact.answer"]));
    assert_eq!(discovery["kind"], "unestablished");
    assert_eq!(discovery["applicability"], "agent_task_assessment_required");
    assert_eq!(discovery["consent"], "none");
    assert!(discovery["response_obligation"]["matching_kind"]["pending_clock"]
        .as_str().unwrap().contains("clock-only"));
    assert!(discovery["response_obligation"]["matching_kind"]["changing_named_source"]
        .as_str().unwrap().contains("named-source"));

    let frozen = packet(&workspace, &state, &["--json", "--frozen", "pull", "fact.answer"]);
    assert!(frozen.get("maintenance_discovery").is_none());

    let choice_dir = state.join("kpopper/first-use/projects")
        .join(kpop_native::onboarding::project_key(&workspace.canonicalize().unwrap()));
    fs::create_dir_all(&choice_dir).unwrap();
    fs::write(choice_dir.join("maintenance-choice.json"),
        json!({"schema":1,"choice":{"state":"shown"}}).to_string()).unwrap();
    let suppressed = packet(&workspace, &state, &["--json", "pull", "fact.answer"]);
    assert!(suppressed.get("maintenance_discovery").is_none());
}

impl Fixture {
    fn new(core: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("project");
        let state = temp.path().join("state");
        fs::create_dir(&workspace).unwrap();
        fs::create_dir(&state).unwrap();
        let known = "known:\n  facts.count:\n    name: Count\n    v: 1\n  facts.other:\n    name: Other\n    v: 2\n";
        fs::write(workspace.join("PROVENANCE.yaml"), format!("meta:\n  name: Pull fixture\n{known}")).unwrap();
        let checked = run(&workspace, &state, &["check"]);
        assert!(checked.status.success(), "{}{}", String::from_utf8_lossy(&checked.stdout), String::from_utf8_lossy(&checked.stderr));
        let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
        let store = Store::at_in_state(&workspace, &state, now).unwrap();
        store.setup(None, "UTC", None, true).unwrap();
        let declaration = json!({
            "schema":"kpopper.maintenance-declaration/v1", "kind":"source", "id":"source-check",
            "title":"Inspect selected source", "why":"Keep evidence current.",
            "how":"Read selected source.", "scope":"Inspect only.", "related":["facts.count"],
            "cadence_days":1, "timezone":"UTC", "check_time":"09:00",
            "use_policy":"allow_cached_until_expiry", "evidence_requirement":"host_attested",
            "first_due_at":now.to_rfc3339(),
            "source":{"source_id":"fixture-source", "locator":"https://example.test/status",
                "publisher":"Example", "selection":"Current status", "adapter":"host-read/v1",
                "evidence_format":"selected text", "tool_policy":"existing authorized read tool",
                "allowed_roots":["https://example.test/"], "max_age_hours":24}
        });
        let proposal = kpop_native::maintenance_contract::compile(&declaration).unwrap();
        store.add(proposal["spec"].clone()).unwrap();
        let item = store.show("source-check").unwrap();
        store.record_maintenance_attempt(json!({"id":"source-check",
            "policy_digest":item["spec"]["maintenance"]["policy_digest"],
            "source_ref":item["spec"]["maintenance"]["source_ref"],
            "reason":"provider refused selected read", "evidence":"fixture://failure"})).unwrap();
        if core {
            fs::write(workspace.join("PROVENANCE.yaml"), format!("meta:\n  name: Pull fixture\n  reasoning: {{version: 1, profile: core/v1, requires: [arithmetic/v1]}}\n{known}")).unwrap();
            install_core_resources(&workspace);
            let checked = run(&workspace, &state, &["check"]);
            assert!(checked.status.success(), "{}{}", String::from_utf8_lossy(&checked.stdout), String::from_utf8_lossy(&checked.stderr));
        }
        Self { _temp: temp, workspace, state, _store: store }
    }

    fn pull(&self, args: &[&str]) -> Value { packet(&self.workspace, &self.state, args) }
}

#[test]
fn json_pull_preserves_outer_output_text_and_places_continuity_beside_it() {
    let fixture = Fixture::new(false);
    let covered = fixture.pull(&["--json", "pull", "facts.count"]);
    assert_eq!(covered["command"], "pull");
    assert!(covered["output"].as_str().unwrap().starts_with("facts.count: 1"));
    assert_eq!(covered["maintenance_continuity"]["subjects"], json!(["facts.count"]));
    assert_eq!(covered["maintenance_continuity"]["obligations"][0]["failure"], "provider refused selected read");
    let other = fixture.pull(&["--json", "pull", "facts.other"]);
    assert!(other.get("maintenance_continuity").is_none());
}

#[test]
fn prefix_pull_scopes_to_delivered_ids_and_respects_line_budget() {
    let fixture = Fixture::new(false);
    let prefix = fixture.pull(&["--json", "pull", "facts"]);
    assert!(prefix["output"].as_str().unwrap().contains("facts.count: 1"));
    assert_eq!(prefix["maintenance_continuity"]["subjects"], json!(["facts.count", "facts.other"]));
    let zero = fixture.pull(&["--json", "pull", "facts", "--budget", "0"]);
    assert!(!zero["output"].as_str().unwrap().contains("facts.count: 1"));
    assert!(zero.get("maintenance_continuity").is_none());
    assert!(zero.get("maintenance_discovery").is_none());
}

#[test]
fn corrupt_local_ledger_is_typed_unknown_in_delivered_pull() {
    let fixture = Fixture::new(false);
    let ledgers = fs::read_dir(fixture.state.join("kpopper/followups")).unwrap()
        .map(|entry| entry.unwrap().path().join("followups.yaml"))
        .filter(|path| path.is_file()).collect::<Vec<_>>();
    assert_eq!(ledgers.len(), 1);
    fs::write(&ledgers[0], "invalid: [\n").unwrap();
    let result = fixture.pull(&["--json", "pull", "facts.count"]);
    assert!(result["output"].as_str().unwrap().contains("facts.count: 1"));
    assert_eq!(result["maintenance_continuity"]["state"], "unknown");
    assert_eq!(result["maintenance_continuity"]["subjects"], json!(["facts.count"]));
}

#[test]
fn core_pull_delivers_scoped_continuity_without_changing_consumer_payload() {
    let fixture = Fixture::new(true);
    let result = fixture.pull(&["--json", "pull", "facts"]);
    let inner: Value = serde_json::from_str(result["output"].as_str().unwrap()).unwrap();
    assert_eq!(inner["profile"], "core/v1-consumer/v1");
    assert_eq!(inner["selection"], json!(["facts.count", "facts.other"]));
    assert_eq!(result["maintenance_continuity"]["subjects"], inner["selection"]);
    assert_eq!(result["maintenance_continuity"]["obligations"][0]["failure"], "provider refused selected read");
    let plain = run(&fixture.workspace, &fixture.state, &["pull", "facts.count"]);
    assert!(plain.status.success(), "{}", String::from_utf8_lossy(&plain.stderr));
    let inner: Value = serde_json::from_slice(&plain.stdout).unwrap();
    assert_eq!(inner["selection"], json!(["facts.count"]));
    assert_eq!(inner["maintenance_continuity"]["subjects"], json!(["facts.count"]));
}

#[test]
fn unrelated_nested_record_with_same_id_stays_quiet() {
    let fixture = Fixture::new(false);
    let other = fixture.workspace.join("other");
    fs::create_dir(&other).unwrap();
    fs::write(other.join("PROVENANCE.yaml"), "meta:\n  name: Other project\nknown:\n  facts.count: {v: 99}\n").unwrap();
    let result = packet(&other, &fixture.state, &["--json", "pull", "facts.count"]);
    assert!(result["output"].as_str().unwrap().contains("facts.count: 99"));
    assert!(result.get("maintenance_continuity").is_none());
}
