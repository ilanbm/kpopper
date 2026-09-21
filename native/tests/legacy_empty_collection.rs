use std::{fs, process::Command};

#[test]
fn first_entry_opens_bare_collection_without_changing_the_other_source() {
    for (header, rendered) in [
        ("known:", "known:"),
        (
            "known: # keep collection comment",
            "known: # keep collection comment",
        ),
        ("known: {}", "known:"),
    ] {
        for explicit in [true, false] {
            let temp = tempfile::tempdir().unwrap();
            let record = temp.path().join("GROUNDING.yaml");
            let before = format!(
                "meta: {{updated: 2026-09-01}}\nsources:\n  s.source: {{name: A source}}\n{header}\n"
            );
            fs::write(&record, &before).unwrap();
            let mut args = vec![
                "add",
                "p.a",
                "v=2",
                "--as-of",
                "2026-09-19",
                "GROUNDING.yaml",
            ];
            if explicit {
                args.extend(["--in", "known"]);
            }
            let output = Command::new(env!("CARGO_BIN_EXE_kpop-native"))
                .current_dir(temp.path())
                .args(args)
                .env_remove("KPOPPER_AGENT_SESSION")
                .env_remove("CODEX_THREAD_ID")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                String::from_utf8(output.stdout).unwrap(),
                "add p.a into known, its first entry\n\nthe record needs a person on 0 judgments - check says the rest\n"
            );
            assert!(output.stderr.is_empty());
            assert_eq!(
                fs::read_to_string(&record).unwrap(),
                format!(
                    "meta: {{updated: 2026-09-19}}\nsources:\n  s.source: {{name: A source}}\n{rendered}\n  p.a:\n    v: 2\n"
                )
            );
        }
    }
}

#[test]
fn scalar_collection_and_invalid_inline_edit_leave_record_unchanged() {
    for value in ["42", "false", "null"] {
        let temp = tempfile::tempdir().unwrap();
        let record = temp.path().join("GROUNDING.yaml");
        let before = format!(
            "meta: {{updated: 2026-09-01}}\nsources:\n  s.source: {{name: A source}}\nknown: {value}\n"
        );
        fs::write(&record, &before).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_kpop-native"))
            .current_dir(temp.path())
            .args([
                "add",
                "p.a",
                "v=2",
                "--in",
                "known",
                "--as-of",
                "2026-09-19",
                "GROUNDING.yaml",
            ])
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "{value}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(fs::read_to_string(&record).unwrap(), before);
    }
}
