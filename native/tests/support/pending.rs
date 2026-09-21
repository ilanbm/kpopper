use base64::{Engine, engine::general_purpose::STANDARD};
use kpop_native::{pending_state as P, project_modes::Project};
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};
pub fn git(root: &Path, args: &[&str], input: Option<&[u8]>) -> Vec<u8> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(root)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    if let Some(input) = input {
        child.stdin.take().unwrap().write_all(input).unwrap();
    }
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    result.stdout
}
pub fn fixture(root: &Path, case: &serde_json::Value) {
    git(
        root,
        &[
            "init",
            "-b",
            "trunk",
            &format!(
                "--object-format={}",
                case["object_format"].as_str().unwrap()
            ),
        ],
        None,
    );
    for object in case["objects"].as_array().unwrap() {
        let raw = STANDARD.decode(object["raw"].as_str().unwrap()).unwrap();
        let got = git(
            root,
            &[
                "hash-object",
                "-w",
                "--stdin",
                "-t",
                object["kind"].as_str().unwrap(),
            ],
            Some(&raw),
        );
        assert_eq!(
            String::from_utf8(got).unwrap().trim(),
            object["oid"].as_str().unwrap()
        );
    }
    if let Some(head) = case["head"].as_str() {
        git(root, &["update-ref", P::REF, head], None);
    }
    for (path, raw) in case["files"].as_object().unwrap() {
        std::fs::write(root.join(path), raw.as_str().unwrap()).unwrap();
    }
    let project = Project::open(root).unwrap();
    if let Some(raw) = case["config"].as_str() {
        std::fs::create_dir_all(&project.state).unwrap();
        std::fs::write(&project.config_path, raw).unwrap();
    }
    if let Some(target) = case["target_ref"].as_str() {
        git(
            root,
            &["update-ref", "refs/remotes/origin/trunk", target],
            None,
        );
    }
    if let Some(raw) = case["publication"].as_str() {
        std::fs::create_dir_all(&project.state).unwrap();
        std::fs::write(project.state.join("publication.json"), raw).unwrap();
    }
}
