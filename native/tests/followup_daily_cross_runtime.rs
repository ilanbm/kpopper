//! Opt-in immutable-oracle comparison. Every boundary compares the entire result
//! and all followup durable files (unrelated Python parser caches are excluded). Both runtimes see identical paths and an exact clock;
//! only new UUID tokens and validated runtime command arrays differ.
use chrono::{DateTime, Duration, TimeZone, Utc};
use kpop_native::{
    followup_daily as D,
    followup_store::{Store, digest, parse_input},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const PY: &str = r#"
import datetime as dt, json, os, sys
from scripts import followups as F, followup_daily as D, followup_install as I
request=json.load(sys.stdin)
s=F.Store(request['workspace'], now=lambda: dt.datetime.fromisoformat(request['now'].replace('Z','+00:00')))
a=request['args']; op=request['op']
try:
    if op=='setup': result=s.setup(timezone=a.get('timezone','UTC'), private=True)
    elif op=='add': result=s.add(a)
    elif op=='status':
        data=s.load(required=False)
        result={'configured':bool(data),'ledger':str(s.path),'suggested_store':s.suggested_store() if not data else None}
        if data: result.update(config=data['config'],daily=D.status(s))
    elif op=='plan': result=D.plan(s,a.get('time','09:00'))
    elif op=='daily_status': result=D.status(s)
    elif op=='bind': result=D.binding(s,a)
    elif op=='start': result=D.start(s,a.get('owner','owner'))
    elif op=='finish': result=D.finish(s,a['token'],a['evidence'])
    elif op=='renew': result=D.renew(s,a['token'])
    elif op=='recover': result=D.recover(s,a['evidence'])
    elif op=='claim': result=s.claim(a['id'],a['occurrence'],a['owner'],a.get('daily_token'))
    elif op=='item_finish': result=s.finish(a['id'],a['token'],a['outcome'],a['evidence'],a.get('next_at'))
    elif op=='item_recover': result=s.recover(a['id'],a['evidence'])
    elif op=='begin': result=I.begin(s,owner=a.get('owner'),time=a.get('time'),timezone=a.get('timezone'),destination=a.get('destination'),private=a.get('private',False),check=a.get('check',False),resume=a.get('resume',False))
    elif op=='inspect': result=I.inspect_host(s,a['token'],a['report'])
    elif op=='result': result=I.finish(s,a['token'],a['report'])
    elif op=='fail': result=I.fail(s,a['token'],a['reason'],a.get('unchanged',False))
    elif op=='reconcile': result=I.reconcile(s,a['token'],a['report'])
    else: raise AssertionError(op)
    result={'ok':result}
except F.Refused as exc: result={'error':str(exc)}
print(json.dumps(F.T.normalize(result),ensure_ascii=False,default=str))
"#;

fn files(path: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, path: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        if !path.exists() {
            return;
        }
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, out);
            } else {
                out.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(path, path, &mut result);
    result
}
fn restore(path: &Path, snapshot: &BTreeMap<PathBuf, Vec<u8>>) {
    if path.exists() {
        fs::remove_dir_all(path).unwrap();
    }
    fs::create_dir_all(path).unwrap();
    for (name, raw) in snapshot {
        let file = path.join(name);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, raw).unwrap();
    }
}
fn durable(raw: &BTreeMap<PathBuf, Vec<u8>>) -> Value {
    let rows = raw
        .iter()
        .filter(|(name, _)| name.starts_with("kpopper/followups"))
        .filter(|(name, _)| name.extension().is_none_or(|ext| ext != "lock"))
        .map(|(name, raw)| {
            (
                name.to_string_lossy().into_owned(),
                if name.extension().is_some_and(|ext| ext == "yaml") {
                    parse_input(raw).unwrap()
                } else {
                    json!(String::from_utf8(raw.clone()).unwrap())
                },
            )
        })
        .collect::<serde_json::Map<_, _>>();
    Value::Object(rows)
}

struct Harness {
    _temp: tempfile::TempDir,
    workspace: PathBuf,
    state: PathBuf,
    now: DateTime<Utc>,
    python: String,
    root: String,
    step: usize,
}
impl Harness {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let workspace = base.join("project");
        fs::create_dir(&workspace).unwrap();
        fs::write(
            workspace.join("PROVENANCE.yaml"),
            "meta:\n  name: Daily contract\nknown:\n  facts.count:\n    name: Count\n    v: 1\n",
        )
        .unwrap();
        Self {
            _temp: temp,
            workspace,
            state: base.join("state"),
            now: Utc.with_ymd_and_hms(2026, 9, 11, 22, 0, 0).unwrap(),
            python: std::env::var("KPOP_SESSION_ORACLE_PYTHON")
                .expect("set KPOP_SESSION_ORACLE_PYTHON"),
            root: std::env::var("KPOP_SESSION_ORACLE_ROOT").expect("set KPOP_SESSION_ORACLE_ROOT"),
            step: 0,
        }
    }
    fn store(&self) -> Store {
        Store::at_in_state(&self.workspace, &self.state, self.now).unwrap()
    }
    fn step(&mut self, op: &str, args: Value) -> Value {
        self.step += 1;
        let before = files(&self.state);
        let mut child = Command::new(&self.python)
            .args(["-c", PY])
            .env("PYTHONPATH", &self.root)
            .env("XDG_STATE_HOME", &self.state)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let request =
            json!({"workspace":self.workspace,"now":self.now.to_rfc3339(),"op":op,"args":args});
        child
            .stdin
            .take()
            .unwrap()
            .write_all(serde_json::to_string(&request).unwrap().as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "Python {op}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let py: Value = serde_json::from_slice(&output.stdout).unwrap();
        let py_disk = files(&self.state);
        restore(&self.state, &before);
        let s = self.store();
        let a = &args;
        let get = |name: &str| a[name].as_str().unwrap();
        let native = match op {
            "setup" => s.setup(None, a["timezone"].as_str().unwrap_or("UTC"), None, true),
            "add" => s.add(a.clone()),
            "status" => {
                let mut status = s.status().unwrap();
                if status["configured"] == true {
                    status["daily"] = D::status(&s).unwrap();
                }
                Ok(status)
            }
            "plan" => D::plan(&s, a["time"].as_str().unwrap_or("09:00")),
            "daily_status" => D::status(&s),
            "bind" => D::bind(&s, a.clone()),
            "start" => D::start(&s, a["owner"].as_str().unwrap_or("owner")),
            "finish" => D::finish(&s, get("token"), get("evidence")),
            "renew" => D::renew(&s, get("token")),
            "recover" => D::recover(&s, get("evidence")),
            "claim" => s.claim(
                get("id"),
                get("occurrence"),
                get("owner"),
                a["daily_token"].as_str(),
            ),
            "item_finish" => s.finish(
                get("id"),
                get("token"),
                get("outcome"),
                get("evidence"),
                a["next_at"].as_str(),
            ),
            "item_recover" => s.recover(get("id"), get("evidence")),
            "begin" => D::install_begin(
                &s,
                a["owner"].as_str(),
                a["time"].as_str(),
                a["timezone"].as_str(),
                a["destination"].as_str().map(Path::new),
                a["private"].as_bool().unwrap_or(false),
                a["check"].as_bool().unwrap_or(false),
                a["resume"].as_bool().unwrap_or(false),
            ),
            "inspect" => D::install_inspect(&s, get("token"), a["report"].clone()),
            "result" => D::install_finish(&s, get("token"), a["report"].clone()),
            "fail" => D::install_fail(
                &s,
                get("token"),
                get("reason"),
                a["unchanged"].as_bool().unwrap_or(false),
            ),
            "reconcile" => D::install_reconcile(&s, get("token"), a["report"].clone()),
            _ => panic!("{op}"),
        };
        let native = match native {
            Ok(value) => json!({"ok":value}),
            Err(error) => json!({"error":error.to_string()}),
        };
        let rs_disk = files(&self.state);
        let py_all = json!({"result":py,"disk":durable(&py_disk)});
        let rs_all = json!({"result":native,"disk":durable(&rs_disk)});
        let mut compare = Comparison::new(self, &args, &py_all, &rs_all);
        compare.equal(&py_all, &rs_all, "");
        if py.get("error").is_some() {
            let only_followups = |all: &BTreeMap<PathBuf, Vec<u8>>| {
                all.iter()
                    .filter(|(name, _)| name.starts_with("kpopper/followups"))
                    .map(|(p, b)| (p.clone(), b.clone()))
                    .collect::<BTreeMap<_, _>>()
            };
            assert_eq!(
                only_followups(&before),
                only_followups(&py_disk),
                "oracle failure changed durable bytes at {op}"
            );
            assert_eq!(
                only_followups(&before),
                only_followups(&rs_disk),
                "native failure changed durable bytes at {op}"
            );
        }
        native
    }
    fn ok(&mut self, op: &str, a: Value) -> Value {
        let v = self.step(op, a);
        assert!(v.get("ok").is_some(), "{op}: {v}");
        v["ok"].clone()
    }
    fn refuse(&mut self, op: &str, a: Value) {
        let v = self.step(op, a);
        assert!(v.get("error").is_some(), "{op}: {v}");
    }
    fn begin(&mut self, owner: &str) -> Value {
        self.ok("begin", json!({"owner":owner}))["installation"].clone()
    }
    fn report(&self, schedules: Value) -> Value {
        json!({"host":"fake-host","observed_at":self.now.to_rfc3339(),"complete":true,"schedules":schedules,"evidence":"Independent fake host inventory"})
    }
    fn schedule(&self, prompt: Value) -> Value {
        json!({"id":"schedule-1","state":"active","workspace_key":self.store().load(true).unwrap().unwrap()["workspace_key"],"cadence":"daily","time":"09:00","timezone":"UTC","prompt":prompt,"access_verified":true})
    }
}

struct Comparison {
    py_hashes: BTreeMap<String, String>,
    rs_hashes: BTreeMap<String, String>,
    tokens: BTreeMap<String, String>,
    reverse: BTreeMap<String, String>,
    py_command: String,
    rs_command: String,
    label: String,
}
impl Comparison {
    fn new(h: &Harness, args: &Value, py: &Value, rs: &Value) -> Self {
        let py_command = format!(
            "\"command\": [{}, {}]",
            serde_json::to_string(&h.python).unwrap(),
            serde_json::to_string(&format!("{}/scripts/cli.py", h.root)).unwrap()
        );
        let rs_command = format!(
            "\"command\": [{}]",
            serde_json::to_string(&std::env::current_exe().unwrap()).unwrap()
        );
        let mut value = Self {
            py_hashes: BTreeMap::new(),
            rs_hashes: BTreeMap::new(),
            tokens: BTreeMap::new(),
            reverse: BTreeMap::new(),
            py_command,
            rs_command,
            label: format!("boundary {}", h.step),
        };
        fn prompts(value: &Value, out: &mut Vec<String>) {
            match value {
                Value::String(s) if s.contains("KPOPPER_DAILY_WORKSPACE=") => out.push(s.clone()),
                Value::Array(rows) => {
                    for row in rows {
                        prompts(row, out)
                    }
                }
                Value::Object(rows) => {
                    for row in rows.values() {
                        prompts(row, out)
                    }
                }
                _ => {}
            }
        }
        for (input, is_python) in [(py, true), (rs, false), (args, true), (args, false)] {
            let mut strings = vec![];
            prompts(input, &mut strings);
            for raw in strings {
                let normalized = value.string(&raw, is_python);
                let actual = digest(&json!(raw)).unwrap();
                let common = digest(&json!(normalized)).unwrap();
                if is_python {
                    value.py_hashes.insert(actual, common);
                } else {
                    value.rs_hashes.insert(actual, common);
                }
            }
        }
        value
    }
    fn string(&self, raw: &str, _python: bool) -> String {
        // Runtime commands are the only sanctioned prompt difference. Preserve
        // every instruction, JSON field/order, path and host customization.
        if !raw.contains("KPOPPER_DAILY_WORKSPACE=") || !raw.contains("\n{\"workspace\": ") {
            return raw.to_owned();
        }
        raw.replace(&self.py_command, "\"command\": [\"${VERIFIED_RUNTIME}\"]")
            .replace(&self.rs_command, "\"command\": [\"${VERIFIED_RUNTIME}\"]")
    }
    fn equal(&mut self, py: &Value, rs: &Value, path: &str) {
        match (py, rs) {
            (Value::Object(a), Value::Object(b)) => {
                assert_eq!(
                    a.keys().collect::<Vec<_>>(),
                    b.keys().collect::<Vec<_>>(),
                    "{} {path}",
                    self.label
                );
                for (key, value) in a {
                    self.equal(value, &b[key], &format!("{path}/{key}"));
                }
            }
            (Value::Array(a), Value::Array(b)) => {
                assert_eq!(a.len(), b.len(), "{} {path}", self.label);
                for (i, (a, b)) in a.iter().zip(b).enumerate() {
                    self.equal(a, b, &format!("{path}/{i}"));
                }
            }
            (Value::String(a), Value::String(b))
                if path.ends_with("/token") || path.ends_with("/daily_token") =>
            {
                assert_eq!(a.len(), 32, "{path}");
                assert_eq!(b.len(), 32, "{path}");
                assert!(
                    a.bytes().chain(b.bytes()).all(|c| c.is_ascii_hexdigit()),
                    "{path}"
                );
                if let Some(previous) = self.tokens.insert(a.clone(), b.clone()) {
                    assert_eq!(previous, *b, "UUID cross-reference {path}");
                }
                if let Some(previous) = self.reverse.insert(b.clone(), a.clone()) {
                    assert_eq!(previous, *a, "UUID alias {path}");
                }
            }
            (Value::String(a), Value::String(b))
                if path.ends_with("/template_hash") || path.ends_with("/prompt_hash") =>
            {
                let a = self.py_hashes.get(a).unwrap_or(a);
                let b = self.rs_hashes.get(b).unwrap_or(b);
                assert_eq!(a, b, "{} prompt digest {path}", self.label);
            }
            (Value::String(a), Value::String(b)) => assert_eq!(
                self.string(a, true),
                self.string(b, false),
                "{} {path}",
                self.label
            ),
            _ => assert_eq!(py, rs, "{} {path}", self.label),
        }
    }
}

#[test]
#[ignore = "requires KPOP_SESSION_ORACLE_PYTHON and KPOP_SESSION_ORACLE_ROOT pointing at immutable Python oracle"]
fn python_and_native_match_daily_lifecycle_and_durable_evidence() {
    let mut h = Harness::new();
    h.ok("status", json!({}));
    h.ok("begin", json!({"check":true}));
    h.refuse("begin", json!({"owner":"owner"}));
    h.refuse("plan", json!({"time":"24:00"}));
    h.ok("setup", json!({}));
    h.ok("add",json!({"id":"review","title":"Review","why":"Keep current","how":"Read it","scope":"Read only","related":["facts.count"],"when":{"at":"2019-12-31"}}));
    h.ok("plan", json!({"time":"10:30"}));
    h.ok("daily_status", json!({}));
    h.refuse("renew", json!({"token":"wrong"}));
    h.refuse("recover", json!({"evidence":"No claim"}));
    h.refuse(
        "bind",
        json!({"host":"fake-host","id":"x","state":"invalid","evidence":"bad"}),
    );
    h.ok(
        "bind",
        json!({"host":"fake-host","id":"schedule-1","state":"paused","evidence":"Read back"}),
    );
    h.ok("status", json!({}));
    h.refuse(
        "bind",
        json!({"host":"fake-host","id":"schedule-2","state":"active","evidence":"Second"}),
    );
    h.now += Duration::hours(24);
    h.ok("status", json!({}));
    h.now += Duration::seconds(1);
    h.ok("status", json!({}));
    h.ok(
        "bind",
        json!({"host":"fake-host","id":"schedule-1","state":"missing","evidence":"Deleted"}),
    );
    h.ok(
        "bind",
        json!({"host":"fake-host","id":"schedule-2","state":"active","evidence":"Replacement"}),
    );
    let started = h.ok("start", json!({"owner":"owner"}));
    let token = started["claim"]["token"].clone();
    assert_eq!(
        started["claim"]["expires_at"],
        json!(
            (h.now + Duration::minutes(30))
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string()
        )
    );
    h.refuse("start", json!({"owner":"other"}));
    h.refuse("finish", json!({"token":"wrong","evidence":"No"}));
    let item=h.ok("claim",json!({"id":"review","occurrence":started["packet"]["items"][0]["occurrence"],"owner":"item-owner","daily_token":token}));
    h.refuse("finish", json!({"token":token,"evidence":"Pending item"}));
    h.now += Duration::minutes(10);
    h.ok("renew", json!({"token":token}));
    h.ok("daily_status", json!({}));
    h.ok("item_finish",json!({"id":"review","token":item["claim"]["token"],"outcome":"released","evidence":"No effects"}));
    h.ok("finish",json!({"token":token,"evidence":"No useful authorized work. 0123456789abcdef0123456789abcdef"}));
    h.ok("finish",json!({"token":token,"evidence":"No useful authorized work. 0123456789abcdef0123456789abcdef"}));
    h.refuse("finish", json!({"token":token,"evidence":"Different"}));
    h.ok("start", json!({}));
    h.ok("status", json!({}));
    h.now += Duration::days(1);
    let started = h.ok("start", json!({}));
    let token = started["claim"]["token"].clone();
    h.refuse("recover", json!({"evidence":"Still live"}));
    h.now += Duration::minutes(30);
    h.ok("status", json!({}));
    h.refuse("renew", json!({"token":token}));
    h.refuse("finish", json!({"token":token,"evidence":"Expired"}));
    h.ok("recover", json!({"evidence":"Effects reconciled"}));
    h.refuse(
        "finish",
        json!({"token":token,"evidence":"Effects reconciled"}),
    );
    h.ok("start", json!({}));
}

#[test]
#[ignore = "requires KPOP_SESSION_ORACLE_PYTHON and KPOP_SESSION_ORACLE_ROOT pointing at immutable Python oracle"]
fn python_and_native_match_installation_receipts_replays_and_reconciliation() {
    let mut h = Harness::new();
    h.ok(
        "begin",
        json!({"owner":"installer","timezone":"UTC","private":true}),
    );
    h.ok("begin", json!({"owner":"installer"}));
    h.ok("begin", json!({"check":true}));
    h.refuse("begin", json!({"owner":"installer","time":"08:30"}));
    h.refuse(
        "begin",
        json!({"owner":"installer","timezone":"Asia/Jerusalem"}),
    );
    let job = h.begin("second");
    let token = job["token"].clone();
    h.refuse(
        "inspect",
        json!({"token":"wrong","report":h.report(json!([]))}),
    );
    let mut stale = h.report(json!([]));
    stale["observed_at"] =
        json!((h.now - Duration::minutes(10) - Duration::seconds(1)).to_rfc3339());
    h.refuse("inspect", json!({"token":token,"report":stale}));
    let mut future = h.report(json!([]));
    future["observed_at"] = json!((h.now + Duration::seconds(1)).to_rfc3339());
    h.refuse("inspect", json!({"token":token,"report":future}));
    let mut incomplete = h.report(json!([]));
    incomplete["complete"] = json!(false);
    h.ok("inspect", json!({"token":token,"report":incomplete}));
    let job = h.begin("third");
    let token = job["token"].clone();
    let apply = h.ok(
        "inspect",
        json!({"token":token,"report":h.report(json!([]))}),
    );
    assert_eq!(apply["action"], "create");
    h.ok("begin", json!({"owner":"another"}));
    h.ok("fail", json!({"token":token,"reason":"Lost reply"}));
    h.refuse(
        "fail",
        json!({"token":token,"reason":"Assumption","unchanged":true}),
    );
    h.ok(
        "inspect",
        json!({"token":token,"report":h.report(json!([]))}),
    );
    let mut reconciliation = h.report(json!([]));
    reconciliation["no_pending_request"] = json!(false);
    h.refuse("reconcile", json!({"token":token,"report":reconciliation}));
    reconciliation["no_pending_request"] = json!(true);
    h.ok("reconcile", json!({"token":token,"report":reconciliation}));
    let job = h.begin("fourth");
    let token = job["token"].clone();
    h.ok(
        "inspect",
        json!({"token":token,"report":h.report(json!([]))}),
    );
    let schedule = h.schedule(job["prompt"].clone());
    let mut receipt = json!({"host":"fake-host","observed_at":h.now.to_rfc3339(),"schedule":schedule,"evidence":"Independent readback"});
    receipt["schedule"]["time"] = json!("08:30");
    h.refuse("result", json!({"token":token,"report":receipt}));
    receipt["schedule"]["time"] = json!("09:00");
    h.ok("result", json!({"token":token,"report":receipt}));
    h.ok("result", json!({"token":token,"report":receipt}));
    receipt["evidence"] = json!("Different");
    h.refuse("result", json!({"token":token,"report":receipt}));
    h.ok("status", json!({}));
    let job = h.begin("fifth");
    let token = job["token"].clone();
    h.ok(
        "inspect",
        json!({"token":token,"report":h.report(json!([h.schedule(job["prompt"].clone())]))}),
    );
    let job = h.begin("sixth");
    let token = job["token"].clone();
    let mut paused = h.schedule(job["prompt"].clone());
    paused["state"] = json!("paused");
    h.ok(
        "inspect",
        json!({"token":token,"report":h.report(json!([paused]))}),
    );
    let job = h.ok("begin", json!({"owner":"resume","resume":true}))["installation"].clone();
    let token = job["token"].clone();
    h.ok(
        "inspect",
        json!({"token":token,"report":h.report(json!([paused]))}),
    );
    h.ok("fail", json!({"token":token,"reason":"Lost resume"}));
    h.ok(
        "inspect",
        json!({"token":token,"report":h.report(json!([paused]))}),
    );
    let mut reconciliation = h.report(json!([paused]));
    reconciliation["no_pending_request"] = json!(true);
    h.ok("reconcile", json!({"token":token,"report":reconciliation}));
    let job = h.begin("custom");
    let token = job["token"].clone();
    let custom = h.schedule(json!(format!(
        "{}\nPreserve this user instruction.",
        job["prompt"].as_str().unwrap()
    )));
    h.ok(
        "inspect",
        json!({"token":token,"report":h.report(json!([custom]))}),
    );
}

#[test]
#[ignore = "requires KPOP_SESSION_ORACLE_PYTHON and KPOP_SESSION_ORACLE_ROOT pointing at immutable Python oracle"]
fn python_and_native_match_host_validation_and_option_preservation() {
    let mut h = Harness::new();
    h.ok("setup", json!({}));
    let job = h.ok(
        "begin",
        json!({"owner":" owner with spaces ","time":"08:45"}),
    )["installation"]
        .clone();
    assert_eq!(job["owner"], " owner with spaces ");
    let token = job["token"].clone();
    let base = h.schedule(job["prompt"].clone());
    for (field, value) in [
        ("id", json!("")),
        ("state", json!("missing")),
        ("workspace_key", json!("f".repeat(64))),
        ("cadence", json!("never")),
        ("time", json!("24:00")),
        ("access_verified", json!(false)),
        ("access_verified", json!(1)),
        (
            "prompt",
            json!(format!(
                "{}\nuser prefix KPOPPER_DAILY_WORKSPACE={}",
                job["prompt"].as_str().unwrap(),
                "f".repeat(64)
            )),
        ),
    ] {
        let mut candidate = base.clone();
        candidate[field] = value;
        h.refuse(
            "inspect",
            json!({"token":token,"report":h.report(json!([candidate]))}),
        );
    }
    let mut extra = base.clone();
    extra["extra"] = json!(true);
    h.refuse(
        "inspect",
        json!({"token":token,"report":h.report(json!([extra]))}),
    );
    let mut old = base.clone();
    old["prompt"] = json!(format!(
        "KPOPPER_DAILY_WORKSPACE={}\nCustomized older daily review",
        old["workspace_key"].as_str().unwrap()
    ));
    assert_eq!(
        h.ok(
            "inspect",
            json!({"token":token,"report":h.report(json!([old]))})
        )["state"],
        "needs_review"
    );
    let job = h.begin("new");
    let token = job["token"].clone();
    assert_eq!(
        h.ok(
            "inspect",
            json!({"token":token,"report":h.report(json!([base.clone(),base]))})
        )["state"],
        "blocked"
    );
    let job = h.begin("new");
    let token = job["token"].clone();
    h.ok(
        "fail",
        json!({"token":token,"reason":"Host does not expose scheduling","unchanged":true}),
    );
    let job = h.begin("new");
    let token = job["token"].clone();
    let mut report = h.report(json!([]));
    report["observed_at"] = json!((h.now - Duration::minutes(10)).to_rfc3339());
    h.ok("inspect", json!({"token":token,"report":report}));
    h.ok(
        "fail",
        json!({"token":token,"reason":"Host rejected without mutation","unchanged":true}),
    );
}

#[test]
fn scoped_comparison_rejects_payload_hash_lease_and_reference_changes() {
    let temp = tempfile::tempdir().unwrap();
    let h = Harness {
        _temp: temp,
        workspace: "/test/project".into(),
        state: "/test/state".into(),
        now: Utc.with_ymd_and_hms(2026, 9, 11, 22, 0, 0).unwrap(),
        python: "/test/python".into(),
        root: "/test/oracle".into(),
        step: 1,
    };
    let prefix = "KPOPPER_DAILY_WORKSPACE=fixed\nInstruction\n";
    let py_prompt = format!(
        "{prefix}{{\"workspace\": \"/test/project\", \"runtime\": {{\"command\": [\"/test/python\", \"/test/oracle/scripts/cli.py\"]}}}}"
    );
    let rs_prompt = format!(
        "{prefix}{{\"workspace\": \"/test/project\", \"runtime\": {{\"command\": [{}]}}}}",
        serde_json::to_string(&std::env::current_exe().unwrap()).unwrap()
    );
    let py = json!({"prompt":py_prompt,"template_hash":digest(&json!(py_prompt)).unwrap(),"attention":"a".repeat(64),"claim":{"token":"0".repeat(32),"expires_at":"2026-09-11T22:30:00Z"},"receipt":{"token":"0".repeat(32)},"evidence":"d".repeat(64)});
    let rs = json!({"prompt":rs_prompt,"template_hash":digest(&json!(rs_prompt)).unwrap(),"attention":"a".repeat(64),"claim":{"token":"1".repeat(32),"expires_at":"2026-09-11T22:30:00Z"},"receipt":{"token":"1".repeat(32)},"evidence":"d".repeat(64)});
    Comparison::new(&h, &json!({}), &py, &rs).equal(&py, &rs, "");
    for pointer in [
        "/prompt",
        "/template_hash",
        "/attention",
        "/claim/expires_at",
        "/receipt/token",
        "/evidence",
    ] {
        let mut changed = rs.clone();
        *changed.pointer_mut(pointer).unwrap() = match pointer {
            "/prompt" => json!(rs_prompt.replace("/test/project", "/wrong/project")),
            "/claim/expires_at" => json!("2026-09-11T22:31:00Z"),
            "/receipt/token" => json!("2".repeat(32)),
            _ => json!("e".repeat(64)),
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Comparison::new(&h, &json!({}), &py, &changed).equal(&py, &changed, "")
        }));
        assert!(result.is_err(), "comparison concealed change to {pointer}");
    }
}

#[test]
#[ignore = "requires KPOP_SESSION_ORACLE_PYTHON and KPOP_SESSION_ORACLE_ROOT pointing at immutable Python oracle"]
fn python_and_native_retain_blocked_configuration_history_and_reconcile_original_options() {
    let mut h = Harness::new();
    h.ok("setup", json!({}));
    h.begin("owner");
    let s = h.store();
    let mut ledger = s.load(true).unwrap().unwrap();
    ledger["config"]["timezone"] = json!("Europe/London");
    fs::write(&s.path, serde_json::to_vec(&ledger).unwrap()).unwrap();
    let job = h.begin("owner");
    let token = job["token"].clone();
    let history = h.store().load(true).unwrap().unwrap()["daily"]["installation_history"].clone();
    assert_eq!(history[0]["state"], "blocked");
    assert_eq!(
        history[0]["reason"],
        "Configuration changed before host mutation"
    );
    h.ok(
        "inspect",
        json!({"token":token,"report":h.report(json!([]))}),
    );
    let s = h.store();
    let mut ledger = s.load(true).unwrap().unwrap();
    ledger["config"]["timezone"] = json!("UTC");
    fs::write(&s.path, serde_json::to_vec(&ledger).unwrap()).unwrap();
    h.refuse("fail", json!({"token":token,"reason":"Config changed"}));
    let mut report = h.report(json!([]));
    report["no_pending_request"] = json!(true);
    let reconciled = h.ok("reconcile", json!({"token":token,"report":report}));
    assert_eq!(reconciled["requested_options"]["timezone"], "Europe/London");
}

#[test]
#[ignore = "requires KPOP_SESSION_ORACLE_PYTHON and KPOP_SESSION_ORACLE_ROOT pointing at immutable Python oracle"]
fn python_and_native_cli_match_complete_readonly_payloads() {
    let mut h = Harness::new();
    h.ok("setup", json!({}));
    for args in [
        vec!["daily", "plan", "--time", "08:45"],
        vec!["daily", "status"],
        vec!["status"],
        vec!["daily", "install", "--check"],
    ] {
        let before = files(&h.state);
        let py = Command::new(&h.python)
            .arg(format!("{}/scripts/cli.py", h.root))
            .args(["--workspace", h.workspace.to_str().unwrap(), "followups"])
            .args(&args)
            .env("PYTHONPATH", &h.root)
            .env("XDG_STATE_HOME", &h.state)
            .output()
            .unwrap();
        assert!(
            py.status.success(),
            "{}",
            String::from_utf8_lossy(&py.stderr)
        );
        let rs = Command::new(env!("CARGO_BIN_EXE_kpop-native"))
            .args(["--workspace", h.workspace.to_str().unwrap(), "followups"])
            .args(&args)
            .env("XDG_STATE_HOME", &h.state)
            .output()
            .unwrap();
        assert!(
            rs.status.success(),
            "{}",
            String::from_utf8_lossy(&rs.stderr)
        );
        let py: Value = serde_json::from_slice(&py.stdout).unwrap();
        let rs: Value = serde_json::from_slice(&rs.stdout).unwrap();
        let mut comparison = Comparison::new(&h, &json!({}), &py, &rs);
        comparison.rs_command = format!(
            "\"command\": [{}]",
            serde_json::to_string(env!("CARGO_BIN_EXE_kpop-native")).unwrap()
        );
        comparison.equal(&py, &rs, "CLI");
        assert_eq!(
            before,
            files(&h.state),
            "read-only command changed durable state {args:?}"
        );
    }
}

#[test]
#[ignore = "requires KPOP_SESSION_ORACLE_PYTHON and KPOP_SESSION_ORACLE_ROOT pointing at immutable Python oracle"]
fn python_and_native_match_text_boundaries() {
    let mut h = Harness::new();
    h.ok("setup", json!({}));
    for owner in [
        format!("{}x", " ".repeat(12_000)),
        "\u{1c}\u{1d}\u{1e}\u{1f}".into(),
        "日".repeat(12_001),
    ] {
        h.refuse("start", json!({"owner":owner}));
        h.refuse("begin", json!({"owner":owner}));
    }
    let job = h.ok("begin", json!({"owner":"日".repeat(12_000)}))["installation"].clone();
    let mut report = h.report(json!([]));
    report["host"] = json!("\u{1c}fake-host\u{1f}");
    report["evidence"] = json!("\u{1d}Read back\u{1e}");
    let applied = h.ok("inspect", json!({"token":job["token"],"report":report}));
    assert_eq!(applied["host"], "fake-host");
    h.ok("start", json!({"owner":"日".repeat(12_000)}));
}
