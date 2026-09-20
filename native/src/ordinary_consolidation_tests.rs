use super::*;
use std::{
    fs,
    path::{Path, PathBuf},
};
const BASE: &str = "schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown:\n  p.value: {v: 1, of: 2026-09-18}\n";
const HYP: &str = "known:\n  p.next: {v: 2}\n";
fn setup() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let entry = root.join("GROUNDING.yaml");
    let hypothesis = root.join(".kpopper/hypotheses/trial.yaml");
    fs::create_dir_all(hypothesis.parent().unwrap()).unwrap();
    fs::write(&entry, BASE).unwrap();
    fs::write(&hypothesis, HYP).unwrap();
    (temp, root, entry, hypothesis)
}
fn options(entry: &Path) -> Options {
    Options {
        names: vec!["trial".into()],
        as_of: Some("2026-09-19".into()),
        record: Some(entry.into()),
        ..Default::default()
    }
}
fn journal(root: &Path) -> PathBuf {
    root.join(
        crate::history_transaction::Layout::for_entry("GROUNDING.yaml")
            .unwrap()
            .journal,
    )
}
#[test]
fn source_hypothesis_membership_view_and_policy_races_refuse_before_mutation() {
    for change in ["source", "hypothesis", "membership", "view", "policy"] {
        let (_temp, root, entry, hypothesis) = setup();
        let mut acted = false;
        let result = dispatch_with_runtime(&options(&entry), &root, None, &mut |stage| {
            if stage == "prepared" && !acted {
                acted = true;
                match change {
                    "source" => fs::write(&entry, format!("# independent edit\n{BASE}"))?,
                    "hypothesis" => fs::write(&hypothesis, "known: {p.next: {v: 9}}\n")?,
                    "membership" => fs::write(
                        hypothesis.parent().unwrap().join("other.yaml"),
                        "known: {p.other: {v: 3}}\n",
                    )?,
                    "view" => fs::write(root.join(".kpopper/view.yaml"), "sections: []\n")?,
                    _ => {
                        let project = crate::project_modes::Project::open(&root)?;
                        let mut config = project.config()?;
                        map_mut(&mut config)?
                            .insert("generation".into(), crate::history_authoring::n("1"));
                        fs::create_dir_all(project.config_path.parent().unwrap())?;
                        fs::write(project.config_path, config.to_json()?.to_string())?;
                    }
                }
            }
            Ok(())
        })
        .unwrap();
        assert!(acted, "{change}");
        assert_eq!(result.code, 1, "{change}: {}", result.stdout);
        assert!(
            (result.stderr.contains("changed") || result.stderr.contains("concurrent_edit")),
            "{change}: {}",
            result.stderr
        );
        assert!(!journal(&root).exists(), "{change}");
        assert!(hypothesis.exists());
        let body = fs::read_to_string(&entry).unwrap();
        assert_eq!(
            body,
            if change == "source" {
                format!("# independent edit\n{BASE}")
            } else {
                BASE.into()
            }
        );
    }
}
#[test]
fn real_partial_publication_recovers_forward_and_backward() {
    for before in [false, true] {
        let (_temp, root, entry, hypothesis) = setup();
        let result = dispatch_with_runtime(&options(&entry), &root, None, &mut |stage| {
            if stage == "validated" {
                fs::remove_file(&entry)?;
                fs::create_dir(&entry)?;
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(result.code, 1);
        assert!(journal(&root).is_file());
        assert!(entry.is_dir());
        // The real publisher deleted the first image before the later rename failed.
        assert!(!hypothesis.exists());
        let mutation = crate::history_transaction::PreparedMutation::from_bytes(
            &fs::read(journal(&root)).unwrap(),
        )
        .unwrap();
        assert!(
            mutation
                .files()
                .iter()
                .any(|f| f.role == "hypothesis" && f.after.is_none())
        );
        fs::remove_dir(&entry).unwrap();
        fs::write(&entry, BASE).unwrap();
        assert!(
            crate::legacy_authoring::recovery_pending(std::slice::from_ref(&entry), &root).unwrap()
        );
        crate::legacy_authoring::recover(std::slice::from_ref(&entry), &root, before).unwrap();
        assert!(!journal(&root).exists());
        assert_eq!(hypothesis.exists(), before);
        let content = fs::read_to_string(&entry).unwrap();
        if before {
            assert_eq!(content, BASE);
            assert_eq!(fs::read_to_string(&hypothesis).unwrap(), HYP);
        } else {
            assert!(content.contains("p.next: {v: 2}"));
        }
        crate::source_capture::capture_source(
            std::slice::from_ref(&entry),
            &root,
            crate::source_capture::ReadMode::Frozen,
            None,
        )
        .unwrap()
        .verify()
        .unwrap();
    }
}
#[test]
fn retained_journal_refuses_independent_edit_and_can_then_recover() {
    let (_temp, root, entry, hypothesis) = setup();
    let result = dispatch_with_runtime(&options(&entry), &root, None, &mut |stage| {
        if stage == "published" {
            return Err(error("interrupted after publication"));
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(result.code, 1);
    assert!(journal(&root).exists());
    assert!(!hypothesis.exists());
    let after = fs::read(&entry).unwrap();
    fs::write(&entry, "known: {p.independent: {v: 99}}\n").unwrap();
    assert!(crate::legacy_authoring::recover(std::slice::from_ref(&entry), &root, false).is_err());
    assert!(journal(&root).exists());
    assert!(
        fs::read_to_string(&entry)
            .unwrap()
            .contains("p.independent")
    );
    fs::write(&entry, after).unwrap();
    crate::legacy_authoring::recover(std::slice::from_ref(&entry), &root, true).unwrap();
    assert_eq!(fs::read_to_string(&entry).unwrap(), BASE);
    assert_eq!(fs::read_to_string(&hypothesis).unwrap(), HYP);
}

#[test]
fn deletion_only_fold_retains_a_recoverable_full_before_image() {
    for before in [false, true] {
        let (_temp, root, entry, hypothesis) = setup();
        let identical = "known:\n  p.value: {v: 1, of: 2026-09-18}\n";
        fs::write(&hypothesis, identical).unwrap();
        let result = dispatch_with_runtime(&options(&entry), &root, None, &mut |stage| {
            if stage == "published" {
                return Err(error("interrupted"));
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(result.code, 1);
        assert!(journal(&root).exists());
        assert_eq!(fs::read_to_string(&entry).unwrap(), BASE);
        assert!(!hypothesis.exists());
        crate::legacy_authoring::recover(std::slice::from_ref(&entry), &root, before).unwrap();
        assert_eq!(fs::read_to_string(&entry).unwrap(), BASE);
        assert_eq!(hypothesis.exists(), before);
        if before {
            assert_eq!(fs::read_to_string(&hypothesis).unwrap(), identical);
        }
    }
}

#[test]
fn advanced_local_fold_has_policy_bound_forward_and_backward_recovery() {
    for before in [false, true] {
        let (_temp, root, entry, hypothesis) = setup();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .arg(&root)
                .status()
                .unwrap()
                .success()
        );
        let result = dispatch_with_runtime(&options(&entry), &root, None, &mut |stage| {
            if stage == "published" {
                return Err(error("interrupted"));
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(result.code, 1);
        assert!(journal(&root).exists());
        assert!(
            crate::legacy_authoring::recovery_pending(std::slice::from_ref(&entry), &root).unwrap()
        );
        crate::legacy_authoring::recover(std::slice::from_ref(&entry), &root, before).unwrap();
        assert!(!journal(&root).exists());
        assert_eq!(hypothesis.exists(), before);
        let content = fs::read_to_string(&entry).unwrap();
        if before {
            assert_eq!(content, BASE);
        } else {
            assert!(content.contains("p.next: {v: 2}"));
        }
    }
}

#[test]
fn supplied_preview_uses_the_same_union_and_report_without_source_files() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../tests/fixtures/ordinary-consolidation-preview.json"
    ))
    .unwrap();
    let document =
        crate::history_yaml::decode_document(fixture["record"].as_str().unwrap().as_bytes())
            .unwrap();
    let proposal =
        crate::history_yaml::decode_document(fixture["proposal"].as_str().unwrap().as_bytes())
            .unwrap();
    let proposals = vec![PreviewHypothesis {
        name: "tree/here".into(),
        document: proposal,
        head: V::from_json(&fixture["head"]).unwrap(),
    }];
    let preview = super::preview(&PreviewRequest {
        document: &document,
        hypotheses: &Map::new(),
        proposals: &proposals,
        context: None,
        as_of: Some("2026-09-19"),
        runtime: None,
    })
    .unwrap();
    assert_eq!(preview.report, fixture["report"].as_str().unwrap());
    assert_eq!(
        preview.exit_code,
        i32::from(fixture["red"].as_bool().unwrap())
    );
    assert_eq!(preview.blocked, fixture["blocked"].as_bool().unwrap());
    assert!(preview.candidate_document.is_some());
}
