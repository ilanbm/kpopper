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
    command.args(["--workspace", root.to_str().unwrap()]);
    if operation == "current-context" {
        command.arg("context");
    } else {
        command.args(["session", operation]);
    }
    command
        .args([
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

fn d0_start(root: &Path, host: &str, env: &[(&str, &str)]) -> std::process::Output {
    d0_start_at(root, root, None, host, env)
}

fn d0_start_at(
    root: &Path,
    cwd: &Path,
    session_id: Option<&str>,
    host: &str,
    env: &[(&str, &str)],
) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kpop"));
    cmd.args(["session-start", "--host", host])
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_STATE_HOME", root.join("host-state"))
        .env("KPOPPER_SESSION_CONFIG", root.join("settings.json"))
        .env("KPOPPER_NATIVE_RESOURCES", root.join("resources"))
        .env("KPOPPER_NATIVE_CACHE", root.join("cache"))
        .env("KPOPPER_READ_MODE", "frozen")
        .env_remove("KPOPPER_CANONICAL_VIEW")
        .env_remove("KPOPPER_CANONICAL_VIEW_FORMAT")
        .env_remove("KPOPPER_SESSION_DISABLE")
        .envs(env.iter().copied())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    let mut payload = json!({"cwd":cwd});
    if let Some(session_id) = session_id {
        payload["session_id"] = json!(session_id);
    }
    child.stdin.take().unwrap().write_all(
        serde_json::to_vec(&payload).unwrap().as_slice()
    ).unwrap();
    child.wait_with_output().unwrap()
}

fn d0_route(text: &str) -> Option<J> {
    text.lines().find_map(|line| line.strip_prefix("KPOPPER_CANONICAL_VIEW_ROUTE "))
        .map(|line| serde_json::from_str(line).unwrap())
}

#[test]
fn d0_codex_defaults_to_tagged_and_other_hosts_keep_ordinary() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    copy_resources(&root);
    fs::write(root.join("GROUNDING.yaml"), RECORD).unwrap();
    for host in ["codex", "claude"] {
        let text = ok(d0_start(&root, host, &[]));
        if host == "codex" {
            let route = d0_route(&text).expect("Codex starts with a canonical route by default");
            assert_eq!(route["view_format"], "checked-text-tagged");
            assert_eq!(route["max_view_bytes"], 39000);
            assert!(text.contains("KPOPPER_OPENING_ATTENTION"));
        } else {
            assert!(d0_route(&text).is_none());
            assert!(text.contains("needs a person") || text.contains("nothing needs a person"));
        }
    }
}

#[test]
fn d0_managed_continuation_resolves_subdirectory_to_startup_workspace() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let sub = root.join("sub");
    fs::create_dir_all(&sub).unwrap();
    copy_resources(&root);
    fs::write(root.join("GROUNDING.yaml"), RECORD).unwrap();

    let session_id = "d0-managed-subdirectory-regression";
    let started = ok(d0_start_at(&root, &sub, Some(session_id), "codex", &[]));
    let route = d0_route(&started).expect("managed Codex startup route");
    let argv = route["argv"].as_array().unwrap();
    let mut view = Command::new(argv[0].as_str().unwrap());
    view.args(argv[1..].iter().map(|arg| arg.as_str().unwrap()))
        .args(["--query", "express delivery"])
        .env("KPOPPER_READ_MODE", "frozen")
        .env("KPOPPER_NATIVE_RESOURCES", root.join("resources"))
        .env("KPOPPER_NATIVE_CACHE", root.join("cache"));
    let queued = view.output().unwrap();
    assert!(queued.status.success(), "{}", String::from_utf8_lossy(&queued.stderr));
    let queued_text = String::from_utf8(queued.stdout).unwrap();
    assert!(queued_text.starts_with("KPOPPER_CONTEXT_QUEUED "), "{queued_text}");

    let transcript = root.join("transcript.jsonl");
    fs::write(
        &transcript,
        serde_json::to_vec(&json!({"type":"session_meta","payload":{"id":session_id}})).unwrap(),
    )
    .unwrap();
    let payload = json!({
        "session_id":session_id,
        "hook_event_name":"PostToolUse",
        "cwd":sub,
        "turn_id":"turn-1",
        "transcript_path":transcript,
        "tool_response":{"output":queued_text}
    });
    let hook = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .args(["_hook", "continuation", "codex"])
        .env("KPOPPER_READ_MODE", "frozen")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut hook = hook;
    hook.stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&payload).unwrap())
        .unwrap();
    let output = hook.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(!output.stdout.is_empty(), "managed PostToolUse dropped the queued frame");
    let delivered: J = serde_json::from_slice(&output.stdout).unwrap();
    let context = delivered["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(context.starts_with("KPOPPER_CONTEXT_FRAME "), "{context}");
}

fn d0_config(root: &Path, tokens: Option<J>) {
    let mut config = json!({"schema":1,"enabled":true,"native":env!("CARGO_BIN_EXE_kpop"),
        "project":"scoped-project","state":root.join("selected-state"),"profile":root.join("profile.json")});
    if let Some(tokens) = tokens { config["tokens"] = tokens; }
    fs::write(root.join("profile.json"), r#"{"groups":{"selected":["p.a"]},"scope":"selected scope"}"#).unwrap();
    fs::write(root.join("settings.json"), serde_json::to_vec(&config).unwrap()).unwrap();
}

#[test]
fn d0_flag_config_and_format_precedence() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    copy_resources(&root);
    fs::write(root.join("GROUNDING.yaml"), RECORD).unwrap();
    for enabled in [None, Some(false), Some(true)] {
        if let Some(enabled) = enabled {
            d0_config(&root, Some(json!(16000)));
            if !enabled { fs::write(root.join("settings.json"), r#"{"schema":1,"enabled":false}"#).unwrap(); }
        } else { let _ = fs::remove_file(root.join("settings.json")); }
        for (flag, disable, format, canonical, warning) in [
            (None, None, None, enabled != Some(false), false),
            (Some("1"), None, None, true, false),
            (Some("0"), None, Some("invalid"), false, false),
            (Some("invalid"), None, Some("invalid"), false, true),
            (Some(""), None, None, false, true),
            (Some("1"), Some("1"), Some("invalid"), false, false),
        ] {
            let env = [("KPOPPER_CANONICAL_VIEW",flag),("KPOPPER_SESSION_DISABLE",disable),("KPOPPER_CANONICAL_VIEW_FORMAT",format)]
                .into_iter().filter_map(|(key,value)| value.map(|value| (key,value))).collect::<Vec<_>>();
            let text = ok(d0_start(&root, "codex", &env));
            assert_eq!(d0_route(&text).is_some(), canonical, "enabled={enabled:?} flag={flag:?}: {text}");
            assert_eq!(text.contains("Invalid KPOPPER_CANONICAL_VIEW"), warning, "{text}");
            if !canonical { assert!(!text.contains("unsupported canonical view format"), "{text}"); }
        }
    }
    // No record wins even over explicit invalid format/configuration.
    fs::remove_file(root.join("GROUNDING.yaml")).unwrap();
    fs::write(root.join("settings.json"), "invalid config").unwrap();
    let text = ok(d0_start(&root, "codex", &[("KPOPPER_CANONICAL_VIEW","1"),("KPOPPER_CANONICAL_VIEW_FORMAT","invalid")]));
    assert!(d0_route(&text).is_none() && !text.contains("view unavailable"), "{text}");
}

#[test]
fn d0_formats_and_configured_budgets_match_explicit_tagged_semantics() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    copy_resources(&root);
    fs::write(root.join("GROUNDING.yaml"), RECORD).unwrap();
    for tokens in [None, Some(64), Some(1000), Some(16000), Some(65536)] {
        d0_config(&root, tokens.map(|n| json!(n)));
        let implicit = ok(d0_start(&root,"codex",&[]));
        let explicit = ok(d0_start(&root,"codex",&[("KPOPPER_CANONICAL_VIEW","1"),("KPOPPER_CANONICAL_VIEW_FORMAT","checked-text-tagged")]));
        match (d0_route(&implicit), d0_route(&explicit)) {
            (Some(a),Some(b)) => {
                assert_eq!(a["view_sha256"], b["view_sha256"], "budget={tokens:?}");
                assert!(a["argv"].as_array().unwrap().windows(2).any(|v| v == [json!("--tokens"),json!(tokens.unwrap_or(16000).to_string())]));
                assert!(a["view_tokens"].as_u64().unwrap() <= tokens.unwrap_or(16000));
            }
            (None,None) => {
                assert!(implicit.contains("Canonical default unavailable"),"{implicit}");
                assert!(explicit.contains("Checked session view unavailable"),"{explicit}");
            }
            _ => panic!("implicit and explicit budgets differ: {implicit}\n{explicit}"),
        }
    }
    d0_config(&root, Some(json!(16000)));
    for format in ["json","checked-text","checked-text-rows","checked-text-tagged"] {
        for explicit in [false,true] {
            let mut env = vec![("KPOPPER_CANONICAL_VIEW_FORMAT",format)];
            if explicit { env.push(("KPOPPER_CANONICAL_VIEW","1")); }
            let text = ok(d0_start(&root,"codex",&env));
            assert_eq!(d0_route(&text).unwrap()["view_format"],format);
        }
    }
    let text=ok(d0_start(&root,"claude",&[("KPOPPER_CANONICAL_VIEW","1")]));
    assert_eq!(d0_route(&text).unwrap()["view_format"],"json");
}

#[test]
fn d0_invalid_budget_and_format_fallback_preserves_configured_route() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    copy_resources(&root);
    fs::write(root.join("GROUNDING.yaml"), RECORD).unwrap();
    for tokens in [json!(0),json!(63),json!(65537),json!(-1),json!(1.5),json!("bad"),J::Null] {
        d0_config(&root,Some(tokens.clone()));
        let text=ok(d0_start(&root,"codex",&[]));
        assert!(d0_route(&text).is_none() && text.contains("1000-token fallback budget"),"{tokens}: {text}");
        assert!(text.contains("project=scoped-project revision=") && text.contains("--profile") && text.contains("selected-state"),"{text}");
        let explicit=ok(d0_start(&root,"codex",&[("KPOPPER_CANONICAL_VIEW","1")]));
        assert!(explicit.contains("Checked session view unavailable") && !explicit.contains("Read via MCP"),"{explicit}");
    }
    d0_config(&root,Some(json!(16000)));
    let text=ok(d0_start(&root,"codex",&[("KPOPPER_CANONICAL_VIEW_FORMAT","invalid")]));
    assert!(text.contains("unsupported canonical view format") && text.contains("Read via MCP") && text.contains("--profile"),"{text}");
    let explicit=ok(d0_start(&root,"codex",&[("KPOPPER_CANONICAL_VIEW","1"),("KPOPPER_CANONICAL_VIEW_FORMAT","invalid")]));
    assert!(explicit.contains("Checked session view unavailable") && !explicit.contains("Read via MCP"),"{explicit}");
    for field in ["profile","state","project"] {
        d0_config(&root,Some(json!(16000)));
        let mut config:J=serde_json::from_slice(&fs::read(root.join("settings.json")).unwrap()).unwrap();
        config[field]=json!({"invalid":"scope"});
        fs::write(root.join("settings.json"),serde_json::to_vec(&config).unwrap()).unwrap();
        let text=ok(d0_start(&root,"codex",&[]));
        assert!(text.contains("no safe ordinary fallback") && !text.contains("Read via MCP") && !text.contains("holds:"),"{text}");
    }
}

fn d0_attention(text: &str) -> J {
    serde_json::from_str(text.lines().find_map(|line| line.strip_prefix("KPOPPER_OPENING_ATTENTION ")).unwrap()).unwrap()
}

#[test]
fn d0_attention_retains_ranked_reversals_hypotheses_and_exact_omissions() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    copy_resources(&root);
    let record=RECORD.replace("    verdict: Keep", "    replaced: ['its condition fired on 2026-09-16']\n    verdict: Keep");
    fs::write(root.join("GROUNDING.yaml"), &record).unwrap();
    fs::create_dir_all(root.join(".kpopper/hypotheses")).unwrap();
    fs::write(root.join(".kpopper/hypotheses/candidate.yaml"), "known:\n  p.later: {v: 2}\n").unwrap();
    let text=ok(d0_start(&root,"codex",&[]));
    let attention=d0_attention(&text);
    assert_eq!(attention["pending_hypotheses"],1);
    assert_eq!(attention["needs_person"],1);
    assert_eq!(attention["items"][0]["id"],"d.keep");
    assert!(attention["items"][0]["reason"].as_str().unwrap().contains("reversed on 2026-09-16"));
    assert_eq!(attention["omitted_items"],0);
    assert_eq!(attention["revision"],d0_route(&text).unwrap()["revision"]);

    let mut record=record;
    for n in 0..35 {
        record.push_str(&format!("  d.missing{n:02}:\n    verdict: Wait\n    rests_on: [p.absent{n:02}]\n    reopened_by: The missing input arrives\n"));
    }
    fs::write(root.join("GROUNDING.yaml"), record).unwrap();
    let text=ok(d0_start(&root,"codex",&[]));
    let attention=d0_attention(&text);
    assert_eq!(attention["needs_person"],36);
    assert_eq!(attention["items"][0]["id"],"d.missing00");
    assert_eq!(attention["items"][0]["reason"],"rests on p.absent00, which is not an entry");
    assert!(attention["omitted_items"].as_u64().unwrap()>0);
    assert_eq!(attention["items"].as_array().unwrap().len() as u64 + attention["omitted_items"].as_u64().unwrap(),36);
    let route=d0_route(&text).unwrap();
    assert_eq!(route["complete_graph_in_hook"],false);
    let opening=text.split("next: $ground").next().unwrap().trim_end();
    assert!(opening.len()<=7000,"{}",opening.len());
    let argv=route["argv"].as_array().unwrap();
    let view=ok(Command::new(argv[0].as_str().unwrap()).args(argv[1..].iter().map(|v|v.as_str().unwrap()))
        .env("KPOPPER_NATIVE_RESOURCES",root.join("resources")).env("KPOPPER_NATIVE_CACHE",root.join("cache")).output().unwrap());
    assert_eq!(kpop_native::identity::sha256(view.as_bytes()),route["view_sha256"]);
    assert!(view.len()<=39000);
}

#[test]
fn d0_attention_overflow_falls_back_and_durable_disable_rolls_back() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    copy_resources(&root);
    let missing="long_missing_".repeat(210);
    fs::write(root.join("GROUNDING.yaml"),format!("schema:\n  deps: rests_on\nknown:\n  p.a: {{v: 1}}\njudgments:\n  d.keep:\n    verdict: Wait\n    rests_on: ['{missing}']\n    reopened_by: The input arrives\n")).unwrap();
    let text=ok(d0_start(&root,"codex",&[]));
    let (warning, ordinary) = text.split_once('\n').expect("visible canonical fallback warning");
    assert!(d0_route(&text).is_none() && warning.contains("highest-priority opening attention cannot fit"),"{text}");
    assert!(ordinary.contains("needs a person"),"{ordinary}");
    let identity = ordinary.lines().find_map(|line| line.strip_prefix("KPOPPER_AGENT_CONTEXT "))
        .map(|line| serde_json::from_str::<J>(line).unwrap()).unwrap();
    assert_eq!(identity["workspace"], root.to_str().unwrap(), "fallback lost its scoped record identity: {ordinary}");
    let baseline=ok(d0_start(&root,"codex",&[("KPOPPER_CANONICAL_VIEW","0")]));
    assert_eq!(ordinary.trim_end(),baseline.trim_end(),"canonical failure must keep the bounded ordinary fallback");
    assert!(ordinary.len()<=4_000,"ordinary fallback exceeded its established output ceiling: {}",ordinary.len());
    fs::write(root.join("GROUNDING.yaml"),RECORD).unwrap();
    ok(Command::new(env!("CARGO_BIN_EXE_kpop")).args(["--workspace",root.to_str().unwrap(),"session","disable"])
        .env("KPOPPER_SESSION_CONFIG",root.join("settings.json")).output().unwrap());
    let text=ok(d0_start(&root,"codex",&[]));
    assert!(d0_route(&text).is_none() && !text.contains("unavailable"),"{text}");
    let explicit=ok(d0_start(&root,"codex",&[("KPOPPER_CANONICAL_VIEW","1")]));
    assert!(d0_route(&explicit).is_some(),"{explicit}");
}

#[test]
fn d0_canonical_warnings_do_not_expand_legacy_fallback_for_any_host() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    copy_resources(&root);
    fs::write(root.join("GROUNDING.yaml"), RECORD).unwrap();
    for (host, warning_env, baseline_env, warning_text) in [
        ("codex", vec![("KPOPPER_CANONICAL_VIEW_FORMAT", "invalid")], vec![("KPOPPER_CANONICAL_VIEW", "0")], "Canonical default unavailable:"),
        ("claude", vec![("KPOPPER_CANONICAL_VIEW", "true")], vec![], "Invalid KPOPPER_CANONICAL_VIEW:"),
    ] {
        let warned=ok(d0_start(&root,host,&warning_env));
        let baseline=ok(d0_start(&root,host,&baseline_env));
        let (warning, ordinary)=warned.split_once('\n').expect("visible warning");
        assert!(warning.starts_with(warning_text),"{warning}");
        assert_eq!(ordinary.trim_end(),baseline.trim_end(),"warning changed {host}'s ordinary opening");
        assert!(ordinary.len()<=4_000,"{host} legacy opening grew to {} bytes",ordinary.len());
        let identity = ordinary.lines().find_map(|line| line.strip_prefix("KPOPPER_AGENT_CONTEXT "))
            .map(|line| serde_json::from_str::<J>(line).unwrap()).unwrap();
        assert_eq!(identity["workspace"], root.to_str().unwrap(), "{host} fallback lost its record identity");
    }
}

#[test]
fn d0_noncanonical_hosts_keep_strict_legacy_budget_validation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    copy_resources(&root);
    fs::write(root.join("GROUNDING.yaml"),RECORD).unwrap();
    d0_config(&root,None);
    for (host,env) in [("claude",vec![]),("codex",vec![("KPOPPER_CANONICAL_VIEW","0")])] {
        let text=ok(d0_start(&root,host,&env));
        assert!(text.contains("session token budget must be 64..65536"),"{text}");
        assert!(!text.contains("Read via MCP") && d0_route(&text).is_none(),"{text}");
    }
    let text=ok(d0_start(&root,"codex",&[("KPOPPER_CANONICAL_VIEW","bad")]));
    assert!(text.contains("Invalid KPOPPER_CANONICAL_VIEW") && text.contains("session token budget must be 64..65536"),"{text}");
    fs::write(root.join("settings.json"),"malformed settings").unwrap();
    let text=ok(d0_start(&root,"codex",&[("KPOPPER_CANONICAL_VIEW","bad")]));
    assert!(text.contains("Invalid KPOPPER_CANONICAL_VIEW") && text.contains("Checked session view unavailable"),"{text}");
}

#[test]
fn d0_core_answer_flags_remain_actionable_at_startup() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    copy_resources(&root);
    fs::write(root.join("GROUNDING.yaml"), "meta: {reasoning: {version: 2, profile: core/v1, requires: [arithmetic/v1]}}\nknown:\n  p.x: {v: 1}\nquestions:\n  q.one:\n    v: What is x?\n    answered: {by: p.gone, said: 1}\n").unwrap();
    let text=ok(d0_start(&root,"codex",&[]));
    let attention=d0_attention(&text);
    assert_eq!(attention["needs_person"],1,"{text}");
    assert_eq!(attention["items"][0],json!({"id":"q.one","reason":"its answer p.gone is no longer an entry"}));
    assert_eq!(attention["omitted_items"],0);
    let ordinary=ok(d0_start(&root,"codex",&[("KPOPPER_CANONICAL_VIEW","0")]));
    assert!(ordinary.contains("q.one: its answer p.gone is no longer an entry"));
}

fn revision(open: &str) -> String {
    open.lines()
        .find_map(|line| line.strip_prefix("project=fixture revision="))
        .unwrap()
        .into()
}

#[test]
fn public_context_reads_ordinary_sources_without_a_prior_open() {
    let temp = fixture();
    let root = temp.path();
    let before = fs::read(root.join("GROUNDING.yaml")).unwrap();
    let text = ok(command(root, "current-context")
        .args(["d.keep", "--depth", "2", "--tokens", "4000"])
        .output()
        .unwrap());
    let packet: J = serde_json::from_str(&text).unwrap();
    let rev = packet["revision"].as_str().unwrap();
    let old = ok(command(root, "context")
        .args([
            "--id",
            "d.keep",
            "--direction",
            "support",
            "--depth",
            "2",
            "--tokens",
            "4000",
            "--revision",
            rev,
        ])
        .output()
        .unwrap());
    assert_eq!(text, old);
    assert!(
        packet["reads"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == "s.note")
    );
    assert_eq!(fs::read(root.join("GROUNDING.yaml")).unwrap(), before);
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

#[test]
fn canonical_ordinary_view_preserves_source_fields_and_checks_budget_and_revision() {
    let temp=fixture(); let root=temp.path();
    let rev=revision(&ok(command(root,"open").args(["--tokens","4000"]).output().unwrap()));
    let broad:J=serde_json::from_str(&ok(command(root,"view").args(["--revision",&rev]).output().unwrap())).unwrap();
    let expanded:J=serde_json::from_str(&ok(command(root,"view").args(["--revision",&rev,"--expand","group:/"]).output().unwrap())).unwrap();
    assert_eq!(broad["schema"],"kpopper.canonical-graph-view/v3");
    assert_eq!(broad["coverage"]["count"],expanded["coverage"]["count"]);
    assert_eq!(broad["coverage"]["source_ids_sha256"],expanded["coverage"]["source_ids_sha256"]);
    let reference=expanded["dictionary"].as_object().unwrap().iter().find(|(_,value)|value["kind"]=="node" && value["original"]=="p.a").unwrap().0;
    let reading=expanded["nodes"].as_array().unwrap().iter().find(|row|row[0].as_str()==Some(reference)).unwrap();
    assert_eq!(kpop_native::value::TypedValue::from_tagged(&reading[1]).unwrap().to_json().unwrap()["v"],12);
    assert_eq!(reading[1][0],"map");
    assert!(reading[2]["record_metadata"]["kind"].is_string());
    assert!(reading[2]["record_metadata"].get("body").is_none(), "proven duplicate body is not transmitted twice");
    let limited=command(root,"view").args(["--revision",&rev,"--tokens","64"]).output().unwrap();
    assert!(!limited.status.success());assert!(String::from_utf8_lossy(&limited.stderr).contains("exceeds token budget"));
    let described=ok(command(root,"describe").args(["--revision",&rev]).output().unwrap());
    fs::write(root.join("old-index.json"),&described).unwrap();
    let cached:J=serde_json::from_str(&ok(command(root,"view").args(["--revision",&rev,"--description-cache","old-index.json"]).output().unwrap())).unwrap();
    assert_eq!(cached["coverage"],broad["coverage"]);
    let bypass=command(root,"describe").args(["--revision",&rev,"--description-cache","old-index.json"]).output().unwrap();
    assert!(!bypass.status.success());
    fs::write(root.join("GROUNDING.yaml"),RECORD.replace("v: 12","v: 13")).unwrap();
    assert!(!command(root,"view").args(["--revision",&rev]).output().unwrap().status.success());
}

#[test]
fn canonical_hook_route_is_executable_complete_and_explicitly_bounded() {
    let temp=fixture();let root=temp.path();
    let hook=ok(command(root,"hook-view").args(["--tokens","16000","--encoding","cl100k_base"]).output().unwrap());
    let route:J=serde_json::from_str(hook.lines().find_map(|s|s.strip_prefix("KPOPPER_CANONICAL_VIEW_ROUTE ")).unwrap()).unwrap();
    assert_eq!(route["complete_graph_in_hook"],true);
    let inline=hook.lines().find_map(|line|line.strip_prefix("KPOPPER_CANONICAL_GRAPH_VIEW ")).unwrap().to_owned()+"\n";
    assert_eq!(kpop_native::identity::sha256(inline.as_bytes()),route["view_sha256"]);
    let args=route["argv"].as_array().unwrap().iter().map(|s|s.as_str().unwrap()).collect::<Vec<_>>();
    assert!(args.windows(2).any(|pair|pair==["--encoding","cl100k_base"]),"recovery must preserve the selected tokenizer and its budget decisions");
    let received=ok(Command::new(args[0]).args(&args[1..]).env("KPOPPER_NATIVE_RESOURCES",root.join("resources")).env("KPOPPER_NATIVE_CACHE",root.join("cache")).output().unwrap());
    assert_eq!(kpop_native::identity::sha256(received.as_bytes()),route["view_sha256"]);
    let view:J=serde_json::from_str(&received).unwrap();
    assert_eq!(received,inline);
    assert_eq!(view["revision"],route["revision"]);assert_eq!(view["scope"],route["scope"]);
    let bounded=kpop_native::session_admin::canonical_hook_delivery(&hook,2_000).unwrap();
    assert!(!bounded.contains("KPOPPER_CANONICAL_GRAPH_VIEW "));
    let bounded_route:J=serde_json::from_str(bounded.lines().next().unwrap().strip_prefix("KPOPPER_CANONICAL_VIEW_ROUTE ").unwrap()).unwrap();
    assert_eq!(bounded_route["complete_graph_in_hook"],false);
    assert_eq!(bounded_route["argv"],route["argv"]);
    assert_eq!(bounded_route["view_sha256"],kpop_native::identity::sha256(received.as_bytes()));
    assert!(bounded.len()<=2_000);
    assert!(!command(root,"hook-view").args(["--tokens","64"]).output().unwrap().status.success());
}

#[test]
fn canonical_hook_folds_navigation_text_before_refusing_a_complete_overview() {
    let temp=fixture(); let root=temp.path();
    let mut added=String::new();
    for index in 0..30 {
        added.push_str(&format!("  p.extra{index}: {{name: 'Important navigation item {index}', v: {}}}\n",
            serde_json::to_string(&"large exact evidence ".repeat(500)).unwrap()));
    }
    fs::write(root.join("GROUNDING.yaml"),RECORD.replace("judgments:\n",&(added+"judgments:\n"))).unwrap();
    let opened=ok(command(root,"open").args(["--tokens","16000"]).output().unwrap());
    let rev=revision(&opened);
    let detailed=ok(command(root,"view").args(["--revision",&rev]).output().unwrap());
    let budget=kpop_native::tokenizer::Encoding::O200kBase.count(&detailed)-1;
    let hook=ok(command(root,"hook-view").args(["--tokens",&budget.to_string()]).output().unwrap());
    let line=hook.lines().find_map(|line|line.strip_prefix("KPOPPER_CANONICAL_GRAPH_VIEW ")).unwrap();
    let view:J=serde_json::from_str(line).unwrap();
    assert_eq!(view["navigation_detail"],"labels");
    assert_eq!(view["coverage"]["count"],34);
    assert!(kpop_native::tokenizer::Encoding::O200kBase.count(&(line.to_owned()+"\n"))<=budget);
}

#[test]
fn checked_text_view_and_hook_recover_the_same_graph_with_the_actual_wire_budget() {
    use kpop_native::view_format::decode_checked_text;
    let temp=fixture();let root=temp.path();
    let opened=ok(command(root,"open").args(["--tokens","16000"]).output().unwrap());
    let rev=revision(&opened);
    let json_text=ok(command(root,"view").args(["--revision",&rev]).output().unwrap());
    let text=ok(command(root,"view").args(["--revision",&rev,"--view-format","checked-text"]).output().unwrap());
    assert_eq!(decode_checked_text(&text).unwrap(),serde_json::from_str::<J>(&json_text).unwrap());
    let hook=ok(command(root,"hook-view").args(["--tokens","16000","--view-format","checked-text"]).output().unwrap());
    let route:J=serde_json::from_str(hook.lines().next().unwrap().strip_prefix("KPOPPER_CANONICAL_VIEW_ROUTE ").unwrap()).unwrap();
    assert_eq!(route["view_format"],"checked-text");
    let args=route["argv"].as_array().unwrap().iter().map(|v|v.as_str().unwrap()).collect::<Vec<_>>();
    let received=ok(Command::new(args[0]).args(&args[1..]).env("KPOPPER_NATIVE_RESOURCES",root.join("resources")).env("KPOPPER_NATIVE_CACHE",root.join("cache")).output().unwrap());
    assert_eq!(kpop_native::identity::sha256(received.as_bytes()),route["view_sha256"]);
    assert_eq!(decode_checked_text(&received).unwrap()["revision"],rev);
    let bounded=kpop_native::session_admin::canonical_hook_delivery(&hook,2_000).unwrap();
    assert!(!bounded.contains("KPOPPER_CANONICAL_GRAPH_VIEW_TEXT_BEGIN"));
    assert!(bounded.contains("tool_read_required"));
    assert!(!command(root,"view").args(["--revision",&rev,"--view-format","checked-text","--tokens","64"]).output().unwrap().status.success());
}

#[test]
fn checked_rows_byte_guard_preserves_evidence_and_recovery_arguments() {
    use kpop_native::view_format::decode_checked_text;
    for format in ["checked-text-rows","checked-text-tagged"] {
    let temp=fixture();let root=temp.path();
    let hook=ok(command(root,"hook-view").args([
        "--tokens","16000","--encoding","cl100k_base",
        "--view-format",format,"--max-view-bytes","39000",
    ]).output().unwrap());
    let route:J=serde_json::from_str(hook.lines().next().unwrap()
        .strip_prefix("KPOPPER_CANONICAL_VIEW_ROUTE ").unwrap()).unwrap();
    assert_eq!(route["max_view_bytes"],39000);
    assert_eq!(route["view_format"],format);
    let args=route["argv"].as_array().unwrap().iter().map(|v|v.as_str().unwrap()).collect::<Vec<_>>();
    for expected in [["--max-view-bytes","39000"],["--view-format",format],
                     ["--encoding","cl100k_base"],["--revision",route["revision"].as_str().unwrap()]] {
        assert!(args.windows(2).any(|pair|pair==expected));
    }
    let received=ok(Command::new(args[0]).args(&args[1..])
        .env("KPOPPER_NATIVE_RESOURCES",root.join("resources"))
        .env("KPOPPER_NATIVE_CACHE",root.join("cache")).output().unwrap());
    assert!(received.len()<=39000);
    assert_eq!(kpop_native::identity::sha256(received.as_bytes()),route["view_sha256"]);
    let packet=decode_checked_text(&received).unwrap();
    assert_eq!(packet["revision"],route["revision"]);
    assert_eq!(packet["scope"],route["scope"]);
    let bounded=kpop_native::session_admin::canonical_hook_delivery(&hook,2_000).unwrap();
    assert!(bounded.contains("tool_read_required") && !bounded.contains("KPOPPER_CANONICAL_GRAPH_VIEW_TEXT_BEGIN"));
    let reference=packet["dictionary"].as_object().unwrap().iter()
        .find(|(_,v)|v["kind"]=="node" && v["original"]=="p.a").unwrap().0;
    let row=packet["nodes"].as_array().unwrap().iter().find(|row|row[0].as_str()==Some(reference)).unwrap();
    assert_eq!(kpop_native::value::TypedValue::from_tagged(&row[1]).unwrap().to_json().unwrap()["v"],12);
    let refused=command(root,"view").args(["--revision",route["revision"].as_str().unwrap(),
        "--id","p.a","--tokens","100000","--view-format",format,
        "--max-view-bytes","64"]).output().unwrap();
    assert!(!refused.status.success());
    assert!(refused.stdout.is_empty(),"refusal must not emit a partial graph");
    let error=String::from_utf8_lossy(&refused.stderr);
    assert!(error.contains("transport byte budget") && error.contains("no source body was cropped"),"{error}");
    }
}

#[test]
fn query_view_edge_handle_can_be_read_with_its_current_focus() {
    let temp=fixture(); let root=temp.path();
    let mut background=String::new();
    for index in 0..20 {
        background.push_str(&format!("  background.item{index}: {{v: '{}', from: s.note}}\n", "background details ".repeat(250)));
    }
    fs::write(root.join("GROUNDING.yaml"), RECORD.replace("judgments:\n", &(background+"judgments:\n"))).unwrap();
    let rev=revision(&ok(command(root,"open").args(["--tokens","4000"]).output().unwrap()));
    for byte_cap in [None,Some("15000")] {
    let mut initial=command(root,"view");initial.args(["--revision",&rev,"--query","p.a","--tokens","5000"]);
    if let Some(cap)=byte_cap {initial.args(["--max-view-bytes",cap]);}
    let view:J=serde_json::from_str(&ok(initial.output().unwrap())).unwrap();
    assert_eq!(view["selection"]["policy"],"ranked-evidence-budget/v2");
    let edge=view["links"].as_array().unwrap().iter().min_by_key(|row|row[3].as_u64().unwrap()).unwrap();
    let handle=view["edge_set_handle_template"].as_str().unwrap().replace("{edge_set_ref}",edge[4].as_str().unwrap());
    let mut read=command(root,"view");read.args(["--revision",&rev,"--tokens","5000","--expand",&handle]);
    if let Some(cap)=byte_cap {read.args(["--max-view-bytes",cap]);}
    for id in view["selection"]["focus_ids"].as_array().unwrap() {read.arg(format!("--id={}",id.as_str().unwrap()));}
    let received:J=serde_json::from_str(&ok(read.output().unwrap())).unwrap();
    assert_eq!(received["view_id"],view["view_id"]);
    assert_eq!(received["expanded_edges"].as_array().unwrap().len(),1);
    }
}

#[test]
fn byte_limited_query_reserves_room_for_evidence_by_folding_unrequested_bodies() {
    use kpop_native::view_format::decode_checked_text;
    let temp=fixture();let root=temp.path();
    let unrelated=format!("  v.unrelated: {{v: {}}}\njudgments:\n",serde_json::to_string(&"unrelated display text ".repeat(1000)).unwrap());
    fs::write(root.join("GROUNDING.yaml"),RECORD.replace("judgments:\n",&unrelated)).unwrap();
    let rev=revision(&ok(command(root,"open").args(["--tokens","4000"]).output().unwrap()));
    let text=ok(command(root,"view").args(["--revision",&rev,"--query","p.a","--tokens","16000",
        "--max-view-bytes","10000","--view-format","checked-text-tagged"]).output().unwrap());
    assert!(text.len()<=10000);
    let view=decode_checked_text(&text).unwrap();
    assert_eq!(view["coverage"]["count"],5);
    let alias=view["dictionary"].as_object().unwrap().iter()
        .find(|(_,value)|value["kind"]=="node" && value["original"]=="p.a").unwrap().0;
    let row=view["nodes"].as_array().unwrap().iter().find(|row|row[0].as_str()==Some(alias)).unwrap();
    assert_eq!(kpop_native::value::TypedValue::from_tagged(&row[1]).unwrap().to_json().unwrap()["v"],12);
    let refused=command(root,"view").args(["--revision",&rev,"--id","v.unrelated","--tokens","16000",
        "--max-view-bytes","10000","--view-format","checked-text-tagged"]).output().unwrap();
    assert!(!refused.status.success() && refused.stdout.is_empty(),"explicit evidence must never be folded away to meet a budget");
}
