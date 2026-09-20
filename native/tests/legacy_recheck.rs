use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn command(root: &Path, args: &[&str], zone: &str) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kpop-native"));
    cmd.current_dir(root)
        .env_remove("KPOPPER_AGENT_SESSION")
        .env_remove("CODEX_THREAD_ID")
        .env("TZ", zone)
        .args(args);
    if args.first() == Some(&"add") && !root.join("GROUNDING.yaml").exists() {
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
        cmd.env("KPOPPER_NATIVE_RESOURCES", resources)
            .env("KPOPPER_NATIVE_CACHE", root.join(".test-cache"));
    }
    kpop_native::reasoning_runtime::run_command_capture(
        &mut cmd,
        Vec::new(),
        std::time::Duration::from_secs(90),
        2 * 1024 * 1024,
    ).unwrap_or_else(|error| panic!("{zone} {args:?}: {error}"))
}

#[test]
fn citation_order_indentation_and_long_reading_reports_match_python() {
    let cases: serde_json::Value =
        serde_json::from_slice(include_bytes!("fixtures/legacy-review-recheck.json")).unwrap();
    for case in cases.as_array().unwrap() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("GROUNDING.yaml"),
            case["before"].as_str().unwrap(),
        )
        .unwrap();
        let args = case["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap())
            .collect::<Vec<_>>();
        let output = command(temp.path(), &args, "UTC");
        assert_eq!(
            output.status.code(),
            Some(case["status"].as_i64().unwrap() as i32),
            "{}: {}",
            case["name"],
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            case["stdout"],
            "{}",
            case["name"]
        );
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            case["stderr"],
            "{}",
            case["name"]
        );
        assert_eq!(
            fs::read_to_string(temp.path().join("GROUNDING.yaml")).unwrap(),
            case["after"],
            "{}",
            case["name"]
        );
    }
}

#[cfg(unix)]
#[test]
fn default_dates_follow_the_local_timezone_for_set_review_and_first_add() {
    for zone in [
        chrono_tz::America::Los_Angeles,
        chrono_tz::Pacific::Kiritimati,
        chrono_tz::Etc::GMTPlus12,
    ] {
        for operation in ["set", "review", "first-add"] {
            let temp = tempfile::tempdir().unwrap();
            let record = temp.path().join("GROUNDING.yaml");
            let args = match operation {
                "set" => {
                    fs::write(
                        &record,
                        "meta: {updated: 2026-01-01}\nknown:\n  p.a: {v: 1, of: 2026-01-01}\n",
                    )
                    .unwrap();
                    vec!["set", "p.a", "2", "GROUNDING.yaml"]
                }
                "review" => {
                    fs::write(&record,"meta: {updated: 2026-01-01}\nknown:\n  p.a: {v: 1}\njudgments:\n  d.a:\n    verdict: continue\n    rests_on: [p.a]\n    seen: {p.a: 1}\n    wrong_if: p.a > 9\n    reviewed: 2026-01-01\n").unwrap();
                    vec!["review", "d.a", "GROUNDING.yaml"]
                }
                _ => vec!["add", "p.a", "v=2"],
            };
            let before = chrono::Utc::now()
                .with_timezone(&zone)
                .date_naive()
                .to_string();
            let output = command(temp.path(), &args, zone.name());
            let after = chrono::Utc::now()
                .with_timezone(&zone)
                .date_naive()
                .to_string();
            assert!(
                output.status.success(),
                "{zone}/{operation}: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let value =
                kpop_native::history_yaml::decode_document(&fs::read(&record).unwrap()).unwrap();
            fn date_at(value: &kpop_native::value::TypedValue, path: &[&str]) -> String {
                use kpop_native::value::TypedValue as V;
                let field = path.iter().fold(value, |v, key| match v {
                    V::Map(m) => &m[*key],
                    _ => panic!("missing date container"),
                });
                match field {
                    V::Date(day) => day.as_str().into(),
                    V::Text(day) => day.clone(),
                    _ => panic!("unexpected date {field:?}"),
                }
            }
            let updated = date_at(&value, &["meta", "updated"]);
            assert!(
                updated == before || updated == after,
                "{zone}/{operation}: {value:?}"
            );
            let date = match operation {
                "set" => date_at(&value, &["known", "p.a", "of"]),
                "review" => date_at(&value, &["judgments", "d.a", "reviewed"]),
                _ => updated,
            };
            assert!(
                date == before || date == after,
                "{zone}/{operation}: {date}"
            );
        }
    }
}

#[test]
fn arrangement_replacement_matches_python_for_both_collection_routes() {
    let before = "meta:\n  updated: 2026-09-01\nsources:\n  s.q: {asked: Inspect this record}\nknown:\n  p.a: {v: 1}\njudgments:\n  d.arr:\n    verdict: continue\n    rests_on: [s.q, graph.entries]\n    seen: {s.q: Inspect this record, graph.entries: 1}\n    wrong_if: graph.entries > 0\n";
    let cases: serde_json::Value =
        serde_json::from_slice(include_bytes!("fixtures/legacy-recheck-arrangement.json")).unwrap();
    for (index, collection) in [None, Some("known")].into_iter().enumerate() {
        let temp = tempfile::tempdir().unwrap();
        let record = temp.path().join("GROUNDING.yaml");
        fs::write(&record, before).unwrap();
        let mut args = vec![
            "add",
            "d.arr",
            "verdict=stop",
            "rests_on=[s.q, graph.entries]",
            "wrong_if=graph.entries > 0",
            "because=new evidence",
            "--as-of",
            "2026-09-19",
            "GROUNDING.yaml",
        ];
        if let Some(collection) = collection {
            args.extend(["--in", collection]);
        }
        let output = command(temp.path(), &args, "UTC");
        let expected = &cases[index];
        assert_eq!(
            output.status.code().map(i64::from),
            expected["code"].as_i64()
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            expected["stdout"]
        );
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            expected["stderr"]
        );
        for (path, bytes) in expected["files"].as_object().unwrap() {
            assert_eq!(
                fs::read_to_string(temp.path().join(path)).unwrap(),
                bytes.as_str().unwrap()
            );
        }
        assert!(
            !temp
                .path()
                .join(
                    kpop_native::history_transaction::Layout::for_entry("GROUNDING.yaml")
                        .unwrap()
                        .journal
                )
                .exists()
        );
    }
}
