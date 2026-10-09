mod support;

use kpop_native::{
    pending_control::{self, Configure},
    pending_publication::Publisher,
    project_modes::Project,
    public_pending::{self, PublishOptions, StatusOptions},
    publication_provider::{Provider, PullRequest},
};
use serde_json::Value;
use std::{collections::BTreeMap, io::Write, path::Path, process::Stdio};
use support::pending::{fixture, git};

fn replace(value: &mut Value, replacements: &[(String, String)]) {
    match value {
        Value::String(text) => {
            for (from, to) in replacements {
                *text = text.replace(from, to);
            }
        }
        Value::Array(values) => values
            .iter_mut()
            .for_each(|value| replace(value, replacements)),
        Value::Object(values) => {
            let old = std::mem::take(values);
            for (mut key, mut value) in old {
                for (from, to) in replacements {
                    key = key.replace(from, to);
                }
                replace(&mut value, replacements);
                values.insert(key, value);
            }
        }
        _ => {}
    }
}

#[derive(Default)]
struct ProviderFixture {
    rows: Vec<PullRequest>,
    creates: usize,
    updates: usize,
    unavailable: bool,
    uncertain_create: bool,
    uncertain_before_create: bool,
    revoke_on_list: Option<Project>,
    remote: std::path::PathBuf,
}
impl Provider for ProviderFixture {
    fn list(&mut self, _scope: &Value) -> kpop_native::Result<Vec<PullRequest>> {
        if let Some(project) = self.revoke_on_list.take() {
            pending_control::configure(
                &project,
                &Configure {
                    remote: None,
                    target: None,
                    branch: None,
                    grant: false,
                    revoke: true,
                },
            )?;
        }
        if self.unavailable {
            return Err(kpop_native::Error("provider fixture offline".into()));
        }
        Ok(self.rows.clone())
    }
    fn create(
        &mut self,
        scope: &Value,
        _title: &str,
        body: &str,
        _request_id: &str,
    ) -> kpop_native::Result<PullRequest> {
        self.creates += 1;
        if self.uncertain_before_create {
            return Err(kpop_native::Error(
                "fixture disconnected without a confirmed create".into(),
            ));
        }
        let head = String::from_utf8(git(
            &self.remote,
            &[
                "rev-parse",
                &format!("refs/heads/{}", scope["branch"].as_str().unwrap()),
            ],
            None,
        ))
        .unwrap()
        .trim()
        .to_owned();
        let row = PullRequest {
            id: (self.rows.len() + 1).to_string(),
            state: "open".into(),
            url: format!("fixture://pr/{}", self.rows.len() + 1),
            head,
            body: body.into(),
        };
        self.rows.push(row.clone());
        if self.uncertain_create {
            self.uncertain_create = false;
            return Err(kpop_native::Error(
                "fixture lost create response after remote side effect".into(),
            ));
        }
        Ok(row)
    }
    fn update(
        &mut self,
        scope: &Value,
        id: &str,
        _title: &str,
        body: &str,
    ) -> kpop_native::Result<PullRequest> {
        self.updates += 1;
        let head = String::from_utf8(git(
            &self.remote,
            &[
                "rev-parse",
                &format!("refs/heads/{}", scope["branch"].as_str().unwrap()),
            ],
            None,
        ))
        .unwrap()
        .trim()
        .to_owned();
        let row = self.rows.iter_mut().find(|row| row.id == id).unwrap();
        row.body = body.into();
        row.head = head;
        Ok(row.clone())
    }
}

fn named<'a>(cases: &'a Value, name: &str) -> &'a Value {
    cases
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == name)
        .unwrap()
}

fn setup() -> (tempfile::TempDir, Project, ProviderFixture, String) {
    setup_target(TargetRecord::Plain)
}

/// What the configured target holds at the record path before publication.
#[derive(Clone, Copy, PartialEq)]
enum TargetRecord {
    /// The fixture's ordinary record.
    Plain,
    /// No record: neither the target nor the checkout has one yet.
    Absent,
    /// The fixture's record already converted to compact history.
    Compact,
}

/// A target commit with fixed dates, so independent fixtures share its identity.
fn commit_fixed(root: &Path, message: &str) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["commit", "-q", "-m", message])
        .env("GIT_AUTHOR_DATE", "2026-09-25T00:00:00+00:00")
        .env("GIT_COMMITTER_DATE", "2026-09-25T00:00:00+00:00")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn setup_target(kind: TargetRecord) -> (tempfile::TempDir, Project, ProviderFixture, String) {
    let ledgers: Value = serde_json::from_str(include_str!("fixtures/pending-state.json")).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    let remote = temp.path().join("remote.git");
    std::fs::create_dir(&root).unwrap();
    fixture(&root, named(&ledgers, "one"));
    git(&root, &["config", "user.name", "Fixture"], None);
    git(
        &root,
        &["config", "user.email", "fixture@example.test"],
        None,
    );
    std::fs::write(root.join("app.txt"), b"target code\n").unwrap();
    match kind {
        TargetRecord::Plain => {
            git(&root, &["add", "GROUNDING.yaml", "app.txt"], None);
            git(&root, &["commit", "-m", "Target"], None);
        }
        TargetRecord::Absent => {
            std::fs::remove_file(root.join("GROUNDING.yaml")).unwrap();
            std::fs::create_dir_all(root.join(".kpopper")).unwrap();
            std::fs::write(root.join(".kpopper/measure.yaml"), b"measure: {}\n").unwrap();
            git(&root, &["add", "app.txt", ".kpopper/measure.yaml"], None);
            commit_fixed(&root, "Target");
        }
        TargetRecord::Compact => {
            let plan = kpop_native::history_node_import::Plan::prepare(
                Path::new("GROUNDING.yaml"),
                &root,
                kpop_native::history_migration::Options {
                    operation: "import-target".into(),
                    recorded_at: "2026-09-25T00:00:00+00:00".into(),
                    record_id: Some("record-target".into()),
                    read_mode: kpop_native::source_capture::ReadMode::Frozen,
                    route: false,
                    as_of: None,
                },
                None,
            )
            .unwrap();
            let files = plan.files().clone();
            std::fs::remove_file(root.join("GROUNDING.yaml")).unwrap();
            for (path, raw) in &files {
                let path = root.join(path);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, raw).unwrap();
            }
            git(&root, &["add", "-A"], None);
            commit_fixed(&root, "Target");
        }
    }
    git(
        temp.path(),
        &["init", "--bare", remote.to_str().unwrap()],
        None,
    );
    git(
        &root,
        &["remote", "add", "team", remote.to_str().unwrap()],
        None,
    );
    git(&root, &["push", "team", "trunk"], None);
    let project = Project::open(&root).unwrap();
    pending_control::configure(
        &project,
        &Configure {
            remote: Some("team"),
            target: Some("trunk"),
            branch: None,
            grant: true,
            revoke: false,
        },
    )
    .unwrap();
    let revision = "8887db9b513dddd4f3d7d26155713251effcfaa5e97a8edbcb1d7ced6d7297dd".into();
    let provider = ProviderFixture {
        remote,
        ..Default::default()
    };
    (temp, project, provider, revision)
}

fn remote_head(remote: &Path, branch: &str) -> Option<String> {
    let output = std::process::Command::new("git")
        .args([
            "-C",
            remote.to_str().unwrap(),
            "rev-parse",
            "--verify",
            &format!("refs/heads/{branch}"),
        ])
        .output()
        .unwrap();
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).unwrap().trim().to_owned())
}

fn decode_files(value: &Value) -> BTreeMap<String, Vec<u8>> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    value
        .as_object()
        .unwrap()
        .iter()
        .map(|(path, raw)| {
            (
                path.clone(),
                STANDARD.decode(raw.as_str().unwrap()).unwrap(),
            )
        })
        .collect()
}

#[path = "support/git_path.rs"]
mod git_path;

fn git_input(root: &Path, args: &[&str], input: &[u8], index: Option<&Path>) -> Vec<u8> {
    let mut command = std::process::Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", git_path::argument(index));
    }
    let mut child = command.spawn().unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn install_pending_bundle(root: &Path, bundle: &Value, files: &BTreeMap<String, Vec<u8>>) {
    let decoded = kpop_native::value::TypedValue::from_tagged(bundle).unwrap();
    let kpop_native::value::TypedValue::Map(value) = decoded else {
        panic!("bundle map")
    };
    let kpop_native::value::TypedValue::Text(revision) = &value["revision"] else {
        panic!("revision text")
    };
    let revision = revision.clone();
    let manifest = value["manifest"].clone();
    let mut images = BTreeMap::new();
    images.insert(
        format!("contributions/{revision}/manifest.json"),
        serde_json::to_vec(&manifest.to_tagged().unwrap()).unwrap(),
    );
    for (path, raw) in files {
        images.insert(
            format!("contributions/{revision}/evidence/{path}"),
            raw.clone(),
        );
    }
    images.insert(
        "events/00000000000000000001.json".into(),
        serde_json::to_vec(&serde_json::json!({
            "captured_at_ns":1,"contribution_id":"history","event_id":"history",
            "revision":revision,"sequence":1
        }))
        .unwrap(),
    );
    let index = root.join("pending-index");
    git_input(root, &["read-tree", "--empty"], b"", Some(&index));
    for (path, raw) in images {
        let oid = String::from_utf8(git_input(
            root,
            &["hash-object", "-w", "--stdin"],
            &raw,
            None,
        ))
        .unwrap();
        git_input(
            root,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("100644,{},{}", oid.trim(), path),
            ],
            b"",
            Some(&index),
        );
    }
    let tree = String::from_utf8(git_input(root, &["write-tree"], b"", Some(&index))).unwrap();
    let commit = String::from_utf8(git_input(
        root,
        &["commit-tree", tree.trim()],
        b"ledger\n",
        None,
    ))
    .unwrap();
    git(
        root,
        &[
            "update-ref",
            "refs/kpopper/pending_grounding",
            commit.trim(),
        ],
        None,
    );
    let _ = std::fs::remove_file(index);
}

#[test]
fn publish_creates_target_only_commit_and_verify_tracks_content_acceptance() {
    let (_temp, project, mut provider, revision) = setup();
    let root = project.root.clone();
    let checkout_head = remote_head(&provider.remote, "trunk").unwrap();
    let result = public_pending::publish_with_provider(
        &root,
        &PublishOptions {
            authorize: false,
            retry: true,
        },
        &mut provider,
        || 123.5,
    )
    .unwrap()
    .to_json()
    .unwrap();
    assert_eq!(result["outcome"], "proposed");
    assert_eq!(result["states"][&revision], "proposed");
    assert_eq!(provider.creates, 1);
    let proposed = remote_head(&provider.remote, "pending_grounding").unwrap();
    assert_ne!(proposed, checkout_head);
    assert_eq!(
        git(
            &provider.remote,
            &["show", "pending_grounding:app.txt"],
            None
        ),
        git(&provider.remote, &["show", "trunk:app.txt"], None),
    );
    assert!(
        String::from_utf8(git(
            &provider.remote,
            &["show", "pending_grounding:GROUNDING.yaml"],
            None
        ))
        .unwrap()
        .contains("api.limit")
    );
    assert_eq!(
        git(
            &provider.remote,
            &["show", "pending_grounding:evidence/vendor.txt"],
            None
        ),
        b"The limit is 10.\n"
    );
    let oracle: Value =
        serde_json::from_str(include_str!("fixtures/pending-publication-oracle.json")).unwrap();
    let mappings = vec![
        (
            oracle["revision"].as_str().unwrap().into(),
            revision.clone(),
        ),
        (
            oracle["proposed"]["ledger_ref"].as_str().unwrap().into(),
            result["ledger_ref"].as_str().unwrap().into(),
        ),
        (
            oracle["proposed"]["last_verified"]["target"]
                .as_str()
                .unwrap()
                .into(),
            checkout_head.clone(),
        ),
        (
            oracle["proposed"]["expected_head"].as_str().unwrap().into(),
            proposed.clone(),
        ),
        (
            oracle["proposed"]["last_verified"]["scope"]
                .as_str()
                .unwrap()
                .into(),
            result["last_verified"]["scope"].as_str().unwrap().into(),
        ),
    ];
    let mut expected_proposed = oracle["proposed"].clone();
    replace(&mut expected_proposed, &mappings);
    assert_eq!(result, expected_proposed);
    let actual_state: Value =
        serde_json::from_slice(&std::fs::read(project.state.join("publication.json")).unwrap())
            .unwrap();
    let mut expected_state: Value =
        serde_json::from_str(oracle["proposed_state"].as_str().unwrap()).unwrap();
    let expected_request = expected_state["receipts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|receipt| receipt["event"] == "create_intended")
        .unwrap()["request_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let actual_request = actual_state["receipts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|receipt| receipt["event"] == "create_intended")
        .unwrap()["request_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut state_mappings = mappings.clone();
    state_mappings.push((expected_request, actual_request));
    replace(&mut expected_state, &state_mappings);
    assert_eq!(actual_state, expected_state);

    let proof = Publisher::with_clock(project.clone(), &mut provider, || 123.5)
        .verify()
        .unwrap()
        .to_json()
        .unwrap();
    assert_eq!(proof["unresolved"], serde_json::json!([revision]));
    let mut expected_unresolved = oracle["unresolved"].clone();
    replace(&mut expected_unresolved, &mappings);
    assert_eq!(proof, expected_unresolved);
    let status = public_pending::status_with_provider(
        &root,
        &StatusOptions { verify: true },
        &mut provider,
        || 123.5,
    )
    .unwrap()
    .to_json()
    .unwrap();
    assert_eq!(
        status["verification"]["unresolved"],
        serde_json::json!([revision])
    );

    let tree = String::from_utf8(git(
        &provider.remote,
        &["rev-parse", "pending_grounding^{tree}"],
        None,
    ))
    .unwrap();
    let target = remote_head(&provider.remote, "trunk").unwrap();
    let commit = String::from_utf8(git(
        &provider.remote,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "commit-tree",
            tree.trim(),
            "-p",
            &target,
        ],
        Some(b"accept\n"),
    ))
    .unwrap();
    git(
        &provider.remote,
        &["update-ref", "refs/heads/trunk", commit.trim(), &target],
        None,
    );
    git(
        &provider.remote,
        &[
            "update-ref",
            "-d",
            "refs/heads/pending_grounding",
            &proposed,
        ],
        None,
    );
    provider.rows[0].state = "merged".into();
    let proof = Publisher::with_clock(project, &mut provider, || 123.5)
        .verify()
        .unwrap()
        .to_json()
        .unwrap();
    assert_eq!(proof["unresolved"], serde_json::json!([]));
    assert_eq!(proof["terminal"][revision], "accepted");
    assert_eq!(provider.creates, 1);
    assert!(root.join("GROUNDING.yaml").exists());
    let mut verified_mappings = mappings;
    verified_mappings.push((
        oracle["verified"]["target"].as_str().unwrap().into(),
        commit.trim().into(),
    ));
    let mut expected_verified = oracle["verified"].clone();
    replace(&mut expected_verified, &verified_mappings);
    assert_eq!(proof, expected_verified);
}

#[test]
fn publish_without_grant_is_idle_and_does_not_push_or_create() {
    let (_temp, project, mut provider, _revision) = setup();
    pending_control::configure(
        &project,
        &Configure {
            remote: None,
            target: None,
            branch: None,
            grant: false,
            revoke: true,
        },
    )
    .unwrap();
    let before = remote_head(&provider.remote, "pending_grounding");
    let result = Publisher::with_clock(project.clone(), &mut provider, || 123.5)
        .run(false, true)
        .unwrap()
        .to_json()
        .unwrap();
    assert_eq!(result["outcome"], "idle");
    assert_eq!(remote_head(&provider.remote, "pending_grounding"), before);
    assert_eq!(provider.creates, 0);
    let result = Publisher::with_clock(project, &mut provider, || 123.5)
        .run(true, true)
        .unwrap()
        .to_json()
        .unwrap();
    assert_eq!(result["outcome"], "proposed");
    assert!(remote_head(&provider.remote, "pending_grounding").is_some());
    assert_eq!(provider.creates, 1);
}

#[test]
fn provider_failure_is_unknown_and_enters_bounded_backoff() {
    let (_temp, project, mut provider, _revision) = setup();
    provider.unavailable = true;
    let result = Publisher::with_clock(project.clone(), &mut provider, || 123.5)
        .run(false, true)
        .unwrap()
        .to_json()
        .unwrap();
    assert_eq!(result["outcome"], "unknown");
    assert_eq!(result["failures"], 1);
    assert_eq!(result["retry_at"], 125.5);
    let result = Publisher::with_clock(project, &mut provider, || 123.5)
        .run(false, false)
        .unwrap()
        .to_json()
        .unwrap();
    assert_eq!(result["outcome"], "backoff");
    assert_eq!(provider.creates, 0);
}

#[test]
fn uncertain_create_is_reconciled_without_a_duplicate() {
    let (_temp, project, mut provider, revision) = setup();
    provider.uncertain_create = true;
    let result = Publisher::with_clock(project.clone(), &mut provider, || 123.5)
        .run(false, true)
        .unwrap()
        .to_json()
        .unwrap();
    assert_eq!(result["outcome"], "unknown");
    assert_eq!(result["intent"]["kind"], "create");
    let result = Publisher::with_clock(project, &mut provider, || 123.5)
        .run(false, true)
        .unwrap()
        .to_json()
        .unwrap();
    assert_eq!(result["states"][revision], "proposed");
    assert_eq!(provider.creates, 1);
}

#[test]
fn uncertain_create_without_remote_evidence_is_never_repeated() {
    let (_temp, project, mut provider, _revision) = setup();
    provider.uncertain_before_create = true;
    let result = Publisher::with_clock(project.clone(), &mut provider, || 123.5)
        .run(false, true)
        .unwrap()
        .to_json()
        .unwrap();
    assert_eq!(result["outcome"], "unknown");
    for _ in 0..3 {
        let result = Publisher::with_clock(project.clone(), &mut provider, || 123.5)
            .run(false, true)
            .unwrap()
            .to_json()
            .unwrap();
        assert_eq!(result["outcome"], "unknown");
    }
    assert_eq!(provider.creates, 1);
}

#[test]
fn unexpected_managed_head_is_attention_and_never_overwritten() {
    let (_temp, project, mut provider, _revision) = setup();
    Publisher::with_clock(project.clone(), &mut provider, || 123.5)
        .run(false, true)
        .unwrap();
    let expected = remote_head(&provider.remote, "pending_grounding").unwrap();
    let target = remote_head(&provider.remote, "trunk").unwrap();
    git(
        &provider.remote,
        &[
            "update-ref",
            "refs/heads/pending_grounding",
            &target,
            &expected,
        ],
        None,
    );
    let result = Publisher::with_clock(project, &mut provider, || 123.5)
        .run(false, true)
        .unwrap()
        .to_json()
        .unwrap();
    assert_eq!(result["outcome"], "attention");
    assert_eq!(
        result["detail"],
        "remote managed head changed; it was not overwritten"
    );
    assert_eq!(
        remote_head(&provider.remote, "pending_grounding"),
        Some(target)
    );
    assert_eq!(provider.creates, 1);
}

#[test]
fn real_cli_publish_rejects_non_github_provider_before_any_push() {
    let (_temp, project, provider, _revision) = setup();
    let before = remote_head(&provider.remote, "pending_grounding");
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_kpop"))
        .args([
            "--workspace",
            project.root.to_str().unwrap(),
            "pending",
            "publish",
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let value: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(value["outcome"], "attention");
    assert_eq!(
        value["detail"],
        "configure a supported GitHub repository URL"
    );
    assert_eq!(remote_head(&provider.remote, "pending_grounding"), before);
}

#[test]
fn authority_revoked_after_reconciliation_blocks_push() {
    let (_temp, project, mut provider, _revision) = setup();
    provider.revoke_on_list = Some(project.clone());
    let result = Publisher::with_clock(project, &mut provider, || 123.5)
        .run(false, true)
        .unwrap()
        .to_json()
        .unwrap();
    assert_eq!(result["outcome"], "attention");
    assert_eq!(
        result["detail"],
        "publication authority was revoked or its destination changed"
    );
    assert!(remote_head(&provider.remote, "pending_grounding").is_none());
    assert_eq!(provider.creates, 0);
}

#[test]
fn status_verify_with_only_local_terminal_decisions_needs_no_provider() {
    let (_temp, project, _provider, revision) = setup();
    pending_control::action(
        &project,
        "withdraw",
        std::slice::from_ref(&revision),
        "local decision",
        None,
    )
    .unwrap();
    let value = kpop_native::public_pending::status(
        &project.root,
        &kpop_native::public_pending::StatusOptions { verify: true },
    )
    .unwrap()
    .to_json()
    .unwrap();
    assert_eq!(value["verification"]["unresolved"], serde_json::json!([]));
    assert_eq!(value["verification"]["terminal"][revision], "withdrawn");
}

#[test]
fn history_publication_unions_complete_same_authority_bytes() {
    history_publication_case(false, "v3-history-union");
}

#[test]
fn compact_publication_unions_complete_same_authority_and_verifies_after_landing() {
    history_publication_case(true, "v3-history-union");
}

#[test]
fn compact_publication_refuses_a_complete_foreign_authority_without_a_proposal() {
    history_publication_case(true, "v3-other-authority");
}

fn history_publication_case(compact: bool, case_name: &str) {
    let cases: Value =
        serde_json::from_str(include_str!("fixtures/pending-equivalence-oracle.json")).unwrap();
    let case = named(&cases, case_name);
    let source_files = decode_files(&case["files"]);
    let target_files = decode_files(&case["target_history"]);
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    let remote = temp.path().join("remote.git");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-b", "trunk"], None);
    git(&root, &["config", "user.name", "Fixture"], None);
    git(
        &root,
        &["config", "user.email", "fixture@example.test"],
        None,
    );
    for (name, raw) in target_files {
        let path = if name == "entry.yaml" {
            root.join("GROUNDING.yaml")
        } else if name == "authority.yaml" {
            root.join(".kpopper/history.yaml")
        } else if let Some(name) = name.strip_prefix("commits/") {
            root.join(".kpopper/history-commits").join(name)
        } else if let Some(name) = name.strip_prefix("cancellations/") {
            root.join(".kpopper/history-cancellations").join(name)
        } else if let Some(name) = name.strip_prefix("objects/") {
            root.join(".kpopper/history").join(name)
        } else {
            continue;
        };
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, raw).unwrap();
    }
    if compact {
        let converted = temp.path().join("converted");
        let plan = kpop_native::history_node_migration::Plan::prepare(&root.join("GROUNDING.yaml"))
            .unwrap();
        plan.publish(&converted).unwrap();
        std::fs::remove_dir_all(root.join(".kpopper")).unwrap();
        for (path, bytes) in plan.files() {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
    }
    git(&root, &["add", "."], None);
    git(&root, &["commit", "-m", "History target"], None);
    git(
        temp.path(),
        &["init", "--bare", remote.to_str().unwrap()],
        None,
    );
    git(
        &root,
        &["remote", "add", "team", remote.to_str().unwrap()],
        None,
    );
    git(&root, &["push", "team", "trunk"], None);
    install_pending_bundle(&root, &case["bundle"], &source_files);
    if compact {
        // Exercise the public full-v3 materialization boundary, which must route
        // before generic evidence copying and keep old control bytes archived.
        let bundle = kpop_native::value::TypedValue::from_tagged(&case["bundle"]).unwrap();
        let kpop_native::value::TypedValue::Map(bundle_fields) = &bundle else { panic!("bundle map") };
        let kpop_native::value::TypedValue::Text(revision) = &bundle_fields["revision"] else { panic!("revision text") };
        let snapshot = temp.path().join("materialized-full-history");
        let materialized = kpop_native::public_knowledge::materialize(
            &root,
            &kpop_native::public_knowledge::MaterializeOptions {
                revision: revision.clone(),
                out: snapshot.clone(),
                reference: None,
            },
        ).unwrap().to_json().unwrap();
        assert_eq!(materialized["state"], "materialized");
        let captured = kpop_native::history_node_capture::Capture::read(&snapshot).unwrap();
        for (name, raw) in &source_files {
            if !name.starts_with("history-closure/objects/") { continue; }
            let object = kpop_native::history_yaml::decode_document(raw).unwrap();
            let kpop_native::value::TypedValue::Map(fields) = &object else { panic!("object map") };
            let kpop_native::value::TypedValue::Text(subject) = &fields["subject"] else { panic!("subject text") };
            let kpop_native::value::TypedValue::Text(id) = &fields["id"] else { panic!("id text") };
            assert_eq!(captured.object(subject, id).unwrap(), object);
        }
    }
    let project = Project::open(&root).unwrap();
    pending_control::configure(
        &project,
        &Configure {
            remote: Some("team"),
            target: Some("trunk"),
            branch: None,
            grant: true,
            revoke: false,
        },
    )
    .unwrap();
    let mut provider = ProviderFixture {
        remote,
        ..Default::default()
    };
    let local_head = git(&root, &["rev-parse", "HEAD"], None);
    let local_index = git(&root, &["write-tree"], None);
    let local_record = std::fs::read(root.join("GROUNDING.yaml")).unwrap();
    let result = Publisher::with_clock(project, &mut provider, || 123.5)
        .run(false, true)
        .unwrap()
        .to_json()
        .unwrap();
    if case_name == "v3-other-authority" {
        assert_eq!(result["outcome"], "attention", "{result}");
        assert!(
            result["detail"].as_str().unwrap().contains("authority"),
            "{result}"
        );
        assert_eq!(provider.creates, 0);
        assert!(remote_head(&provider.remote, "pending_grounding").is_none());
        assert_eq!(git(&root, &["rev-parse", "HEAD"], None), local_head);
        assert_eq!(git(&root, &["write-tree"], None), local_index);
        assert_eq!(
            std::fs::read(root.join("GROUNDING.yaml")).unwrap(),
            local_record
        );
        return;
    }
    assert_eq!(result["outcome"], "proposed", "{result}");
    assert_eq!(provider.creates, 1);
    assert_eq!(git(&root, &["rev-parse", "HEAD"], None), local_head);
    assert_eq!(git(&root, &["write-tree"], None), local_index);
    assert_eq!(
        std::fs::read(root.join("GROUNDING.yaml")).unwrap(),
        local_record
    );

    let proposed = remote_head(&provider.remote, "pending_grounding").unwrap();
    let tree = String::from_utf8(git(
        &provider.remote,
        &["rev-parse", "pending_grounding^{tree}"],
        None,
    ))
    .unwrap();
    let target = remote_head(&provider.remote, "trunk").unwrap();
    let commit = String::from_utf8(git(
        &provider.remote,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "commit-tree",
            tree.trim(),
            "-p",
            &target,
        ],
        Some(b"accept history\n"),
    ))
    .unwrap();
    git(
        &provider.remote,
        &["update-ref", "refs/heads/trunk", commit.trim(), &target],
        None,
    );
    git(
        &provider.remote,
        &[
            "update-ref",
            "-d",
            "refs/heads/pending_grounding",
            &proposed,
        ],
        None,
    );
    provider.rows[0].state = "merged".into();
    let proof = Publisher::with_clock(Project::open(&root).unwrap(), &mut provider, || 123.5)
        .verify()
        .unwrap()
        .to_json()
        .unwrap();
    assert_eq!(proof["unresolved"], serde_json::json!([]), "{proof}");
    if compact {
        let landed = temp.path().join("landed");
        git(
            temp.path(),
            &[
                "clone",
                "--branch",
                "trunk",
                provider.remote.to_str().unwrap(),
                landed.to_str().unwrap(),
            ],
            None,
        );
        let captured = kpop_native::history_node_capture::Capture::read(&landed).unwrap();
        for (name, raw) in &source_files {
            if !name.starts_with("history-closure/objects/") {
                continue;
            }
            let object = kpop_native::history_yaml::decode_document(raw).unwrap();
            let kpop_native::value::TypedValue::Map(fields) = &object else {
                panic!("object map")
            };
            let kpop_native::value::TypedValue::Text(id) = &fields["id"] else {
                panic!("id text")
            };
            let kpop_native::value::TypedValue::Text(subject) = &fields["subject"] else {
                panic!("subject text")
            };
            assert_eq!(captured.object(subject, id).unwrap(), object);
        }
        assert!(
            String::from_utf8(std::fs::read(landed.join(".kpopper/history.yaml")).unwrap())
                .unwrap()
                .contains("node-history/v1")
        );
    }
}

/// Land the open proposal on the target branch, as a reviewer merging it would.
fn land(provider: &mut ProviderFixture) -> String {
    let proposed = remote_head(&provider.remote, "pending_grounding").unwrap();
    let tree = String::from_utf8(git(
        &provider.remote,
        &["rev-parse", "pending_grounding^{tree}"],
        None,
    ))
    .unwrap();
    let target = remote_head(&provider.remote, "trunk").unwrap();
    let commit = String::from_utf8(git(
        &provider.remote,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "commit-tree",
            tree.trim(),
            "-p",
            &target,
        ],
        Some(b"accept first record\n"),
    ))
    .unwrap();
    git(
        &provider.remote,
        &["update-ref", "refs/heads/trunk", commit.trim(), &target],
        None,
    );
    git(
        &provider.remote,
        &[
            "update-ref",
            "-d",
            "refs/heads/pending_grounding",
            &proposed,
        ],
        None,
    );
    provider.rows[0].state = "merged".into();
    commit.trim().to_owned()
}

#[test]
fn first_publication_without_a_target_record_proposes_compact_history_that_lands() {
    let (temp, project, mut provider, revision) = setup_target(TargetRecord::Absent);
    let root = project.root.clone();
    let local_head = git(&root, &["rev-parse", "HEAD"], None);
    let local_index = git(&root, &["write-tree"], None);
    let result = Publisher::with_clock(project.clone(), &mut provider, || 123.5)
        .run(false, true)
        .unwrap()
        .to_json()
        .unwrap();
    assert_eq!(result["outcome"], "proposed", "{result}");
    assert_eq!(result["states"][&revision], "proposed");
    assert_eq!(provider.creates, 1);
    // The checkout and its index are untouched; the record exists only in the proposal.
    assert_eq!(git(&root, &["rev-parse", "HEAD"], None), local_head);
    assert_eq!(git(&root, &["write-tree"], None), local_index);
    assert!(!root.join("GROUNDING.yaml").exists());
    assert!(!root.join(".kpopper/history.yaml").exists());

    let show = |path: &str| {
        git(
            &provider.remote,
            &["show", &format!("pending_grounding:{path}")],
            None,
        )
    };
    let marker = String::from_utf8(show(".kpopper/history.yaml")).unwrap();
    assert!(marker.contains("node-history/v1"), "{marker}");
    assert_eq!(show("evidence/vendor.txt"), b"The limit is 10.\n");
    assert_eq!(show("app.txt"), b"target code\n");
    assert_eq!(show(".kpopper/measure.yaml"), b"measure: {}\n");
    assert!(
        String::from_utf8(show(".gitattributes"))
            .unwrap()
            .contains("/GROUNDING.yaml -text")
    );
    // No legacy history authority, event or migration control was synthesized.
    let listed = String::from_utf8(git(
        &provider.remote,
        &["ls-tree", "-r", "--name-only", "pending_grounding"],
        None,
    ))
    .unwrap();
    assert!(
        !listed.lines().any(|path| {
            path.starts_with(".kpopper/history-commits/") && path.ends_with(".yaml")
                || path.starts_with(".kpopper-history-migration/")
                || path.starts_with(".kpopper/history/") && path.ends_with(".yaml")
        }),
        "{listed}"
    );

    // The proposal reads as compact history with the contribution's exact meaning:
    // profile, types, source and scope, never reinterpreted as core.
    let proposal = temp.path().join("proposal");
    git(
        temp.path(),
        &[
            "clone",
            "-q",
            "--branch",
            "pending_grounding",
            provider.remote.to_str().unwrap(),
            proposal.to_str().unwrap(),
        ],
        None,
    );
    let captured = kpop_native::history_node_capture::Capture::read(&proposal).unwrap();
    let document = captured.document().to_json().unwrap();
    assert_eq!(document["known"]["api.limit"]["v"], 10, "{document}");
    assert_eq!(document["known"]["api.limit"]["at"], "table 1");
    assert_eq!(document["known"]["api.limit"]["from"], "s.vendor");
    assert_eq!(
        document["known"]["api.limit"]["scope"],
        serde_json::json!({"environment": "API v2", "kind": "external"})
    );
    assert_eq!(
        document["sources"]["s.vendor"]["file"],
        "evidence/vendor.txt"
    );
    assert_ne!(document["meta"]["reasoning"]["profile"], "core/v1");

    // Landing is verified by the ordinary acceptance rule on the compact target.
    let landed_commit = land(&mut provider);
    let proof = Publisher::with_clock(project.clone(), &mut provider, || 123.5)
        .verify()
        .unwrap()
        .to_json()
        .unwrap();
    assert_eq!(proof["unresolved"], serde_json::json!([]), "{proof}");
    assert_eq!(proof["terminal"][&revision], "accepted", "{proof}");
    assert_eq!(provider.creates, 1);

    // The landed record supports ordinary compact reads and writes afterwards.
    let landed = temp.path().join("landed");
    git(
        temp.path(),
        &[
            "clone",
            "-q",
            "--branch",
            "trunk",
            provider.remote.to_str().unwrap(),
            landed.to_str().unwrap(),
        ],
        None,
    );
    assert_eq!(
        String::from_utf8(git(&landed, &["rev-parse", "HEAD"], None))
            .unwrap()
            .trim(),
        landed_commit
    );
    let options = kpop_native::history_authoring::Options {
        operation: "after-landing".into(),
        recorded_at: "2026-09-25T12:00:00+00:00".into(),
        recording_day: "2026-09-25".into(),
        by: kpop_native::value::TypedValue::Text("writer".into()),
        strict: false,
        paths: kpop_native::history_paths::Scheme::Hashed,
        receipt_version: None,
    };
    let action = kpop_native::value::TypedValue::from_json(&serde_json::json!({
        "kind": "add", "id": "api.retries",
        "body": {"v": 3, "scope": {"environment": "API v2", "kind": "external"}}
    }))
    .unwrap();
    let prepared =
        kpop_native::history_node_writer::prepare(&landed, &action, &options, None).unwrap();
    kpop_native::history_node_writer::publish(&landed, &prepared, None, |_| Ok(())).unwrap();
    let after = kpop_native::history_node_capture::Capture::read(&landed).unwrap();
    let document = after.document().to_json().unwrap();
    assert_eq!(document["known"]["api.retries"]["v"], 3, "{document}");
    assert_eq!(document["known"]["api.limit"]["v"], 10, "{document}");
}

#[test]
fn first_compact_publication_is_identical_when_retried_against_the_same_target() {
    let proposal = || {
        let (_temp, project, mut provider, _) = setup_target(TargetRecord::Absent);
        let result = Publisher::with_clock(project, &mut provider, || 123.5)
            .run(false, true)
            .unwrap()
            .to_json()
            .unwrap();
        assert_eq!(result["outcome"], "proposed", "{result}");
        let target = remote_head(&provider.remote, "trunk").unwrap();
        let tree = String::from_utf8(git(
            &provider.remote,
            &["rev-parse", "pending_grounding^{tree}"],
            None,
        ))
        .unwrap();
        (target, tree)
    };
    let first = proposal();
    let second = proposal();
    assert_eq!(first.0, second.0, "fixed target commits must match");
    assert_eq!(first.1, second.1, "retried proposal bytes differ");
}

#[test]
fn first_core_contribution_keeps_its_profile_in_a_new_compact_record() {
    use kpop_native::{history_authority::Files, value::TypedValue as V};
    let (temp, project, mut provider, _) = setup_target(TargetRecord::Absent);
    let scope = V::from_json(&serde_json::json!({"kind":"project", "environment":"fixture"})).unwrap();
    let document = V::from_json(&serde_json::json!({
        "meta":{"reasoning":{"version":2,"profile":"core/v1","requires":["arithmetic/v1"]}},
        "known":{"p.first":{"v":7,"scope":{"kind":"project","environment":"fixture"}}}
    })).unwrap();
    let bundle = kpop_native::pending_bundle::prepare(&document, &["p.first".into()], &scope, "project", &Files::new()).unwrap();
    install_pending_bundle(&project.root, &bundle.to_tagged().unwrap(), &Files::new());
    let result = Publisher::with_clock(project.clone(), &mut provider, || 123.5)
        .run(false, true).unwrap().to_json().unwrap();
    assert_eq!(result["outcome"], "proposed", "{result}");
    let proposal = temp.path().join("core-proposal");
    git(temp.path(), &["clone", "-q", "--branch", "pending_grounding", provider.remote.to_str().unwrap(), proposal.to_str().unwrap()], None);
    let captured = kpop_native::history_node_capture::Capture::read(&proposal).unwrap();
    let view = captured.document().to_json().unwrap();
    assert_eq!(view["meta"]["reasoning"]["profile"], "core/v1");
    assert_eq!(view["known"]["p.first"]["v"], 7);
    land(&mut provider);
    let proof = Publisher::with_clock(project, &mut provider, || 123.5).verify().unwrap().to_json().unwrap();
    assert_eq!(proof["unresolved"], serde_json::json!([]), "{proof}");
}

#[test]
fn a_plain_revision_is_named_when_the_target_already_holds_compact_history() {
    let (_temp, project, mut provider, revision) = setup_target(TargetRecord::Compact);
    let root = project.root.clone();
    let local_head = git(&root, &["rev-parse", "HEAD"], None);
    let local_index = git(&root, &["write-tree"], None);
    let result = Publisher::with_clock(project, &mut provider, || 123.5)
        .run(false, true)
        .unwrap()
        .to_json()
        .unwrap();
    assert_eq!(result["outcome"], "attention", "{result}");
    let detail = result["detail"].as_str().unwrap();
    assert!(
        detail.contains(&revision) && detail.contains("plain contribution"),
        "{detail}"
    );
    assert_eq!(provider.creates, 0);
    assert!(remote_head(&provider.remote, "pending_grounding").is_none());
    assert_eq!(git(&root, &["rev-parse", "HEAD"], None), local_head);
    assert_eq!(git(&root, &["write-tree"], None), local_index);
}

#[test]
fn first_publication_refuses_evidence_that_would_populate_record_control() {
    let (_temp, project, mut provider, _) = setup_target(TargetRecord::Absent);
    let root = project.root.clone();
    let scope = serde_json::json!({"environment": "workspace", "kind": "project"});
    let locator = ".kpopper/hypotheses/injected.yaml";
    let document = kpop_native::value::TypedValue::from_json(&serde_json::json!({
        "known": {"api.flag": {"v": 1, "scope": scope.clone(), "note": {"file": locator}}}
    }))
    .unwrap();
    let files = BTreeMap::from([(
        locator.to_owned(),
        b"hypothesis: {claim: injected}\nknown:\n  api.flag: {v: 9}\n".to_vec(),
    )]);
    let bundle = kpop_native::pending_bundle::prepare(
        &document,
        &["api.flag".to_owned()],
        &kpop_native::value::TypedValue::from_json(&scope).unwrap(),
        "project",
        &files,
    )
    .unwrap();
    install_pending_bundle(&root, &bundle.to_tagged().unwrap(), &files);
    let result = Publisher::with_clock(project, &mut provider, || 123.5)
        .run(false, true)
        .unwrap()
        .to_json()
        .unwrap();
    assert_eq!(result["outcome"], "attention", "{result}");
    assert!(
        result["detail"]
            .as_str()
            .unwrap()
            .contains("compact_publication_reserved_evidence"),
        "{result}"
    );
    assert_eq!(provider.creates, 0);
    assert!(remote_head(&provider.remote, "pending_grounding").is_none());
    assert!(!root.join(locator).exists());
}
