//! Gemini CLI and Copilot CLI openings from `session-start`, and the adapter scripts that
//! run it from a kpopper checkout.
use serde_json::{Value, json};
use std::{
    ffi::OsStr,
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
};

const RECORD: &str = "meta:\n  name: Adapter fixture\n  scope: Opening through a host adapter\nsources:\n  source.request: {asked: \"Check the deadline\", name: \"Current request\"}\nknown:\n  deadline.days: {v: 7, from: source.request, name: \"Days remaining\"}\n";
const OPENING: &str = "Opening through a host adapter\n2 entries, 0 judgments";
const GEMINI_UNOPENED: &str = "kpopper could not open the record; check it before relying on it.";

/// A project directory and the private state of one host session.
struct Session {
    home: tempfile::TempDir,
}
impl Session {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        for name in ["Project with spaces", "tmp"] {
            fs::create_dir(home.path().join(name)).unwrap();
        }
        Self { home }
    }
    fn with_record() -> Self {
        let session = Self::new();
        fs::write(session.work().join("GROUNDING.yaml"), RECORD).unwrap();
        session
    }
    fn work(&self) -> PathBuf {
        self.home.path().join("Project with spaces")
    }
    fn baseline(&self, session: &str) -> PathBuf {
        self.home
            .path()
            .join("tmp")
            .join(format!("kpopper-base-{session}"))
    }
    /// A process kept apart from the caller's kpopper state and runtime selection.
    fn command(&self, program: impl AsRef<OsStr>) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(self.home.path())
            .env("TMPDIR", self.home.path().join("tmp"))
            .env("XDG_STATE_HOME", self.home.path().join("state"))
            .env("XDG_CONFIG_HOME", self.home.path().join("config"))
            .env("XDG_CACHE_HOME", self.home.path().join("cache"))
            .env("KPOPPER_NATIVE_CACHE", self.home.path().join("cache"));
        for name in [
            "KPOPPER_RUNTIME",
            "KPOPPER_NATIVE_RESOURCES",
            "KPOPPER_SESSION_CONFIG",
            "KPOPPER_SESSION_DISABLE",
            "KPOPPER_READ_MODE",
            "KPOPPER_AGENT_SESSION",
            "CODEX_THREAD_ID",
        ] {
            command.env_remove(name);
        }
        command
    }
    fn start(&self, flag: &str, payload: &Value) -> Output {
        run(
            self.command(env!("CARGO_BIN_EXE_kpop"))
                .args(["session-start", flag]),
            payload.to_string().as_bytes(),
        )
    }
}

fn run(command: &mut Command, input: &[u8]) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // A hook may answer without reading its payload.
    if let Err(error) = child.stdin.take().unwrap().write_all(input) {
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
    }
    child.wait_with_output().unwrap()
}
fn diagnostic(output: &Output) -> String {
    format!(
        "status={:?}\nstdout={}\nstderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
/// The one JSON object a hook printed, after checking that it exited 0.
fn json_output(output: &Output) -> Value {
    assert_eq!(output.status.code(), Some(0), "{}", diagnostic(output));
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| panic!("{e}\n{}", diagnostic(output)))
}
fn gemini_context(reply: &Value) -> &str {
    assert_eq!(reply["hookSpecificOutput"]["hookEventName"], "SessionStart");
    reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_else(|| panic!("{reply}"))
}
fn copilot_context(reply: &Value) -> &str {
    reply["additionalContext"]
        .as_str()
        .unwrap_or_else(|| panic!("{reply}"))
}
/// The routing the opening hands the agent.
fn agent_context(context: &str) -> Value {
    serde_json::from_str(
        context
            .lines()
            .find_map(|line| line.strip_prefix("KPOPPER_AGENT_CONTEXT "))
            .unwrap_or_else(|| panic!("{context}")),
    )
    .unwrap()
}

#[test]
fn gemini_start_gives_the_model_the_opening_through_its_context_field() {
    let session = Session::with_record();
    let payload = json!({"cwd":session.work(),"session_id":"gemini-fixture","hook_event_name":"SessionStart","source":"startup"});
    let output = session.start("--gemini", &payload);
    assert!(output.stderr.is_empty(), "{}", diagnostic(&output));
    let reply = json_output(&output);
    assert_eq!(reply.as_object().unwrap().len(), 1, "{reply}");
    let context = gemini_context(&reply);
    assert!(context.starts_with(OPENING), "{context}");
    assert_eq!(
        agent_context(context)["environment"]["KPOPPER_AGENT_SESSION"],
        "gemini-fixture"
    );
    assert!(session.baseline("gemini-fixture").is_file());
}

#[test]
fn copilot_start_takes_its_camel_case_session_and_replies_in_its_format() {
    let session = Session::with_record();
    let payload = json!({"cwd":session.work(),"sessionId":"copilot-fixture","source":"startup"});
    let output = session.start("--copilot", &payload);
    assert!(output.stderr.is_empty(), "{}", diagnostic(&output));
    let reply = json_output(&output);
    assert_eq!(reply.as_object().unwrap().len(), 1, "{reply}");
    let context = copilot_context(&reply);
    assert!(context.starts_with(OPENING), "{context}");
    assert_eq!(
        agent_context(context)["environment"]["KPOPPER_AGENT_SESSION"],
        "copilot-fixture"
    );
    assert!(session.baseline("copilot-fixture").is_file());
    // A payload that already names session_id keeps it.
    let named = session.start(
        "--copilot",
        &json!({"cwd":session.work(),"session_id":"named","sessionId":"camel"}),
    );
    let context = agent_context(copilot_context(&json_output(&named)));
    assert_eq!(context["environment"]["KPOPPER_AGENT_SESSION"], "named");
    assert!(!session.baseline("camel").exists());
}

#[test]
fn subagents_get_empty_json() {
    let session = Session::with_record();
    for flag in ["--gemini", "--copilot"] {
        let output = session.start(
            flag,
            &json!({"cwd":session.work(),"session_id":"child","agent_id":"worker"}),
        );
        assert_eq!(json_output(&output), json!({}), "{flag}");
        assert!(output.stderr.is_empty(), "{flag}: {}", diagnostic(&output));
    }
    assert!(!session.baseline("child").exists());
}

#[test]
fn a_payload_that_cannot_be_read_fails_open_with_json() {
    let session = Session::with_record();
    let start = |flag: &str, raw: &[u8]| {
        run(
            session
                .command(env!("CARGO_BIN_EXE_kpop"))
                .args(["session-start", flag]),
            raw,
        )
    };
    for raw in [&b"not json"[..], b"[\"invalid\"]"] {
        let gemini = start("--gemini", raw);
        assert_eq!(gemini_context(&json_output(&gemini)), GEMINI_UNOPENED);
        let copilot = start("--copilot", raw);
        assert_eq!(json_output(&copilot), json!({}));
        for output in [gemini, copilot] {
            assert!(
                String::from_utf8_lossy(&output.stderr)
                    .starts_with("kpop: record was not opened: "),
                "{}",
                diagnostic(&output)
            );
        }
    }
}

#[test]
fn first_use_gives_guidance_without_creating_a_record() {
    let session = Session::new();
    let gemini = json_output(&session.start(
        "--gemini",
        &json!({"cwd":session.work(),"session_id":"first-gemini"}),
    ));
    let copilot = json_output(&session.start(
        "--copilot",
        &json!({"cwd":session.work(),"sessionId":"first-copilot"}),
    ));
    for context in [gemini_context(&gemini), copilot_context(&copilot)] {
        assert!(context.starts_with("KPOPPER_START"), "{context}");
        assert!(context.contains("`kpop add` keeps findings"), "{context}");
    }
    assert_eq!(fs::read_dir(session.work()).unwrap().count(), 0);
    assert!(session.baseline("first-copilot").is_file());
}

#[test]
fn host_reply_flags_exclude_each_other() {
    let session = Session::new();
    for flags in [
        ["--gemini", "--copilot"],
        ["--cursor", "--gemini"],
        ["--cursor", "--copilot"],
    ] {
        let output = run(
            session
                .command(env!("CARGO_BIN_EXE_kpop"))
                .arg("session-start")
                .args(flags),
            b"{}",
        );
        assert_eq!(output.status.code(), Some(2), "{flags:?}");
        assert!(output.stdout.is_empty(), "{flags:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("cannot be used with"),
            "{}",
            diagnostic(&output)
        );
    }
}

/// The adapter scripts at their places in a kpopper checkout, with this build as the
/// checkout's runtime when `runtime` is set.
#[cfg(unix)]
mod checkout {
    use super::*;
    use std::path::Path;

    pub fn repo() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_owned()
    }

    pub const SCRIPTS: [&str; 4] = [
        "scripts/native_runtime.sh",
        "adapters/gemini/scripts/session-start.sh",
        "adapters/gemini/hooks/hooks.json",
        "adapters/copilot/cli/hook.sh",
    ];

    pub struct Checkout {
        _dir: tempfile::TempDir,
        pub root: PathBuf,
    }
    impl Checkout {
        pub fn new(name: &str, runtime: bool) -> Self {
            // Beside the build, so the runtime can be a hard link: running a fresh copy can
            // fail while a process another test started still holds it open for writing.
            let dir = tempfile::Builder::new()
                .prefix("adapter checkout ")
                .tempdir_in(Path::new(env!("CARGO_BIN_EXE_kpop")).parent().unwrap())
                .unwrap();
            let root = dir.path().join(name);
            for relative in SCRIPTS {
                let path = root.join(relative);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::copy(repo().join(relative), path).unwrap();
            }
            if runtime {
                let binary = runtime_binary(&root);
                fs::create_dir_all(binary.parent().unwrap()).unwrap();
                fs::hard_link(env!("CARGO_BIN_EXE_kpop"), binary).unwrap();
            }
            Self {
                root: root.canonicalize().unwrap(),
                _dir: dir,
            }
        }
        pub fn gemini(&self) -> PathBuf {
            self.root.join("adapters/gemini/scripts/session-start.sh")
        }
        pub fn copilot(&self) -> PathBuf {
            self.root.join("adapters/copilot/cli/hook.sh")
        }
    }
    pub fn runtime_binary(root: &Path) -> PathBuf {
        root.join("scripts/runtime")
            .join(kpop_native::reasoning_runtime::target_name().unwrap())
            .join("kpop")
    }
    /// An executable script, written by a child process so that no process this test
    /// binary starts can hold it open for writing when it runs.
    pub fn script(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let written = run(
            Command::new("sh")
                .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
                .arg(path),
            body.as_bytes(),
        );
        assert!(written.status.success(), "{}", diagnostic(&written));
    }
}

#[cfg(unix)]
#[test]
fn gemini_extension_hook_opens_the_record_with_its_checkouts_runtime() {
    use checkout::*;
    let session = Session::with_record();
    let checkout = Checkout::new("kpopper checkout", true);
    let manifest: Value = serde_json::from_slice(
        &fs::read(checkout.root.join("adapters/gemini/hooks/hooks.json")).unwrap(),
    )
    .unwrap();
    let hooks = manifest["hooks"].as_object().unwrap();
    assert_eq!(hooks.keys().collect::<Vec<_>>(), ["SessionStart"]);
    let command = hooks["SessionStart"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .replace(
            "${extensionPath}",
            checkout.root.join("adapters/gemini").to_str().unwrap(),
        );
    let payload = json!({"cwd":session.work(),"session_id":"gemini-hook","hook_event_name":"SessionStart","source":"startup"});
    let output = run(
        session.command("sh").args(["-c", &command]),
        payload.to_string().as_bytes(),
    );
    assert!(output.stderr.is_empty(), "{}", diagnostic(&output));
    let reply = json_output(&output);
    let context = gemini_context(&reply);
    assert!(context.starts_with(OPENING), "{context}");
    let routed = agent_context(context);
    assert_eq!(routed["command"][0], json!(runtime_binary(&checkout.root)));
    assert_eq!(
        routed["environment"]["KPOPPER_AGENT_SESSION"],
        "gemini-hook"
    );
    // A payload the opener cannot read still gives the model a JSON context.
    let failed = run(session.command("sh").arg(checkout.gemini()), b"not json");
    assert_eq!(gemini_context(&json_output(&failed)), GEMINI_UNOPENED);
}

#[cfg(unix)]
#[test]
fn copilot_hook_configuration_runs_the_hook_that_printed_it() {
    use checkout::*;
    let session = Session::with_record();
    let checkout = Checkout::new("kpopper checkout", true);
    let config = run(
        session.command("sh").arg(checkout.copilot()).arg("config"),
        b"",
    );
    let config = json_output(&config);
    assert_eq!(
        config,
        json!({"version": 1, "hooks": {"sessionStart": [{"type": "command", "exec": "sh",
            "args": [checkout.copilot(), "start"], "timeoutSec": 65}]}})
    );
    let hook = &config["hooks"]["sessionStart"][0];
    let args = hook["args"].as_array().unwrap();
    let payload = json!({"cwd":session.work(),"sessionId":"copilot-hook","source":"new"});
    let output = run(
        session
            .command(hook["exec"].as_str().unwrap())
            .args(args.iter().map(|arg| arg.as_str().unwrap())),
        payload.to_string().as_bytes(),
    );
    assert!(output.stderr.is_empty(), "{}", diagnostic(&output));
    let reply = json_output(&output);
    let context = copilot_context(&reply);
    assert!(context.starts_with(OPENING), "{context}");
    let routed = agent_context(context);
    assert_eq!(routed["command"][0], json!(runtime_binary(&checkout.root)));
    assert_eq!(
        routed["environment"]["KPOPPER_AGENT_SESSION"],
        "copilot-hook"
    );
    assert!(session.baseline("copilot-hook").is_file());
    let stopped = run(
        session.command("sh").arg(checkout.copilot()).arg("stop"),
        payload.to_string().as_bytes(),
    );
    assert_eq!(json_output(&stopped), json!({}));
    let unknown = run(session.command("sh").arg(checkout.copilot()), b"");
    assert_eq!(unknown.status.code(), Some(2));
    assert!(unknown.stdout.is_empty());
}

#[cfg(unix)]
#[test]
fn a_checkout_without_its_runtime_reports_the_install_command_as_context() {
    use checkout::*;
    let session = Session::with_record();
    // Every character here needs escaping in a JSON string.
    let checkout = Checkout::new("kpopper \"quoted\" back\\slash\ttab", false);
    let expected = format!(
        "kpopper: native runtime is not installed for {} in this package.\n\
         Install this active copy: sh \"{}/scripts/install_native.sh\"\n\
         kpopper did not open this session, and its hooks never download or install the runtime.\n\
         Offer to run the command above (it downloads this version's runtime from the kpopper \
         GitHub release and checks its SHA-256), then ask the user to start a new session.",
        kpop_native::reasoning_runtime::target_name().unwrap(),
        checkout.root.display()
    );
    let payload = json!({"cwd":session.work(),"session_id":"unopened","sessionId":"unopened"});
    let gemini = run(
        session.command("sh").arg(checkout.gemini()),
        payload.to_string().as_bytes(),
    );
    assert!(gemini.stderr.is_empty(), "{}", diagnostic(&gemini));
    assert_eq!(gemini_context(&json_output(&gemini)), expected);
    let copilot = run(
        session.command("sh").arg(checkout.copilot()).arg("start"),
        payload.to_string().as_bytes(),
    );
    assert!(copilot.stderr.is_empty(), "{}", diagnostic(&copilot));
    assert_eq!(
        json_output(&copilot),
        json!({"additionalContext": expected})
    );
    assert!(!session.baseline("unopened").exists());
    // The configuration names this checkout's hook whatever its path.
    let config = run(
        session.command("sh").arg(checkout.copilot()).arg("config"),
        b"",
    );
    assert_eq!(
        json_output(&config)["hooks"]["sessionStart"][0]["args"],
        json!([checkout.copilot(), "start"])
    );
}

#[cfg(unix)]
#[test]
fn a_runtime_released_before_the_host_flags_still_opens_the_record() {
    use checkout::*;
    let session = Session::with_record();
    let checkout = Checkout::new("kpopper checkout", false);
    // An earlier release: session-start without --gemini or --copilot.
    script(
        &runtime_binary(&checkout.root),
        r#"#!/bin/sh
case "$*" in
  'session-start --help') printf 'Usage: kpop session-start [OPTIONS]\n      --host <HOST>\n      --cursor\n' ;;
  session-start) cat >/dev/null; printf 'an earlier "opening"\n\tand its second line\n' ;;
  *) printf "error: unexpected argument '%s' found\n" "$2" >&2; exit 2 ;;
esac
"#,
    );
    let payload = json!({"cwd":session.work(),"session_id":"earlier"}).to_string();
    let expected = "an earlier \"opening\"\n\tand its second line";
    let gemini = run(
        session.command("sh").arg(checkout.gemini()),
        payload.as_bytes(),
    );
    let reply = json_output(&gemini);
    assert_eq!(gemini_context(&reply), expected, "{}", diagnostic(&gemini));
    let copilot = run(
        session.command("sh").arg(checkout.copilot()).arg("start"),
        payload.as_bytes(),
    );
    let reply = json_output(&copilot);
    assert_eq!(
        copilot_context(&reply),
        expected,
        "{}",
        diagnostic(&copilot)
    );
    assert!(gemini.stderr.is_empty(), "{}", diagnostic(&gemini));
    assert!(copilot.stderr.is_empty(), "{}", diagnostic(&copilot));
}

#[cfg(unix)]
#[test]
fn a_runtime_choice_other_than_rust_is_refused_without_reaching_an_interpreter() {
    use checkout::*;
    let session = Session::with_record();
    let checkout = Checkout::new("kpopper checkout", true);
    let stubs = session.home.path().join("stubs");
    script(
        &stubs.join("python3"),
        "#!/bin/sh\n: > \"$HOOK_TRACE.called\"\nprintf '{\"python\": true}\\n'\n",
    );
    let trace = session.home.path().join("trace");
    let path = format!(
        "{}:{}",
        stubs.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let payload = json!({"cwd":session.work(),"session_id":"compat","sessionId":"compat"});
    for (hook, args) in [
        (checkout.gemini(), &[][..]),
        (checkout.copilot(), &["start"][..]),
    ] {
        for choice in ["python", "java"] {
            let refused = run(
                session
                    .command("sh")
                    .arg(&hook)
                    .args(args)
                    .env("PATH", &path)
                    .env("HOOK_TRACE", &trace)
                    .env("KPOPPER_RUNTIME", choice),
                payload.to_string().as_bytes(),
            );
            assert_eq!(json_output(&refused), json!({}), "{}", diagnostic(&refused));
            assert_eq!(
                String::from_utf8_lossy(&refused.stderr),
                "kpopper: KPOPPER_RUNTIME must be rust\n"
            );
            assert!(!trace.with_extension("called").exists());
        }
    }
    assert!(!session.baseline("compat").exists());
}

#[cfg(unix)]
#[test]
fn windsurf_hook_checks_only_after_a_record_write_and_stays_quiet() {
    use checkout::*;
    let session = Session::new();
    let checkout = session.home.path().join("kpopper checkout");
    let trace = session.home.path().join("checks");
    script(
        &checkout.join("bin/kpop"),
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$HOOK_TRACE\"\necho unchecked\necho problems >&2\nexit 1\n",
    );
    let manifest: Value =
        serde_json::from_slice(&fs::read(repo().join("adapters/windsurf/hooks.json")).unwrap())
            .unwrap();
    let hook = &manifest["hooks"]["post_write_code"][0];
    assert_eq!(hook["show_output"], false);
    let command = hook["command"]
        .as_str()
        .unwrap()
        .replace("/absolute/path/to/kpopper", checkout.to_str().unwrap());
    let written = |file: &str| {
        run(
            session
                .command("sh")
                .args(["-c", &command])
                .env("HOOK_TRACE", &trace),
            json!({"agent_action_name":"post_write_code","tool_info":{"file_path":session.work().join(file),"edits":[]}})
                .to_string()
                .as_bytes(),
        )
    };
    for (file, checks) in [
        ("notes.md", ""),
        ("GROUNDING.yaml", "check\n"),
        ("README.md", "check\n"),
        ("PROVENANCE.yaml", "check\ncheck\n"),
    ] {
        let output = written(file);
        assert_eq!(output.status.code(), Some(0), "{}", diagnostic(&output));
        assert!(
            output.stdout.is_empty() && output.stderr.is_empty(),
            "{}",
            diagnostic(&output)
        );
        assert_eq!(
            fs::read_to_string(&trace).unwrap_or_default(),
            checks,
            "{file}"
        );
    }
}
