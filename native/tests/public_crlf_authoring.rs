use std::{fs, path::Path, process::Command};

fn run(root: &Path, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(root)
        .env_remove("KPOPPER_AGENT_SESSION")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("KPOPPER_NATIVE_RESOURCES")
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn check_edit(source: &str, args: &[&str], expected: &str) {
    for newline in ["\n", "\r\n"] {
        for final_newline in [true, false] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("GROUNDING.yaml");
            let encode = |s: &str| {
                let s = if final_newline {
                    s
                } else {
                    s.trim_end_matches('\n')
                };
                s.replace('\n', newline)
            };
            fs::write(&path, encode(source)).unwrap();
            run(temp.path(), &["pull", "p.price"]);
            run(temp.path(), args);
            assert_eq!(
                fs::read_to_string(&path).unwrap(),
                encode(expected),
                "{args:?}, {newline:?}, final={final_newline}"
            );
        }
    }
}

#[test]
fn set_preserves_line_endings_and_source_layout() {
    let source = "# מחיר €\nknown:\n    p.price: {v: 10, from: 'מחירון', at: 'עמוד 2', of: '2026-09-22'} # keep\n    p.other: {v: unchanged} # untouched\n";
    check_edit(
        source,
        &["set", "p.price", "11", "--as-of", "2026-09-23"],
        &source
            .replace("v: 10", "v: 11")
            .replace("2026-09-22", "2026-09-23"),
    );
}

#[test]
fn review_preserves_line_endings_in_flow_and_block_judgments() {
    for judgment in [
        "    d.price: {verdict: 'תקין', rests_on: [p.price], seen: {p.price: 9}, wrong_if: 'p.price > 100'} # keep\n",
        "    d.price:\n        verdict: תקין\n        rests_on: [p.price]\n        seen: {p.price: 9}\n        wrong_if: p.price > 100 # keep\n",
    ] {
        let source = format!(
            "# מחיר €\nknown:\n    p.price: {{v: 10, from: 'מחירון'}}\njudgments:\n{judgment}"
        );
        check_edit(
            &source,
            &["review", "d.price", "--as-of", "2026-09-23"],
            &source.replace("p.price: 9", "p.price: 10"),
        );
    }
}

#[test]
fn add_preserves_line_endings_and_neighbor_bytes() {
    let source =
        "# מחיר €\nknown:\n    p.price: {v: 10, from: 'מחירון'} # keep\n    p.z: {v: unchanged}\n";
    check_edit(
        source,
        &["add", "p.tax", "v=2"],
        &source.replace("    p.z:", "    p.tax: {v: 2}\n    p.z:"),
    );
}

#[test]
fn set_preserves_block_fields_and_generated_line_endings() {
    let source = "# מחיר €\nknown:\n    p.price:\n        v: 10\n        from: מחירון\n        at: עמוד 2\n    p.z: {v: unchanged}\n";
    check_edit(
        source,
        &["set", "p.price", "11", "--as-of", "2026-09-23"],
        &source.replace("v: 10\n", "v: 11\n        of: \"2026-09-23\"\n"),
    );
}

#[test]
fn named_review_preserves_the_hypothesis_line_endings_and_base_bytes() {
    for newline in ["\n", "\r\n"] {
        for final_newline in [true, false] {
            let temp = tempfile::tempdir().unwrap();
            let base = "known:\r\n    p.price: {v: 10, from: 'מחירון'}\r\n";
            fs::write(temp.path().join("GROUNDING.yaml"), base).unwrap();
            let path = temp.path().join(".kpopper/hypotheses/price.yaml");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let source = "hypothesis: {born: '2026-09-22'}\njudgments:\n    d.price: {verdict: 'תקין', rests_on: [p.price], seen: {p.price: 9}, wrong_if: 'p.price > 100'} # keep\n";
            let source = if final_newline {
                source
            } else {
                source.trim_end_matches('\n')
            }
            .replace('\n', newline);
            fs::write(&path, &source).unwrap();
            run(
                temp.path(),
                &[
                    "review",
                    "d.price",
                    "--hypothesis",
                    "price",
                    "--as-of",
                    "2026-09-23",
                ],
            );
            assert_eq!(
                fs::read_to_string(path).unwrap(),
                source.replace("p.price: 9", "p.price: 10")
            );
            assert_eq!(
                fs::read_to_string(temp.path().join("GROUNDING.yaml")).unwrap(),
                base
            );
        }
    }
}

#[test]
fn minimal_set_keeps_the_final_newline_choice() {
    check_edit(
        "known:\n  p.price: {v: 10}\n",
        &["set", "p.price", "11", "--as-of", "2026-09-23"],
        "known:\n  p.price: {v: 11, of: \"2026-09-23\"}\n",
    );
}

#[test]
fn a_new_hypothesis_carries_crlf_source_as_its_own_line_contents() {
    let temp = tempfile::tempdir().unwrap();
    let base = "known:\r\n    p.price: {v: 10, from: 'מחירון'} # keep\r\n";
    fs::write(temp.path().join("GROUNDING.yaml"), base).unwrap();
    run(
        temp.path(),
        &[
            "set",
            "p.price",
            "11",
            "--hypothesis",
            "price",
            "--as-of",
            "2026-09-23",
        ],
    );
    let proposal = fs::read_to_string(temp.path().join(".kpopper/hypotheses/price.yaml")).unwrap();
    assert!(
        proposal.contains("p.price: {v: 11, of: \"2026-09-23\", from: 'מחירון'} # keep"),
        "{proposal}"
    );
    assert!(
        !proposal.contains('\r'),
        "a new hypothesis uses LF consistently: {proposal:?}"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("GROUNDING.yaml")).unwrap(),
        base
    );
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn commit(root: &Path) -> String {
    git(root, &["add", "GROUNDING.yaml"]);
    git(
        root,
        &["-c", "commit.gpgsign=false", "commit", "-qm", "record"],
    );
    git(root, &["rev-parse", "HEAD"]).trim().to_owned()
}

#[test]
fn branch_fold_preserves_destination_endings_and_source_entry_bytes() {
    for source_newline in ["\n", "\r\n"] {
        for destination_newline in ["\n", "\r\n"] {
            for final_newline in [true, false] {
                let temp = tempfile::tempdir().unwrap();
                let root = temp.path();
                git(root, &["init", "-q", "-b", "main"]);
                git(root, &["config", "core.autocrlf", "false"]);
                git(root, &["config", "user.name", "Fixture"]);
                git(root, &["config", "user.email", "fixture@example.invalid"]);
                let old = "    p.price: {v: 10, of: '2026-09-22'} # old\n";
                let updated = "    p.price: {v: 11, of: '2026-09-23', from: 'מחירון', at: 'עמוד 2'} # מחיר €\n";
                let added = "    p.tax: {v: 2} # source comment\n";
                let proposal = format!("known:\n{updated}{added}").replace('\n', source_newline);
                let base =
                    format!("# destination\nknown:\n{old}    p.z: {{v: untouched}} # keep\n");
                let encode = |s: &str| {
                    if final_newline {
                        s
                    } else {
                        s.trim_end_matches('\n')
                    }
                    .replace('\n', destination_newline)
                };
                fs::write(root.join("GROUNDING.yaml"), encode(&base)).unwrap();
                let head = commit(root);
                git(root, &["checkout", "-q", "-b", "source"]);
                fs::write(root.join("GROUNDING.yaml"), &proposal).unwrap();
                let source = commit(root);
                git(root, &["checkout", "-q", "main"]);
                run(root, &["consolidate", "--from", &source]);
                let expected = base.replace(old, &format!("{updated}{added}"));
                assert_eq!(
                    fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
                    encode(&expected),
                    "source={source_newline:?}, destination={destination_newline:?}, final={final_newline}"
                );
                assert_eq!(
                    git(root, &["show", &format!("{source}:GROUNDING.yaml")]),
                    proposal
                );
                assert_eq!(git(root, &["rev-parse", "HEAD"]).trim(), head);
            }
        }
    }
}

#[test]
fn named_fold_preserves_block_text_and_destination_endings() {
    for source_newline in ["\n", "\r\n"] {
        for destination_newline in ["\n", "\r\n"] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            let old = "    p.price: {v: 10, of: '2026-09-22'}\n";
            let updated = "    p.price:\n        v: 11\n        of: '2026-09-23'\n        from: מחירון # מקור €\n        at: |\n            עמוד 2\n            שורה 3\n";
            let base = format!("# keep\nknown:\n{old}    p.z: {{v: untouched}}\n");
            fs::write(
                root.join("GROUNDING.yaml"),
                base.replace('\n', destination_newline),
            )
            .unwrap();
            let path = root.join(".kpopper/hypotheses/price.yaml");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(
                &path,
                format!("known:\n{updated}").replace('\n', source_newline),
            )
            .unwrap();
            run(root, &["consolidate", "price", "--as-of", "2026-09-23"]);
            assert_eq!(
                fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
                base.replace(old, updated)
                    .replace('\n', destination_newline)
            );
            assert!(!path.exists());
        }
    }
}

#[test]
fn refuting_a_hypothesis_keeps_the_destination_line_endings() {
    let mut lf_result: Option<String> = None;
    for newline in ["\n", "\r\n"] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let base = "# מחיר €\nknown:\n    p.price: {v: 10, from: 'מחירון'} # keep\nsources:\n    s.fixture: {asked: check the price, name: Fixture}\n";
        fs::write(root.join("GROUNDING.yaml"), base.replace('\n', newline)).unwrap();
        let path = root.join(".kpopper/hypotheses/price.yaml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "known:\r\n    p.tax: {v: 2}\r\n").unwrap();
        run(
            root,
            &[
                "consolidate",
                "--refute",
                "price",
                "superseded by the price list",
                "--as-of",
                "2026-09-23",
            ],
        );
        let result = fs::read_to_string(root.join("GROUNDING.yaml")).unwrap();
        assert!(result.starts_with("# מחיר €"));
        for line in base.lines().filter(|line| line.starts_with("    ")) {
            assert!(result.contains(&format!("{line}{newline}")));
        }
        assert!(run(root, &["pull", "hyp.price"]).contains("refuted"));
        assert!(!path.exists());
        if let Some(lf_result) = &lf_result {
            assert_eq!(result, lf_result.replace('\n', "\r\n"));
        } else {
            lf_result = Some(result);
        }
    }
}

#[test]
fn fold_preserves_each_shards_endings_and_untouched_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let index = "# index\r\nrecord: [prices.yaml, taxes.yaml, untouched.yaml]\r\n";
    let prices = "# מחיר €\r\nknown:\r\n    p.price: {v: 10, of: '2026-09-22'} # price\r\n";
    let taxes = "# tax\nknown:\n    p.tax: {v: 1, of: '2026-09-22'} # tax\n";
    let untouched = "known:\r\n    p.other: {v: unchanged} # keep\r\n";
    for (name, source) in [
        ("GROUNDING.yaml", index),
        ("prices.yaml", prices),
        ("taxes.yaml", taxes),
        ("untouched.yaml", untouched),
    ] {
        fs::write(root.join(name), source).unwrap();
    }
    let path = root.join(".kpopper/hypotheses/price.yaml");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "known:\r\n    p.price: {v: 11, of: '2026-09-23'} # price\r\n    p.tax: {v: 2, of: '2026-09-23'} # tax\r\n").unwrap();
    run(root, &["consolidate", "price", "--as-of", "2026-09-23"]);
    assert_eq!(
        fs::read_to_string(root.join("prices.yaml")).unwrap(),
        prices
            .replace("v: 10", "v: 11")
            .replace("2026-09-22", "2026-09-23")
    );
    assert_eq!(
        fs::read_to_string(root.join("taxes.yaml")).unwrap(),
        taxes
            .replace("v: 1", "v: 2")
            .replace("2026-09-22", "2026-09-23")
    );
    assert_eq!(
        fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
        index
    );
    assert_eq!(
        fs::read_to_string(root.join("untouched.yaml")).unwrap(),
        untouched
    );
    assert!(!path.exists());
}
