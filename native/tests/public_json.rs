//! `--json` on the public read and write commands answers as the Python reference does:
//! `open` with its own object, every other command with the output it prints without
//! `--json`, wrapped as `{"command", "exit_code", "output", "error"}`.
use serde_json::{Value as J, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const WORKSHOP: &str = include_str!("../../examples/workshop/GROUNDING.yaml");
const CORE: &str = "meta:\n  reasoning: {version: 1, profile: core/v1, requires: [arithmetic/v1]}\nschema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown:\n  p.load: {v: 61, from: s.note}\n  s.note: {name: source}\njudgments:\n  d.work:\n    verdict: continue\n    rests_on: [p.load]\n    seen: {p.load: 44}\n    wrong_if: p.load > 80\n";
const UNREADABLE_ROLES: &str = "known:\n  local.one: {v: 1}\njudgments:\n  d.use_limit:\n    verdict: Batch requests at the vendor limit\n    rests_on: [api.limit]\n    seen: {api.limit: 10}\n    wrong_if: api.limit > 20\n";

/// Runs kpop in a workspace whose private state lives beside it: a followups store
/// refuses to live inside the workspace it serves.
fn kpop_with(root: &Path, args: &[&str], session: Option<&str>) -> Output {
    let private = root.parent().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_kpop"));
    command
        .current_dir(root)
        .args(args)
        .env_remove("KPOPPER_NATIVE_RESOURCES")
        .env_remove("KPOPPER_READ_MODE")
        .env_remove("KPOPPER_SESSION_CONFIG")
        .env_remove("KPOPPER_AGENT_SESSION")
        .env_remove("CODEX_THREAD_ID")
        .env("KPOPPER_PRIVATE_HOME", private.join("private"))
        .env("XDG_STATE_HOME", private.join("state"))
        .env("XDG_CONFIG_HOME", private.join("config"));
    if let Some(session) = session {
        command.env("KPOPPER_AGENT_SESSION", session);
    }
    command.output().unwrap()
}
fn kpop(root: &Path, args: &[&str]) -> Output {
    kpop_with(root, args, None)
}
/// A temporary `work/` directory, optionally holding a record, beside its private state.
struct Space(tempfile::TempDir);
impl Space {
    fn new(record: Option<&str>) -> Self {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("work")).unwrap();
        if let Some(record) = record {
            fs::write(temp.path().join("work/GROUNDING.yaml"), record).unwrap();
        }
        Self(temp)
    }
    fn root(&self) -> PathBuf {
        self.0.path().canonicalize().unwrap().join("work")
    }
}
fn text(output: &[u8]) -> String {
    String::from_utf8(output.to_vec()).unwrap()
}
/// The object Python's `--json` prints around the output of the same command run without it.
fn wrapped(command: &str, plain: &Output) -> J {
    json!({
        "command": command,
        "exit_code": plain.status.code().unwrap(),
        "output": text(&plain.stdout),
        "error": text(&plain.stderr),
    })
}
fn assert_wraps(command: &str, plain: &Output, structured: &Output) {
    let value: J = serde_json::from_slice(&structured.stdout).unwrap_or_else(|error| {
        panic!(
            "{command}: not JSON ({error}): {}",
            text(&structured.stdout)
        )
    });
    assert_eq!(value, wrapped(command, plain), "{command}");
    assert_eq!(structured.status.code(), plain.status.code(), "{command}");
    assert!(
        structured.stderr.is_empty(),
        "{command}: {}",
        text(&structured.stderr)
    );
}
/// Where a path leads, so paths spelled differently compare alike (a Windows verbatim
/// prefix, a symlinked temporary directory); a path that leads nowhere keeps its name.
fn resolved(path: &Path) -> PathBuf {
    path.canonicalize()
        .unwrap_or_else(|_| resolved(path.parent().unwrap()).join(path.file_name().unwrap()))
}
/// `open --json`'s object without its two paths, once they are shown to name the
/// workspace and the record.
fn without_paths(mut value: J, workspace: &Path, record: &Path) -> J {
    for (key, expected) in [("workspace", workspace), ("record", record)] {
        let path = PathBuf::from(value[key].as_str().unwrap_or_else(|| panic!("{value}")));
        assert_eq!(resolved(&path), resolved(expected), "{key}");
        value.as_object_mut().unwrap().remove(key);
    }
    value
}

#[test]
fn reads_wrap_the_text_they_print_with_its_exit_status() {
    let space = Space::new(Some(WORKSHOP));
    let root = &space.root();
    for args in [
        vec!["check"],
        vec!["pull", "workshop.guests"],
        vec!["pull", "workshop.guests", "--history"],
        vec!["pull", "workshop.nothing"],
        vec!["affects", "workshop.guests"],
        vec!["remeasure"],
    ] {
        let plain = kpop(root, &args);
        let mut last = args.clone();
        last.push("--json");
        let first = [&["--json"][..], &args].concat();
        for structured in [&first, &last] {
            assert_wraps(args[0], &plain, &kpop(root, structured));
        }
    }
    // The unknown entry is the failing case above: its reason travels as the error.
    let missing: J =
        serde_json::from_slice(&kpop(root, &["--json", "pull", "workshop.nothing"]).stdout)
            .unwrap();
    assert_eq!(missing["exit_code"], 1);
    assert_eq!(missing["output"], "");
    assert!(
        missing["error"]
            .as_str()
            .unwrap()
            .starts_with("workshop.nothing is not an entry or a prefix in this record.")
    );
}

#[test]
fn refused_arguments_are_wrapped_where_the_command_output_is() {
    let space = Space::new(Some(WORKSHOP));
    let root = space.root();
    for (command, args, reason) in [
        (
            "add",
            vec!["--json", "add"],
            "the following required arguments were not provided",
        ),
        (
            "check",
            vec!["check", "--bogus", "--json"],
            "unexpected argument '--bogus'",
        ),
    ] {
        let plain: Vec<_> = args.iter().copied().filter(|a| *a != "--json").collect();
        let plain = kpop(&root, &plain);
        assert_eq!(plain.status.code(), Some(2), "{args:?}");
        assert!(
            text(&plain.stderr).contains(reason),
            "{}",
            text(&plain.stderr)
        );
        assert_wraps(command, &plain, &kpop(&root, &args));
    }
    // open answers --json with its own object, so a refused argument stays plain there;
    // asking for help is no refusal.
    let open = kpop(&root, &["--json", "open", "--bogus"]);
    assert_eq!(open.status.code(), Some(2));
    assert!(open.stdout.is_empty(), "{}", text(&open.stdout));
    let help = kpop(&root, &["--json", "check", "--help"]);
    assert!(help.status.success());
    assert!(serde_json::from_slice::<J>(&help.stdout).is_err());
}

#[test]
fn a_refused_read_wraps_its_reason_instead_of_printing_it() {
    let space = Space::new(Some(UNREADABLE_ROLES));
    let root = &space.root();
    for args in [
        vec!["check"],
        vec!["pull", "d.use_limit"],
        vec!["affects", "api.limit"],
    ] {
        let plain = kpop(root, &args);
        assert_eq!(plain.status.code(), Some(1), "{args:?}");
        assert_wraps(
            args[0],
            &plain,
            &kpop(root, &[&["--json"][..], &args].concat()),
        );
    }
}

#[test]
fn writes_wrap_the_text_they_print_and_change_the_record_alike() {
    let (plain_space, json_space) = (Space::new(Some(WORKSHOP)), Space::new(Some(WORKSHOP)));
    let (plain_dir, json_dir) = (plain_space.root(), json_space.root());
    for args in [
        vec!["set", "workshop.guests", "21", "--as-of", "2026-09-22"],
        vec!["set", "workshop.nothing", "21", "--as-of", "2026-09-22"],
        vec![
            "add",
            "workshop.room",
            "v=Hall B",
            "from=prep",
            "at=Room booking",
            "--as-of",
            "2026-09-09",
        ],
        vec![
            "add",
            "workshop.room",
            "v=Hall C",
            "from=prep",
            "at=Room booking",
            "--as-of",
            "2026-09-09",
        ],
        vec!["review", "workshop.stock_ready", "--as-of", "2026-09-22"],
        vec![
            "distinct",
            "workshop.guests",
            "stock.packages",
            "guests are people, packages are things",
        ],
        vec![
            "add",
            "workshop.hall",
            "v=Hall B",
            "from=prep",
            "at=Room booking",
            "--as-of",
            "2026-09-09",
        ],
        vec!["same", "workshop.room", "workshop.hall"],
    ] {
        let plain = kpop(&plain_dir, &args);
        let structured = kpop(&json_dir, &[&["--json"][..], &args].concat());
        assert_wraps(args[0], &plain, &structured);
    }
    let (plain, structured) = (
        fs::read_to_string(plain_dir.join("GROUNDING.yaml")).unwrap(),
        fs::read_to_string(json_dir.join("GROUNDING.yaml")).unwrap(),
    );
    assert_eq!(plain, structured);
    assert!(plain.contains("workshop.guests: {v: 21"), "{plain}");
    assert!(plain.contains("workshop.room"), "{plain}");
    assert!(!plain.contains("workshop.hall:"), "{plain}");
}

#[test]
fn core_reads_wrap_like_ordinary_ones() {
    let space = Space::new(Some(CORE));
    let root = &space.root();
    for args in [vec!["check"], vec!["pull", "p"], vec!["affects", "p.load"]] {
        let plain = kpop(root, &args);
        assert_wraps(
            args[0],
            &plain,
            &kpop(root, &[&args[..], &["--json"]].concat()),
        );
    }
}

#[test]
fn open_json_is_the_view_with_the_record_digest() {
    let space = Space::new(Some(WORKSHOP));
    let root = space.root();
    let entry = root.join("GROUNDING.yaml");
    let view = kpop(&root, &["open"]);
    assert!(view.status.success(), "{}", text(&view.stderr));
    for args in [
        vec!["open", "--json"],
        vec!["--json", "open", "GROUNDING.yaml"],
    ] {
        let output = kpop(&root, &args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            text(&output.stderr)
        );
        let value: J = serde_json::from_slice(&output.stdout).unwrap();
        let expected_view = if args.len() == 2 {
            text(&view.stdout)
        } else {
            text(&kpop(&root, &["open", "GROUNDING.yaml"]).stdout)
        };
        assert_eq!(
            without_paths(value, &root, &entry),
            json!({
                "status": "found",
                "record_sha256": kpop_native::identity::sha256(&fs::read(&entry).unwrap()),
                "view": expected_view,
                "checked": false,
            }),
            "{args:?}"
        );
    }
}

#[test]
fn open_follows_the_view_with_followups_and_the_mapping_task() {
    let space = Space::new(Some(WORKSHOP));
    let root = space.root();
    let setup = kpop(
        &root,
        &[
            "followups",
            "setup",
            "--private",
            "--timezone",
            "Asia/Jerusalem",
        ],
    );
    assert!(setup.status.success(), "{}", text(&setup.stderr));
    let spec = root.parent().unwrap().join("followup.json");
    fs::write(
        &spec,
        json!({"id":"count","title":"Count the confirmed guests","why":"Stock depends on it","how":"Read the sign-up sheet","related":["workshop.guests"],"when":{"at":"2020-01-01"},"scope":"Count and report"}).to_string(),
    )
    .unwrap();
    let add = kpop(
        &root,
        &["followups", "add", "--file", spec.to_str().unwrap()],
    );
    assert!(add.status.success(), "{}", text(&add.stderr));
    let map = kpop_with(&root, &["--json", "map"], Some("fixture"));
    assert!(map.status.success(), "{}", text(&map.stderr));

    let value: J = serde_json::from_slice(&kpop(&root, &["open", "--json"]).stdout).unwrap();
    let view = value["view"].as_str().unwrap();
    let followups = value["followups"].as_str().unwrap();
    assert!(
        !view.contains("KPOPPER_FOLLOWUPS") && !view.contains("Mapping:"),
        "{view}"
    );
    assert!(followups.starts_with("KPOPPER_FOLLOWUPS {"), "{followups}");
    assert!(followups.contains("\"ready\":1"), "{followups}");
    assert_eq!(value["mapping"]["mapping"], "ready");
    assert_eq!(value["mapping"]["mode"], "map");

    let open = kpop(&root, &["open"]);
    assert!(open.status.success(), "{}", text(&open.stderr));
    assert_eq!(
        text(&open.stdout),
        format!("{view}\n{followups}\nMapping: ready\n")
    );
}

#[test]
fn a_record_open_cannot_read_keeps_the_object_and_the_text_failure() {
    for record in [UNREADABLE_ROLES, "known: [unclosed\n"] {
        let space = Space::new(Some(record));
        let root = space.root();
        let entry = root.join("GROUNDING.yaml");
        let plain = kpop(&root, &["open"]);
        assert!(!plain.status.success(), "{record}");
        assert!(plain.stdout.is_empty(), "{record}");
        let output = kpop(&root, &["open", "--json"]);
        assert_eq!(output.status.code(), plain.status.code(), "{record}");
        assert!(output.stderr.is_empty(), "{}", text(&output.stderr));
        let value = serde_json::from_slice::<J>(&output.stdout).unwrap();
        assert_eq!(
            without_paths(value, &root, &entry),
            json!({
                "status": "found",
                "record_sha256": kpop_native::identity::sha256(record.as_bytes()),
                "view": "",
                "checked": false,
                "error": text(&plain.stderr).trim(),
            }),
            "{record}"
        );
    }
}

#[test]
fn a_core_record_refuses_the_opener_options_with_the_error_alone() {
    let space = Space::new(Some(CORE));
    let root = space.root();
    let plain = kpop(&root, &["open", "--budget", "5"]);
    assert!(!plain.status.success());
    assert!(
        text(&plain.stderr).contains("core_profile_option_unsupported: --budget"),
        "{}",
        text(&plain.stderr)
    );
    let output = kpop(&root, &["open", "--budget", "5", "--json"]);
    assert_eq!(output.status.code(), plain.status.code());
    assert_eq!(
        serde_json::from_slice::<J>(&output.stdout).unwrap(),
        json!({"error": text(&plain.stderr).trim()})
    );
}

#[test]
fn core_option_refusals_use_exit_one_and_unprefixed_diagnostics() {
    let space = Space::new(Some(include_str!("fixtures/core-page/GROUNDING.yaml")));
    let root = space.root();
    for (args, message) in [
        (
            vec!["pull", "--history", "d.safe"],
            "core_profile_option_unsupported: --history; core pull already includes captured history",
        ),
        (
            vec!["open", "--chars", "2000"],
            "core_profile_option_unsupported: --chars",
        ),
        (
            vec!["open", "--budget", "5"],
            "core_profile_option_unsupported: --budget",
        ),
        (
            vec!["open", "--host", "codex"],
            "core_profile_option_unsupported: --host",
        ),
    ] {
        let plain = kpop(&root, &args);
        assert_eq!(
            plain.status.code(),
            Some(1),
            "{args:?}: {}",
            text(&plain.stderr)
        );
        assert!(plain.stdout.is_empty());
        assert_eq!(text(&plain.stderr), format!("{message}\n"));
        let output = kpop(&root, &[&args[..], &["--json"]].concat());
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(output.stderr.is_empty());
        assert_eq!(
            serde_json::from_slice::<J>(&output.stdout).unwrap(),
            if args[0] == "open" {
                json!({"error": message})
            } else {
                wrapped(args[0], &plain)
            }
        );
    }
}

#[test]
fn open_reports_a_record_path_that_is_no_readable_file() {
    let space = Space::new(None);
    let root = space.root();
    fs::create_dir(root.join("folder.yaml")).unwrap();
    let mut cases = vec![(
        vec!["open", "folder.yaml"],
        root.join("folder.yaml"),
        "The requested record is unavailable: ",
    )];
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("nowhere.yaml", root.join("GROUNDING.yaml")).unwrap();
        cases.push((
            vec!["open"],
            root.join("GROUNDING.yaml"),
            "The record path exists but is not an accessible file.",
        ));
    }
    for (args, record, reason) in cases {
        let output = kpop(&root, &[&args[..], &["--json"]].concat());
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        let value = serde_json::from_slice::<J>(&output.stdout).unwrap();
        // The named record's reason ends with the path as the reader spelled it.
        let message = value["error"].as_str().unwrap().to_owned();
        let named = message
            .strip_prefix(reason)
            .unwrap_or_else(|| panic!("{message}"));
        if !named.is_empty() {
            assert_eq!(resolved(Path::new(named)), resolved(&record), "{args:?}");
        }
        assert_eq!(
            without_paths(value, &root, &record),
            json!({"status": "unavailable", "error": message}),
            "{args:?}"
        );
        let plain = kpop(&root, &args);
        assert_eq!(plain.status.code(), Some(1), "{args:?}");
        assert!(plain.stdout.is_empty(), "{args:?}");
        assert_eq!(text(&plain.stderr), format!("{message}\n"), "{args:?}");
    }
}
