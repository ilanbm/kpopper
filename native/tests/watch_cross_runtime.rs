//! Complete watch results and durable state against the immutable Python oracle.
//! UUID substitution is bijective and scoped; clocks and all hashes remain exact.
use kpop_native::{
    watch_delivery as D, watch_shared as S,
    watch_store::{Clock, Watch},
};
use serde_json::{Value as J, json};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
const PY: &str = r#"
import json,sys,os
from scripts import watch as W,watch_shared as S,watch_delivery as D
r=json.load(sys.stdin)
W.time.time=lambda:r['now'];W.time.time_ns=lambda:int(r['now']*1000000000)
W.launch=lambda w:None
w=W.Watch(r['workspace']);a=r['args'];op=r['op']
try:
 if op=='setup': out=w.setup(a.get('base_ref'),a.get('shared_record'),a.get('shared_private',False))
 elif op=='snapshot': out=w.snapshot()
 elif op=='status': out=w.status()
 elif op=='request': out=w.request()
 elif op=='request_all': out=w.request_all()
 elif op=='process': out=w.process()
 elif op=='pause':
  with W.I._file_lock(w.config_path.with_suffix('.lock')): W.I._save(w.config_path,dict(w.config() or {},enabled=False))
  out={'state':'disabled'}
 elif op=='share': out=S.capture(w,a)
 elif op=='shared_process': out=S.process(w)
 elif op=='shared': out=S.read(w)
 elif op=='resolve': out=S.resolve(w,a['id'],a['evidence'])
 elif op=='offer': out=w.offer(a['session'],a.get('consume',True))
 elif op=='reserve': out=D.reserve(w,a['recipient'])
 elif op=='wait': out=D.wait(w,a['id'],a.get('timeout',0))
 elif op=='complete': out=D.complete(w,a['id'],a['token'],a['outcome'])
 else: raise AssertionError(op)
 out={'ok':out}
except (Exception,SystemExit) as e: out={'error':str(e)}
from scripts.reasoning import adapter_identity
def transport(value):
 if isinstance(value,bytes):
  import hashlib
  return {'encoding':'hex','data':value.hex(),'sha256':hashlib.sha256(value).hexdigest()}
 return str(value)
print(json.dumps({"result":out,"adapter":adapter_identity()},ensure_ascii=False,default=transport))
"#;
fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn scan(root: &Path, path: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        if !path.is_dir() {
            return;
        }
        for e in fs::read_dir(path).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                scan(root, &p, out)
            } else {
                out.insert(p.strip_prefix(root).unwrap().into(), fs::read(p).unwrap());
            }
        }
    }
    let mut v = BTreeMap::new();
    scan(root, root, &mut v);
    v
}
fn restore(root: &Path, files: &BTreeMap<PathBuf, Vec<u8>>) {
    if root.exists() {
        fs::remove_dir_all(root).unwrap();
    }
    fs::create_dir_all(root).unwrap();
    for (p, b) in files {
        let p = root.join(p);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b).unwrap();
    }
}
fn image(files: &BTreeMap<PathBuf, Vec<u8>>) -> J {
    J::Object(
        files
            .iter()
            .filter(|(p, _)| p.extension().is_none_or(|e| e != "lock"))
            .map(|(p, b)| {
                let value = if p.extension().is_some_and(|e| e == "json") {
                    serde_json::from_slice(b).unwrap()
                } else if p.extension().is_some_and(|e| e == "yaml") {
                    kpop_native::followup_store::parse_input(b).unwrap()
                } else {
                    json!(String::from_utf8_lossy(b))
                };
                (p.to_string_lossy().into_owned(), value)
            })
            .collect(),
    )
}
struct H {
    _temp: tempfile::TempDir,
    root: PathBuf,
    state: PathBuf,
    now: f64,
    python: String,
    oracle: String,
    index: usize,
}
impl H {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let root = base.join("project");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("PROVENANCE.yaml"),"meta:\n  name: Watch oracle\nknown:\n  facts.count:\n    name: Count\n    v: 1\n    of: 2020-01-01\njudgments:\n  choice.ok:\n    verdict: yes\n    rests_on: [facts.count]\n    seen: {facts.count: 1}\n    wrong_if: facts.count < 0\n").unwrap();
        for args in [
            vec!["init", "--quiet", "--initial-branch=main"],
            vec!["add", "PROVENANCE.yaml"],
            vec![
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "Fixture",
            ],
        ] {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&root)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        Self {
            _temp: temp,
            root,
            state: base.join("state"),
            now: 1_700_000_000.0,
            python: std::env::var("KPOP_SESSION_ORACLE_PYTHON")
                .expect("set KPOP_SESSION_ORACLE_PYTHON"),
            oracle: std::env::var("KPOP_SESSION_ORACLE_ROOT")
                .expect("set KPOP_SESSION_ORACLE_ROOT"),
            index: 0,
        }
    }
    fn watch(&self) -> Watch {
        Watch::in_state(&self.root, &self.state, Clock::at(self.now)).unwrap()
    }
    fn step(&mut self, op: &str, a: J) -> J {
        self.index += 1;
        let config = self.root.join(".git/kpopper-watch");
        let state = self.state.join("kpopper/watch");
        let before_config = files(&config);
        let before_state = files(&state);
        let mut child = Command::new(&self.python)
            .args(["-c", PY])
            .env("PYTHONPATH", &self.oracle)
            .env("XDG_STATE_HOME", &self.state)
            .env("CODEX_SESSION_ID", "watch-test-recipient")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(
                serde_json::to_string(
                    &json!({"workspace":self.root,"now":self.now,"op":op,"args":a}),
                )
                .unwrap()
                .as_bytes(),
            )
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let wrapper: J = serde_json::from_slice(&output.stdout).unwrap();
        let py = wrapper["result"].clone();
        let oracle_adapter = wrapper["adapter"].as_str().unwrap();
        let py_config = files(&config);
        let py_state = files(&state);
        restore(&config, &before_config);
        restore(&state, &before_state);
        let w = self.watch();
        let no_launch = |_: &Watch| Ok(());
        let get = |key: &str| a[key].as_str().unwrap();
        let native = match op {
            "setup" => w.setup(
                a["base_ref"].as_str(),
                a["shared_record"].as_str().map(Path::new),
                a["shared_private"].as_bool().unwrap_or(false),
            ),
            "snapshot" => w.snapshot().map(|v| v.value),
            "status" => w.status(),
            "request" => w.request_with(&no_launch),
            "request_all" => w.request_all_with(&no_launch),
            "process" => w.process_with(&no_launch),
            "pause" => w.pause(),
            "share" => S::capture_with(&w, a.clone(), &no_launch),
            "shared_process" => S::process(&w).map(|v| json!(v)),
            "shared" => S::read(&w),
            "resolve" => S::resolve(&w, get("id"), get("evidence")),
            "offer" => w
                .offer(get("session"), a["consume"].as_bool().unwrap_or(true), None)
                .map(|v| json!(v)),
            "reserve" => D::reserve_with_host(&w, get("recipient"), Some("watch-test-recipient")),
            "wait" => D::wait(&w, get("id"), a["timeout"].as_f64().unwrap_or(0.0)),
            "complete" => D::complete(&w, get("id"), get("token"), get("outcome")),
            _ => panic!("{op}"),
        };
        let native = match native {
            Ok(v) => json!({"ok":v}),
            Err(e) => json!({"error":e.to_string()}),
        };
        let py_all = json!({"result":py,"config":image(&py_config),"state":image(&py_state)});
        let mut rs_all =
            json!({"result":native,"config":image(&files(&config)),"state":image(&files(&state))});
        if let Ok(debug) = std::env::var("KPOP_WATCH_ORACLE_DEBUG_DIR") {
            let scenario = std::thread::current().name().unwrap_or("watch").to_owned();
            let debug = Path::new(&debug).join(scenario);
            fs::create_dir_all(&debug).unwrap();
            fs::write(
                debug.join(format!("{:03}-{op}-python.json", self.index)),
                serde_json::to_vec_pretty(&py_all).unwrap(),
            )
            .unwrap();
            fs::write(
                debug.join(format!("{:03}-{op}-native.json", self.index)),
                serde_json::to_vec_pretty(&rs_all).unwrap(),
            )
            .unwrap();
        }
        normalize_native_audit(&mut rs_all, oracle_adapter, "");
        let mut cmp = Compare {
            forward: BTreeMap::new(),
            reverse: BTreeMap::new(),
            label: format!("{} {op}", self.index),
            py: format!(
                "{} {}/scripts/watch.py --workspace {}",
                self.python,
                self.oracle,
                self.root.display()
            ),
            rs: format!(
                "{} --workspace {} watch",
                std::env::current_exe().unwrap().display(),
                self.root.display()
            ),
        };
        cmp.equal(&py_all, &rs_all, "");
        native
    }
    fn ok(&mut self, op: &str, a: J) -> J {
        let v = self.step(op, a);
        assert!(v.get("ok").is_some(), "{op}: {v}");
        v["ok"].clone()
    }
}
fn typed_digest(value: &J) -> String {
    kpop_native::value::TypedValue::from_json(value)
        .unwrap()
        .digest()
        .unwrap()
}
// Preserve both original images in the optional debug capture. Validate the actual
// provenance and checksums before comparing the sole implementation binding.
fn normalize_assessment(value: &mut J, oracle_adapter: &str) {
    match value {
        J::Object(object) => {
            let assessment = object.get("assessment_revision").cloned();
            if let Some(revision) = &assessment {
                let mut payload = object.clone();
                payload.remove("assessment_revision");
                assert_eq!(
                    *revision,
                    json!(typed_digest(&J::Object(payload))),
                    "native assessment checksum"
                );
            }
            if object
                .get("implementation")
                .and_then(|v| v.get("adapter_source_sha256"))
                .is_some()
            {
                let implementation = object["implementation"].clone();
                assert_eq!(
                    implementation["adapter_source_sha256"],
                    env!("KPOP_REASONING_ADAPTER_SHA256"),
                    "actual native adapter identity"
                );
                assert_eq!(
                    object["assurance"]["implementation"],
                    typed_digest(&implementation),
                    "native implementation checksum"
                );
                object.get_mut("implementation").unwrap()["adapter_source_sha256"] =
                    json!(oracle_adapter);
                let expected = typed_digest(&object["implementation"]);
                object.get_mut("assurance").unwrap()["implementation"] = json!(expected);
            }
            for (key, child) in object.iter_mut() {
                if key != "body" {
                    normalize_assessment(child, oracle_adapter);
                }
            }
            if assessment.is_some() {
                let mut payload = object.clone();
                payload.remove("assessment_revision");
                object.insert(
                    "assessment_revision".into(),
                    json!(typed_digest(&J::Object(payload))),
                );
            }
        }
        J::Array(values) => {
            for value in values {
                normalize_assessment(value, oracle_adapter)
            }
        }
        _ => {}
    }
}
fn normalize_native_audit(value: &mut J, oracle_adapter: &str, path: &str) {
    if path.ends_with("/core/assessment") {
        normalize_assessment(value, oracle_adapter);
        return;
    }
    match value {
        J::Object(rows) => {
            for (key, value) in rows {
                normalize_native_audit(value, oracle_adapter, &format!("{path}/{key}"));
            }
        }
        J::Array(rows) => {
            for (index, value) in rows.iter_mut().enumerate() {
                normalize_native_audit(value, oracle_adapter, &format!("{path}/{index}"));
            }
        }
        _ => {}
    }
}
fn dynamic_uuid(path: &str) -> bool {
    [
        "/result/ok/job_id",
        "/result/ok/claim_token",
        "/result/ok/episode",
        "/job_id",
    ]
    .contains(&path)
        || path.starts_with("/state/")
            && (path.ends_with("/request.json/id")
                || path.contains("/native/")
                    && ["/id", "/claim_token", "/episode"]
                        .iter()
                        .any(|suffix| path.ends_with(suffix))
                || path.ends_with("/result.json/episode")
                || path.contains("/delivery/") && path.ends_with("/episode"))
}
struct Compare {
    forward: BTreeMap<String, String>,
    reverse: BTreeMap<String, String>,
    label: String,
    py: String,
    rs: String,
}
impl Compare {
    fn equal(&mut self, a: &J, b: &J, path: &str) {
        match (a, b) {
            (J::Object(a), J::Object(b)) => {
                assert_eq!(
                    a.keys().collect::<Vec<_>>(),
                    b.keys().collect::<Vec<_>>(),
                    "{} {path}",
                    self.label
                );
                for (k, v) in a {
                    self.equal(v, &b[k], &format!("{path}/{k}"));
                }
            }
            (J::Array(a), J::Array(b)) => {
                assert_eq!(a.len(), b.len(), "{} {path}", self.label);
                for (i, (a, b)) in a.iter().zip(b).enumerate() {
                    self.equal(a, b, &format!("{path}/{i}"));
                }
            }
            (J::String(a), J::String(b))
                if dynamic_uuid(path)
                    && a.len() == 32
                    && b.len() == 32
                    && a.bytes().chain(b.bytes()).all(|c| c.is_ascii_hexdigit()) =>
            {
                if let Some(old) = self.forward.insert(a.clone(), b.clone()) {
                    assert_eq!(&old, b, "token reference {path}");
                }
                if let Some(old) = self.reverse.insert(b.clone(), a.clone()) {
                    assert_eq!(&old, a, "token alias {path}");
                }
            }
            (J::String(a), J::String(b))
                if ["/result/ok/wait_command", "/result/ok/complete_command"].contains(&path) =>
            {
                let a = a.replace(&self.py, "${RUNTIME}");
                let b = b.replace(&self.rs, "${RUNTIME}");
                let (ap, at) = a.rsplit_once(' ').unwrap();
                let (bp, bt) = b.rsplit_once(' ').unwrap();
                assert_eq!(ap, bp);
                self.equal(&json!(at), &json!(bt), "/job_id");
            }
            _ => assert_eq!(a, b, "{} {path}", self.label),
        }
    }
}
#[test]
#[ignore = "requires KPOP_SESSION_ORACLE_PYTHON and KPOP_SESSION_ORACLE_ROOT"]
fn python_and_native_watch_queue_snapshot_processor_and_delivery() {
    let mut h = H::new();
    h.ok("status", json!({}));
    h.ok("setup", json!({}));
    h.ok("snapshot", json!({}));
    h.ok("request", json!({}));
    h.ok("request", json!({}));
    h.ok("status", json!({}));
    h.ok("process", json!({}));
    h.ok("status", json!({}));
    h.ok("offer", json!({"session":"session"}));
    let original = fs::read_to_string(h.root.join("PROVENANCE.yaml")).unwrap();
    fs::write(
        h.root.join("PROVENANCE.yaml"),
        original.replace("v: 1", "v: -1"),
    )
    .unwrap();
    h.now += 1.0;
    h.ok("request_all", json!({}));
    h.ok("process", json!({}));
    h.ok("status", json!({}));
    h.ok("offer", json!({"session":"session","consume":false}));
    h.ok("offer", json!({"session":"session"}));
    h.ok("offer", json!({"session":"session"}));
    let job = h.ok("reserve", json!({"recipient":"watch-test-recipient"}));
    let notice = h.ok("wait", json!({"id":job["job_id"]}));
    assert_eq!(notice["state"], "attention");
    h.ok(
        "complete",
        json!({"id":job["job_id"],"token":notice["claim_token"],"outcome":"unknown"}),
    );
    h.ok("pause", json!({}));
    h.ok("status", json!({}));
}
#[test]
#[ignore = "requires KPOP_SESSION_ORACLE_PYTHON and KPOP_SESSION_ORACLE_ROOT"]
fn python_and_native_shared_capture_publication_and_receipts() {
    let mut h = H::new();
    h.ok("setup", json!({"shared_private":true}));
    let report = json!({"event_id":"source-1","id":"external.count","name":"Count","value":3,"date":"2020-01-02","scope":{"kind":"external","environment":"fixture"},"source":{"url":"https://example.invalid/report","at":"row 3"},"source_quote":"The count is 3."});
    h.ok("share", report.clone());
    h.ok("share", report);
    h.ok("status", json!({}));
    h.ok("shared_process", json!({}));
    h.ok("shared", json!({}));
    h.ok("process", json!({}));
    h.ok("status", json!({}));
}

#[test]
#[ignore = "requires KPOP_SESSION_ORACLE_PYTHON and KPOP_SESSION_ORACLE_ROOT"]
fn python_and_native_hypotheses_pointers_and_refused_updates() {
    let mut h = H::new();
    h.ok("setup", json!({}));
    let folder = h.root.join("PROVENANCE.d");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("proposal.yaml"),"hypothesis:\n  claim: A bounded hypothesis\n  wrong_if: facts.count > 0\n  folds: never\nknown:\n  facts.other:\n    name: Other\n    v: 2\n").unwrap();
    let observed = h.ok("snapshot", json!({}));
    assert_eq!(
        observed["working"]["hypotheses"].as_array().unwrap().len(),
        1
    );
    h.ok("process", json!({}));
    fs::write(
        folder.join("rival.yaml"),
        "known:\n  facts.other:\n    name: Other\n    v: 3\n",
    )
    .unwrap();
    h.now += 1.0;
    h.ok("process", json!({}));
    fs::remove_file(folder.join("rival.yaml")).unwrap();
    fs::remove_file(folder.join("proposal.yaml")).unwrap();
    let original = fs::read_to_string(h.root.join("PROVENANCE.yaml")).unwrap();
    fs::write(
        h.root.join("PROVENANCE.yaml"),
        original
            .replace("v: 1", "v: 2")
            .replace("of: 2020-01-01", "of: 2020-01-02"),
    )
    .unwrap();
    h.now += 1.0;
    h.ok("process", json!({}));
    fs::write(
        h.root.join("extra.yaml"),
        "known:\n  external.extra:\n    name: Extra\n    v: 7\n",
    )
    .unwrap();
    let text = fs::read_to_string(h.root.join("PROVENANCE.yaml")).unwrap();
    fs::write(
        h.root.join("PROVENANCE.yaml"),
        format!("{text}also: extra.yaml\n"),
    )
    .unwrap();
    h.now += 1.0;
    h.ok("snapshot", json!({}));
    h.ok("process", json!({}));
}

#[test]
#[ignore = "requires KPOP_SESSION_ORACLE_PYTHON and KPOP_SESSION_ORACLE_ROOT"]
fn python_and_native_registered_worktrees_rename_and_missing_records() {
    let mut h = H::new();
    h.ok("setup", json!({}));
    let peer = h.root.parent().unwrap().join("peer");
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&h.root)
            .args(["worktree", "add", "--quiet", "-b", "topic"])
            .arg(&peer)
            .status()
            .unwrap()
            .success()
    );
    let registered = Watch::in_state(&peer, &h.state, Clock::at(h.now)).unwrap();
    registered.request_with(&|_| Ok(())).unwrap();
    let request = h.ok("request_all", json!({}));
    assert_eq!(request["worktrees"].as_array().unwrap().len(), 2);
    h.now += 31.0;
    h.ok("request_all", json!({}));
    fs::rename(
        h.root.join("PROVENANCE.yaml"),
        h.root.join("GROUNDING.yaml"),
    )
    .unwrap();
    h.ok("snapshot", json!({}));
    h.ok("process", json!({}));
    fs::remove_file(h.root.join("GROUNDING.yaml")).unwrap();
    h.ok("status", json!({}));
    h.ok("process", json!({}));
}

#[test]
#[ignore = "requires KPOP_SESSION_ORACLE_PYTHON and KPOP_SESSION_ORACLE_ROOT"]
fn python_and_native_core_watch_snapshots_and_comparison() {
    let mut h = H::new();
    let record = "meta:\n  name: Core watch\n  reasoning: {version: 1, profile: core/v1, requires: [arithmetic/v1]}\nknown:\n  facts.count: {name: Count, v: 1}\njudgments:\n  choice.ok: {verdict: keep, rests_on: [facts.count], seen: {facts.count: 1}, wrong_if: {op: lt, args: [{ref: facts.count}, {num: '0'}]}}\n";
    fs::write(h.root.join("PROVENANCE.yaml"), record).unwrap();
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&h.root)
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--quiet",
                "-am",
                "Core fixture"
            ])
            .status()
            .unwrap()
            .success()
    );
    h.ok("setup", json!({}));
    h.ok("snapshot", json!({}));
    h.ok("process", json!({}));
    fs::write(
        h.root.join("PROVENANCE.yaml"),
        record.replace("v: 1", "v: -1"),
    )
    .unwrap();
    h.now += 1.0;
    h.ok("process", json!({}));
}

#[test]
#[ignore = "requires KPOP_SESSION_ORACLE_PYTHON and KPOP_SESSION_ORACLE_ROOT"]
fn python_and_native_active_history_watch_preserves_captured_evidence() {
    let mut h = H::new();
    fs::remove_file(h.root.join("PROVENANCE.yaml")).unwrap();
    let corpus: J = serde_json::from_str(include_str!("fixtures/history-bootstrap.json")).unwrap();
    use base64::{Engine, engine::general_purpose::STANDARD};
    use kpop_native::value::TypedValue as V;
    let V::Map(mutation) = V::from_tagged(&corpus[0]["output"]).unwrap() else {
        panic!("mutation")
    };
    let V::List(files) = &mutation["files"] else {
        panic!("files")
    };
    for file in files {
        let V::Map(file) = file else { panic!("file") };
        let V::Text(name) = &file["path"] else {
            panic!("path")
        };
        let V::Map(after) = &file["after"] else {
            continue;
        };
        let V::Text(encoded) = &after["data"] else {
            panic!("bytes")
        };
        let path = h.root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, STANDARD.decode(encoded).unwrap()).unwrap();
    }
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&h.root)
            .args(["add", "-A"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&h.root)
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "History fixture"
            ])
            .status()
            .unwrap()
            .success()
    );
    h.ok("setup", json!({}));
    h.ok("snapshot", json!({}));
    assert_eq!(h.ok("process", json!({}))["state"], "clear");
    h.ok("status", json!({}));
    let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .arg("--workspace")
        .arg(&h.root)
        .args(["add", "p.other", "v=2", "--as-of", "2026-09-02"])
        .env("XDG_STATE_HOME", &h.state)
        .env_remove("KPOPPER_AGENT_SESSION")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("CODEX_SESSION_ID")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    h.now += 1.0;
    h.ok("snapshot", json!({}));
    assert_eq!(h.ok("process", json!({}))["state"], "clear");
}

#[test]
#[ignore = "requires KPOP_SESSION_ORACLE_PYTHON and KPOP_SESSION_ORACLE_ROOT"]
fn python_and_native_shared_journal_recovery_and_conflicting_capture() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let mut h = H::new();
    h.ok("setup", json!({"shared_private":true}));
    let report = json!({"event_id":"recover-1","id":"external.count","name":"Count","value":3,"date":"2020-01-02","scope":{"kind":"external","environment":"fixture"},"source":{"url":"https://example.invalid/report","at":"row 3"},"source_quote":"The count is 3."});
    let captured = h.ok("share", report.clone());
    h.ok("shared_process", json!({}));
    let event_path = PathBuf::from(captured["receipt"].as_str().unwrap());
    let inbox = event_path.parent().unwrap().parent().unwrap();
    let journal_path = inbox.join("journals/recover-1.json");
    let mut journal: J = serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
    let bytes = STANDARD
        .decode(journal["prepared"].as_str().unwrap())
        .unwrap();
    let mutation = kpop_native::history_transaction::PreparedMutation::from_bytes(&bytes).unwrap();
    let record = PathBuf::from(captured["record"].as_str().unwrap());
    fs::write(&record, mutation.files()[0].before.as_ref().unwrap()).unwrap();
    let primary = record.parent().unwrap().join(
        kpop_native::history_transaction::Layout::for_entry(
            record.file_name().unwrap().to_str().unwrap(),
        )
        .unwrap()
        .journal,
    );
    fs::create_dir_all(primary.parent().unwrap()).unwrap();
    fs::write(primary, &bytes).unwrap();
    journal["phase"] = json!("prepared");
    journal.as_object_mut().unwrap().remove("mutation_digest");
    fs::write(journal_path, serde_json::to_vec(&journal).unwrap()).unwrap();
    let mut event: J = serde_json::from_slice(&fs::read(&event_path).unwrap()).unwrap();
    event["state"] = json!("recovery_required");
    fs::write(&event_path, serde_json::to_vec(&event).unwrap()).unwrap();
    h.ok("shared_process", json!({}));
    let shared = h.ok("shared", json!({}));
    assert_eq!(shared["reports"][0]["recovered"], true);
    let mut changed = report;
    changed["event_id"] = json!("conflict-2");
    changed["value"] = json!(4);
    h.ok("share", changed);
    let contents = fs::read_to_string(&record).unwrap();
    fs::write(&record, contents.replace("v: 3", "v: 9")).unwrap();
    h.ok("shared_process", json!({}));
    let shared = h.ok("shared", json!({}));
    assert_eq!(shared["reports"][0]["state"], "needs_review");
    h.ok("pause", json!({}));
    h.ok(
        "resolve",
        json!({"id":"conflict-2","evidence":"User reconciled source discrepancy"}),
    );
    h.ok("shared", json!({}));
}

#[test]
#[ignore = "requires KPOP_SESSION_ORACLE_PYTHON and KPOP_SESSION_ORACLE_ROOT"]
fn python_and_native_refuse_invalid_host_inputs_and_preserve_delivery_leases() {
    let mut h = H::new();
    assert!(
        h.step("setup", json!({"base_ref":"-bad"}))
            .get("error")
            .is_some()
    );
    assert!(
        h.step(
            "setup",
            json!({"shared_record":h.root.join("PROVENANCE.yaml")})
        )
        .get("error")
        .is_some()
    );
    h.ok("setup", json!({"shared_private":true}));
    let valid = json!({"event_id":"validation-1","id":"external.count","name":"Count","value":3,"date":"2020-01-02","scope":{"kind":"external","environment":"fixture"},"source":{"url":"https://example.invalid/report","at":"row 3"},"source_quote":"The count is 3."});
    for (pointer, value) in [
        ("/id", json!("x")),
        ("/name", json!("\u{1c}")),
        ("/scope/environment", json!("\u{1f}")),
        ("/scope/kind", json!("branch")),
        ("/value", J::Null),
        ("/value", json!([])),
        ("/date", json!("2020-02-30")),
        ("/source/url", json!("ftp://example.invalid/report")),
        ("/source/at", json!("")),
        ("/event_id", json!("../escape")),
    ] {
        let mut report = valid.clone();
        *report.pointer_mut(pointer).unwrap() = value;
        assert!(h.step("share", report).get("error").is_some(), "{pointer}");
    }
    h.ok("share", valid.clone());
    let mut report = valid;
    report["value"] = json!(4);
    assert!(h.step("share", report).get("error").is_some());
    assert!(
        h.step("reserve", json!({"recipient":"another-task"}))
            .get("error")
            .is_some()
    );
    let reserved = h.ok("reserve", json!({"recipient":"watch-test-recipient"}));
    assert_eq!(
        h.ok("reserve", json!({"recipient":"watch-test-recipient"}))["dispatch_required"],
        false
    );
    assert_eq!(
        h.ok("wait", json!({"id":reserved["job_id"]}))["state"],
        "timeout"
    );
    h.now += 121.0;
    let reserved = h.ok("reserve", json!({"recipient":"watch-test-recipient"}));
    h.now += 121.0;
    assert_eq!(
        h.ok("wait", json!({"id":reserved["job_id"]}))["state"],
        "unavailable"
    );
    h.ok("pause", json!({}));
    assert!(h.step("share", json!({"id":"bad"})).get("error").is_some());
}

#[test]
fn comparison_preserves_authored_hex_ids_hashes_and_exact_lease_times() {
    for (path, left, right) in [
        (
            "/result/ok/working/doc/known/user/id",
            json!("a".repeat(32)),
            json!("b".repeat(32)),
        ),
        (
            "/result/ok/identity",
            json!("a".repeat(64)),
            json!("b".repeat(64)),
        ),
        (
            "/state/project/worktrees/tree/native/job.json/expires",
            json!(120.0),
            json!(121.0),
        ),
        (
            "/state/project/shared-inbox/root/journals/event.json/prepared",
            json!("proof-a"),
            json!("proof-b"),
        ),
    ] {
        let mut c = Compare {
            forward: BTreeMap::new(),
            reverse: BTreeMap::new(),
            label: "normalization guard".into(),
            py: "python".into(),
            rs: "native".into(),
        };
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| c.equal(&left, &right, path)))
                .is_err(),
            "concealed {path}"
        );
    }
}
