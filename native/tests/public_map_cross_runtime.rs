//! Full map/onboarding oracle comparison.  This is opt-in because it needs the pinned Python 1.8
//! distribution and a built native executable; no model or host agent is started.
use serde_json::Value;
use std::{fs, path::{Path, PathBuf}, process::Command};

fn invoke(program: &Path, script: Option<&Path>, workspace: &Path, state: &Path) -> Value {
    let mut c=Command::new(program);
    if let Some(s)=script { c.arg(s); }
    let o=c.args(["--workspace",workspace.to_str().unwrap(),"--json","map"])
        .env("XDG_STATE_HOME",state).env("KPOPPER_AGENT_SESSION","fixture").output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr)); serde_json::from_slice(&o.stdout).unwrap()
}
fn request(v:&Value)->&str { v["request"].as_str().unwrap() }
#[test]
#[ignore = "requires KPOP_SESSION_ORACLE_PYTHON and KPOP_SESSION_ORACLE_ROOT"]
fn python_and_native_map_packets_match_except_explicit_provenance() {
    let python=PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_PYTHON").expect("oracle python"));
    let oracle=PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_ROOT").expect("oracle root"));
    let native=PathBuf::from(env!("CARGO_BIN_EXE_kpop-native")); let t=tempfile::tempdir().unwrap();
    let root=t.path().canonicalize().unwrap(); let state=root.join("state"); fs::create_dir_all(&state).unwrap();
    let py=invoke(&python,Some(&oracle.join("scripts/cli.py")),&root,&state);
    let py_id=request(&py).to_owned();
    let key=fs::read_dir(state.join("kpopper/first-use/projects")).unwrap().next().unwrap().unwrap().file_name();
    fs::remove_dir_all(state.join("kpopper/first-use/projects").join(key)).unwrap();
    let nat=invoke(&native,None,&root,&state); let nat_id=request(&nat).to_owned();
    assert_eq!(py["instructions"], nat["instructions"]);
    assert_eq!(py["mode"], nat["mode"]); assert_eq!(py["owner"], nat["owner"]); assert_eq!(py["status"], nat["status"]);
    assert_ne!(py_id,nat_id); // UUIDs are independent; all other packet fields are compared above.
    let pstate=fs::read_dir(state.join("kpopper/first-use/projects")).unwrap().next().unwrap().unwrap().path().join("mapping.json");
    let saved:Value=serde_json::from_slice(&fs::read(pstate).unwrap()).unwrap(); assert_eq!(saved["schema"],1); assert_eq!(saved["mode"],"map"); assert_eq!(saved["mapping"],"ready"); assert_eq!(saved["owner"],"fixture"); assert_eq!(saved["request"],nat_id);
}

#[test]
fn native_protocol_receipt_refusals_and_completion() {
    let native=PathBuf::from(env!("CARGO_BIN_EXE_kpop-native")); let t=tempfile::tempdir().unwrap();
    let root=t.path().canonicalize().unwrap(); let state=root.join("state"); fs::create_dir_all(&state).unwrap();
    let packet=invoke(&native,None,&root,&state); let id=request(&packet); let reused=invoke(&native,None,&root,&state); assert_eq!(request(&reused), id);
    let run=|args: &[&str]| { let o=Command::new(&native).args(args).env("XDG_STATE_HOME",&state).env("KPOPPER_AGENT_SESSION","fixture").output().unwrap(); (o.status.success(),String::from_utf8_lossy(&o.stdout).to_string(),String::from_utf8_lossy(&o.stderr).to_string()) };
    assert!(!run(&["--workspace",root.to_str().unwrap(),"--json","_agent","complete","--request",id,"--report","missing"] ).0);
    let wrong=Command::new(&native).args(["--workspace",root.to_str().unwrap(),"--json","_agent","task"]).env("XDG_STATE_HOME",&state).env("KPOPPER_AGENT_SESSION","other").output().unwrap(); assert!(!wrong.status.success());
    assert!(run(&["--workspace",root.to_str().unwrap(),"_agent","accept","--request",id]).0);
    let report=root.join("report.md"); fs::write(&report,"map report\n").unwrap();
    assert!(run(&["--workspace",root.to_str().unwrap(),"_agent","complete","--request",id,"--report",report.to_str().unwrap()]).0);
    let status=run(&["--workspace",root.to_str().unwrap(),"--json","_agent","status"]); assert!(status.0); let value:Value=serde_json::from_str(&status.1).unwrap(); assert_eq!(value["mapping"],"complete"); assert_eq!(value["report"],report.to_str().unwrap());
    let bad=root.join("state/kpopper/first-use/projects").join(fs::read_dir(root.join("state/kpopper/first-use/projects")).unwrap().next().unwrap().unwrap().file_name()).join("mapping.json"); fs::write(&bad,b"{\"schema\":1,\"mode\":\"map\",\"mapping\":\"ready\",\"request\":\"BAD\",\"owner\":\"fixture\"}\n").unwrap();
    assert!(!run(&["--workspace",root.to_str().unwrap(),"--json","_agent","status"]).0);
}
