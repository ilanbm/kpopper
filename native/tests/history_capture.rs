use kpop_native::history_capture;
use serde_json::Value;
use std::{collections::BTreeMap, fs, path::Path};
use tempfile::TempDir;

fn fixtures() -> Value {
    serde_json::from_str(include_str!("fixtures/history-capture.json")).unwrap()
}
fn write(root: &Path, case: &Value) {
    for (name, raw) in case["files"].as_object().unwrap() {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, raw.as_str().unwrap()).unwrap();
    }
}
fn files(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for item in fs::read_dir(dir).unwrap() {
            let path = item.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                out.insert(
                    path.strip_prefix(root).unwrap().to_str().unwrap().into(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}
#[test]
fn exact_capture_matches_python_in_both_record_layouts_without_writes() {
    for case in fixtures()["cases"].as_array().unwrap() {
        let temp = TempDir::new().unwrap();
        write(temp.path(), case);
        let before = files(temp.path());
        let result = history_capture::capture(
            &temp.path().join(case["entry"].as_str().unwrap()),
            None,
            None,
        );
        if case.get("error").is_some() {
            assert!(result.is_err(), "accepted {}", case["name"]);
        } else {
            let capture = result.unwrap_or_else(|e| panic!("{}: {e}", case["name"]));
            assert_eq!(
                capture.evidence().to_tagged().unwrap(),
                case["output"],
                "{}",
                case["name"]
            );
            capture.verify_current().unwrap();
        }
        assert_eq!(files(temp.path()), before, "wrote during {}", case["name"]);
    }
}
#[test]
fn inventory_revalidation_catches_bytes_membership_and_pending_journals() {
    let data = fixtures();
    let case = &data["cases"][0];
    for mutation in [
        "bytes",
        "added",
        "removed",
        "journal",
        "hidden-cancellation",
    ] {
        let temp = TempDir::new().unwrap();
        write(temp.path(), case);
        let capture =
            history_capture::capture(&temp.path().join("GROUNDING.yaml"), None, None).unwrap();
        match mutation {
            "bytes" => fs::write(temp.path().join("GROUNDING.yaml"), b"changed").unwrap(),
            "added" => fs::write(
                temp.path().join(".kpopper/history-commits/extra.yaml"),
                b"new",
            )
            .unwrap(),
            "removed" => {
                fs::remove_file(temp.path().join(".kpopper/history-commits/root.yaml")).unwrap()
            }
            "journal" => {
                let path = temp.path().join(&capture.layout.journal);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, b"{}").unwrap();
            }
            _ => {
                let dir = temp.path().join(".kpopper/history-cancellations");
                fs::create_dir_all(&dir).unwrap();
                fs::write(dir.join(".hidden"), b"{}").unwrap();
            }
        }
        assert!(capture.verify_current().is_err(), "accepted {mutation}");
    }
}
#[cfg(unix)]
#[test]
fn capture_rejects_links_below_the_chosen_root() {
    use std::os::unix::fs::symlink;
    let data = fixtures();
    let case = &data["cases"][0];
    let temp = TempDir::new().unwrap();
    write(temp.path(), case);
    let target = temp.path().join("GROUNDING.yaml");
    fs::rename(&target, temp.path().join("outside.yaml")).unwrap();
    symlink("outside.yaml", target).unwrap();
    assert!(history_capture::capture(&temp.path().join("GROUNDING.yaml"), None, None).is_err());
}
