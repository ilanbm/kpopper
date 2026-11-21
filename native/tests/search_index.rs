use rusqlite::Connection;
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

struct Project {
    temp: tempfile::TempDir,
    root: PathBuf,
    cache: PathBuf,
}
impl Project {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        let cache = temp.path().join("cache");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("memo.md"), "copycat source text").unwrap();
        fs::write(root.join("GROUNDING.yaml"),"meta: {name: Search index, updated: '2026-11-19'}\nschema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nsources:\n  s.memo: {name: Measurement, file: memo.md, read: '2026-11-19'}\nknown:\n  p.qty: {v: 3, from: s.memo, at: line 1, of: '2026-11-19'}\n  p.other: {v: stationary, from: s.memo}\n  p.total: {name: Copycat total, rule: {expr: 'p.qty + 1'}}\njudgments:\n  c.limit: {rests_on: [p.total], seen: {p.total: 4}, verdict: Copycat limit, wrong_if: 'p.total > 9'}\n").unwrap();
        Self { temp, root, cache }
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_kpop"));
        c.current_dir(&self.root)
            .args(args)
            .env("KPOPPER_NATIVE_CACHE", &self.cache)
            .env("XDG_STATE_HOME", self.temp.path().join("state"));
        c
    }
    fn run(&self, args: &[&str]) -> Output {
        let o = self.command(args).output().unwrap();
        assert!(
            o.status.success(),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        o
    }
    fn search(&self, query: &str) -> Value {
        let live = self.run(&["search", query, "--chars", "20000"]);
        let reference = self.run(&["--no-cache", "search", query, "--chars", "20000"]);
        assert_eq!(live.stdout, reference.stdout, "{query}");
        serde_json::from_slice(&live.stdout).unwrap()
    }
    fn db(&self) -> Connection {
        let path = fs::read_dir(self.cache.join("search-v1"))
            .unwrap()
            .flatten()
            .map(|f| f.path())
            .find(|p| p.extension() == Some(std::ffi::OsStr::new("sqlite")))
            .unwrap();
        Connection::open(path).unwrap()
    }
}
fn stored(db: &Connection, id: &str) -> (i64, String, String) {
    db.query_row(
        "SELECT id,text_hash,data FROM documents WHERE ref=?1",
        [format!("node:{id}")],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .unwrap()
}
#[test]
fn writes_publish_changed_rows_and_their_dependents_before_the_next_search() {
    let project = Project::new();
    let first = project.search("copycat");
    let db = project.db();
    let unaffected = stored(&db, "p.other");
    let old = stored(&db, "c.limit");
    project.run(&[
        "set",
        "p.qty",
        "12",
        "--as-of",
        "2026-11-22",
        "--why",
        "new counted quantity",
    ]);
    let newer: String = db
        .query_row("SELECT revision FROM head", [], |r| r.get(0))
        .unwrap();
    assert_ne!(Some(newer.as_str()), first["revision"].as_str());
    assert_eq!(unaffected, stored(&db, "p.other"));
    assert!(stored(&db, "p.qty").2.contains("12"));
    if std::env::var_os("KPOPPER_NATIVE_RESOURCES").is_some() {
        assert_ne!(old.2, stored(&db, "c.limit").2);
        assert_eq!(
            serde_json::from_str::<Value>(&stored(&db, "c.limit").2).unwrap()["status"],
            "FIRED"
        );
    }
    assert_eq!(project.search("copycat")["revision"], newer);
    let external = project.root.join("memo.md");
    let modified = fs::metadata(&external).unwrap().modified().unwrap();
    let original_length = fs::metadata(&external).unwrap().len();
    fs::write(&external, "newword source text").unwrap();
    assert_eq!(fs::metadata(&external).unwrap().len(), original_length);
    fs::File::options()
        .write(true)
        .open(&external)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    assert!(
        project.search("newword")["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["ref"] == "source:s.memo")
    );
    fs::remove_file(project.root.join("memo.md")).unwrap();
    assert!(
        project.search("newword")["results"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
#[test]
fn independent_writers_preserve_both_edits_and_the_index_converges() {
    let project = Project::new();
    project.search("stationary");
    let mut a = project
        .command(&[
            "add",
            "p.parallel_one",
            "--in",
            "known",
            "v=firstword",
            "from=s.memo",
        ])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut b = project
        .command(&[
            "add",
            "p.parallel_two",
            "--in",
            "known",
            "v=secondword",
            "from=s.memo",
        ])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    assert!(a.wait().unwrap().success());
    assert!(b.wait().unwrap().success());
    let result = project.search("firstword secondword");
    let hits = result["results"].as_array().unwrap();
    for id in ["p.parallel_one", "p.parallel_two"] {
        assert!(hits.iter().any(|r| r["id"] == id));
    }
}

#[test]
fn prepared_reports_merge_disjoint_changes_and_name_a_changed_premise() {
    use serde_json::json;
    let project = Project::new();
    let prepare = |name: &str| {
        let record = project.search("stationary")["record_sha256"].clone();
        let report = json!({"event_id":name,"date":"2026-11-22","source_quote":"Reviewer's observation from the local memo.",
            "source":"s.memo","at":"line 1","read_ids":["p.qty"],"record_sha256":record,
            "updates":[{"kind":"add","id":format!("p.{name}"),"into":"known","body":{"v":name}}]});
        let input = project.temp.path().join(format!("{name}-input.json"));
        let output = project.temp.path().join(format!("{name}-prepared.json"));
        fs::write(&input, report.to_string()).unwrap();
        let prepared = project.run(&["update", "--prepare", "--file", input.to_str().unwrap()]);
        let value: Value = serde_json::from_slice(&prepared.stdout).unwrap();
        assert!(value["write_basis"]["entries"]["p.qty"].is_array());
        assert!(value.get("record_sha256").is_none());
        fs::write(&output, prepared.stdout).unwrap();
        output
    };
    let one = prepare("first_reading");
    let two = prepare("second_reading");
    let stale = prepare("stale_reading");
    for f in [one, two] {
        let result = project.run(&["update", "--file", f.to_str().unwrap()]);
        let receipt: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(receipt["state"], "applied");
    }
    project.run(&[
        "set",
        "p.qty",
        "15",
        "--as-of",
        "2026-11-22",
        "--why",
        "new quantity",
    ]);
    let failed = project
        .command(&["update", "--file", stale.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!failed.status.success());
    let message = String::from_utf8_lossy(&failed.stdout);
    assert!(
        message.contains("write conflict") && message.contains("p.qty"),
        "{message}"
    );
    assert!(
        !fs::read_to_string(project.root.join("GROUNDING.yaml"))
            .unwrap()
            .contains("p.stale_reading:")
    );
}

#[cfg(unix)]
#[test]
fn readers_wait_for_a_live_journal_and_preserve_an_abandoned_one() {
    use fs2::FileExt;
    use kpop_native::history_transaction::Layout;
    let project = Project::new();
    let journal = project
        .root
        .join(Layout::for_entry("GROUNDING.yaml").unwrap().journal);
    fs::create_dir_all(journal.parent().unwrap()).unwrap();
    let lock = fs::File::open(&project.root).unwrap();
    lock.lock_exclusive().unwrap();
    fs::write(&journal, b"{}").unwrap();
    let mut child = project
        .command(&["--no-cache", "search", "stationary"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(child.try_wait().unwrap().is_none());
    fs::remove_file(&journal).unwrap();
    drop(lock);
    assert!(child.wait().unwrap().success());
    fs::write(&journal, b"{}").unwrap();
    let failed = project
        .command(&["--no-cache", "search", "stationary"])
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("recovery_required"));
    assert!(journal.exists());
}

#[test]
fn rebuilding_an_incomplete_private_index_preserves_ranking_and_record_bytes() {
    let p = Project::new();
    let before = fs::read(p.root.join("GROUNDING.yaml")).unwrap();
    p.search("copycat");
    let db = p.db();
    db.execute(
        "DELETE FROM evidence WHERE rowid=(SELECT id FROM documents WHERE ref='source:s.memo')",
        [],
    )
    .unwrap();
    db.execute(
        "UPDATE documents SET data='{}' WHERE ref='node:p.total'",
        [],
    )
    .unwrap();
    assert!(
        !p.search("copycat")["results"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(before, fs::read(p.root.join("GROUNDING.yaml")).unwrap());
    db.execute_batch("BEGIN EXCLUSIVE").unwrap();
    p.search("stationary");
    db.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn core_reuses_only_matching_runtime_requests_across_revisions() {
    if std::env::var_os("KPOPPER_NATIVE_RESOURCES").is_none() {
        return;
    }
    let p = Project::new();
    let fixture: Value =
        serde_json::from_slice(include_bytes!("fixtures/search-corpus.json")).unwrap();
    let case = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "core-basic")
        .unwrap();
    for (path, body) in case["input"].as_object().unwrap() {
        let file = p.root.join(path.strip_prefix("source/").unwrap());
        fs::write(file, body.as_str().unwrap()).unwrap();
    }
    let first = p.search("Alpha");
    let db = p.db();
    let keys = || {
        let mut q = db.prepare("SELECT key FROM calculations").unwrap();
        q.query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<std::collections::BTreeSet<_>>>()
            .unwrap()
    };
    let old = keys();
    assert!(!old.is_empty());
    let record = p.root.join("GROUNDING.yaml");
    let changed = fs::read_to_string(&record)
        .unwrap()
        .replace("p.a: {v: 2,", "p.a: {v: 7,");
    fs::write(record, changed).unwrap();
    let next = p.search("Alpha");
    assert_ne!(first["revision"], next["revision"]);
    let new = keys();
    assert_ne!(old, new);
    assert!(
        old.intersection(&new).next().is_some(),
        "unchanged independent inputs should retain their frame"
    );
}

#[test]
fn a_global_value_order_error_cannot_reuse_a_previous_rule_calculation() {
    let p = Project::new();
    p.search("copycat");
    let path = p.root.join("GROUNDING.yaml");
    let record = fs::read_to_string(&path)
        .unwrap()
        .replace("v: stationary", "v: {1: one, text: two}");
    fs::write(&path, record).unwrap();
    let cached = p
        .command(&["search", "copycat", "--chars", "20000"])
        .output()
        .unwrap();
    let canonical = p
        .command(&["--no-cache", "search", "copycat", "--chars", "20000"])
        .output()
        .unwrap();
    assert_eq!(cached.status.code(), canonical.status.code());
    assert_eq!(cached.stdout, canonical.stdout);
    assert_eq!(cached.stderr, canonical.stderr);
}
