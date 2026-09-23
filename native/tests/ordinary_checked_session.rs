mod support;

use kpop_native::{
    ordinary_runtime::Program,
    reasoning_runtime::{OperationalBounds, target_name},
};
use serde_json::{Value as J, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use support::pending::fixture as pending_fixture;

const RECORD: &str = "sources:\n  s.note: {file: note.txt, read: 2026-09-20}\nknown:\n  p.a: {v: 12, from: s.note}\n  p.map:\n    v:\n      true: yes\n      1: one\njudgments:\n  d.keep:\n    wrong_if: 'p.a > 20'\n    seen: {p.a: 12}\n    verdict: Keep\n    rests_on: [p.a]\n";

fn copy_resources(root: &Path) {
    let target = target_name().unwrap();
    fs::create_dir_all(root.join("resources/reasoning")).unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(format!("{target}.kpopper-runtime")),
        root.join("resources/reasoning")
            .join(format!("{target}.zip")),
    )
    .unwrap();
    fs::create_dir_all(root.join("resources/ordinary").join(&target)).unwrap();
    let ordinary = std::env::var_os("KPOP_TEST_ORDINARY_PROGRAM")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap())
                .join(".cache/kpopper/lean")
                .join(&target)
                .join(env!("KPOP_ORDINARY_SOURCE_SHA256"))
        });
    for name in [
        "build.json",
        if cfg!(windows) {
            "epistemic-core.exe"
        } else {
            "epistemic-core"
        },
    ] {
        fs::copy(
            ordinary.join(name),
            root.join("resources/ordinary").join(&target).join(name),
        )
        .unwrap();
    }
}

fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("GROUNDING.yaml"), RECORD).unwrap();
    fs::write(root.path().join("note.txt"), "source evidence\n").unwrap();
    copy_resources(root.path());
    root
}

fn command(root: &Path, operation: &str) -> Command {
    let mut command = live_command(root, operation);
    command.arg("--frozen");
    command
}

fn live_command(root: &Path, operation: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_kpop"));
    command
        .args([
            "--workspace",
            root.to_str().unwrap(),
            "session",
            operation,
            "--no-settings",
            "--input",
            "GROUNDING.yaml",
            "--project",
            "fixture",
            "--state",
            "state",
            "--assessment-profile",
            "checked-reader/v1",
        ])
        .env("KPOPPER_NATIVE_RESOURCES", root.join("resources"))
        .env("KPOPPER_NATIVE_CACHE", root.join("cache"))
        .env_remove("KPOPPER_READ_MODE");
    command
}

fn ok(output: std::process::Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn revision(open: &str) -> String {
    open.lines()
        .find_map(|line| line.strip_prefix("project=fixture revision="))
        .unwrap()
        .into()
}

#[test]
fn ordinary_public_open_read_sources_keys_and_stale_inputs() {
    let temp = fixture();
    let root = temp.path();
    let open = ok(command(root, "open")
        .args(["--tokens", "8000"])
        .output()
        .unwrap());
    assert!(open.contains("d.keep"));
    let rev = revision(&open);
    let node = ok(command(root, "read")
        .args([
            "--ref",
            "node:d.keep",
            "--revision",
            &rev,
            "--tokens",
            "8000",
        ])
        .output()
        .unwrap());
    let node: J = serde_json::from_str(&node).unwrap();
    assert_eq!(node["value"]["checked_bundle_ref"], "checked:d.keep");
    assert!(
        node["value"]["epistemic_card"]
            .as_str()
            .unwrap()
            .contains("RECORDED CLAIM d.keep")
    );
    let scalar = ok(command(root, "read")
        .args([
            "--ref",
            "node:p.map#/body/v",
            "--revision",
            &rev,
            "--tokens",
            "8000",
        ])
        .output()
        .unwrap());
    let scalar: J = serde_json::from_str(&scalar).unwrap();
    assert_eq!(scalar["value"], json!({"true":"one"}));
    let source = ok(command(root, "read")
        .args([
            "--ref",
            "source:record",
            "--revision",
            &rev,
            "--tokens",
            "8000",
        ])
        .output()
        .unwrap());
    let source: J = serde_json::from_str(&source).unwrap();
    assert_eq!(source["value"]["text"], RECORD);
    fs::write(
        root.join("GROUNDING.yaml"),
        RECORD.replace("v: 12", "v: 13"),
    )
    .unwrap();
    let stale = command(root, "read")
        .args(["--ref", "node:p.a", "--revision", &rev, "--tokens", "8000"])
        .output()
        .unwrap();
    assert_eq!(stale.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&stale.stderr).contains("reopen"));
}

#[test]
fn ordinary_verify_claims_returns_acceptance_and_rejected_checks() {
    let temp = fixture();
    let root = temp.path();
    let rev = revision(&ok(command(root, "open")
        .args(["--tokens", "8000"])
        .output()
        .unwrap()));
    let input=[json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}),json!({"jsonrpc":"2.0","method":"notifications/initialized"}),json!({"jsonrpc":"2.0","id":"accepted","method":"tools/call","params":{"name":"kpopper_verify_claims","arguments":{"judgment":"d.keep","revision":rev,"assertions":[{"kind":"current","id":"p.a","expected":12}]}}}),json!({"jsonrpc":"2.0","id":"rejected","method":"tools/call","params":{"name":"kpopper_verify_claims","arguments":{"judgment":"d.keep","revision":rev,"assertions":[{"kind":"current","id":"p.a","expected":99}]}}})].into_iter().map(|value|value.to_string()+"\n").collect::<String>();
    let mut child = command(root, "serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = ok(child.wait_with_output().unwrap());
    let messages = output
        .lines()
        .map(|line| serde_json::from_str::<J>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(messages[1]["result"]["isError"], false);
    assert_eq!(
        serde_json::from_str::<J>(
            messages[1]["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
        )
        .unwrap()["accepted"],
        true
    );
    assert_eq!(messages[2]["result"]["isError"], true);
    assert!(
        messages[2]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("accepted")
    );
}

#[test]
fn ordinary_search_context_and_proposals_are_revision_bound_and_idempotent() {
    let temp = fixture();
    let root = temp.path();
    let revision = revision(&ok(command(root, "open")
        .args(["--tokens", "8000"])
        .output()
        .unwrap()));
    let search: J = serde_json::from_str(&ok(command(root, "search")
        .args([
            "--query",
            "p.a",
            "--revision",
            &revision,
            "--tokens",
            "8000",
        ])
        .output()
        .unwrap()))
    .unwrap();
    assert_eq!(search["hits"][0]["id"], "p.a");
    let context: J = serde_json::from_str(&ok(command(root, "context")
        .args([
            "--id",
            "d.keep",
            "--direction",
            "support",
            "--revision",
            &revision,
            "--tokens",
            "8000",
        ])
        .output()
        .unwrap()))
    .unwrap();
    assert_eq!(context["reads"].as_array().unwrap().len(), 2);
    let proposal_args = [
        "--revision",
        &revision,
        "--kind",
        "question",
        "--text",
        "Later?",
    ];
    let first = ok(command(root, "propose")
        .args(proposal_args)
        .output()
        .unwrap());
    let first: J = serde_json::from_str(&first).unwrap();
    let proposal = root
        .join("state")
        .join(format!("proposal-{}.json", first["id"].as_str().unwrap()));
    let held = fs::read(&proposal).unwrap();
    let second: J = serde_json::from_str(&ok(command(root, "propose")
        .args(proposal_args)
        .output()
        .unwrap()))
    .unwrap();
    assert_eq!(second, first);
    assert_eq!(fs::read(&proposal).unwrap(), held);
    let retained: J = serde_json::from_str(&ok(command(root, "read")
        .args([
            "--ref",
            first["read"].as_str().unwrap(),
            "--revision",
            &revision,
            "--tokens",
            "8000",
        ])
        .output()
        .unwrap()))
    .unwrap();
    assert_eq!(retained["value"]["id"], first["id"]);
    assert_eq!(retained["value"]["stale_base"], false);
    let fresh_open = ok(command(root, "open").output().unwrap());
    assert!(fresh_open.contains("pending=1 stale=0; native hypotheses=0"));
    let before = fs::read_dir(root.join("state")).unwrap().count();
    fs::write(
        root.join("GROUNDING.yaml"),
        RECORD.replace("v: 12", "v: 13"),
    )
    .unwrap();
    let stale = command(root, "propose")
        .args(proposal_args)
        .output()
        .unwrap();
    assert_eq!(stale.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&stale.stderr).contains("reopen"));
    assert_eq!(fs::read_dir(root.join("state")).unwrap().count(), before);
    let stale_open = ok(command(root, "open").output().unwrap());
    assert!(stale_open.contains("pending=1 stale=1; native hypotheses=0"));
}

#[test]
fn ordinary_default_open_folds_and_advertised_handles_are_readable() {
    let temp = fixture();
    let root = temp.path();
    let more = (0..40)
        .map(|i| format!("  p.extra{i:02}: {{v: {i}, from: s.note}}\n"))
        .collect::<String>();
    let record = RECORD
        .replace("  p.map:", &(more + "  p.map:"))
        .replace("seen: {p.a: 12}", "seen: {p.a: 5}")
        + "open:\n  q.later: {text: 'Which evidence is missing?'}\n";
    fs::write(root.join("GROUNDING.yaml"), record).unwrap();
    let opened = ok(command(root, "open").output().unwrap());
    assert!(kpop_native::tokenizer::Encoding::O200kBase.count(&opened) <= 700);
    assert!(opened.contains("+ /known/p [42] source_count=41"));
    assert!(opened.contains("p.a seen=5 current=12"));
    assert!(opened.contains("questions=1"));
    assert!(opened.contains("review=1"));
    let revision = revision(&opened);
    for (reference, expected) in [
        ("/known/p", "+ /known/p [42] source_count=41"),
        ("links:/known/p", "+ links:/known/p [41] source_count=41"),
    ] {
        let text = ok(command(root, "read")
            .args([
                "--ref",
                reference,
                "--revision",
                &revision,
                "--tokens",
                "100",
            ])
            .output()
            .unwrap());
        assert!(text.contains(expected));
        assert!(kpop_native::tokenizer::Encoding::O200kBase.count(&text) <= 100);
    }
    for reference in [
        "orientation",
        "p.a",
        "p.map#/body/v",
        "node:d.keep#/state_tags_ref",
        "node:d.keep#/state_tags_scope",
    ] {
        let response: J = serde_json::from_str(&ok(command(root, "read")
            .args(["--ref", reference, "--revision", &revision])
            .output()
            .unwrap()))
        .unwrap();
        assert_eq!(response["complete"], true);
    }
    let source = command(root, "read")
        .args(["--ref", "source:p.a", "--revision", &revision])
        .output()
        .unwrap();
    assert_eq!(source.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&source.stderr)
            .contains("this is a record entry; read node:p.a with this revision")
    );
}

#[test]
fn ordinary_live_open_reports_each_local_contribution() {
    let ledgers: J = serde_json::from_str(include_str!("fixtures/pending-state.json")).unwrap();
    let scope = r#"{"environment":"API v2","kind":"external"}"#;
    for (case, expected) in [
        (
            "one",
            vec![format!(
                "PENDING 8887db9b513d captured locally {scope} @native"
            )],
        ),
        (
            "two",
            vec![
                format!("PENDING 8887db9b513d captured locally {scope} @native"),
                format!("PENDING b5bea064401e captured locally {scope} @native"),
            ],
        ),
        // A withdrawn contribution is no longer a hypothesis; its status is still reported.
        (
            "retired",
            vec![format!(
                "PENDING 8887db9b513d accepted (last observed) {scope} @native"
            )],
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        pending_fixture(
            &root,
            ledgers
                .as_array()
                .unwrap()
                .iter()
                .find(|ledger| ledger["name"] == case)
                .unwrap(),
        );
        copy_resources(&root);
        let opened = ok(live_command(&root, "open").output().unwrap());
        let pending = opened
            .lines()
            .filter(|line| line.starts_with("PENDING "))
            .collect::<Vec<_>>();
        assert_eq!(pending, expected, "{case}");
        let frozen = ok(command(&root, "open").output().unwrap());
        assert!(!frozen.contains("PENDING "), "{case}");
    }
}

#[test]
fn ordinary_read_directories_preserve_authored_order_and_scalar_key_identity() {
    let fixture_data: J =
        serde_json::from_str(include_str!("fixtures/ordinary-session-view-oracle.json")).unwrap();
    for case in fixture_data["source_order_cases"].as_array().unwrap() {
        let temp = fixture();
        let root = temp.path();
        fs::write(
            root.join("GROUNDING.yaml"),
            case["record"].as_str().unwrap(),
        )
        .unwrap();
        if let Some(profile) = case["profile_text"].as_str() {
            fs::create_dir_all(root.join(".kpopper")).unwrap();
            fs::write(root.join(".kpopper/session.json"), profile).unwrap();
        }
        if let Some(hypothesis) = case["hypothesis"].as_str() {
            fs::create_dir_all(root.join(".kpopper/hypotheses")).unwrap();
            fs::write(root.join(".kpopper/hypotheses/candidate.yaml"), hypothesis).unwrap();
        }
        for (filename, contents) in case["extra_hypotheses"].as_object().unwrap() {
            fs::write(
                root.join(".kpopper/hypotheses").join(filename),
                contents.as_str().unwrap(),
            )
            .unwrap();
        }
        let opened = ok(command(root, "open").output().unwrap());
        let revision = revision(&opened);
        for (reference, keys) in case["directories"].as_object().unwrap() {
            let response: J = serde_json::from_str(&ok(command(root, "read")
                .args([
                    "--ref",
                    reference,
                    "--revision",
                    &revision,
                    "--tokens",
                    "300",
                ])
                .output()
                .unwrap()))
            .unwrap();
            assert_eq!(response["complete"], false);
            let prefix = format!(
                "{reference}{}/",
                if reference.contains('#') { "" } else { "#" }
            );
            let expected = keys
                .as_array()
                .unwrap()
                .iter()
                .map(|key| {
                    format!(
                        "{prefix}{}",
                        key.as_str().unwrap().replace('~', "~0").replace('/', "~1")
                    )
                })
                .collect::<Vec<_>>();
            assert_eq!(
                response["children"],
                json!(expected),
                "{} {reference}",
                case["name"]
            );
        }
        let response: J = serde_json::from_str(&ok(command(root, "read")
            .args([
                "--ref",
                "node:p.map#/body/v",
                "--revision",
                &revision,
                "--tokens",
                "65536",
            ])
            .output()
            .unwrap()))
        .unwrap();
        assert_eq!(response["value"], case["value"], "{}", case["name"]);
        for (name, expected) in case["hypothesis_values"].as_object().unwrap() {
            let reference = format!("native#/{name}");
            let mut response: J = serde_json::from_str(&ok(command(root, "read")
                .args([
                    "--ref",
                    &reference,
                    "--revision",
                    &revision,
                    "--tokens",
                    "65536",
                ])
                .output()
                .unwrap()))
            .unwrap();
            response["value"]["path"] = json!("PATH");
            assert_eq!(response["value"], *expected, "hypothesis {name}");
        }
    }
}

#[test]
fn ordinary_runtime_retains_exit_two_but_compute_still_refuses_it() {
    let temp = fixture();
    let root = temp.path();
    let target = target_name().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let _ = cache;
    let program = Program::open(&root.join("resources/ordinary").join(target)).unwrap();
    let graph = json!({"nodes":{"p.a":{"kind":"known","states":[],"body":{"v":12}},"d.keep":{"kind":"judgment","states":[],"body":{"verdict":"Keep","rests_on":["p.a"],"seen":{"p.a":12},"wrong_if":"p.a > 20"},"assessment_body":{"verdict":"Keep","rests_on":["p.a"],"seen":{"p.a":12},"wrong_if":"p.a > 20"},"assessment_fields":{"deps":"rests_on","snapshot":"seen","predicate":"wrong_if"}}},"edges":[],"topics":{"p.a":["known","p"],"d.keep":["judgments","d"]}});
    let rejected = program
        .assess(
            &graph,
            "d.keep",
            &[json!({"kind":"current","id":"p.a","expected":99})],
            &OperationalBounds::default(),
        )
        .unwrap();
    assert_eq!(rejected["assertions_accepted"], false);
    let legacy = json!({"record":graph,"id":"d.keep","assertions":[{"kind":"current","id":"p.a","expected":99}]});
    assert_eq!(
        program
            .request(&legacy, &OperationalBounds::default())
            .unwrap_err()
            .0,
        "native reasoning process failed"
    );
}

#[test]
#[ignore = "requires immutable Python oracle; set KPOP_PYTHON_SESSION_ORACLE and KPOP_PYTHON_SESSION_ORACLE_SCRIPT"]
fn pinned_python_oracle_matches_native_open_read_and_verify_packets() {
    let python = std::env::var_os("KPOP_PYTHON_SESSION_ORACLE")
        .expect("set KPOP_PYTHON_SESSION_ORACLE to the pinned Python interpreter");
    let script = std::env::var_os("KPOP_PYTHON_SESSION_ORACLE_SCRIPT")
        .expect("set KPOP_PYTHON_SESSION_ORACLE_SCRIPT to the immutable oracle driver");
    let temp = fixture();
    let root = temp.path();
    let oracle = Command::new(python)
        .arg(script)
        .arg(root.join("GROUNDING.yaml"))
        .arg(root.join("python-state"))
        .output()
        .unwrap();
    assert!(
        oracle.status.success(),
        "{}",
        String::from_utf8_lossy(&oracle.stderr)
    );
    let oracle: J = serde_json::from_slice(&oracle.stdout).unwrap();
    let open = ok(command(root, "open")
        .args(["--tokens", "65536"])
        .output()
        .unwrap());
    let revision = revision(&open);
    let oracle_revision = oracle["revision"].as_str().unwrap();
    assert_eq!(
        open.replace(&revision, "REVISION"),
        oracle["open"]
            .as_str()
            .unwrap()
            .replace(oracle_revision, "REVISION")
    );
    assert!(open.contains("d.keep"));
    for row in oracle["budget_openings"]
        .as_array()
        .expect("oracle driver must include default and constrained budget openings")
    {
        let budget = row["budget"].to_string();
        let output = command(root, "open")
            .args(["--tokens", &budget])
            .output()
            .unwrap();
        if row.get("error").is_some() {
            assert_eq!(output.status.code(), Some(2));
            // Context revisions differ between runtimes and can have different
            // BPE lengths. Exact token/error parity is covered by frozen graphs.
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("minimum complete opening needs")
            );
        } else {
            assert_eq!(
                ok(output).replace(&revision, "REVISION"),
                row["text"]
                    .as_str()
                    .unwrap()
                    .replace(oracle_revision, "REVISION")
            );
        }
    }
    for (reference, expected) in oracle["handle_reads"]
        .as_object()
        .expect("oracle driver must include advertised handles")
    {
        let mut actual: J = serde_json::from_str(&ok(command(root, "read")
            .args([
                "--ref",
                reference,
                "--revision",
                &revision,
                "--tokens",
                "1600",
            ])
            .output()
            .unwrap()))
        .unwrap();
        let mut expected = expected.clone();
        actual["revision"] = json!("REVISION");
        expected["revision"] = json!("REVISION");
        assert_eq!(actual, expected, "{reference}");
    }
    let mut native: J = serde_json::from_str(&ok(command(root, "read")
        .args([
            "--ref",
            "node:d.keep",
            "--revision",
            &revision,
            "--tokens",
            "65536",
        ])
        .output()
        .unwrap()))
    .unwrap();
    native["revision"] = json!("REVISION");
    let mut expected = oracle["node"].clone();
    expected["revision"] = json!("REVISION");
    assert_eq!(native, expected);
    let mut native: J = serde_json::from_str(&ok(command(root, "read")
        .args([
            "--ref",
            "node:p.map#/body/v",
            "--revision",
            &revision,
            "--tokens",
            "65536",
        ])
        .output()
        .unwrap()))
    .unwrap();
    native["revision"] = json!("REVISION");
    let mut expected = oracle["scalar_key"].clone();
    expected["revision"] = json!("REVISION");
    assert_eq!(native, expected);
    let mut native: J = serde_json::from_str(&ok(command(root, "read")
        .args([
            "--ref",
            "source:record",
            "--revision",
            &revision,
            "--tokens",
            "65536",
        ])
        .output()
        .unwrap()))
    .unwrap();
    native["revision"] = json!("REVISION");
    let mut expected = oracle["source"].clone();
    expected["revision"] = json!("REVISION");
    assert_eq!(native, expected);

    let native_search_text = ok(command(root, "search")
        .args([
            "--query",
            "p.a",
            "--revision",
            &revision,
            "--tokens",
            "65536",
        ])
        .output()
        .unwrap());
    let mut native_search: J = serde_json::from_str(&native_search_text).unwrap();
    native_search["revision"] = json!("REVISION");
    let mut expected_search = oracle["search"].clone();
    expected_search["revision"] = json!("REVISION");
    assert_eq!(native_search, expected_search);
    let native_context_text = ok(command(root, "context")
        .args([
            "--id",
            "d.keep",
            "--direction",
            "support",
            "--revision",
            &revision,
            "--tokens",
            "65536",
        ])
        .output()
        .unwrap());
    let mut native_context: J = serde_json::from_str(&native_context_text).unwrap();
    native_context["revision"] = json!("REVISION");
    let mut expected_context = oracle["context"].clone();
    expected_context["revision"] = json!("REVISION");
    assert_eq!(native_context, expected_context);
    let native_proposal_text = ok(command(root, "propose")
        .args([
            "--revision",
            &revision,
            "--kind",
            "question",
            "--text",
            "Later?",
        ])
        .output()
        .unwrap());
    let native_proposal: J = serde_json::from_str(&native_proposal_text).unwrap();
    let mut normalized_proposal = native_proposal.clone();
    normalized_proposal["id"] = json!("PROPOSAL");
    normalized_proposal["read"] = json!("proposal:PROPOSAL");
    let mut expected_proposal = oracle["proposal"].clone();
    expected_proposal["id"] = json!("PROPOSAL");
    expected_proposal["read"] = json!("proposal:PROPOSAL");
    assert_eq!(normalized_proposal, expected_proposal);

    let input=[json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}),json!({"jsonrpc":"2.0","method":"notifications/initialized"}),json!({"jsonrpc":"2.0","id":"accepted","method":"tools/call","params":{"name":"kpopper_verify_claims","arguments":{"judgment":"d.keep","revision":revision,"assertions":[{"kind":"current","id":"p.a","expected":12}]}}}),json!({"jsonrpc":"2.0","id":"rejected","method":"tools/call","params":{"name":"kpopper_verify_claims","arguments":{"judgment":"d.keep","revision":revision,"assertions":[{"kind":"current","id":"p.a","expected":99}]}}})].into_iter().map(|value|value.to_string()+"\n").collect::<String>();
    let mut child = command(root, "serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = ok(child.wait_with_output().unwrap());
    let messages = output
        .lines()
        .map(|line| serde_json::from_str::<J>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(messages[1]["result"]["isError"], false);
    assert_eq!(
        serde_json::from_str::<J>(
            messages[1]["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
        )
        .unwrap(),
        oracle["accepted"]
    );
    assert_eq!(messages[2]["result"]["isError"], true);
    assert_eq!(
        serde_json::from_str::<J>(
            messages[2]["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
        )
        .unwrap(),
        oracle["rejected"]
    );

    let input=[json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}),json!({"jsonrpc":"2.0","method":"notifications/initialized"}),json!({"jsonrpc":"2.0","id":"search","method":"tools/call","params":{"name":"kpopper_search","arguments":{"query":"p.a","tokens":65536,"revision":revision}}}),json!({"jsonrpc":"2.0","id":"context","method":"tools/call","params":{"name":"kpopper_context","arguments":{"ids":["d.keep"],"direction":"support","tokens":65536,"revision":revision}}}),json!({"jsonrpc":"2.0","id":"propose","method":"tools/call","params":{"name":"kpopper_propose","arguments":{"revision":revision,"kind":"question","text":"Later?","basis":[],"revisit":""}}})].into_iter().map(|value|value.to_string()+"\n").collect::<String>();
    let mut child = command(root, "serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = ok(child.wait_with_output().unwrap());
    let messages = output
        .lines()
        .map(|line| serde_json::from_str::<J>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        messages[1]["result"],
        json!({"content":[{"type":"text","text":native_search_text}],"isError":false})
    );
    assert_eq!(
        messages[2]["result"],
        json!({"content":[{"type":"text","text":native_context_text}],"isError":false})
    );
    assert_eq!(
        messages[3]["result"],
        json!({"content":[{"type":"text","text":native_proposal_text.trim_end_matches('\n')}],"isError":false})
    );
    let mut mcp_search: J = serde_json::from_str(
        messages[1]["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    mcp_search["revision"] = json!("REVISION");
    assert_eq!(mcp_search, native_search);
    let mut mcp_context: J = serde_json::from_str(
        messages[2]["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    mcp_context["revision"] = json!("REVISION");
    assert_eq!(mcp_context, native_context);
    let mcp_proposal: J = serde_json::from_str(
        messages[3]["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(mcp_proposal, native_proposal);
    let fresh = ok(command(root, "open").output().unwrap());
    assert_eq!(
        fresh.replace(&revision, "REVISION"),
        oracle["fresh_open"]
            .as_str()
            .unwrap()
            .replace(oracle_revision, "REVISION")
    );
    fs::write(
        root.join("GROUNDING.yaml"),
        RECORD.replace("v: 12", "v: 13"),
    )
    .unwrap();
    let stale = ok(command(root, "open").output().unwrap());
    let stale_revision = stale
        .lines()
        .find_map(|line| line.strip_prefix("project=fixture revision="))
        .unwrap();
    assert_eq!(
        stale.replace(stale_revision, "REVISION"),
        oracle["stale_open"]
            .as_str()
            .unwrap()
            .replace(oracle["stale_revision"].as_str().unwrap(), "REVISION")
    );
}

fn live_read(root: &Path, reference: &str, revision: &str) -> J {
    serde_json::from_str(&ok(live_command(root, "read")
        .args([
            "--ref",
            reference,
            "--revision",
            revision,
            "--tokens",
            "8000",
        ])
        .output()
        .unwrap()))
    .unwrap()
}

// A live session reads the entries a pending contribution adds together with the
// record. The expected openings, nodes and source texts are the reference session's
// on the same ledgers.
#[test]
fn ordinary_live_session_reads_entries_pending_contributions_add() {
    let ledgers: J = serde_json::from_str(include_str!("fixtures/pending-state.json")).unwrap();
    let first = "8887db9b513dddd4f3d7d26155713251effcfaa5e97a8edbcb1d7ced6d7297dd";
    let second = "b5bea064401e23c2f7f3b95022324ff396479ea22a7663c3da58f7871d4c36cc";
    let scope = r#"{"environment":"API v2","kind":"external"}"#;
    let pending = |revision: &str| {
        format!(
            "PENDING {} captured locally {scope} @native",
            &revision[..12]
        )
    };
    let tail = "LINK MAP — folded endpoints; all links counted; exact links at links:/\napi.limit from s.vendor [1]\nCounts describe links: dependency_count=rests_on, source_count=from; not proof. Read node:ID, /topic, links:ID or @ref without @; pass revision.\n";
    let head = |contested: usize, events: usize| {
        format!(
            "record: 3 ids; 0 judgments; contested={contested}; changed=0; unreadable=0\nexecutable falsifiers: triggered=0 not_triggered=0 unknown=0 absent=0\nprose declarations (not evaluated): blocked_on=0 reopened_by=0 @conditions:/\ncurrent=recorded; seen=review snapshot. Not triggered does not mean verified.\nevents={events} @events:/\n"
        )
    };
    let limit = |v: u64| json!({"at":"table 1","from":"s.vendor","name":"Limit","scope":{"environment":"API v2","kind":"external"},"v":v});
    let vendor = json!({"file":"evidence/vendor.txt","name":"Vendor","read":"2026-09-14"});
    let local = json!({"body":{"v":1},"kind":"known","record_source":"record","states":[]});
    let added = |body: J, kind: &str, states: &[&str]| json!({"body":body,"kind":kind,"record_source":format!("contribution.{first}"),"states":states});
    let source = |v: u64| {
        format!(
            "known:\n  api.limit:\n    at: table 1\n    from: s.vendor\n    name: Limit\n    scope:\n      environment: API v2\n      kind: external\n    v: {v}\nsources:\n  s.vendor:\n    file: evidence/vendor.txt\n    name: Vendor\n    read: '2026-09-14'\n"
        )
    };
    let first_source = (
        first,
        source(10),
        "82b7aee5d4977f8750d8bb542c2f0d0001093d23817cbdfdd0d248237ed23763",
    );
    let second_source = (
        second,
        source(11),
        "6121e0b44aa4e0ae72dc66ec917ff1e0f92f2f6cea8c1377a5fedced8eb1fd59",
    );
    for (case, record, opening, nodes, sources) in [
        (
            "one",
            None,
            format!(
                "{}{}\nMAP / — declared navigation; names do not establish claims:\n@ /known/api\n- node:api.limit source_count=1\n@ /known/local\n- node:local.one\n@ /sources/s\n- node:s.vendor\n{tail}",
                head(0, 0),
                pending(first)
            ),
            [
                ("api.limit", added(limit(10), "known", &["pending"])),
                ("s.vendor", added(vendor.clone(), "sources", &["pending"])),
                ("local.one", local.clone()),
            ],
            vec![first_source.clone()],
        ),
        // The contributions disagree on api.limit: the first in ledger order supplies
        // its body, and the id is contested.
        (
            "two",
            None,
            format!(
                "{}{}\n{}\n@event:e1 api.limit CONTESTED\nMAP / — declared navigation; names do not establish claims:\n@ /known/api\n- node:api.limit CONTESTED=1 review=1 source_count=1\n@ /known/local\n- node:local.one\n@ /sources/s\n- node:s.vendor\n{tail}",
                head(1, 1),
                pending(first),
                pending(second)
            ),
            [
                (
                    "api.limit",
                    added(limit(10), "known", &["contested", "pending"]),
                ),
                ("s.vendor", added(vendor.clone(), "sources", &["pending"])),
                ("local.one", local.clone()),
            ],
            vec![first_source.clone(), second_source],
        ),
        // An id the record already holds keeps the record's body and is not pending.
        (
            "one",
            Some(
                "known:\n  local.one: {v: 1}\nsources:\n  s.vendor: {file: other.txt, name: Other vendor, read: 2026-09-01}\n",
            ),
            format!(
                "{}{}\n@event:e1 s.vendor CONTESTED\nMAP / — declared navigation; names do not establish claims:\n@ /known/api\n- node:api.limit source_count=1\n@ /known/local\n- node:local.one\n@ /sources/s\n- node:s.vendor CONTESTED=1 review=1\n{tail}",
                head(1, 1),
                pending(first)
            ),
            [
                ("api.limit", added(limit(10), "known", &["pending"])),
                (
                    "s.vendor",
                    json!({"body":{"file":"other.txt","name":"Other vendor","read":"2026-09-01"},"kind":"sources","record_source":"record","states":["contested"]}),
                ),
                ("local.one", local.clone()),
            ],
            vec![first_source.clone()],
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let ledger = ledgers
            .as_array()
            .unwrap()
            .iter()
            .find(|ledger| ledger["name"] == case)
            .unwrap();
        pending_fixture(&root, ledger);
        if let Some(record) = record {
            fs::write(root.join("GROUNDING.yaml"), record).unwrap();
        }
        copy_resources(&root);
        let opened = ok(live_command(&root, "open").output().unwrap());
        let revision = revision(&opened);
        assert_eq!(
            &opened[opened.find("record: ").unwrap()..],
            opening,
            "{case} {record:?}"
        );
        for (id, expected) in nodes {
            let node = live_read(&root, &format!("node:{id}#"), &revision);
            assert_eq!(node["value"], expected, "{case} {record:?} {id}");
        }
        for (contribution, text, sha256) in sources {
            let source = live_read(
                &root,
                &format!("source:contribution.{contribution}"),
                &revision,
            );
            assert_eq!(
                source["value"],
                json!({"text":text,"sha256":sha256,"location":format!("git:{}:{contribution}", ledger["head"].as_str().unwrap())}),
                "{case} {record:?} {contribution}"
            );
        }
        let search: J = serde_json::from_str(&ok(live_command(&root, "search")
            .args([
                "--query",
                "limit",
                "--revision",
                &revision,
                "--tokens",
                "8000",
            ])
            .output()
            .unwrap()))
        .unwrap();
        assert_eq!(search["hits"][0]["id"], "api.limit", "{case} {record:?}");
        let frozen = ok(command(&root, "open").output().unwrap());
        assert!(frozen.contains("record: "), "{case}");
        assert!(!frozen.contains("api.limit"), "{case} {record:?}");
    }
}

// The record itself cannot depend on a pending entry: `add` refuses such a judgment and
// `check` reports it. A judgment written by hand to rest on one still reads over the
// contributed entry in the live session, as it does in the reference session.
#[test]
fn ordinary_live_session_reads_record_judgments_over_pending_entries() {
    let ledgers: J = serde_json::from_str(include_str!("fixtures/pending-state.json")).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    pending_fixture(
        &root,
        ledgers
            .as_array()
            .unwrap()
            .iter()
            .find(|ledger| ledger["name"] == "one")
            .unwrap(),
    );
    fs::write(
        root.join("GROUNDING.yaml"),
        "known:\n  local.one: {v: 1}\njudgments:\n  d.limit:\n    rests_on: [api.limit]\n    wrong_if: api.limit > 20\n    seen: {api.limit: 10}\n    verdict: The limit is low enough\n",
    )
    .unwrap();
    copy_resources(&root);
    let opened = ok(live_command(&root, "open").output().unwrap());
    assert_eq!(
        &opened[opened.find("record: ").unwrap()..],
        "record: 4 ids; 1 judgments; contested=0; changed=0; unreadable=0\nexecutable falsifiers: triggered=0 not_triggered=1 unknown=0 absent=0\nprose declarations (not evaluated): blocked_on=0 reopened_by=0 @conditions:/\ncurrent=recorded; seen=review snapshot. Not triggered does not mean verified.\nevents=0 @events:/\nPENDING 8887db9b513d captured locally {\"environment\":\"API v2\",\"kind\":\"external\"} @native\nMAP / — declared navigation; names do not establish claims:\n@ /judgments/d\n- node:d.limit dependency_count=1\n@ /known/api\n- node:api.limit source_count=1\n@ /known/local\n- node:local.one\n@ /sources/s\n- node:s.vendor\nLINK MAP — folded endpoints; all links counted; exact links at links:/\napi.limit from s.vendor [1]\nd.limit rests_on api.limit [1]\nCounts describe links: dependency_count=rests_on, source_count=from; not proof. Read node:ID, /topic, links:ID or @ref without @; pass revision.\n"
    );
    let body = json!({"rests_on":["api.limit"],"seen":{"api.limit":10},"verdict":"The limit is low enough","wrong_if":"api.limit > 20"});
    assert_eq!(
        live_read(&root, "node:d.limit#", &revision(&opened))["value"],
        json!({"assessment_body":body,"assessment_fields":{"deps":"rests_on","predicate":"wrong_if","snapshot":"seen"},"body":body,"kind":"judgment","record_source":"record","states":[]})
    );
}
