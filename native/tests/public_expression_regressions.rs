//! Focused immutable-oracle regressions found outside the original expression corpus.
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn normalize(text: &str, root: &Path) -> String {
    let text = text.replace(root.to_str().unwrap(), "$ROOT");
    regex::Regex::new(r"\.GROUNDING\.yaml\.pre-core-[A-Za-z0-9_-]+\.bak")
        .unwrap()
        .replace_all(&text, ".GROUNDING.yaml.pre-core-$BACKUP.bak")
        .into_owned()
}
fn images(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, path: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for item in fs::read_dir(path).unwrap() {
            let path = item.unwrap().path();
            let relative = path.strip_prefix(root).unwrap();
            if relative.starts_with("cache")
                || relative.starts_with("native-cache")
                || relative.starts_with("state/kpopper/cache")
            {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                out.insert(
                    normalize(relative.to_str().unwrap(), root),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    walk(root, root, &mut result);
    result
}
fn run(program: &Path, oracle: Option<&Path>, root: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(program);
    if let Some(oracle) = oracle {
        command.arg(oracle.join("scripts/cli.py"));
        command.env_remove("KPOPPER_NATIVE_RESOURCES");
        command.env_remove("KPOPPER_NATIVE_CACHE");
        command.env_remove("XDG_CACHE_HOME");
    } else {
        command.env("KPOPPER_NATIVE_CACHE", root.join("native-cache"));
    }
    command
        .args(args)
        .current_dir(root)
        .env("KPOPPER_SESSION_DISABLE", "1")
        .env("XDG_STATE_HOME", root.join("state"))
        .output()
        .unwrap()
}
fn compare(record: Option<&[u8]>, args: &[&str]) {
    let python =
        PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_PYTHON").expect("oracle python"));
    let oracle = PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_ROOT").expect("oracle root"));
    let native = PathBuf::from(env!("CARGO_BIN_EXE_kpop-native"));
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let prepare = || {
        if let Some(record) = record {
            fs::write(root.join("GROUNDING.yaml"), record).unwrap();
        }
    };
    prepare();
    let expected = run(&python, Some(&oracle), &root, args);
    let expected_images = images(&root);
    fs::remove_dir_all(&root).unwrap();
    fs::create_dir(&root).unwrap();
    prepare();
    let actual = run(&native, None, &root, args);
    assert_eq!(
        actual.status.code(),
        expected.status.code(),
        "exit: {args:?}"
    );
    assert_eq!(
        normalize(std::str::from_utf8(&actual.stdout).unwrap(), &root),
        normalize(std::str::from_utf8(&expected.stdout).unwrap(), &root),
        "stdout: {args:?}"
    );
    assert_eq!(actual.stderr, expected.stderr, "stderr: {args:?}");
    assert_eq!(images(&root), expected_images, "files: {args:?}");
}

#[test]
#[ignore = "requires immutable Python oracle and KPOPPER_NATIVE_RESOURCES"]
fn absent_predicates_mixed_scalar_keys_and_bom_match_python() {
    assert!(std::env::var_os("KPOPPER_NATIVE_RESOURCES").is_some());
    let absent = b"known:\n  p.a: {v: 1}\n  p.b: {rule: p.a + 1}\njudgments:\n  d.x: {rests_on: [p.b], seen: {p.b: 2}, verdict: Fine}\n";
    compare(
        Some(absent),
        &[
            "expressions",
            "migrate",
            "--record",
            "GROUNDING.yaml",
            "--apply",
        ],
    );
    for fields in [
        "v: 3, 2: 5",
        "2: 5, v: 3",
        "v: 3, true: 5",
        "true: 5, v: 3",
        "v: 3, 2.5: 5",
        "2.5: 5, v: 3",
        "v: 3, null: 5",
        "null: 5, v: 3",
        "v: 3, 2026-01-02: 5",
        "2026-01-02: 5, v: 3",
        "v: 3, 2026-01-02T03:04:05Z: 5",
        "2026-01-02T03:04:05Z: 5, v: 3",
    ] {
        let mixed = format!(
            "known:\n  p.z: {{v: 2}}\n  p.a: {{{fields}}}\njudgments:\n  d.x: {{rests_on: [p.z], wrong_if: p.z > 40, seen: {{p.z: 2}}, verdict: Fine}}\n"
        );
        compare(
            Some(mixed.as_bytes()),
            &[
                "expressions",
                "migrate",
                "--record",
                "GROUNDING.yaml",
                "--apply",
            ],
        );
    }
    let mut bom = b"\xef\xbb\xbf".to_vec();
    bom.extend_from_slice(b"known:\n  p.a: {v: 2}\n  p.b: {rule: p.a + 1}\njudgments:\n  d.x: {rests_on: [p.b], wrong_if: p.b > 4, seen: {p.b: 3}, verdict: Fine}\n");
    compare(
        Some(&bom),
        &[
            "expressions",
            "migrate",
            "--record",
            "GROUNDING.yaml",
            "--profile",
            "core/v1",
            "--apply",
        ],
    );
}

#[test]
#[ignore = "requires immutable Python oracle"]
fn leading_hyphens_parser_errors_and_migration_diagnostics_match_python() {
    for expression in [
        "- 3",
        "- 3 * 2",
        "-3 + 1",
        "-1.5 + p.a",
        "-x + 1",
        "-p.a + 1",
        "-(p.a + 1)",
    ] {
        compare(None, &["expressions", "convert", expression]);
    }
    for args in [
        vec!["expressions", "convert", ""],
        vec!["expressions", "convert", "01"],
        vec!["expressions", "convert", "max(p.a"],
        vec!["expressions", "convert", "a", "b"],
        vec!["expressions", "convert", "--bogus", "a"],
    ] {
        compare(None, &args);
    }
    for rule in ["2026-01-02", "max(p.a", &"p.a + ".repeat(801)] {
        let record = format!(
            "known:\n  p.a: {{v: 2}}\n  p.b: {{rule: {rule:?}}}\njudgments:\n  d.x: {{rests_on: [p.a], wrong_if: p.a > 4, seen: {{p.a: 2}}, verdict: Fine}}\n"
        );
        compare(
            Some(record.as_bytes()),
            &["expressions", "migrate", "--record", "GROUNDING.yaml"],
        );
        compare(
            Some(record.as_bytes()),
            &[
                "expressions",
                "migrate",
                "--record",
                "GROUNDING.yaml",
                "--profile",
                "core/v1",
            ],
        );
    }
}
