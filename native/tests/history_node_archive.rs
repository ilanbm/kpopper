use kpop_native::{history_node_archive::Archive, value::TypedValue};
use serde_json::json;
use std::{collections::BTreeMap, fs, io::Write};
use zip::{write::SimpleFileOptions, CompressionMethod, DateTime, ZipWriter};

fn wrapped_json(bytes: &[u8]) -> Vec<u8> {
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .last_modified_time(DateTime::default())
        .unix_permissions(0o100644);
    let mut zip = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.start_file("archive.json", options).unwrap();
    zip.write_all(bytes).unwrap();
    zip.finish().unwrap().into_inner()
}

#[test]
fn archive_round_trips_exact_bytes_and_is_deterministic() {
    let files = BTreeMap::from([
        ("records/first.yaml".into(), b"first\r\nsecond\r\n".to_vec()),
        ("records/raw.bin".into(), vec![0, 0xff, 0x80, b'\n', 0]),
    ]);
    let metadata = TypedValue::Map(BTreeMap::from([
        ("kind".into(), TypedValue::Text("copy-only".into())),
        (
            "version".into(),
            TypedValue::Integer(kpop_native::value::Integer::new("1").unwrap()),
        ),
    ]));
    let archive = Archive::new(files.clone(), metadata.clone()).unwrap();
    let first = archive.encode().unwrap();
    let second = archive.encode().unwrap();
    assert_eq!(first, second);

    let decoded = Archive::decode(&first).unwrap();
    assert_eq!(decoded.files(), &files);
    assert_eq!(decoded.metadata(), &metadata);
    assert_eq!(decoded.encode().unwrap(), first);

    let reconstructed = decoded.reconstruct().unwrap();
    for (path, bytes) in files {
        assert_eq!(fs::read(reconstructed.path().join(path)).unwrap(), bytes);
    }
}

#[test]
fn repeated_receipts_compress_across_files() {
    let repeated = b"receipt: same body and same fields\r\n".repeat(12_000);
    let files = (0..24)
        .map(|index| (format!("receipts/{index:02}.json"), repeated.clone()))
        .collect();
    let archive = Archive::new(files, TypedValue::Null)
        .unwrap()
        .encode()
        .unwrap();
    assert!(archive.len() < repeated.len() * 24 / 20);
}

#[test]
fn rejects_tampering_duplicate_fields_extra_members_and_traversal() {
    let original = Archive::new(BTreeMap::new(), TypedValue::Null)
        .unwrap()
        .encode()
        .unwrap();
    let mut tampered = original.clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 1;
    assert!(Archive::decode(&tampered).is_err());

    let duplicate = wrapped_json(br#"{"files":{},"files":{},"metadata":["null"],"version":1}"#);
    assert!(Archive::decode(&duplicate).is_err());

    let traversal = wrapped_json(
        serde_json::to_vec(&json!({
            "files": {"../outside": "eA=="},
            "metadata": ["null"],
            "version": 1,
        }))
        .unwrap()
        .as_slice(),
    );
    assert!(Archive::decode(&traversal).is_err());

    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .last_modified_time(DateTime::default())
        .unix_permissions(0o100644);
    let mut zip = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.start_file("archive.json", options).unwrap();
    zip.write_all(br#"{"files":{},"metadata":["null"],"version":1}"#)
        .unwrap();
    zip.start_file("extra", options).unwrap();
    zip.write_all(b"no").unwrap();
    assert!(Archive::decode(&zip.finish().unwrap().into_inner()).is_err());

    let mut zip = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.add_symlink("archive.json", "outside", options).unwrap();
    assert!(Archive::decode(&zip.finish().unwrap().into_inner()).is_err());
}

#[test]
fn rejects_unsafe_paths_and_size_limits() {
    for path in [
        "/absolute",
        "../parent",
        "a/./b",
        "a/.git/config",
        "a/.GIT/config",
        "a\\b",
        "C:/absolute",
    ] {
        assert!(Archive::new(BTreeMap::from([(path.into(), vec![1])]), TypedValue::Null).is_err());
    }

    assert!(Archive::new(
        BTreeMap::from([("parent".into(), vec![1]), ("parent/child".into(), vec![2])]),
        TypedValue::Null
    )
    .is_err());

    assert!(Archive::new(
        BTreeMap::from([("large.bin".into(), vec![0; 16 * 1024 * 1024 + 1])]),
        TypedValue::Null
    )
    .is_err());

    let aggregate = (0..4)
        .map(|index| (format!("{index}.bin"), vec![0; 16 * 1024 * 1024]))
        .chain(std::iter::once(("z.bin".into(), vec![0])))
        .collect();
    assert!(Archive::new(aggregate, TypedValue::Null).is_err());

    let too_many = (0..=64 * 1024)
        .map(|index| (format!("{index}.bin"), Vec::new()))
        .collect();
    assert!(Archive::new(too_many, TypedValue::Null).is_err());

    assert!(Archive::new(
        BTreeMap::new(),
        TypedValue::Text("x".repeat(1024 * 1024 + 1))
    )
    .is_err());

    let mut too_deep = TypedValue::Null;
    for _ in 0..=kpop_native::value::MAX_DEPTH {
        too_deep = TypedValue::List(vec![too_deep]);
    }
    assert!(Archive::new(BTreeMap::new(), too_deep).is_err());
}

#[test]
fn rejects_noncanonical_base64_and_invalid_typed_metadata() {
    let bad_base64 = wrapped_json(
        serde_json::to_vec(&json!({
            "files": {"valid.bin": "%%"},
            "metadata": ["null"],
            "version": 1,
        }))
        .unwrap()
        .as_slice(),
    );
    assert!(Archive::decode(&bad_base64).is_err());

    let bad_metadata = wrapped_json(
        serde_json::to_vec(&json!({
            "files": {},
            "metadata": ["not-a-tag"],
            "version": 1,
        }))
        .unwrap()
        .as_slice(),
    );
    assert!(Archive::decode(&bad_metadata).is_err());
}
