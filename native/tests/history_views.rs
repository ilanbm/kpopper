use kpop_native::{
    history_store::Store, history_transaction::PreparedMutation, history_view,
    value::TypedValue as V,
};
use serde_json::Value;
use std::{collections::BTreeMap, fs, path::Path};
use tempfile::TempDir;
fn fixtures() -> Value {
    serde_json::from_str(include_str!("fixtures/history-views.json")).unwrap()
}
fn write(root: &Path, files: &Value) {
    for (path, raw) in files.as_object().unwrap() {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, raw.as_str().unwrap()).unwrap();
    }
}
fn tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, p: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for e in fs::read_dir(p).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(root, &p, out);
            } else {
                out.insert(
                    p.strip_prefix(root).unwrap().to_str().unwrap().into(),
                    fs::read(p).unwrap(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}
fn check<T: std::fmt::Debug>(
    result: kpop_native::Result<T>,
    c: &Value,
    key: &str,
    encode: impl FnOnce(T) -> Value,
) {
    match result {
        Ok(value) => assert_eq!(encode(value), c[key], "{} {key}", c["name"]),
        Err(e) => assert_eq!(
            Some(e.0.split(':').next().unwrap()),
            c[format!("{key}_error")].as_str(),
            "{} {key}: {e}",
            c["name"]
        ),
    }
}
#[test]
fn retained_views_reconcile_and_rebuild_like_python() {
    for c in fixtures()["cases"].as_array().unwrap() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), &c["files"]);
        let before = tree(tmp.path());
        let store = Store::new(&tmp.path().join(c["entry"].as_str().unwrap())).unwrap();
        let conflicts = c["allow_conflicts"].as_bool().unwrap();
        check(
            store
                .capture_reconciliation(conflicts)
                .and_then(|cap| store.render(&cap)),
            c,
            "render",
            |b| Value::String(String::from_utf8(b).unwrap()),
        );
        check(
            store.prepare_reconciliation(None, conflicts, &[]),
            c,
            "reconciliation",
            |v| v.to_tagged().unwrap(),
        );
        check(
            store.rebuild(None, false, conflicts, &[]),
            c,
            "rebuild",
            |b| Value::String(String::from_utf8(b).unwrap()),
        );
        assert_eq!(before, tree(tmp.path()), "read modified {}", c["name"]);
        if let Some(raw) = c["mutation"].as_str() {
            let mutation = PreparedMutation::from_bytes(raw.as_bytes()).unwrap();
            let mut calls = 0;
            check(
                store.commit(&mutation, &mut |_| {
                    calls += 1;
                    Ok(())
                }),
                c,
                "commit",
                |v| v.to_tagged().unwrap(),
            );
            assert_eq!(calls, 1);
            let after: BTreeMap<_, _> = tree(tmp.path())
                .into_iter()
                .map(|(k, v)| (k, v.iter().map(|b| format!("{b:02x}")).collect::<String>()))
                .collect();
            assert_eq!(serde_json::to_value(after).unwrap(), c["after"]);
            store.commit(&mutation, &mut |_| Ok(())).unwrap();
        } else if let Some(bytes) = c["rebuild"].as_str() {
            store.rebuild(None, true, conflicts, &[]).unwrap();
            let mut expected = before;
            expected.insert(
                c["entry"].as_str().unwrap().into(),
                bytes.as_bytes().to_vec(),
            );
            assert_eq!(tree(tmp.path()), expected);
        }
    }
}
#[test]
fn typed_leaf_differences_keep_missing_and_null_separate() {
    let before = V::from_json(&serde_json::json!({"a":null,"b":1,"c":true})).unwrap();
    let after = V::from_json(&serde_json::json!({"b":true,"c":1,"d":null})).unwrap();
    let changes = history_view::changes(&before, &after).unwrap();
    assert_eq!(changes.len(), 4);
}
fn hex(s: &str) -> Vec<u8> {
    s.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect()
}
#[test]
fn publication_prefixes_recover_or_refuse_exactly_like_python() {
    let data: Value =
        serde_json::from_str(include_str!("fixtures/history-store-recovery.json")).unwrap();
    for c in data["cases"].as_array().unwrap() {
        let tmp = TempDir::new().unwrap();
        for (path, raw) in c["files"].as_object().unwrap() {
            let path = tmp.path().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, hex(raw.as_str().unwrap())).unwrap();
        }
        let entry = tmp.path().join("GROUNDING.yaml");
        let store = Store::new(&entry).unwrap();
        let m = PreparedMutation::from_bytes(c["mutation"].as_str().unwrap().as_bytes()).unwrap();
        let mut calls = 0;
        let result = store.commit(&m, &mut |_| {
            calls += 1;
            match c["sabotage"].as_str().unwrap() {
                "callback" => {
                    fs::write(&entry, b"callback edit\n").unwrap();
                    Ok(())
                }
                "verifier" => Err(kpop_native::Error("caller_refused".into())),
                _ => Ok(()),
            }
        });
        match result {
            Ok(v) => assert_eq!(v.to_tagged().unwrap(), c["output"], "{}", c["name"]),
            Err(e) => assert_eq!(
                Some(e.0.split(':').next().unwrap()),
                c["error"].as_str(),
                "{}: {e}",
                c["name"]
            ),
        }
        assert_eq!(calls, c["calls"].as_u64().unwrap());
        let expected = c["after"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), hex(v.as_str().unwrap())))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(tree(tmp.path()), expected, "{}", c["name"]);
    }
}
#[test]
fn complete_conflicts_match_python_without_treating_yaml_text_as_markers() {
    let data: Value =
        serde_json::from_str(include_str!("fixtures/history-conflicts.json")).unwrap();
    for c in data["cases"].as_array().unwrap() {
        let result = history_view::alternatives(c["raw"].as_str().unwrap().as_bytes());
        match result{Ok(a)=>assert_eq!(serde_json::to_value(a.into_iter().map(|a|serde_json::json!({"name":a.name,"raw":String::from_utf8(a.bytes).unwrap(),"document":a.document.to_tagged().unwrap()})).collect::<Vec<_>>()).unwrap(),c["output"],"{}",c["name"]),Err(e)=>assert_eq!(Some(e.0.as_str()),c["error"].as_str(),"{}",c["name"])}
    }
}
#[test]
fn common_preparation_reconstructs_exact_python_transaction_bytes() {
    let data = fixtures();
    let c = data["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c.get("mutation").is_some())
        .unwrap();
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), &c["files"]);
    let store = Store::new(&tmp.path().join("GROUNDING.yaml")).unwrap();
    let capture = store.capture().unwrap();
    let original =
        PreparedMutation::from_bytes(c["mutation"].as_str().unwrap().as_bytes()).unwrap();
    let mut objects = Vec::new();
    let mut manifest = None;
    for f in original.files() {
        if f.role == "history_object" {
            objects.push(
                kpop_native::history_yaml::decode_document(f.after.as_ref().unwrap()).unwrap(),
            );
        }
        if f.role == "history_commit" {
            manifest = Some(
                kpop_native::history_yaml::decode_document(f.after.as_ref().unwrap()).unwrap(),
            );
        }
    }
    let V::Map(m) = manifest.unwrap() else {
        panic!()
    };
    let V::Map(d) = original.to_data() else {
        panic!()
    };
    let prepared = kpop_native::history_preparation::prepare_commit(
        &capture,
        "new",
        &objects,
        &m["view_template"],
        &d["receipt"],
        m.get("requires"),
    )
    .unwrap();
    assert_eq!(prepared.to_bytes().unwrap(), original.to_bytes().unwrap());
}
#[test]
fn render_mapping_and_template_refusals_match_python() {
    let data: Value = serde_json::from_str(include_str!("fixtures/history-render.json")).unwrap();
    for c in data["cases"].as_array().unwrap() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), &c["files"]);
        let store = Store::new(&tmp.path().join("GROUNDING.yaml")).unwrap();
        let capture = store.capture().unwrap();
        let mut objects = BTreeMap::new();
        let mut raw = BTreeMap::new();
        for bytes in c["objects"].as_array().unwrap() {
            let bytes = bytes.as_str().unwrap().as_bytes();
            let obj = kpop_native::history_yaml::decode_document(bytes).unwrap();
            let V::Map(m) = &obj else { panic!() };
            let (V::Text(subject), V::Text(id)) = (&m["subject"], &m["id"]) else {
                panic!()
            };
            raw.insert((subject.clone(), id.clone()), bytes.to_vec());
            objects.insert(id.clone(), obj);
        }
        let commits = c["commits"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(op, b)| (op.clone(), b.as_str().unwrap().as_bytes().to_vec()))
            .collect();
        match history_view::render(&capture, &objects, &raw, &commits) {
            Ok(b) => assert_eq!(
                String::from_utf8(b).unwrap(),
                c["output"].as_str().unwrap(),
                "{}",
                c["name"]
            ),
            Err(e) => assert_eq!(Some(e.0.as_str()), c["error"].as_str(), "{}", c["name"]),
        }
    }
}
#[test]
fn self_consistent_receipt_cannot_publish_a_noncanonical_view() {
    let data = fixtures();
    let c = data["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c.get("mutation").is_some())
        .unwrap();
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), &c["files"]);
    let before = tree(tmp.path());
    let store = Store::new(&tmp.path().join("GROUNDING.yaml")).unwrap();
    let original =
        PreparedMutation::from_bytes(c["mutation"].as_str().unwrap().as_bytes()).unwrap();
    let V::Map(d) = original.to_data() else {
        panic!()
    };
    let mut files = original.files().to_vec();
    let record = files.iter_mut().find(|f| f.role == "record").unwrap();
    record
        .after
        .as_mut()
        .unwrap()
        .extend_from_slice(b"# typed-equal but not generated\n");
    let hash = kpop_native::identity::sha256(record.after.as_ref().unwrap());
    let manifest = files
        .iter_mut()
        .find(|f| f.role == "history_commit")
        .unwrap();
    let V::Map(mut m) =
        kpop_native::history_yaml::decode_document(manifest.after.as_ref().unwrap()).unwrap()
    else {
        panic!()
    };
    m.insert("view_sha256".into(), V::Text(hash));
    manifest.after = Some(kpop_native::history_emit::encode_document(&V::Map(m)).unwrap());
    let mutation = PreparedMutation::prepare(
        "new",
        &d["authority"],
        &d["baseline"],
        files,
        &d["receipt"],
        "GROUNDING.yaml",
        None,
    )
    .unwrap();
    let mut called = false;
    let error = store
        .commit(&mutation, &mut |_| {
            called = true;
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.0, "view_projection_mismatch");
    assert!(!called);
    assert_eq!(tree(tmp.path()), before);
}
