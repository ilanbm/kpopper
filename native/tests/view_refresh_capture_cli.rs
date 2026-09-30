use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const CORE_RECORD: &str = "meta:\n  scope: Water allocation\n  reasoning: {version: 1, profile: core/v1, requires: [arithmetic/v1]}\nknown:\n  p.value: {v: 7}\njudgments:\n  d.keep: {rests_on: [p.value], seen: {p.value: 7}, wrong_if: p.value > 10, verdict: Keep}\n";
const ORDINARY_RECORD: &str = "meta:\n  scope: Water allocation\nknown:\n  p.value: {v: 7}\njudgments:\n  d.keep: {rests_on: [p.value], seen: {p.value: 7}, wrong_if: p.value > 10, verdict: Keep}\n";

struct Probe {
    root: tempfile::TempDir,
    session: String,
    input: String,
    normalized: bool,
    ordinary: bool,
    settings: bool,
    profile: bool,
}

impl Probe {
    fn new(ordinary: bool, normalized: bool, settings: bool, profile: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let session = format!("refresh-capture-{}", uuid::Uuid::new_v4());
        let input = if normalized {
            "input.json"
        } else {
            "GROUNDING.yaml"
        }
        .to_owned();
        if normalized {
            fs::write(
                root.path().join(&input),
                serde_json::to_vec(&json!({
                    "nodes": {
                        "p.value": {"kind":"known", "states":[], "body":{"v":7}},
                        "d.keep": {"kind":"judgment", "states":[], "body":{
                            "rests_on":["p.value"], "seen":{"p.value":7},
                            "wrong_if":"p.value > 10", "verdict":"Keep"
                        }}
                    },
                    "topics":{"p.value":["known"], "d.keep":["judgments"]},
                    "edges":[{"from":"d.keep", "rel":"rests_on", "to":"p.value"}]
                }))
                .unwrap(),
            )
            .unwrap();
        } else {
            fs::write(
                root.path().join(&input),
                if ordinary {
                    ORDINARY_RECORD
                } else {
                    CORE_RECORD
                },
            )
            .unwrap();
        }

        let target = kpop_native::reasoning_runtime::target_name().unwrap();
        let resources = root.path().join("resources");
        fs::create_dir_all(resources.join("reasoning")).unwrap();
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../scripts/reasoning/native")
                .join(format!("{target}.kpopper-runtime")),
            resources.join("reasoning").join(format!("{target}.zip")),
        )
        .unwrap();
        if ordinary || normalized {
            let ordinary = std::env::var_os("KPOP_TEST_ORDINARY_PROGRAM")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(std::env::var_os("HOME").unwrap())
                        .join(".cache/kpopper/lean")
                        .join(&target)
                        .join(env!("KPOP_ORDINARY_SOURCE_SHA256"))
                });
            let destination = resources.join("ordinary").join(&target);
            fs::create_dir_all(&destination).unwrap();
            for name in [
                "build.json",
                if cfg!(windows) {
                    "epistemic-core.exe"
                } else {
                    "epistemic-core"
                },
            ] {
                fs::copy(ordinary.join(name), destination.join(name)).unwrap();
            }
        }
        if profile {
            fs::write(
                root.path().join("view-profile.json"),
                json!({"groups":{"priority":["p.value"]}}).to_string(),
            )
            .unwrap();
        }
        if settings {
            let mut preferences = json!({
                "schema":1,
                "enabled":true,
                "native":env!("CARGO_BIN_EXE_kpop"),
                "tokens":16000,
                "project":"refresh-capture-test",
                "state":"state",
                "unused_display_note":"first"
            });
            if profile {
                preferences["profile"] = json!("view-profile.json");
            }
            fs::write(
                root.path().join("preferences.json"),
                preferences.to_string(),
            )
            .unwrap();
        }
        fs::write(
            root.path().join("transcript.jsonl"),
            json!({"type":"session_meta", "payload":{"id":session}}).to_string() + "\n",
        )
        .unwrap();
        kpop_native::view_continuation::initialize(root.path(), &session, true).unwrap();
        Self {
            root,
            session,
            input,
            normalized,
            ordinary,
            settings,
            profile,
        }
    }

    fn command(&self, operation: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kpop"));
        command
            .args([
                "--workspace",
                self.root.path().to_str().unwrap(),
                "--frozen",
                "session",
                operation,
                "--input",
                &self.input,
            ])
            .current_dir(self.root.path())
            .env("XDG_STATE_HOME", self.root.path().join("xdg-state"))
            .env(
                "KPOPPER_NATIVE_RESOURCES",
                self.root.path().join("resources"),
            )
            .env("KPOPPER_NATIVE_CACHE", self.root.path().join("cache"))
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env_remove("KPOPPER_SESSION_DISABLE")
            .env_remove("KPOPPER_CANONICAL_VIEW");
        if self.normalized {
            command.arg("--normalized");
        }
        command
            .arg("--assessment-profile")
            .arg(if self.ordinary || self.normalized {
                "checked-reader/v1"
            } else {
                "core/v1"
            });
        if !self.settings {
            command.args([
                "--no-settings",
                "--project",
                "refresh-capture-test",
                "--state",
                "state",
            ]);
            if self.profile {
                command.args(["--profile", "view-profile.json"]);
            }
        } else {
            command.env(
                "KPOPPER_SESSION_CONFIG",
                self.root.path().join("preferences.json"),
            );
        }
        command
    }

    fn session(&self, args: &[&str]) -> String {
        let output = self.command("view").args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn hook(&self, event: &str, turn: Option<&str>, extra: Value) -> String {
        let mut payload = json!({
            "session_id":self.session,
            "hook_event_name":event,
            "cwd":self.root.path(),
            "transcript_path":self.root.path().join("transcript.jsonl")
        });
        if let Some(turn) = turn {
            payload["turn_id"] = json!(turn);
        }
        payload
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let mut child = Command::new(env!("CARGO_BIN_EXE_kpop"))
            .args(["_hook", "continuation", "codex"])
            .current_dir(self.root.path())
            .env("XDG_STATE_HOME", self.root.path().join("xdg-state"))
            .env(
                "KPOPPER_NATIVE_RESOURCES",
                self.root.path().join("resources"),
            )
            .env("KPOPPER_NATIVE_CACHE", self.root.path().join("cache"))
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env(
                "KPOPPER_SESSION_CONFIG",
                self.root.path().join("preferences.json"),
            )
            .env_remove("KPOPPER_SESSION_DISABLE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.to_string().as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if output.stdout.is_empty() {
            return String::new();
        }
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["hookSpecificOutput"]["additionalContext"]
            .as_str().unwrap().to_owned()
    }

    fn read_and_acknowledge(&self) -> String {
        let opened = self.command("open").output().unwrap();
        assert!(
            opened.status.success(),
            "{}",
            String::from_utf8_lossy(&opened.stderr)
        );
        let opened = String::from_utf8(opened.stdout).unwrap();
        let revision = opened
            .lines()
            .find_map(|line| line.strip_prefix("project=refresh-capture-test revision="))
            .expect("open output has revision");
        let output = self.session(&[
            "--revision",
            revision,
            "--tokens",
            "16000",
            "--max-view-bytes",
            "39000",
            "--expand",
            "group:/",
            "--context-session",
            &self.session,
            "--view-transport",
            "auto",
        ]);
        assert!(output.starts_with("KPOPPER_CONTEXT_QUEUED "), "{output}");
        let frame = self.hook(
            "PostToolUse",
            Some("first"),
            json!({"tool_response":{"output":output}}),
        );
        assert!(frame.contains("KPOPPER_CONTEXT_FRAME"), "{frame}");
        let mut transcript = fs::OpenOptions::new()
            .append(true)
            .open(self.root.path().join("transcript.jsonl"))
            .unwrap();
        writeln!(transcript, "{}", json!({"type":"response_item", "payload":{"type":"message", "role":"developer", "content":[{"type":"input_text", "text":frame}]}})).unwrap();
        self.hook(
            "Stop",
            Some("first"),
            json!({"last_assistant_message":"The current value is 7."}),
        );
        revision.to_owned()
    }
}

fn assert_unchanged_source_stays_silent(probe: &Probe) {
    probe.read_and_acknowledge();
    let prompt = probe.hook("UserPromptSubmit", None, json!({}));
    assert!(
        prompt.is_empty(),
        "unchanged source unexpectedly refreshed: {prompt}"
    );
}

#[test]
fn unchanged_core_ordinary_and_normalized_captures_stay_current() {
    for (ordinary, normalized) in [(false, false), (true, false), (false, true)] {
        let probe = Probe::new(ordinary, normalized, false, false);
        assert_unchanged_source_stays_silent(&probe);
    }
}

#[test]
fn resolver_settings_bytes_are_excluded_but_selected_profile_bytes_remain_in_the_capture() {
    let settings = Probe::new(false, false, true, false);
    settings.read_and_acknowledge();
    let mut preferences: Value =
        serde_json::from_slice(&fs::read(settings.root.path().join("preferences.json")).unwrap())
            .unwrap();
    preferences["unused_display_note"] = json!("changed without changing the selected route");
    fs::write(
        settings.root.path().join("preferences.json"),
        preferences.to_string(),
    )
    .unwrap();
    assert!(
        settings
            .hook("UserPromptSubmit", None, json!({}))
            .is_empty()
    );

    let profiled = Probe::new(false, false, false, true);
    profiled.read_and_acknowledge();
    fs::write(
        profiled.root.path().join("view-profile.json"),
        json!({"groups":{"priority":[]}}).to_string(),
    )
    .unwrap();
    let refreshed = profiled.hook("UserPromptSubmit", None, json!({}));
    assert!(
        refreshed.contains("KPOPPER_SOURCE_REFRESH"),
        "selected profile change was missed: {refreshed}"
    );
}

#[test]
fn settings_and_profile_freshness_are_consistent_for_ordinary_and_normalized_views() {
    for (ordinary, normalized) in [(true, false), (false, true)] {
        let settings = Probe::new(ordinary, normalized, true, true);
        settings.read_and_acknowledge();
        let path = settings.root.path().join("preferences.json");
        let mut preferences: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        preferences["unused_display_note"] = json!("updated resolver-only bytes");
        fs::write(&path, preferences.to_string()).unwrap();
        assert!(
            settings
                .hook("UserPromptSubmit", None, json!({}))
                .is_empty()
        );

        let profiled = Probe::new(ordinary, normalized, false, true);
        profiled.read_and_acknowledge();
        fs::write(
            profiled.root.path().join("view-profile.json"),
            json!({"groups":{"priority":[]}}).to_string(),
        )
        .unwrap();
        let refreshed = profiled.hook("UserPromptSubmit", None, json!({}));
        assert!(
            refreshed.contains("KPOPPER_SOURCE_REFRESH"),
            "selected profile change was missed for ordinary={ordinary}, normalized={normalized}: {refreshed}"
        );
    }
}
