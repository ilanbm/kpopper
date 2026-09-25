use kpop_native::public_consolidation::{self, Options};
use serde_json::Value as J;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn git(root: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().into()
}
fn commit(root: &Path, message: &str) -> String {
    git(root, &["add", "."]);
    git(
        root,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=f@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            message,
        ],
    );
    git(root, &["rev-parse", "HEAD"])
}
fn image(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, at: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for item in fs::read_dir(at).unwrap() {
            let path = item.unwrap().path();
            if path.file_name().and_then(|v| v.to_str()) == Some(".git") {
                continue;
            }
            if path.is_dir() {
                visit(root, &path, out);
            } else {
                out.insert(
                    path.strip_prefix(root).unwrap().into(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out);
    out
}
fn canonicalize_commit_objects(mut body: String) -> String {
    let Some(start) = body.find("objects:\n") else {
        return body;
    };
    let rows = start + "objects:\n".len();
    let Some(relative_end) = body[rows..].find("\noperation:") else {
        return body;
    };
    let end = rows + relative_end + 1;
    let mut blocks = body[rows..end]
        .split("- id: ")
        .skip(1)
        .map(|value| format!("- id: {value}"))
        .collect::<Vec<_>>();
    blocks.sort();
    body.replace_range(rows..end, &blocks.concat());
    body
}

#[test]
fn generated_object_row_normalization_is_order_independent() {
    let a = "objects:\n- id: $OBJECT-2\n  subject: p.b\n- id: kept\n  subject: p.a\n- id: $OBJECT-1\n  subject: p.c\noperation: $OP\n";
    let b = "objects:\n- id: $OBJECT-1\n  subject: p.c\n- id: $OBJECT-2\n  subject: p.b\n- id: kept\n  subject: p.a\noperation: $OP\n";
    assert_eq!(
        canonicalize_commit_objects(a.into()),
        canonicalize_commit_objects(b.into())
    );
}

#[test]
fn ordinary_branch_preview_ignores_values_inherited_from_the_merge_base() {
    let (_temp, root) = branched(&[]);
    let base = git(&root, &["rev-parse", "HEAD"]);
    git(&root, &["checkout", "-q", "-b", "source"]);
    git(&root, &["checkout", "-q", "main"]);
    fs::write(
        root.join("GROUNDING.yaml"),
        BRANCHED.replace("local.one: {v: 1}", "local.one: {v: 3}"),
    )
    .unwrap();
    commit(&root, "current changed independently");
    let before_head = git(&root, &["rev-parse", "HEAD"]);
    let options = Options {
        from_refs: vec![base],
        dry_run: true,
        ..Default::default()
    };
    let preview = public_consolidation::dispatch(&options, &root);
    assert_eq!(preview.code, 0, "{}", preview.stderr);
    assert!(preview.stdout.contains("arrived (0)"), "{}", preview.stdout);
    assert!(preview.stdout.contains("updates (0)"), "{}", preview.stdout);
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), before_head);
    assert!(
        fs::read_to_string(root.join("GROUNDING.yaml"))
            .unwrap()
            .contains("v: 3")
    );
}

#[test]
fn ordinary_branch_preview_refuses_missing_merge_base() {
    let (_temp, root) = branched(&[]);
    git(&root, &["checkout", "--orphan", "unrelated"]);
    fs::write(root.join("GROUNDING.yaml"), "known:\n  p.other: {v: 1}\n").unwrap();
    commit(&root, "unrelated root");
    git(&root, &["checkout", "-q", "main"]);
    let output = public_consolidation::dispatch(
        &Options {
            from_refs: vec!["unrelated".into()],
            dry_run: true,
            ..Default::default()
        },
        &root,
    );
    assert_eq!(output.code, 1);
    assert!(
        output.stderr.contains("no common Git merge base"),
        "{}",
        output.stderr
    );
}

#[test]
fn ordinary_branch_carries_a_fresh_review_and_marks_newer_target_inputs_moved() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    let base = "meta: {purpose: synthetic}\nschema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nsources:\n  s.request: {name: request, read: 2026-09-15}\nknown:\n  wave.state: {v: complete, of: 2026-09-15}\n  brief.job_state: {v: failing, of: 2026-09-15}\n  brief.rendered_runs: {v: 0, of: 2026-09-15}\njudgments:\n  d.judgment:\n    request: s.request\n    rests_on: [wave.state, brief.job_state, brief.rendered_runs, s.request]\n    verdict: not demonstrated\n    wrong_if: brief.rendered_runs > 0\n    seen: {wave.state: complete, brief.job_state: failing, brief.rendered_runs: 0, s.request: read 2026-09-15}\n";
    fs::write(root.join("GROUNDING.yaml"), base).unwrap();
    commit(&root, "common base");
    git(&root, &["checkout", "-q", "-b", "reviewed"]);
    let branch = base
        .replace(
            "brief.job_state: {v: failing, of: 2026-09-15}",
            "brief.job_state: {v: 'still failing', of: 2026-09-16}",
        )
        .replace(
            "brief.job_state: failing, brief.rendered_runs",
            "brief.job_state: 'still failing', brief.rendered_runs",
        );
    fs::write(root.join("GROUNDING.yaml"), branch).unwrap();
    commit(&root, "reviewed on source branch");
    git(&root, &["checkout", "-q", "main"]);

    let fresh = public_consolidation::dispatch(
        &Options {
            from_refs: vec!["reviewed".into()],
            dry_run: true,
            ..Default::default()
        },
        &root,
    );
    assert_eq!(fresh.code, 0, "{}{}", fresh.stdout, fresh.stderr);
    assert!(
        !fresh.stdout.contains("MOVED     d.judgment"),
        "{}",
        fresh.stdout
    );

    let newer = base.replace(
        "brief.job_state: {v: failing, of: 2026-09-15}",
        "brief.job_state: {v: 'still failing', of: 2026-09-17}",
    );
    fs::write(root.join("GROUNDING.yaml"), newer).unwrap();
    commit(&root, "destination reread after source review");
    let stale = public_consolidation::dispatch(
        &Options {
            from_refs: vec!["reviewed".into()],
            dry_run: true,
            ..Default::default()
        },
        &root,
    );
    assert_eq!(stale.code, 0, "{}{}", stale.stdout, stale.stderr);
    assert!(
        stale.stdout.contains("MOVED d.judgment"),
        "{}",
        stale.stdout
    );

    let changed_decision = fs::read_to_string(root.join("GROUNDING.yaml"))
        .unwrap()
        .replace("verdict: not demonstrated", "verdict: demonstrated");
    fs::write(root.join("GROUNDING.yaml"), changed_decision).unwrap();
    commit(&root, "destination makes a new judgment");
    let cannot_take_old_review = public_consolidation::dispatch(
        &Options {
            from_refs: vec!["reviewed".into()],
            take: vec!["d.judgment".into()],
            dry_run: true,
            ..Default::default()
        },
        &root,
    );
    assert_eq!(cannot_take_old_review.code, 1);
    assert!(
        cannot_take_old_review
            .stdout
            .contains("that review cannot take the older verdict"),
        "{}",
        cannot_take_old_review.stdout
    );
}

const BRANCHED: &str = "known:\n  local.one: {v: 1}\n  local.two: {v: 2, of: 2026-09-10}\n  local.three: {v: two  words}\njudgments:\n  d.a:\n    verdict: a\n    rests_on: [local.one]\n    seen: {local.one: 1}\n    wrong_if: local.one > 5\n";
/// A repository whose `main` holds BRANCHED, with one branch per record given.
fn branched(branches: &[(&str, String)]) -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    // Checking a branch out keeps the record's bytes as written, on every platform: a record
    // turned into CRLF lines on checkout is one neither reader folds into.
    git(&root, &["config", "core.autocrlf", "false"]);
    fs::write(root.join("GROUNDING.yaml"), BRANCHED).unwrap();
    commit(&root, "base");
    for (name, record) in branches {
        git(&root, &["checkout", "-q", "-b", name, "main"]);
        fs::write(root.join("GROUNDING.yaml"), record).unwrap();
        commit(&root, name);
        git(&root, &["checkout", "-q", "main"]);
    }
    (temp, root)
}

#[test]
fn a_branch_is_tested_as_what_it_holds_differently_from_this_record() {
    let (_temp, root) = branched(&[
        (
            "listed",
            BRANCHED.replace("{v: 1}", "{v: 1, tags: [local.two]}"),
        ),
        (
            "twice",
            format!(
                "{BRANCHED}  local.two: {{verdict: dup, rests_on: [local.one], seen: {{local.one: 1}}}}\n"
            ),
        ),
        ("schema", format!("schema: 5\n{BRANCHED}")),
        (
            "rewritten",
            BRANCHED
                .replace("{v: 1}", "{v: 1.0}")
                .replace("two  words", "two words"),
        ),
        (
            "reviewed",
            BRANCHED.replace(
                "    seen: {local.one: 1}\n",
                "    seen: {local.one: 1}\n    reviewed: 2026-09-12\n",
            ),
        ),
        (
            "grounds",
            BRANCHED.replace("rests_on: [local.one]", "rests_on: [local.one, local.two]"),
        ),
        (
            "newer",
            BRANCHED.replace("of: 2026-09-10", "of: 2026-09-12"),
        ),
    ]);
    let before = image(&root);
    let reversed = "reversed (0): a verdict, or other grounds, laid over a standing judgment - by its own condition, by a person's name, or waiting for one\n";
    let rest = "candidates (0): pairs for a person to judge as the same subject or distinct\nnew subjects (0): prefixes the base does not hold\n\n";
    let nothing = |name: &str| {
        format!(
            "arrived (0): what the fold would add\nupdates (0): what the base holds that a hypothesis replaces, and what rests on each\n{reversed}moved / falsified (0): what the union moves or breaks\ncontested (0)\n{rest}clean - and nothing to write: the base already holds everything {name} proposes; consolidate {name} removes the file\n"
        )
    };
    let age = regex::Regex::new(r"\(born [0-9-]+, [^)]*\)").unwrap();
    for (name, body, code) in [
        // What this record already holds is not tested again: a list added to an
        // unchanged claim casts no vote for a field role, the branch's own schema stays
        // the branch's, a claim written another way is the same claim, and the same
        // decision with its review refreshed is the branch's own review.
        ("listed", nothing("listed"), 0),
        ("schema", nothing("schema"), 0),
        ("rewritten", nothing("rewritten"), 0),
        ("reviewed", nothing("reviewed"), 0),
        // An id the branch holds a second time is tested alone, as a judgment laid
        // over an entry, never as the branch contesting itself.
        (
            "twice",
            format!(
                "arrived (0): what the fold would add\nupdates (1): what the base holds that a hypothesis replaces, and what rests on each\n  local.two: 2 -> dup, from twice\n    the base holds an entry under this id and the hypothesis a judgment - a subject does not change kind at the fold: set the entry, or give the judgment a new id - the base keeps what it holds\n{reversed}moved / falsified (1): what the union moves or breaks\n  FAIL local.two: no predicate at all - and nothing says why not, so it can never be re-checked\ncontested (1): the door refuses the reading, so the base keeps what it holds\n  local.two: the base holds an entry under this id and the hypothesis a judgment - a subject does not change kind at the fold: set the entry, or give the judgment a new id\n    the base holds local.two: 2 as of 2026-09-10\n    twice says + local.two: dup\n  read again on a later day - set it in the base or in the hypothesis with --as-of - or refute the hypothesis\n{rest}not clean: a hole, a contested reading - nothing folds until it is read again\n"
            ),
            1,
        ),
        // What differs still does: a verdict on other grounds, and a later reading.
        (
            "grounds",
            format!(
                "arrived (0): what the fold would add\nupdates (0): what the base holds that a hypothesis replaces, and what rests on each\nreversed (1): a verdict, or other grounds, laid over a standing judgment - by its own condition, by a person's name, or waiting for one\n  d.a: the same verdict on other grounds, from grounds\n    the standing judgment holds, and its wrong_if has not fired - take it by name: consolidate grounds --take d.a\n    base: rests_on: [local.one]\n    base: wrong_if: local.one > 5\n    grounds: rests_on: [local.one, local.two]\n    grounds: wrong_if: local.one > 5\nmoved / falsified (1): what the union moves or breaks\n  FAIL d.a: no snapshot for local.two - never checked against it\ncontested (0)\n{rest}not clean: a hole, 1 reversal to take by name - nothing folds until it is read again\n  consolidate grounds --take d.a\n"
            ),
            1,
        ),
        (
            "newer",
            format!(
                "arrived (0): what the fold would add\nupdates (1): what the base holds that a hypothesis replaces, and what rests on each\n  local.two: 2 -> 2, from newer\n    a reading from 2026-09-12 that is newer than the base's\n{reversed}moved / falsified (0): what the union moves or breaks\ncontested (0)\n{rest}clean: newer may fold - consolidate newer\n"
            ),
            0,
        ),
    ] {
        let output = public_consolidation::dispatch(
            &Options {
                from_refs: vec![name.into()],
                dry_run: true,
                ..Default::default()
            },
            &root,
        );
        let commit = git(&root, &["rev-parse", name]);
        assert_eq!(
            (
                output.code,
                output.stderr.as_str(),
                age.replace(&output.stdout, "(born DAY)").as_ref()
            ),
            (
                code,
                "",
                format!(
                    "the base with {name} laid over it\n  {name} (born DAY): what {name} committed ({}), read as a hypothesis\n\n{body}",
                    &commit[..7]
                )
                .as_str()
            ),
            "{name}"
        );
    }
    assert_eq!(image(&root), before);
}

#[test]
fn a_branch_fold_carries_only_what_the_branch_holds_differently() {
    let (_temp, root) = branched(&[(
        "arrives",
        BRANCHED.replace(
            "  local.one: {v: 1}\n",
            "  local.one: {v: 1, tags: [local.two]}\n  local.four: {v: 4, of: 2026-09-12}\n",
        ),
    )]);
    let output = public_consolidation::dispatch(
        &Options {
            from_refs: vec!["arrives".into()],
            as_of: Some("2026-09-20".into()),
            ..Default::default()
        },
        &root,
    );
    assert_eq!(output.code, 0, "{}{}", output.stdout, output.stderr);
    assert!(
        output
            .stdout
            .contains("folded arrives: 1 entry and 0 judgments - 1 added, 0 replaced\n"),
        "{}",
        output.stdout
    );
    // The new entry arrives; the list the branch added to an unchanged claim stays there.
    assert_eq!(
        fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
        BRANCHED.replace("known:\n", "known:\n  local.four: {v: 4, of: 2026-09-12}\n")
    );
}

#[test]
fn a_branch_that_still_holds_an_id_twice_is_refused_before_anything_folds() {
    // Both of the branch's bodies for local.five differ from this record, so neither is
    // left out, and no one body is the branch's.
    let twice = "known:\n  local.five: {v: 5, of: 2026-09-12}\njudgments:\n  local.five: {verdict: five, rests_on: [local.one], seen: {local.one: 1}, wrong_if: local.one > 5}\n";
    let (_temp, root) = branched(&[(
        "held_twice",
        BRANCHED.replace("known:\n", "known:\n  local.five: {v: 5, of: 2026-09-12}\n")
            + "  local.five: {verdict: five, rests_on: [local.one], seen: {local.one: 1}, wrong_if: local.one > 5}\n",
    )]);
    // The same in a hypothesis file the branch carries, beside an unchanged record.
    git(&root, &["checkout", "-q", "-b", "idea_twice", "main"]);
    fs::create_dir_all(root.join(".kpopper/hypotheses")).unwrap();
    fs::write(root.join(".kpopper/hypotheses/idea.yaml"), twice).unwrap();
    commit(&root, "idea_twice");
    git(&root, &["checkout", "-q", "main"]);
    let before = image(&root);
    for reference in ["held_twice", "idea_twice"] {
        for dry_run in [true, false] {
            let output = public_consolidation::dispatch(
                &Options {
                    from_refs: vec![reference.into()],
                    dry_run,
                    as_of: Some("2026-09-20".into()),
                    ..Default::default()
                },
                &root,
            );
            assert_eq!(
                (output.code, output.stdout.as_str(), output.stderr.as_str()),
                (1, "", "duplicate_entry\n"),
                "{reference}, dry run: {dry_run}"
            );
        }
    }
    assert_eq!(image(&root), before);
}

#[test]
fn a_branch_fold_reads_permissions_from_the_branch_s_whole_record() {
    // The branch marks an entry private without changing its claim, so the entry is not
    // laid again; the judgment it adds rests on that entry.
    let (_temp, root) = branched(&[(
        "private",
        BRANCHED.replace("{v: 1}", "{v: 1, privacy: private}")
            + "  d.new: {verdict: new, rests_on: [local.one], seen: {local.one: 1}, wrong_if: local.one > 9}\n",
    )]);
    let before = image(&root);
    let private_home = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(&root)
        .args(["consolidate", "--from", "private", "--as-of", "2026-09-20"])
        .env("KPOPPER_PRIVATE_HOME", private_home.path())
        .env("KPOPPER_SESSION_DISABLE", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("private draft retained")
            && stderr.ends_with("; original hypothesis retained\n"),
        "{stderr}"
    );
    assert_eq!(image(&root), before);
    let drafts = image(private_home.path())
        .into_iter()
        .filter(|(path, _)| path.extension().is_some_and(|v| v == "json"))
        .map(|(_, body)| String::from_utf8(body).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(drafts.len(), 1);
    assert!(
        drafts[0].contains("d.new") && drafts[0].contains("private"),
        "{}",
        drafts[0]
    );
}

#[test]
fn a_branch_record_that_declares_the_core_profile_is_not_laid_over_an_ordinary_one() {
    let (_temp, root) = branched(&[(
        "core",
        format!(
            "meta: {{reasoning: {{version: 1, profile: core/v1, requires: [arithmetic/v1]}}}}\n{BRANCHED}"
        ),
    )]);
    let before = image(&root);
    for dry_run in [true, false] {
        let output = public_consolidation::dispatch(
            &Options {
                from_refs: vec!["core".into()],
                dry_run,
                as_of: Some("2026-09-20".into()),
                ..Default::default()
            },
            &root,
        );
        assert_eq!(
            (output.code, output.stdout.as_str(), output.stderr.as_str()),
            (1, "", "unsupported_capability: use core/v1 consumer\n")
        );
    }
    assert_eq!(image(&root), before);
}

#[test]
fn ordinary_branch_fold_refuses_nonfinite_source_without_changing_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    fs::write(
        root.join("GROUNDING.yaml"),
        "known:\n  p.value: {v: .nan}\n",
    )
    .unwrap();
    let source = commit(&root, "nonfinite source");
    fs::write(root.join("GROUNDING.yaml"), "known:\n  p.value: {v: 3}\n").unwrap();
    commit(&root, "finite current");
    let before = image(&root);
    let output = public_consolidation::dispatch(
        &Options {
            from_refs: vec![source],
            ..Default::default()
        },
        &root,
    );
    assert_eq!(output.code, 1);
    assert_eq!(output.stdout, "");
    assert_eq!(
        output.stderr,
        "invalid_history_value: snapshot data contains a nonfinite value\n"
    );
    assert_eq!(image(&root), before);
}

#[test]
fn ordinary_branch_fold_requires_a_committed_record_and_sidecar() {
    for dirty_sidecar in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        fs::write(
            root.join("GROUNDING.yaml"),
            "known:\n  p.value: {v: 1, of: 2026-09-19}\n",
        )
        .unwrap();
        fs::create_dir_all(root.join(".kpopper")).unwrap();
        fs::write(root.join(".kpopper/replaced.yaml"), "{}\n").unwrap();
        commit(&root, "common base");
        git(&root, &["checkout", "-q", "-b", "source"]);
        fs::write(
            root.join("GROUNDING.yaml"),
            "known:\n  p.value: {v: 2, of: 2026-09-20}\n",
        )
        .unwrap();
        let source = commit(&root, "source change");
        git(&root, &["checkout", "-q", "main"]);
        fs::write(
            root.join("GROUNDING.yaml"),
            "known:\n  p.value: {v: 3, of: 2026-09-18}\n",
        )
        .unwrap();
        commit(&root, "current");
        let path = if dirty_sidecar {
            root.join(".kpopper/replaced.yaml")
        } else {
            root.join("GROUNDING.yaml")
        };
        fs::write(
            &path,
            if dirty_sidecar {
                "changed: true\n"
            } else {
                "known:\n  p.value: {v: 42, note: uncommitted}\n"
            },
        )
        .unwrap();
        let before = image(&root);
        let output = public_consolidation::dispatch(
            &Options {
                from_refs: vec![source],
                as_of: Some("2026-09-20".into()),
                ..Default::default()
            },
            &root,
        );
        assert_eq!(output.code, 1);
        assert!(
            output.stderr.contains("carries uncommitted changes"),
            "{}",
            output.stderr
        );
        assert_eq!(image(&root), before);
    }
}

#[test]
fn ordinary_branch_fold_accepts_ignored_replaced_sidecar() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    fs::write(root.join(".gitignore"), ".kpopper/\n").unwrap();
    fs::write(
        root.join("GROUNDING.yaml"),
        "known:\n  p.value: {v: 1, of: 2026-09-19}\n",
    )
    .unwrap();
    commit(&root, "common base");
    git(&root, &["checkout", "-q", "-b", "source"]);
    fs::write(
        root.join("GROUNDING.yaml"),
        "known:\n  p.value: {v: 2, of: 2026-09-20}\n",
    )
    .unwrap();
    let source = commit(&root, "source change");
    git(&root, &["checkout", "-q", "main"]);
    fs::write(
        root.join("GROUNDING.yaml"),
        "known:\n  p.value: {v: 3, of: 2026-09-18}\n",
    )
    .unwrap();
    commit(&root, "current");
    fs::create_dir_all(root.join(".kpopper")).unwrap();
    fs::write(root.join(".kpopper/replaced.yaml"), "{}\n").unwrap();
    let result = public_consolidation::dispatch(
        &Options {
            from_refs: vec![source],
            as_of: Some("2026-09-20".into()),
            ..Default::default()
        },
        &root,
    );
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    assert!(
        fs::read_to_string(root.join("GROUNDING.yaml"))
            .unwrap()
            .contains("v: 2")
    );
}

#[test]
fn ordinary_branch_fold_is_idempotent_and_accepts_multiple_refs() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    fs::write(
        root.join("GROUNDING.yaml"),
        "known:\n  p.base: {v: 0, of: 2026-09-17}\n",
    )
    .unwrap();
    commit(&root, "common base");
    git(&root, &["checkout", "-q", "-b", "one"]);
    fs::write(
        root.join("GROUNDING.yaml"),
        "known:\n  p.base: {v: 0, of: 2026-09-17}\n  p.a: {v: 1, of: 2026-09-19}\n",
    )
    .unwrap();
    let one = commit(&root, "one");
    git(&root, &["checkout", "-q", "main"]);
    git(&root, &["checkout", "-q", "-b", "two"]);
    fs::write(
        root.join("GROUNDING.yaml"),
        "known:\n  p.base: {v: 0, of: 2026-09-17}\n  p.b: {v: 2, of: 2026-09-19}\n",
    )
    .unwrap();
    let two = commit(&root, "two");
    git(&root, &["checkout", "-q", "main"]);
    fs::write(
        root.join("GROUNDING.yaml"),
        "known:\n  p.base: {v: 0, of: 2026-09-17}\n",
    )
    .unwrap();
    fs::write(root.join("current.txt"), "current destination\n").unwrap();
    commit(&root, "current");
    let options = Options {
        from_refs: vec![one.clone(), two.clone()],
        as_of: Some("2026-09-20".into()),
        ..Default::default()
    };
    let output = public_consolidation::dispatch(&options, &root);
    assert_eq!(output.code, 0, "{}{}", output.stdout, output.stderr);
    let body = fs::read_to_string(root.join("GROUNDING.yaml")).unwrap();
    assert!(body.contains("p.a:"));
    assert!(body.contains("p.b:"));
    commit(&root, "folded");
    let before = image(&root);
    let again = public_consolidation::dispatch(&options, &root);
    assert_eq!(again.code, 0, "{}{}", again.stdout, again.stderr);
    assert!(again.stdout.contains("nothing to write"));
    assert_eq!(image(&root), before);
}

#[test]
fn ordinary_branch_fold_keeps_local_hypotheses_in_the_selected_pool() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    fs::write(
        root.join("GROUNDING.yaml"),
        "known:\n  p.base: {v: 1, of: 2026-09-18}\n",
    )
    .unwrap();
    commit(&root, "common base");
    git(&root, &["checkout", "-q", "-b", "source"]);
    fs::write(
        root.join("GROUNDING.yaml"),
        "known:\n  p.branch: {v: 2, of: 2026-09-19}\n  p.base: {v: 1, of: 2026-09-18}\n",
    )
    .unwrap();
    let source = commit(&root, "source change");
    git(&root, &["checkout", "-q", "main"]);
    fs::write(
        root.join("GROUNDING.yaml"),
        "known:\n  p.base: {v: 1, of: 2026-09-18}\n",
    )
    .unwrap();
    fs::create_dir_all(root.join(".kpopper/hypotheses")).unwrap();
    let local = root.join(".kpopper/hypotheses/local.yaml");
    fs::write(
        &local,
        "hypothesis: {claim: local}\nknown:\n  p.local: {v: 7, of: 2026-09-19}\n",
    )
    .unwrap();
    commit(&root, "current");
    let output = public_consolidation::dispatch(
        &Options {
            from_refs: vec![source.clone()],
            as_of: Some("2026-09-20".into()),
            ..Default::default()
        },
        &root,
    );
    assert_eq!(output.code, 0, "{}{}", output.stdout, output.stderr);
    assert!(
        output.stdout.contains(&format!("folded {source}, local")),
        "{}",
        output.stdout
    );
    let body = fs::read_to_string(root.join("GROUNDING.yaml")).unwrap();
    assert!(body.contains("p.branch:"));
    assert!(body.contains("p.local:"));
    assert!(!local.exists());
}

#[test]
fn explicitly_selected_private_branch_hypothesis_retains_source_metadata_and_refuses() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    fs::write(
        root.join("GROUNDING.yaml"),
        "meta: {privacy: private}\nknown: {p.base: {v: 1}}\n",
    )
    .unwrap();
    fs::create_dir_all(root.join(".kpopper/hypotheses")).unwrap();
    fs::write(
        root.join(".kpopper/hypotheses/scenario.yaml"),
        "hypothesis: {claim: private branch choice}\nknown: {p.secret: {v: 7}}\n",
    )
    .unwrap();
    let source = commit(&root, "private source");
    fs::write(root.join("GROUNDING.yaml"), "known: {p.base: {v: 1}}\n").unwrap();
    fs::remove_dir_all(root.join(".kpopper")).unwrap();
    commit(&root, "public current");
    let before = image(&root);
    let private_temp = tempfile::tempdir().unwrap();
    let private = private_temp.path().to_path_buf();
    let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(&root)
        .args([
            "consolidate",
            &format!("{source}:scenario"),
            "--from",
            &source,
            "--as-of",
            "2026-09-20",
        ])
        .env("KPOPPER_PRIVATE_HOME", &private)
        .env("KPOPPER_SESSION_DISABLE", "1")
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("private draft retained"),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(image(&root), before);
    let drafts = image(&private);
    let bodies = drafts
        .iter()
        .filter(|(path, _)| path.extension().is_some_and(|v| v == "json"))
        .map(|(_, body)| body)
        .collect::<Vec<_>>();
    assert_eq!(bodies.len(), 1);
    let body = String::from_utf8_lossy(bodies[0]);
    assert!(body.contains("source_record_metadata"), "{body}");
    assert!(body.contains("privacy"), "{body}");
    assert!(body.contains("private"), "{body}");
    if let (Some(python), Some(oracle)) = (
        std::env::var_os("KPOP_SESSION_ORACLE_PYTHON"),
        std::env::var_os("KPOP_SESSION_ORACLE_ROOT"),
    ) {
        let python_private = tempfile::tempdir().unwrap();
        let expected = Command::new(python)
            .arg(PathBuf::from(oracle).join("scripts/cli.py"))
            .current_dir(&root)
            .args([
                "consolidate",
                &format!("{source}:scenario"),
                "--from",
                &source,
                "--as-of",
                "2026-09-20",
            ])
            .env("KPOPPER_PRIVATE_HOME", python_private.path())
            .env("KPOPPER_SESSION_DISABLE", "1")
            .output()
            .unwrap();
        assert_eq!(expected.status.code(), output.status.code());
        assert_eq!(expected.stdout, output.stdout);
        let python_files = image(python_private.path());
        let python_body = python_files
            .values()
            .find(|raw| String::from_utf8_lossy(raw).contains("source_record_metadata"))
            .unwrap();
        let normalize = |raw: &[u8]| {
            regex::Regex::new(r"(?:draft-)?[0-9a-f]{32,64}")
                .unwrap()
                .replace_all(&String::from_utf8_lossy(raw), "$ID")
                .into_owned()
        };
        assert_eq!(normalize(bodies[0]), normalize(python_body));
    }
}

#[test]
#[ignore = "requires immutable Python 1.8 oracle"]
fn ordinary_public_cli_matches_python_complete_output_and_files() {
    let source = tempfile::tempdir().unwrap();
    let repository = source.path().join("source");
    fs::create_dir(&repository).unwrap();
    git(&repository, &["init", "-q", "-b", "main"]);
    fs::write(
        repository.join("GROUNDING.yaml"),
        "known:\n  p.value:\n    v: 1\n    of: 2026-09-19\n",
    )
    .unwrap();
    let oid = commit(&repository, "source");
    fs::write(
        repository.join("GROUNDING.yaml"),
        "known:\n  p.value:\n    v: 1\n    of: 2026-09-19\n  p.extra:\n    v: 2\n    of: 2026-09-19\n",
    )
    .unwrap();
    let second = commit(&repository, "second source");
    fs::write(
        repository.join("GROUNDING.yaml"),
        "known:\n  p.value:\n    v: 3\n    of: 2026-09-18\n",
    )
    .unwrap();
    fs::create_dir_all(repository.join(".kpopper")).unwrap();
    fs::write(repository.join(".kpopper/replaced.yaml"), "{}\n").unwrap();
    commit(&repository, "current");
    let native = source.path().join("native");
    let python = source.path().join("python");
    for target in [&native, &python] {
        let out = Command::new("git")
            .args(["clone", "-q", "--no-hardlinks"])
            .arg(&repository)
            .arg(target)
            .output()
            .unwrap();
        assert!(out.status.success());
    }
    let oracle_python = std::env::var_os("KPOP_SESSION_ORACLE_PYTHON").expect("oracle Python");
    let oracle_root =
        PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_ROOT").expect("oracle root"));
    let args = [
        "consolidate",
        "--from",
        oid.as_str(),
        "--from",
        second.as_str(),
        "--as-of",
        "2026-09-20",
    ];
    for relative in ["GROUNDING.yaml", ".kpopper/replaced.yaml"] {
        let np = native.join(relative);
        let pp = python.join(relative);
        let nb = fs::read(&np).unwrap();
        let pb = fs::read(&pp).unwrap();
        let dirty = if relative == "GROUNDING.yaml" {
            b"known: {p.value: {v: 42}}\n".as_slice()
        } else {
            b"changed: true\n".as_slice()
        };
        fs::write(&np, dirty).unwrap();
        fs::write(&pp, dirty).unwrap();
        let ni = image(&native);
        let pi = image(&python);
        let actual = Command::new(env!("CARGO_BIN_EXE_kpop"))
            .current_dir(&native)
            .args(args)
            .env("KPOPPER_SESSION_DISABLE", "1")
            .env("XDG_STATE_HOME", source.path().join("native-state"))
            .output()
            .unwrap();
        let expected = Command::new(&oracle_python)
            .arg(oracle_root.join("scripts/cli.py"))
            .current_dir(&python)
            .args(args)
            .env("KPOPPER_SESSION_DISABLE", "1")
            .env("XDG_STATE_HOME", source.path().join("python-state"))
            .output()
            .unwrap();
        assert_eq!(
            (actual.status.code(), actual.stdout, actual.stderr),
            (expected.status.code(), expected.stdout, expected.stderr),
            "dirty {relative}"
        );
        assert_eq!(image(&native), ni);
        assert_eq!(image(&python), pi);
        fs::write(np, nb).unwrap();
        fs::write(pp, pb).unwrap();
    }
    let actual = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(&native)
        .args(args)
        .env("KPOPPER_SESSION_DISABLE", "1")
        .env("XDG_STATE_HOME", source.path().join("native-state"))
        .output()
        .unwrap();
    let expected = Command::new(&oracle_python)
        .arg(oracle_root.join("scripts/cli.py"))
        .current_dir(&python)
        .args(args)
        .env("KPOPPER_SESSION_DISABLE", "1")
        .env("XDG_STATE_HOME", source.path().join("python-state"))
        .output()
        .unwrap();
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(actual.stdout, expected.stdout, "stdout");
    assert_eq!(actual.stderr, expected.stderr, "stderr");
    assert_eq!(image(&native), image(&python), "complete after image");
    for root in [&native, &python] {
        let committed = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["add", "-A"])
            .status()
            .unwrap();
        assert!(committed.success());
        let committed = Command::new("git")
            .arg("-C")
            .arg(root)
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=f@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "-m",
                "folded",
            ])
            .env("GIT_AUTHOR_DATE", "2026-09-20T12:00:00Z")
            .env("GIT_COMMITTER_DATE", "2026-09-20T12:00:00Z")
            .status()
            .unwrap();
        assert!(committed.success());
    }
    let again = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(&native)
        .args(args)
        .env("KPOPPER_SESSION_DISABLE", "1")
        .env("XDG_STATE_HOME", source.path().join("native-state"))
        .output()
        .unwrap();
    let expected_again = Command::new(&oracle_python)
        .arg(oracle_root.join("scripts/cli.py"))
        .current_dir(&python)
        .args(args)
        .env("KPOPPER_SESSION_DISABLE", "1")
        .env("XDG_STATE_HOME", source.path().join("python-state"))
        .output()
        .unwrap();
    assert_eq!(
        (again.status.code(), again.stdout, again.stderr),
        (
            expected_again.status.code(),
            expected_again.stdout,
            expected_again.stderr
        )
    );
    assert_eq!(image(&native), image(&python));
}

#[test]
#[ignore = "requires immutable Python 1.8 oracle"]
fn history_public_cli_preview_and_adoption_match_complete_outputs_and_images() {
    let oracle_python =
        PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_PYTHON").expect("oracle Python"));
    let oracle_root =
        PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_ROOT").expect("oracle root"));
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let setup = r#"
from pathlib import Path
import json,sys
from scripts import history_migration as M, history_authoring as A, history_store as H, project_modes as G
b=Path(sys.argv[1]); l=b/'legacy'; l.mkdir(); e=l/'GROUNDING.yaml'; e.write_text('known:\n  p.value: {v: 1}\n')
r=b/'source'; M.prepare(e,operation='import',recorded_at='2026-09-17',record_id='branch-record').publish(r)
(r/'.kpopper/view.yaml').write_text('sections: []\n')
G.git(r,'init','-b','main'); G.git(r,'config','user.name','Fixture'); G.git(r,'config','user.email','fixture@example.invalid'); G.git(r,'add','.'); G.git(r,'-c','commit.gpgsign=false','commit','-m','source')
source=G.git(r,'rev-parse','HEAD').stdout.decode().strip(); head=H.Store(r/'GROUNDING.yaml').state()['subjects']['p.value']['head']
A.commit(r/'GROUNDING.yaml',A.prepare(r/'GROUNDING.yaml',{'kind':'set','id':'p.value','value':3}),verify=lambda d:None)
G.git(r,'add','.'); G.git(r,'-c','commit.gpgsign=false','commit','-m','current')
print(json.dumps({'source':source,'head':head}))
"#;
    let setup = Command::new(&oracle_python)
        .args(["-c", setup])
        .arg(&base)
        .env("PYTHONPATH", &oracle_root)
        .output()
        .unwrap();
    assert!(
        setup.status.success(),
        "{}",
        String::from_utf8_lossy(&setup.stderr)
    );
    let ids: J = serde_json::from_slice(&setup.stdout).unwrap();
    let source = ids["source"].as_str().unwrap();
    let head = ids["head"].as_str().unwrap();
    let native = base.join("native");
    let python = base.join("python");
    for target in [&native, &python] {
        let out = Command::new("git")
            .args(["clone", "-q", "--no-hardlinks"])
            .arg(base.join("source"))
            .arg(target)
            .output()
            .unwrap();
        assert!(out.status.success());
    }
    let call = |program: &Path, oracle: bool, root: &Path, args: &[&str]| {
        let mut c = Command::new(program);
        if oracle {
            c.arg(oracle_root.join("scripts/cli.py"));
        }
        c.current_dir(root)
            .args(args)
            .env("KPOPPER_SESSION_DISABLE", "1")
            .env(
                "XDG_STATE_HOME",
                base.join(if oracle {
                    "python-state"
                } else {
                    "native-state"
                }),
            )
            .output()
            .unwrap()
    };
    let preview_args = ["consolidate", "--dry-run", "--from", source];
    let a = call(
        Path::new(env!("CARGO_BIN_EXE_kpop")),
        false,
        &native,
        &preview_args,
    );
    let p = call(&oracle_python, true, &python, &preview_args);
    assert_eq!(
        (a.status.code(), &a.stdout, &a.stderr),
        (p.status.code(), &p.stdout, &p.stderr)
    );
    assert_eq!(image(&native), image(&python));
    let baseline = image(&native);
    let hex = regex::Regex::new(r"[0-9a-f]{64}").unwrap();
    let baseline_ids = baseline
        .values()
        .flat_map(|raw| {
            let text = String::from_utf8_lossy(raw);
            hex.find_iter(&text)
                .map(|m| m.as_str().to_owned())
                .collect::<Vec<_>>()
        })
        .collect::<std::collections::BTreeSet<_>>();
    let choose = format!("p.value={head}");
    let args = [
        "consolidate",
        "--from",
        source,
        "--by",
        "reviewer",
        "--choose",
        &choose,
    ];
    for relative in ["GROUNDING.yaml", ".kpopper/view.yaml"] {
        let np = native.join(relative);
        let pp = python.join(relative);
        let nb = fs::read(&np).unwrap();
        let pb = fs::read(&pp).unwrap();
        let dirty = if relative == "GROUNDING.yaml" {
            b"known: {p.value: {v: 42}}\n".as_slice()
        } else {
            b"sections: [{title: dirty}]\n".as_slice()
        };
        fs::write(&np, dirty).unwrap();
        fs::write(&pp, dirty).unwrap();
        let before_n = image(&native);
        let before_p = image(&python);
        let na = call(Path::new(env!("CARGO_BIN_EXE_kpop")), false, &native, &args);
        let py = call(&oracle_python, true, &python, &args);
        assert_eq!(
            (na.status.code(), na.stdout, na.stderr),
            (py.status.code(), py.stdout, py.stderr),
            "dirty {relative}"
        );
        assert_eq!(image(&native), before_n);
        assert_eq!(image(&python), before_p);
        fs::write(np, nb).unwrap();
        fs::write(pp, pb).unwrap();
    }
    let mut too_many = vec!["consolidate".to_owned(), "--dry-run".into()];
    for _ in 0..17 {
        too_many.extend(["--from".into(), source.into()]);
    }
    let borrowed = too_many.iter().map(String::as_str).collect::<Vec<_>>();
    let na = call(
        Path::new(env!("CARGO_BIN_EXE_kpop")),
        false,
        &native,
        &borrowed,
    );
    let py = call(&oracle_python, true, &python, &borrowed);
    assert_eq!(
        (na.status.code(), na.stdout, na.stderr),
        (py.status.code(), py.stdout, py.stderr)
    );
    let a = call(Path::new(env!("CARGO_BIN_EXE_kpop")), false, &native, &args);
    let p = call(&oracle_python, true, &python, &args);
    assert_eq!(
        a.status.code(),
        p.status.code(),
        "{}{}",
        String::from_utf8_lossy(&a.stdout),
        String::from_utf8_lossy(&a.stderr)
    );
    fn operation(raw: &[u8]) -> String {
        String::from_utf8_lossy(raw)
            .lines()
            .find_map(|l| l.trim().strip_prefix("operation: "))
            .unwrap()
            .to_owned()
    }
    let ao = operation(&a.stdout);
    let po = operation(&p.stdout);
    let normalize = |raw: &[u8], op: &str| String::from_utf8_lossy(raw).replace(op, "$OP");
    assert_eq!(normalize(&a.stdout, &ao), normalize(&p.stdout, &po));
    assert_eq!(a.stderr, p.stderr);
    let native_raw = image(&native);
    let python_raw = image(&python);
    let object_id =
        |mine: &BTreeMap<PathBuf, Vec<u8>>, other: &BTreeMap<PathBuf, Vec<u8>>, op: &str| {
            mine.keys()
                .filter(|p| !other.contains_key(*p) && !p.to_string_lossy().contains(op))
                .filter_map(|p| p.file_stem().and_then(|v| v.to_str()))
                .find(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
                .unwrap()
                .to_owned()
        };
    let aid = object_id(&native_raw, &python_raw, &ao);
    let pid = object_id(&python_raw, &native_raw, &po);
    let normalize_image = |files: BTreeMap<PathBuf, Vec<u8>>, op: &str, id: &str| {
        let timestamp = regex::Regex::new(r"20[0-9]{2}-[0-9]{2}-[0-9]{2}T[0-9:.+\-Z]+").unwrap();
        let digest = regex::Regex::new(r"(committed_set_digest: )[0-9a-f]{64}").unwrap();
        files
            .into_iter()
            .map(|(path, raw)| {
                let body = normalize(&raw, op).replace(id, "$OBJECT");
                let body = timestamp.replace_all(&body, "$TIME");
                let body = digest.replace_all(&body, "${1}$SET");
                let body = hex.replace_all(&body, |caps: &regex::Captures<'_>| {
                    if baseline_ids.contains(&caps[0]) {
                        caps[0].to_owned()
                    } else {
                        "$GENERATED".into()
                    }
                });
                let body = canonicalize_commit_objects(body.into_owned());
                (
                    PathBuf::from(
                        path.to_string_lossy()
                            .replace(op, "$OP")
                            .replace(id, "$OBJECT"),
                    ),
                    body.into_bytes(),
                )
            })
            .collect::<BTreeMap<_, _>>()
    };
    let ai = normalize_image(native_raw, &ao, &aid);
    let pi = normalize_image(python_raw, &po, &pid);
    assert_eq!(ai.keys().collect::<Vec<_>>(), pi.keys().collect::<Vec<_>>());
    for (path, left) in &ai {
        let right = &pi[path];
        assert_eq!(
            left,
            right,
            "image differs: {}\nnative: {}\npython: {}",
            path.display(),
            String::from_utf8_lossy(left),
            String::from_utf8_lossy(right)
        );
    }
}
