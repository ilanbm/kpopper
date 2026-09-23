use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn run(root: &Path, args: &[&str]) -> Output {
    command(root).args(args).output().unwrap()
}
fn run_unbundled(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(root)
        .env_remove("KPOPPER_AGENT_SESSION")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("KPOPPER_NATIVE_RESOURCES")
        .args(args)
        .output()
        .unwrap()
}
fn command(root: &Path) -> Command {
    let resources = root.join(".test-runtime");
    let target = kpop_native::reasoning_runtime::target_name().unwrap();
    fs::create_dir_all(resources.join("reasoning")).unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(format!("{target}.kpopper-runtime")),
        resources.join("reasoning").join(format!("{target}.zip")),
    )
    .unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_kpop"));
    command
        .current_dir(root)
        .env_remove("KPOPPER_AGENT_SESSION")
        .env_remove("CODEX_THREAD_ID")
        .env("KPOPPER_NATIVE_RESOURCES", resources)
        .env("KPOPPER_NATIVE_CACHE", root.join(".test-cache"));
    command
}
fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn git(root: &Path, args: &[&str]) -> Output {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn advanced_ordinary(source: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "user.name", "Fixture"]);
    git(&root, &["config", "user.email", "fixture@example.test"]);
    fs::write(root.join("GROUNDING.yaml"), source).unwrap();
    git(&root, &["add", "GROUNDING.yaml"]);
    git(
        &root,
        &["-c", "commit.gpgsign=false", "commit", "-qm", "fixture"],
    );
    (temp, root)
}

fn pending_manifest(root: &Path, revision: &str) -> kpop_native::value::TypedValue {
    let path = format!("refs/kpopper/pending_grounding:contributions/{revision}/manifest.json");
    let output = git(root, &["show", &path]);
    let tagged: Value = serde_json::from_slice(&output.stdout).unwrap();
    kpop_native::value::TypedValue::from_tagged(&tagged).unwrap()
}

#[test]
fn advanced_explicit_add_captures_without_mutating_the_record_and_replays() {
    use kpop_native::value::TypedValue as V;
    let (_temp, root) = advanced_ordinary("known:\n  p.base: {v: 1}\n");
    let before = fs::read(root.join("GROUNDING.yaml")).unwrap();
    let args = [
        "add",
        "p.new",
        "v=2",
        "--shareability",
        "project",
        "--scope",
        "external",
        "--environment",
        "vendor",
        "--event-id",
        "event-fixed",
        "--contribution-id",
        "new",
    ];
    let first: Value = serde_json::from_str(&success(run_unbundled(&root, &args))).unwrap();
    assert_eq!(first["state"], "captured");
    assert_eq!(first["replay"], false);
    assert_eq!(fs::read(root.join("GROUNDING.yaml")).unwrap(), before);
    let revision = first["revision"].as_str().unwrap();
    let manifest = pending_manifest(&root, revision);
    let V::Map(manifest) = manifest else {
        panic!("manifest")
    };
    let V::Map(document) = &manifest["document"] else {
        panic!("document")
    };
    let V::Map(known) = &document["known"] else {
        panic!("known")
    };
    let V::Map(body) = &known["p.new"] else {
        panic!("body")
    };
    let V::Map(scope) = &body["scope"] else {
        panic!("scope")
    };
    assert_eq!(scope["kind"], V::Text("external".into()));
    let replay: Value = serde_json::from_str(&success(run_unbundled(&root, &args))).unwrap();
    assert_eq!(replay["replay"], true);
    let changed = run_unbundled(
        &root,
        &[
            "add",
            "p.new",
            "v=3",
            "--shareability",
            "project",
            "--scope",
            "external",
            "--environment",
            "vendor",
            "--event-id",
            "event-fixed",
            "--contribution-id",
            "new",
        ],
    );
    assert!(!changed.status.success());
    assert!(String::from_utf8_lossy(&changed.stderr).contains("event ID was already used"));
}

#[test]
fn advanced_first_explicit_add_enters_pending_without_creating_a_record() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "user.name", "Fixture"]);
    git(&root, &["config", "user.email", "fixture@example.test"]);
    let receipt: Value = serde_json::from_str(&success(run_unbundled(
        &root,
        &[
            "add",
            "p.first",
            "v=1",
            "--shareability",
            "project",
            "--scope",
            "project",
            "--environment",
            "workspace",
            "--event-id",
            "first-event",
        ],
    )))
    .unwrap();
    assert_eq!(receipt["state"], "captured");
    assert!(!root.join("GROUNDING.yaml").exists());
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["rev-parse", "--verify", "refs/kpopper/pending_grounding"])
            .output()
            .unwrap()
            .status
            .success()
    );
}

#[test]
fn advanced_history_add_captures_a_version_three_contribution() {
    use kpop_native::value::TypedValue as V;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "user.name", "Fixture"]);
    git(&root, &["config", "user.email", "fixture@example.test"]);
    success(run(&root, &["add", "p.base", "v=1"]));
    let before = fs::read(root.join("GROUNDING.yaml")).unwrap();
    let args = [
        "add",
        "p.history",
        "v=2",
        "--shareability",
        "project",
        "--scope",
        "external",
        "--environment",
        "vendor",
        "--event-id",
        "history-authoring",
    ];
    let receipt: Value = serde_json::from_str(&success(run(&root, &args))).unwrap();
    assert_eq!(receipt["state"], "captured");
    assert_eq!(fs::read(root.join("GROUNDING.yaml")).unwrap(), before);
    let manifest = pending_manifest(&root, receipt["revision"].as_str().unwrap());
    let V::Map(manifest) = manifest else {
        panic!("manifest")
    };
    assert_eq!(
        manifest["version"],
        V::Integer(kpop_native::value::Integer::new("3").unwrap())
    );
    assert!(manifest.contains_key("history"));
    let replay: Value = serde_json::from_str(&success(run(&root, &args))).unwrap();
    assert_eq!(replay["replay"], true);
    assert_eq!(replay["revision"], receipt["revision"]);

    let set: Value = serde_json::from_str(&success(run(
        &root,
        &[
            "set",
            "p.base",
            "1",
            "--shareability",
            "project",
            "--scope",
            "project",
            "--environment",
            "workspace",
            "--event-id",
            "history-set",
        ],
    )))
    .unwrap();
    let set_manifest = pending_manifest(&root, set["revision"].as_str().unwrap());
    let V::Map(set_manifest) = set_manifest else {
        panic!("manifest")
    };
    assert_eq!(
        set_manifest["version"],
        V::Integer(kpop_native::value::Integer::new("3").unwrap())
    );
    assert_eq!(fs::read(root.join("GROUNDING.yaml")).unwrap(), before);
}

#[test]
fn advanced_local_scopes_are_written_and_project_review_is_refused() {
    let (_temp, root) = advanced_ordinary("known:\n  p.base: {v: 1}\n");
    success(run_unbundled(
        &root,
        &[
            "add",
            "p.feature",
            "v=2",
            "--shareability",
            "project",
            "--scope",
            "feature",
            "--environment",
            "checkout",
        ],
    ));
    let record = fs::read_to_string(root.join("GROUNDING.yaml")).unwrap();
    assert!(record.contains("kind: feature"));
    assert!(record.contains("environment: checkout"));
    let review = run_unbundled(
        &root,
        &[
            "review",
            "p.feature",
            "--shareability",
            "project",
            "--scope",
            "external",
            "--environment",
            "vendor",
        ],
    );
    assert!(!review.status.success());
    assert!(String::from_utf8_lossy(&review.stderr).contains("not a review refresh"));
}

#[test]
fn advanced_scoped_flow_set_matches_python_field_order() {
    let (_temp, root) = advanced_ordinary(
        "known:\n  p.base: {name: Package count, v: 1, at: line 2}\n",
    );
    success(run_unbundled(
        &root,
        &["set", "p.base", "5", "--shareability", "project", "--scope", "feature", "--environment", "x", "--as-of", "2026-09-20"],
    ));
    assert_eq!(
        fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
        "known:\n  p.base:\n    name: \"Package count\"\n    v: 5\n    at: \"line 2\"\n    of: \"2026-09-20\"\n    scope: {kind: feature, environment: x}\n"
    );
}

#[test]
fn advanced_scoped_block_set_quotes_multiword_name_like_python() {
    let (_temp, root) = advanced_ordinary(
        "known:\n  p.base:\n    name: Package count\n    v: 1\n    of: \"2026-09-01\"\n",
    );
    success(run_unbundled(
        &root,
        &["set", "p.base", "5", "--shareability", "project", "--scope", "feature", "--environment", "x", "--as-of", "2026-09-20"],
    ));
    assert_eq!(
        fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
        "known:\n  p.base:\n    name: \"Package count\"\n    v: 5\n    of: \"2026-09-20\"\n    scope: {kind: feature, environment: x}\n"
    );
}

#[test]
fn advanced_scoped_block_set_quotes_multiword_at_like_python() {
    let (_temp, root) = advanced_ordinary(
        "known:\n  p.base:\n    v: 1\n    of: \"2026-09-01\"\n    at: line 2\n",
    );
    success(run_unbundled(
        &root,
        &["set", "p.base", "5", "--shareability", "project", "--scope", "feature", "--environment", "x", "--as-of", "2026-09-20"],
    ));
    assert_eq!(
        fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
        "known:\n  p.base:\n    v: 5\n    of: \"2026-09-20\"\n    at: \"line 2\"\n    scope: {kind: feature, environment: x}\n"
    );
}

#[test]
fn advanced_set_and_judgment_capture_complete_authored_bodies() {
    use kpop_native::value::TypedValue as V;
    let (_temp, root) = advanced_ordinary(
        "known:\n  p.base: {v: 1}\njudgments:\n  d.old:\n    verdict: proceed\n    rests_on: [p.base]\n    reopened_by: new reading\n    seen: {p.base: 1}\n",
    );
    let before = fs::read(root.join("GROUNDING.yaml")).unwrap();
    let set: Value = serde_json::from_str(&success(run_unbundled(
        &root,
        &[
            "set",
            "p.base",
            "1",
            "--shareability",
            "project",
            "--scope",
            "project",
            "--environment",
            "workspace",
            "--event-id",
            "set-event",
        ],
    )))
    .unwrap();
    let manifest = pending_manifest(&root, set["revision"].as_str().unwrap());
    let V::Map(manifest) = manifest else {
        panic!("manifest")
    };
    let V::Map(document) = &manifest["document"] else {
        panic!("document")
    };
    let V::Map(known) = &document["known"] else {
        panic!("known")
    };
    let V::Map(body) = &known["p.base"] else {
        panic!("body")
    };
    assert!(body.contains_key("of"));
    let V::Map(scope) = &body["scope"] else {
        panic!("scope")
    };
    assert_eq!(scope["environment"], V::Text("workspace".into()));

    let judgment_output = success(run_unbundled(
        &root,
        &[
            "add",
            "d.new",
            "verdict=continue",
            "rests_on=[p.base]",
            "reopened_by=new reading",
            "--shareability",
            "project",
            "--scope",
            "external",
            "--environment",
            "vendor",
            "--event-id",
            "judgment-event",
        ],
    ));
    let (notice, receipt) = judgment_output.trim_end().rsplit_once('\n').unwrap();
    assert_eq!(notice, "nearest existing:\n  d.old: rests on p.base too - verdicts differ, a pair to judge\n  one subject: same <id> d.new folds it in · two: distinct d.new <id> \"why\" keeps them apart");
    let judgment: Value = serde_json::from_str(receipt).unwrap();
    let manifest = pending_manifest(&root, judgment["revision"].as_str().unwrap());
    let V::Map(manifest) = manifest else {
        panic!("manifest")
    };
    let V::Map(document) = &manifest["document"] else {
        panic!("document")
    };
    let V::Map(judgments) = &document["judgments"] else {
        panic!("judgments")
    };
    let V::Map(body) = &judgments["d.new"] else {
        panic!("body")
    };
    let V::Map(seen) = &body["seen"] else {
        panic!("seen")
    };
    assert_eq!(
        seen["p.base"],
        V::Integer(kpop_native::value::Integer::new("1").unwrap())
    );
    assert_eq!(fs::read(root.join("GROUNDING.yaml")).unwrap(), before);
}

#[test]
fn advanced_private_and_code_routes_never_enter_the_pending_queue() {
    let (_temp, root) = advanced_ordinary("known:\n  p.base: {v: 1}\n");
    let private = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(&root)
        .env_remove("KPOPPER_NATIVE_RESOURCES")
        .env("KPOPPER_PRIVATE_HOME", private.path())
        .args([
            "add",
            "p.secret",
            "v=2",
            "--shareability",
            "private",
            "--scope",
            "external",
            "--environment",
            "vendor",
            "--event-id",
            "private-event",
        ])
        .output()
        .unwrap();
    let draft: Value = serde_json::from_str(&success(output)).unwrap();
    assert_eq!(draft["state"], "private draft");
    assert!(
        !Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["rev-parse", "--verify", "refs/kpopper/pending_grounding"])
            .output()
            .unwrap()
            .status
            .success()
    );

    let bad = run_unbundled(
        &root,
        &[
            "add",
            "p.code",
            "v=2",
            "--shareability",
            "project",
            "--scope",
            "code",
            "--environment",
            "checkout",
            "--commit",
            "ABC",
        ],
    );
    assert!(!bad.status.success());
    let commit = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    success(run_unbundled(
        &root,
        &[
            "add",
            "p.code",
            "v=2",
            "--shareability",
            "project",
            "--scope",
            "code",
            "--environment",
            "checkout",
            "--commit",
            commit,
        ],
    ));
    let record = fs::read_to_string(root.join("GROUNDING.yaml")).unwrap();
    assert!(record.contains("kind: code"));
    assert!(record.contains(commit));
}
#[test]
fn empty_tmpdir_keeps_publication_receipts_outside_the_workspace() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let sessions = tempfile::tempdir().unwrap();
    let sid = format!(
        "empty-tmp-{}",
        &kpop_native::identity::sha256(root.to_string_lossy().as_bytes())[..16]
    );
    success(
        command(&root)
            .env("TMPDIR", "")
            .env("TEMP", sessions.path())
            .env_remove("TMP")
            .env("KPOPPER_AGENT_SESSION", &sid)
            .args(["add", "p.x", "v=1"])
            .output()
            .unwrap(),
    );
    assert!(!root.join(format!("kpopper-session-{sid}")).exists());
    let receipt_home = sessions.path().join(format!("kpopper-session-{sid}"));
    assert!(receipt_home.is_dir());
    assert!(fs::read_dir(receipt_home).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("writes-")
    }));
}

#[test]
fn first_add_creates_history_and_subsequent_set_retains_the_original_version() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let first = success(run(&root, &["add", "p.x", "v=1"]));
    assert!(first.contains("history committed:"));
    assert!(first.contains("born with its first entry"));
    let before: Value = serde_json::from_str(&success(run(&root, &["history", "status"]))).unwrap();
    let old = before["subjects"]["p.x"]["heads"][0].clone();
    let yesterday = chrono::Utc::now()
        .date_naive()
        .max(chrono::Local::now().date_naive())
        .pred_opt()
        .unwrap()
        .to_string();
    success(run(
        &root,
        &[
            "set",
            "p.x",
            "2",
            "--as-of",
            &yesterday,
            "--why",
            "new reading",
        ],
    ));
    let after: Value = serde_json::from_str(&success(run(&root, &["history", "status"]))).unwrap();
    assert_eq!(after["commits"], 2);
    assert_ne!(after["subjects"]["p.x"]["heads"][0], old);
    assert!(
        fs::read_to_string(root.join("GROUNDING.yaml"))
            .unwrap()
            .contains("v: 2")
    );
    success(run(
        &root,
        &[
            "add",
            "d.work",
            "verdict=continue",
            "rests_on=[p.x]",
            "reopened_by=new readings",
        ],
    ));
    success(run(
        &root,
        &["set", "p.x", "3", "--why", "a further observation"],
    ));
    assert!(success(run(&root, &["review", "d.work"])).contains("(review d.work)"));
}

fn history_record_with_one_source() -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::create_dir(root.join("notes")).unwrap();
    fs::write(root.join("notes/c.txt"), "Hours: 10\nRate: 50\n").unwrap();
    fs::write(root.join("notes/d.txt"), "Days: 3\n").unwrap();
    let first = success(run(
        &root,
        &[
            "add",
            "src.c",
            "file=notes/c.txt",
            "name=C",
            "read=2026-09-20",
        ],
    ));
    assert!(first.starts_with("history committed: "), "{first}");
    (temp, root)
}

#[test]
fn history_add_names_the_nearest_existing_entries_once_before_the_commit() {
    let (_temp, root) = history_record_with_one_source();
    let day = ["--as-of", "2026-09-20"];
    success(run(
        &root,
        &[&["add", "p.hours", "v=10", "from=src.c"][..], &day].concat(),
    ));
    let output = success(run(
        &root,
        &[&["add", "p.rate", "v=50", "from=src.c"][..], &day].concat(),
    ));
    let (notice, committed) = output.split_once("history committed: ").unwrap();
    assert_eq!(
        notice,
        "nearest existing:\n  p.hours: same from (src.c)\n  one subject: same <id> p.rate folds it in · two: distinct p.rate <id> \"why\" keeps them apart\n"
    );
    assert!(committed.ends_with(" (add p.rate)\n"), "{output}");
    let record = fs::read_to_string(root.join("GROUNDING.yaml")).unwrap();
    assert!(
        record.contains("  p.rate:\n    from: src.c\n    v: 50\n"),
        "{record}"
    );

    // Judgments read the record's dependency field: shared premises make a pair.
    let fine = [
        "add",
        "d.cost",
        "verdict=Cost is fine",
        "rests_on=[p.hours, p.rate]",
        "wrong_if={expr: \"p.rate > 100\"}",
    ];
    let first = success(run(&root, &[&fine[..], &day].concat()));
    assert!(first.starts_with("history committed: "), "{first}");
    let high = [
        "add",
        "d.cost2",
        "verdict=Cost is high",
        "rests_on=[p.hours, p.rate]",
        "wrong_if={expr: \"p.rate > 200\"}",
    ];
    let output = success(run(&root, &[&high[..], &day].concat()));
    let (notice, committed) = output.split_once("history committed: ").unwrap();
    assert_eq!(
        notice,
        "nearest existing:\n  d.cost: rests on p.hours, p.rate too - verdicts differ, a pair to judge\n  one subject: same <id> d.cost2 folds it in · two: distinct d.cost2 <id> \"why\" keeps them apart\n"
    );
    assert!(committed.ends_with(" (add d.cost2)\n"), "{output}");

    // A core rule is stored as an expression and named by its text.
    let rule = ["rule={expr: \"p.hours * p.rate\"}"];
    let first = success(run(&root, &[&["add", "p.total"][..], &rule, &day].concat()));
    assert!(first.starts_with("history committed: "), "{first}");
    let output = success(run(
        &root,
        &[&["add", "p.total2"][..], &rule, &day].concat(),
    ));
    assert!(
        output.starts_with("nearest existing:\n  p.total: same rule (p.hours * p.rate)\n"),
        "{output}"
    );
}

#[test]
fn history_hypothesis_add_names_the_nearest_existing_entries_under_its_layer() {
    let (_temp, root) = history_record_with_one_source();
    let day = ["--as-of", "2026-09-20"];
    let into = ["--hypothesis", "rates"];
    let note = |output: &str| {
        output
            .split_once("history committed: ")
            .unwrap()
            .0
            .to_owned()
    };
    success(run(
        &root,
        &[&["add", "p.hours", "v=10", "from=src.c"][..], &day].concat(),
    ));
    let output = success(run(
        &root,
        &[&["add", "p.rate", "v=50", "from=src.c"][..], &into, &day].concat(),
    ));
    assert_eq!(
        note(&output),
        "nearest existing:\n  p.hours: same from (src.c)\n  one subject: same <id> p.rate folds it in · two: distinct p.rate <id> \"why\" keeps them apart\n"
    );
    assert!(output.ends_with(" (add p.rate)\n"), "{output}");
    // A later write into the same hypothesis also reads what it alone holds.
    let output = success(run(
        &root,
        &[&["add", "p.rate2", "v=51", "from=src.c"][..], &into, &day].concat(),
    ));
    assert_eq!(
        note(&output),
        "nearest existing:\n  p.hours: same from (src.c)\n  p.rate: same from (src.c)\n  one subject: same <id> p.rate2 folds it in · two: distinct p.rate2 <id> \"why\" keeps them apart\n"
    );
    // The base does not, and neither does another hypothesis.
    let output = success(run(
        &root,
        &[&["add", "p.fee", "v=5", "from=src.c"][..], &day].concat(),
    ));
    assert_eq!(
        note(&output),
        "nearest existing:\n  p.hours: same from (src.c)\n  one subject: same <id> p.fee folds it in · two: distinct p.fee <id> \"why\" keeps them apart\n"
    );
    let other = ["--hypothesis", "other"];
    let output = success(run(
        &root,
        &[&["add", "p.rate3", "v=52", "from=src.c"][..], &other, &day].concat(),
    ));
    assert_eq!(
        note(&output),
        "nearest existing:\n  p.fee: same from (src.c)\n  p.hours: same from (src.c)\n  one subject: same <id> p.rate3 folds it in · two: distinct p.rate3 <id> \"why\" keeps them apart\n"
    );
}

#[test]
fn history_writes_with_nothing_near_say_only_the_commit() {
    let (_temp, root) = history_record_with_one_source();
    let day = ["--as-of", "2026-09-20"];
    for (subject, fields) in [
        ("p.hours", ["v=10", "from=src.c"]),
        ("src.d", ["file=notes/d.txt", "name=D"]),
        ("p.days", ["v=3", "from=src.d"]),
    ] {
        let output = success(run(&root, &[&["add", subject][..], &fields, &day].concat()));
        assert!(output.starts_with("history committed: "), "{output}");
        assert!(output.ends_with(&format!(" (add {subject})\n")), "{output}");
    }
    success(run(
        &root,
        &[&["add", "p.rate", "v=50", "from=src.c"][..], &day].concat(),
    ));
    // Only an add hears the note: a newer reading of an entry with a near
    // neighbour is set without one.
    let output = success(run(
        &root,
        &["set", "p.hours", "11", "--as-of", "2026-09-21"],
    ));
    assert!(output.starts_with("history committed: "), "{output}");
}

#[test]
fn named_hypothesis_cli_writes_stay_out_of_base_and_support_add_set_review() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    success(run(&root, &["add", "p.base", "v=10"]));
    success(run(
        &root,
        &["add", "p.only", "v=1", "--hypothesis", "alpha"],
    ));
    success(run(&root, &["set", "p.only", "2", "--hypothesis", "alpha"]));
    success(run(
        &root,
        &[
            "add",
            "d.named",
            "verdict=continue",
            "rests_on=[p.only]",
            "reopened_by=new readings",
            "--hypothesis",
            "alpha",
        ],
    ));
    assert!(
        success(run(&root, &["review", "d.named", "--hypothesis", "alpha"],))
            .contains("(review d.named)")
    );
    let record = kpop_native::history_yaml::decode_source_document(
        &fs::read(root.join("GROUNDING.yaml")).unwrap(),
    )
    .unwrap();
    let known = record.get("known").unwrap();
    assert_eq!(
        known
            .get("p.base")
            .unwrap()
            .get("v")
            .unwrap()
            .typed()
            .to_tagged()
            .unwrap(),
        serde_json::json!(["int", "10"])
    );
    assert!(known.get("p.only").is_none());
    assert!(
        record
            .get("judgments")
            .and_then(|v| v.get("d.named"))
            .is_none()
    );
    let status: Value = serde_json::from_str(&success(run(&root, &["history", "status"]))).unwrap();
    assert_eq!(status["subjects"]["p.base"]["acceptance"], "accepted");
    assert_eq!(status["subjects"]["p.only"]["acceptance"], "proposed");
    assert_eq!(status["subjects"]["d.named"]["acceptance"], "proposed");
}

#[test]
fn first_named_add_bootstraps_a_proposal_without_accepting_it() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let output = success(run(
        &root,
        &["add", "p.first", "v=1", "--hypothesis", "opening"],
    ));
    assert!(output.contains("born with its first entry"));
    let record = kpop_native::history_yaml::decode_source_document(
        &fs::read(root.join("GROUNDING.yaml")).unwrap(),
    )
    .unwrap();
    assert!(record.get("known").unwrap().get("p.first").is_none());
    let status: Value = serde_json::from_str(&success(run(&root, &["history", "status"]))).unwrap();
    assert_eq!(status["subjects"]["p.first"]["acceptance"], "proposed");
}

#[test]
fn named_hypothesis_rejects_invalid_and_physical_authority_names() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    success(run(&root, &["add", "p.base", "v=1"]));
    let invalid = run(&root, &["add", "p.bad", "v=2", "--hypothesis", "../bad"]);
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("invalid_history_hypothesis"));
    fs::create_dir_all(root.join(".kpopper/hypotheses")).unwrap();
    fs::write(
        root.join(".kpopper/hypotheses/taken.yaml"),
        "known: {p.physical: {v: 3}}\n",
    )
    .unwrap();
    let collision = run(&root, &["add", "p.bad", "v=2", "--hypothesis", "taken"]);
    assert!(!collision.status.success());
    assert!(String::from_utf8_lossy(&collision.stderr).contains("hypothesis_authority_collision"));
}

#[test]
fn named_privacy_checks_use_hypothesis_only_dependencies() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let private = tempfile::tempdir().unwrap();
    success(run(&root, &["add", "p.base", "v=1"]));
    success(run(
        &root,
        &["add", "p.only", "v=2", "--hypothesis", "alpha"],
    ));
    let before = fs::read(root.join("GROUNDING.yaml")).unwrap();
    let output = command(&root)
        .env("KPOPPER_PRIVATE_HOME", private.path())
        .args([
            "add",
            "d.private",
            "verdict=stop",
            "rests_on=[p.only]",
            "private=true",
            "--hypothesis",
            "alpha",
        ])
        .output()
        .unwrap();
    let result: Value = serde_json::from_str(&success(output)).unwrap();
    assert_eq!(result["state"], "private draft");
    assert_eq!(fs::read(root.join("GROUNDING.yaml")).unwrap(), before);
    assert!(!success(run(&root, &["history", "status"])).contains("d.private"));
}
#[test]
fn refused_first_add_leaves_no_partial_record_or_authority() {
    let temp = tempfile::tempdir().unwrap();
    let output = run(
        temp.path(),
        &[
            "add",
            "d.bad",
            "rests_on=[p.missing]",
            "wrong_if={expr: p.missing > 3}",
        ],
    );
    assert!(!output.status.success());
    assert!(!temp.path().join("GROUNDING.yaml").exists());
    assert!(!temp.path().join(".kpopper/history.yaml").exists());
}

#[test]
fn public_recovery_finishes_or_cancels_a_retained_first_write() {
    use kpop_native::{
        history_bootstrap as B,
        history_transaction::Layout,
        reasoning_runtime::{OperationalBounds, Runtime},
        value::TypedValue as V,
    };
    for rollback in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let entry = root.join("GROUNDING.yaml");
        let archive = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(format!(
                "{}.kpopper-runtime",
                kpop_native::reasoning_runtime::target_name().unwrap()
            ));
        let runtime = Runtime::open(
            &archive,
            &root.join(".test-cache"),
            OperationalBounds::default(),
        )
        .unwrap();
        let policy = kpop_native::project_modes::Project::open(&root)
            .unwrap()
            .config()
            .unwrap();
        let action =
            V::from_json(&serde_json::json!({"kind":"add","id":"p.x","body":{"v":1}})).unwrap();
        let mutation = B::prepare(
            &entry,
            &action,
            &policy,
            &B::BootstrapOptions {
                operation: "first-recovery".into(),
                recorded_at: "2026-09-19T12:00:00+00:00".into(),
                recording_day: "2026-09-19".into(),
                record_id: "fixture".into(),
                by: V::Null,
            },
            Some(&runtime),
        )
        .unwrap();
        let journal = root.join(Layout::for_entry("GROUNDING.yaml").unwrap().journal);
        fs::create_dir_all(journal.parent().unwrap()).unwrap();
        fs::write(&journal, mutation.to_bytes().unwrap()).unwrap();
        let result = success(run(
            &root,
            if rollback {
                &["recover", "--json", "--rollback"]
            } else {
                &["recover", "--json"]
            },
        ));
        let result: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(
            result["state"],
            if rollback { "restored" } else { "recovered" }
        );
        assert_eq!(entry.exists(), !rollback);
        assert!(!journal.exists());
    }
}

#[test]
fn a_private_first_or_later_add_stays_outside_the_record() {
    for existing in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let private = tempfile::tempdir().unwrap();
        // Initialize only the explicitly selected test runtime if no record exists.
        if existing {
            success(run(&root, &["add", "p.public", "v=1"]));
        } else {
            assert!(
                !run(&root, &["add", "d.bad", "rests_on=[missing]"])
                    .status
                    .success()
            );
        }
        let path = root.join("GROUNDING.yaml");
        let before = fs::read(&path).ok();
        let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
            .current_dir(&root)
            .env("KPOPPER_NATIVE_RESOURCES", root.join(".test-runtime"))
            .env("KPOPPER_NATIVE_CACHE", root.join(".test-cache"))
            .env("KPOPPER_PRIVATE_HOME", private.path())
            .args(["add", "p.private", "v=secret", "private=true"])
            .output()
            .unwrap();
        let result: Value = serde_json::from_str(&success(output)).unwrap();
        assert_eq!(result["state"], "private draft");
        assert!(
            Path::new(result["path"].as_str().unwrap())
                .starts_with(private.path().canonicalize().unwrap())
        );
        assert_eq!(fs::read(&path).ok(), before);
    }
}

#[test]
fn explicit_core_acts_and_proposals_need_only_the_core_program() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    success(run(&root, &["add", "p.x", "v=1"]));
    let state: Value = serde_json::from_str(&success(run(&root, &["history", "status"]))).unwrap();
    let version = state["subjects"]["p.x"]["heads"][0].as_str().unwrap();
    for action in ["retire", "accept"] {
        let result: Value = serde_json::from_str(&success(run(
            &root,
            &[
                "history",
                action,
                "--subject",
                "p.x",
                "--of",
                version,
                "--because",
                "explicit test disposition",
            ],
        )))
        .unwrap();
        assert_eq!(result["state"], "committed");
    }
    let record = root.join("GROUNDING.yaml");
    let text = fs::read_to_string(&record).unwrap();
    assert!(text.contains("v: 1"));
    fs::write(&record, text.replace("v: 1", "v: 8")).unwrap();
    let result: Value = serde_json::from_str(&success(run(
        &root,
        &[
            "history",
            "reconcile",
            "--record-proposals",
            "--proposal-subject",
            "p.x",
            "--because",
            "retain an edited reading",
        ],
    )))
    .unwrap();
    assert_eq!(result["state"], "proposed");
    assert!(!root.join(".test-runtime/ordinary").exists());
}

#[test]
fn unresolved_candidate_references_reach_the_authoring_diagnostic() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    success(run(&root, &["add", "p.seed", "v=1"]));
    let before = fs::read(root.join("GROUNDING.yaml")).unwrap();
    for (args, expected) in [
        (
            vec!["add", "d.bad", "verdict=stop", "rests_on=[p.missing]"],
            "rests on p.missing, which is not an entry",
        ),
        (
            vec!["add", "p.bad", "v={{p.missing}}"],
            "v references p.missing, which is not an entry",
        ),
    ] {
        let output = run(&root, &args);
        assert!(!output.status.success());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains(expected), "{error}");
        assert!(!error.contains("missing_contribution_dependency"));
        assert_eq!(fs::read(root.join("GROUNDING.yaml")).unwrap(), before);
    }
}

#[test]
fn session_receipts_follow_successful_publication_and_cannot_break_a_write() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let sessions = tempfile::tempdir().unwrap();
    let call = |args: &[&str]| {
        command(&root)
            .env("KPOPPER_AGENT_SESSION", "publication-test")
            .env("TMPDIR", sessions.path())
            .args(args)
            .output()
            .unwrap()
    };
    success(call(&["add", "p.x", "v=1"]));
    let record = root.join("GROUNDING.yaml");
    let key = kpop_native::identity::sha256(record.to_str().unwrap().as_bytes());
    let receipt = sessions
        .path()
        .join("kpopper-session-publication-test")
        .join(format!("writes-{key}.json"));
    let current =
        kpop_native::history_yaml::decode_source_document(&fs::read(&record).unwrap()).unwrap();
    let state: Value = serde_json::from_slice(&fs::read(&receipt).unwrap()).unwrap();
    assert_eq!(
        state["p.x"],
        current
            .get("known")
            .unwrap()
            .get("p.x")
            .unwrap()
            .typed()
            .digest()
            .unwrap()
    );
    let before = fs::read(&receipt).unwrap();
    assert!(
        !call(&["add", "d.bad", "rests_on=[missing]"])
            .status
            .success()
    );
    assert_eq!(fs::read(&receipt).unwrap(), before);
    let captured = kpop_native::source_capture::capture_source(
        std::slice::from_ref(&record),
        &root,
        kpop_native::source_capture::ReadMode::Frozen,
        None,
    )
    .unwrap();
    assert_eq!(captured.origins()["known"]["p.x"], record);
    let status: Value = serde_json::from_str(&success(call(&["history", "status"]))).unwrap();
    let version = status["subjects"]["p.x"]["heads"][0].as_str().unwrap();
    success(call(&[
        "history",
        "retire",
        "--subject",
        "p.x",
        "--of",
        version,
        "--because",
        "explicitly retired",
    ]));
    let state: Value = serde_json::from_slice(&fs::read(&receipt).unwrap()).unwrap();
    assert!(state.get("p.x").is_none());
    // An unusable optional attribution directory never changes publication success.
    let bad = sessions.path().join("kpopper-session-unavailable");
    fs::write(&bad, b"not a directory").unwrap();
    let output = command(&root)
        .env("KPOPPER_AGENT_SESSION", "unavailable")
        .env("TMPDIR", sessions.path())
        .args(["add", "p.other", "v=2"])
        .output()
        .unwrap();
    success(output);
    assert_eq!(fs::read(bad).unwrap(), b"not a directory");
}

const CUSTOM_ROLES: &str = "schema: {deps: requires, predicate: fails_if}\n";
const CUSTOM_RECORD: &str = "known:\n  api.limit: {v: 10}\njudgments:\n  d.w: {verdict: known, requires: [api.limit], fails_if: \"api.limit > 100\"}\n";
const CUSTOM_JUDGMENT: &[&str] = &[
    "add",
    "d.z",
    "verdict=x",
    "requires=[api.limit]",
    "fails_if=api.limit > 50",
];

fn simple_workspace(source: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::write(root.join("GROUNDING.yaml"), source).unwrap();
    (temp, root)
}

fn record(root: &Path) -> String {
    fs::read_to_string(root.join("GROUNDING.yaml")).unwrap()
}

// The expected records in the tests below are what the Python writer leaves
// for the same commands. Where no judgment carries a snapshot yet, the first
// judgment written keeps what it saw under `seen`.

#[test]
fn custom_dependency_and_predicate_fields_write_as_the_python_writer_does() {
    for declared in [CUSTOM_ROLES, ""] {
        let (_temp, root) = simple_workspace(&format!("{declared}{CUSTOM_RECORD}"));
        assert_eq!(
            success(run_unbundled(&root, &["add", "api.window", "v=5"])),
            "add api.window into known, after api.limit\n\nthe record needs a person on 0 judgments - check says the rest\n"
        );
        assert_eq!(
            record(&root),
            format!(
                "{declared}known:\n  api.limit: {{v: 10}}\n  api.window: {{v: 5}}\njudgments:\n  d.w: {{verdict: known, requires: [api.limit], fails_if: \"api.limit > 100\"}}\n"
            )
        );

        let (_temp, root) = simple_workspace(&format!("{declared}{CUSTOM_RECORD}"));
        let output = success(run_unbundled(&root, CUSTOM_JUDGMENT));
        assert!(
            output.contains("add d.z into judgments, after d.w\nthe new judgment holds: wrong_if does not hold (api.limit > 50)\n"),
            "{output}"
        );
        assert_eq!(
            record(&root),
            format!(
                "{declared}{CUSTOM_RECORD}\n  d.z:\n    verdict: x\n    requires: [api.limit]\n    fails_if: \"api.limit > 50\"\n    seen: {{api.limit: 10}}\n"
            )
        );
    }
}

#[test]
fn a_record_without_snapshots_is_set_reviewed_and_proposed_to_as_the_python_writer_does() {
    let (_temp, root) = simple_workspace(&format!("{CUSTOM_ROLES}{CUSTOM_RECORD}"));
    assert_eq!(
        success(run_unbundled(
            &root,
            &["set", "api.limit", "11", "--as-of", "2026-01-01"]
        )),
        "set api.limit: 10 -> 11 (as of 2026-01-01)\nrests on it:\n  HOLDS     d.w: wrong_if does not hold (api.limit > 100)\n\nthe record needs a person on 0 judgments - check says the rest\n"
    );
    assert_eq!(
        record(&root),
        format!(
            "{CUSTOM_ROLES}known:\n  api.limit: {{v: 11, of: \"2026-01-01\"}}\njudgments:\n  d.w: {{verdict: known, requires: [api.limit], fails_if: \"api.limit > 100\"}}\n"
        )
    );

    // The snapshot a review takes closes the judgment's block, after its comment.
    let block = "known:\n  api.limit: {v: 10}\njudgments:\n  d.w:\n    verdict: known\n    requires: [api.limit]\n    fails_if: \"api.limit > 100\"\n    # kept with the judgment\n";
    let (_temp, root) = simple_workspace(block);
    assert_eq!(
        success(run_unbundled(
            &root,
            &["review", "d.w", "--as-of", "2026-01-01"]
        )),
        "review d.w: seen rewritten from what the record holds (2026-01-01)\n  api.limit: 10 (never checked against it before)\n  d.w holds: wrong_if does not hold (api.limit > 100)\n"
    );
    assert_eq!(
        record(&root),
        format!("{block}    seen: {{api.limit: 10}}\n")
    );

    let (_temp, root) = simple_workspace(&format!("{CUSTOM_ROLES}{CUSTOM_RECORD}"));
    let mut proposal = CUSTOM_JUDGMENT.to_vec();
    proposal.extend(["--hypothesis", "proposal", "--as-of", "2026-01-01"]);
    let output = success(run_unbundled(&root, &proposal));
    assert!(
        output.ends_with("\nthe base is untouched; proposal holds 0 entries and 1 judgment\n"),
        "{output}"
    );
    assert_eq!(record(&root), format!("{CUSTOM_ROLES}{CUSTOM_RECORD}"));
    assert_eq!(
        fs::read_to_string(root.join(".kpopper/hypotheses/proposal.yaml")).unwrap(),
        "hypothesis: {born: \"2026-01-01\"}\n\njudgments:\n  d.z:\n    verdict: x\n    requires: [api.limit]\n    fails_if: \"api.limit > 50\"\n    seen: {api.limit: 10}\n"
    );
}

#[test]
fn a_review_writes_inside_the_braces_of_a_judgment_written_on_one_line() {
    // Nothing is written under it: there it would be read as another judgment of the
    // collection, or break the file.
    let judgment = r#"verdict: known, requires: [api.limit], fails_if: "api.limit > 100""#;
    let trail = r#"replaced: ["the limit rose on 2025-12-01"]"#;
    let cases = [
        (
            judgment.to_owned(),
            format!("{judgment}, seen: {{api.limit: 10}}"),
        ),
        (
            format!("{judgment}, seen: {{api.limit: 5}}"),
            format!("{judgment}, seen: {{api.limit: 10}}"),
        ),
        (
            format!("{judgment}, seen: {{api.limit: 10}}, {trail}"),
            format!(r#"{judgment}, seen: {{api.limit: 10}}, {trail}, reviewed: "2026-01-01""#),
        ),
        (
            format!("{judgment}, {trail}, seen: {{api.limit: 5}}"),
            format!(r#"{judgment}, {trail}, reviewed: "2026-01-01", seen: {{api.limit: 10}}"#),
        ),
        (
            format!("{judgment}, seen: {{api.limit: 10}}, reviewed: '2025-12-01'"),
            format!("{judgment}, seen: {{api.limit: 10}}, reviewed: '2026-01-01'"),
        ),
    ];
    for indent in ["  ", "    "] {
        for (body, want) in &cases {
            let head = format!("known:\n{indent}api.limit: {{v: 10}}\njudgments:\n{indent}d.w: {{");
            let (_temp, root) = simple_workspace(&format!("{head}{body}}}  # read by hand {{x\n"));
            success(run_unbundled(
                &root,
                &["review", "d.w", "--as-of", "2026-01-01"],
            ));
            assert_eq!(
                record(&root),
                format!("{head}{want}}}  # read by hand {{x\n")
            );
        }
    }
}

#[test]
fn a_review_in_a_named_hypothesis_writes_inside_the_braces_of_a_one_line_judgment() {
    let judgment = r#"verdict: known, requires: [api.limit], fails_if: "api.limit > 100""#;
    for indent in ["  ", "    "] {
        for (body, want) in [
            (
                judgment.to_owned(),
                format!("{judgment}, seen: {{api.limit: 10}}"),
            ),
            (
                format!(r#"{judgment}, seen: {{api.limit: 10}}, reviewed: "2025-12-01""#),
                format!(r#"{judgment}, seen: {{api.limit: 10}}, reviewed: "2026-01-01""#),
            ),
        ] {
            let base = format!("known:\n{indent}api.limit: {{v: 10}}\n");
            let (_temp, root) = simple_workspace(&base);
            let path = root.join(".kpopper/hypotheses/limit.yaml");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let head =
                format!("hypothesis: {{born: \"2025-12-01\"}}\n\njudgments:\n{indent}d.w: {{");
            fs::write(&path, format!("{head}{body}}}\n")).unwrap();
            success(run_unbundled(
                &root,
                &[
                    "review",
                    "d.w",
                    "--as-of",
                    "2026-01-01",
                    "--hypothesis",
                    "limit",
                ],
            ));
            assert_eq!(
                fs::read_to_string(&path).unwrap(),
                format!("{head}{want}}}\n")
            );
            assert_eq!(record(&root), base);
        }
    }
}

#[test]
fn a_review_of_a_judgment_that_is_an_alias_leaves_the_record() {
    let source = "known:\n  api.limit: {v: 10}\njudgments:\n  d.v: &same {verdict: known, requires: [api.limit], fails_if: \"api.limit > 100\", seen: {api.limit: 5}}\n  d.w: *same\n";
    let (_temp, root) = simple_workspace(source);
    let output = run_unbundled(&root, &["review", "d.w", "--as-of", "2026-01-01"]);
    assert!(!output.status.success());
    assert_eq!(record(&root), source);
}

#[test]
fn a_snapshot_written_by_hand_is_replaced_in_place_as_the_python_writer_does() {
    let mut add = CUSTOM_JUDGMENT.to_vec();
    add.insert(4, "seen={api.limit: 1}");
    let judgment = "  d.z:\n    verdict: x\n    requires: [api.limit]\n    seen: {api.limit: 10}\n    fails_if: \"api.limit > 50\"\n";
    let (_temp, root) = simple_workspace(&format!("{CUSTOM_ROLES}{CUSTOM_RECORD}"));
    success(run_unbundled(&root, &add));
    assert_eq!(
        record(&root),
        format!("{CUSTOM_ROLES}{CUSTOM_RECORD}\n{judgment}")
    );

    let (_temp, root) = simple_workspace(&format!("{CUSTOM_ROLES}{CUSTOM_RECORD}"));
    add.extend(["--hypothesis", "proposal", "--as-of", "2026-01-01"]);
    success(run_unbundled(&root, &add));
    assert_eq!(record(&root), format!("{CUSTOM_ROLES}{CUSTOM_RECORD}"));
    assert_eq!(
        fs::read_to_string(root.join(".kpopper/hypotheses/proposal.yaml")).unwrap(),
        format!("hypothesis: {{born: \"2026-01-01\"}}\n\njudgments:\n{judgment}")
    );
}

#[test]
fn custom_field_roles_contribute_what_the_python_writer_captures() {
    use kpop_native::value::{Integer, TypedValue as V};
    let (_temp, root) = advanced_ordinary(&format!("{CUSTOM_ROLES}{CUSTOM_RECORD}"));
    let before = fs::read(root.join("GROUNDING.yaml")).unwrap();
    let mut args = CUSTOM_JUDGMENT.to_vec();
    args.extend([
        "--shareability",
        "project",
        "--scope",
        "external",
        "--environment",
        "vendor",
        "--event-id",
        "e1",
    ]);
    let output = success(run_unbundled(&root, &args));
    let receipt: Value = serde_json::from_str(output.trim_end().lines().last().unwrap()).unwrap();
    assert_eq!(receipt["state"], "captured");
    // The Python writer captures this revision for the same command.
    assert_eq!(
        receipt["revision"],
        "ee81ab5b470e7b3ffe1bcb11c73da80058ea74de93492fbdb8557c39dfcf4e93"
    );
    assert_eq!(fs::read(root.join("GROUNDING.yaml")).unwrap(), before);
    let V::Map(manifest) = pending_manifest(&root, receipt["revision"].as_str().unwrap()) else {
        panic!("manifest")
    };
    let V::Map(document) = &manifest["document"] else {
        panic!("document")
    };
    let V::Map(judgments) = &document["judgments"] else {
        panic!("judgments")
    };
    let V::Map(body) = &judgments["d.z"] else {
        panic!("body")
    };
    assert_eq!(body["requires"], V::List(vec![V::Text("api.limit".into())]));
    let V::Map(seen) = &body["seen"] else {
        panic!("seen")
    };
    assert_eq!(seen["api.limit"], V::Integer(Integer::new("10").unwrap()));
}

#[test]
fn added_fields_follow_the_authored_ones_in_the_python_writers_order() {
    let scope = [
        "--shareability",
        "project",
        "--scope",
        "feature",
        "--environment",
        "checkout",
    ];
    // A snapshot field named before `scope` still comes after it.
    let saw = "known:\n  api.limit: {v: 10}\njudgments:\n  d.w: {verdict: known, requires: [api.limit], fails_if: \"api.limit > 100\", saw: {api.limit: 10}}\n";
    let (_temp, root) = simple_workspace(saw);
    success(run_unbundled(&root, &[CUSTOM_JUDGMENT, &scope].concat()));
    assert_eq!(
        record(&root),
        format!(
            "{saw}\n  d.z:\n    verdict: x\n    requires: [api.limit]\n    fails_if: \"api.limit > 50\"\n    scope: {{kind: feature, environment: checkout}}\n    saw: {{api.limit: 10}}\n"
        )
    );

    // An arrangement's birth day follows the scope and precedes the snapshot.
    let arrangement = "known:\n  s.ask: {asked: \"what next\", read: 2026-01-01}\njudgments:\n  d.plan: {verdict: go, rests_on: [s.ask, graph.open], wrong_if: \"graph.open > 3\", seen: {s.ask: present, graph.open: 0}}\n";
    let (_temp, root) = simple_workspace(arrangement);
    let add = [
        "add",
        "d.next",
        "verdict=wait",
        "rests_on=[s.ask, graph.open]",
        "wrong_if=graph.open > 5",
        "--as-of",
        "2026-01-01",
    ];
    success(run_unbundled(&root, &[&add[..], &scope].concat()));
    assert_eq!(
        record(&root),
        "known:\n  s.ask: {asked: \"what next\", read: 2026-01-01}\njudgments:\n  d.next:\n    verdict: wait\n    rests_on: [s.ask, graph.open]\n    wrong_if: \"graph.open > 5\"\n    scope: {kind: feature, environment: checkout}\n    born: \"2026-01-01\"\n    seen: {s.ask: \"read 2026-01-01\", graph.open: 0}\n  d.plan: {verdict: go, rests_on: [s.ask, graph.open], wrong_if: \"graph.open > 3\", seen: {s.ask: present, graph.open: 0}}\n"
    );
}

#[test]
fn same_and_distinct_rewrite_a_record_without_snapshots() {
    let two = "known:\n  api.limit: {v: 10}\n  api.cap: {v: 10}\njudgments:\n  d.w:\n    verdict: known\n    requires: [api.limit, api.cap]\n    fails_if: \"api.cap > 100\"\n";
    let (_temp, root) = simple_workspace(two);
    // The check line counts the note that the record has no snapshot field.
    assert_eq!(
        success(run_unbundled(&root, &["same", "api.limit", "api.cap"])),
        concat!(
            "same api.limit api.cap: api.cap retired into api.limit\n",
            "  rewritten - requires: d.w · fails_if: d.w\n",
            "  1 mention in GROUNDING.yaml\n",
            "  check: 1 judgments, 2 entries, 0 problems, 1 declared\n",
            "rests on it:\n",
            "  HOLDS     d.w: wrong_if does not hold (api.limit > 100)\n",
            "\n",
            "the record needs a person on 0 judgments - check says the rest\n",
        )
    );
    assert_eq!(
        record(&root),
        "known:\n  api.limit: {v: 10, also: [api.cap]}\njudgments:\n  d.w:\n    verdict: known\n    requires: [api.limit]\n    fails_if: \"api.limit > 100\"\n"
    );

    let (_temp, root) = simple_workspace(two);
    assert_eq!(
        success(run_unbundled(
            &root,
            &[
                "distinct",
                "api.limit",
                "api.cap",
                "different endpoints",
                "--as-of",
                "2026-01-01"
            ]
        )),
        "distinct api.limit from api.cap: different endpoints\n  the pair returns as no candidate; api.limit carries distinct_from: api.cap\nrests on it:\n  HOLDS     d.w: wrong_if does not hold (api.cap > 100)\n\nthe record needs a person on 0 judgments - check says the rest\n"
    );
    assert_eq!(
        record(&root),
        "known:\n  api.limit: {v: 10, distinct_from: api.cap}\n    # distinct 2026-01-01: different endpoints\n  api.cap: {v: 10}\njudgments:\n  d.w:\n    verdict: known\n    requires: [api.limit, api.cap]\n    fails_if: \"api.cap > 100\"\n"
    );
}

#[test]
#[ignore = "requires KPOP_SESSION_ORACLE_PYTHON and KPOP_SESSION_ORACLE_ROOT pointing at an immutable Python oracle"]
fn custom_field_roles_match_the_python_writer_oracle() {
    let python =
        std::env::var_os("KPOP_SESSION_ORACLE_PYTHON").expect("set KPOP_SESSION_ORACLE_PYTHON");
    let cli = Path::new(
        &std::env::var_os("KPOP_SESSION_ORACLE_ROOT").expect("set KPOP_SESSION_ORACLE_ROOT"),
    )
    .join("scripts/cli.py");
    let block = "known:\n  api.limit: {v: 10}\njudgments:\n  d.w:\n    verdict: known\n    requires: [api.limit]\n    fails_if: \"api.limit > 100\"\n";
    let mut cases = vec![];
    for declared in [CUSTOM_ROLES, ""] {
        let flow = format!("{declared}{CUSTOM_RECORD}");
        cases.push((flow.clone(), vec!["add", "api.window", "v=5"]));
        cases.push((flow.clone(), CUSTOM_JUDGMENT.to_vec()));
        cases.push((
            flow,
            vec!["set", "api.limit", "11", "--as-of", "2026-01-01"],
        ));
        let block = format!("{declared}{block}");
        cases.push((
            block.clone(),
            vec!["review", "d.w", "--as-of", "2026-01-01"],
        ));
        cases.push((block, CUSTOM_JUDGMENT.to_vec()));
    }
    for (source, args) in cases {
        let (_native_temp, native) = simple_workspace(&source);
        success(run_unbundled(&native, &args));
        let (_oracle_temp, oracle) = simple_workspace(&source);
        // No checked-session core, as for the unbundled native binary.
        let cache = tempfile::tempdir().unwrap();
        success(
            Command::new(&python)
                .arg(&cli)
                .args(&args)
                .current_dir(&oracle)
                .env_remove("KPOPPER_AGENT_SESSION")
                .env_remove("CODEX_THREAD_ID")
                .env("XDG_CACHE_HOME", cache.path())
                .output()
                .unwrap(),
        );
        assert_eq!(record(&native), record(&oracle), "{args:?} on {source}");
    }
}
