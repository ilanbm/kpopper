//! Every example record shipped under examples/ passes `check` with the summary it prints
//! in this repository. Each record is copied into a fresh git repository together with the
//! files beside it that it cites as sources, so a cited file absent from the example is
//! absent here too. The files are embedded with literal include_str! paths, so CI's rust
//! lane selects on any edit to them.

use std::{fs, path::PathBuf, process::Command};

/// Native resources from KPOP_CONSOLIDATION_RESOURCES. Without them the check is skipped,
/// except under CI, which prepares them for every leg.
fn resources() -> Option<PathBuf> {
    match std::env::var_os("KPOP_CONSOLIDATION_RESOURCES").filter(|path| !path.is_empty()) {
        Some(path) => Some(PathBuf::from(path)),
        None if std::env::var_os("CI").is_some() => {
            panic!("KPOP_CONSOLIDATION_RESOURCES is not set: CI checks every example record")
        }
        None => {
            eprintln!(
                "example record check skipped: set KPOP_CONSOLIDATION_RESOURCES to native resources"
            );
            None
        }
    }
}

fn check(name: &str, record: &str, sources: &[(&str, &str)], summary: &str) {
    let Some(resources) = resources() else {
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("project");
    fs::create_dir(&workspace).unwrap();
    let empty = temp.path().join("gitconfig");
    fs::write(&empty, "").unwrap();
    let init = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&workspace)
        .env("HOME", temp.path())
        .env("GIT_CONFIG_GLOBAL", &empty)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .status()
        .unwrap();
    assert!(init.success());
    fs::write(workspace.join("GROUNDING.yaml"), record).unwrap();
    for (path, text) in sources {
        let file = workspace.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(&workspace)
        .args(["--frozen", "check"])
        .env("HOME", temp.path())
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("KPOPPER_PRIVATE_HOME", temp.path().join("private-data"))
        .env("KPOPPER_NATIVE_RESOURCES", resources)
        .env_remove("KPOPPER_READ_MODE")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "{name}: check exited with {}\n{stdout}\n{stderr}",
        output.status
    );
    assert_eq!(
        stdout.lines().rev().find(|line| !line.trim().is_empty()),
        Some(summary),
        "{name}: unexpected summary\n{stdout}\n{stderr}"
    );
}

macro_rules! example {
    ($test:ident, $record:expr, [$(($path:literal, $text:expr)),* $(,)?], $summary:literal) => {
        #[test]
        fn $test() {
            check(stringify!($test), $record, &[$(($path, $text)),*], $summary);
        }
    };
}

example!(
    context_conversation,
    include_str!("../../examples/context-conversation/GROUNDING.yaml"),
    [(
        "sources/delivery-policy.md",
        include_str!("../../examples/context-conversation/sources/delivery-policy.md")
    )],
    "0 judgments, 5 entries, 0 problems"
);
example!(
    cowork_workshop_before,
    include_str!("../../examples/cowork-workshop/before/GROUNDING.yaml"),
    [],
    "5 judgments, 25 entries, 0 problems, 5 declared"
);
example!(
    cowork_workshop_after,
    include_str!("../../examples/cowork-workshop/after/GROUNDING.yaml"),
    [],
    "5 judgments, 26 entries, 0 problems, 5 moved, 5 declared"
);
example!(
    dark_matter,
    include_str!("../../examples/dark-matter/GROUNDING.yaml"),
    [(
        "README.md",
        include_str!("../../examples/dark-matter/README.md")
    )],
    "7 judgments, 58 entries, 0 problems, 7 declared"
);
example!(
    greenhouse_report,
    include_str!("../../examples/greenhouse-report/GROUNDING.yaml"),
    [(
        "sources/boiler-service.md",
        include_str!("../../examples/greenhouse-report/sources/boiler-service.md")
    )],
    "1 judgments, 12 entries, 0 problems"
);
example!(
    launch_party,
    include_str!("../../examples/launch-party/GROUNDING.yaml"),
    [],
    "1 judgments, 3 entries, 0 problems"
);
example!(
    merge_assumptions_cache_base,
    include_str!("../../examples/merge-assumptions/cache/base/GROUNDING.yaml"),
    [(
        "search.py",
        include_str!("../../examples/merge-assumptions/cache/base/search.py")
    )],
    "0 judgments, 2 entries, 0 problems"
);
example!(
    merge_assumptions_cache_pr_b,
    include_str!("../../examples/merge-assumptions/cache/pr-b/GROUNDING.yaml"),
    [
        (
            "cache.py",
            include_str!("../../examples/merge-assumptions/cache/pr-b/cache.py")
        ),
        (
            "test_cache.py",
            include_str!("../../examples/merge-assumptions/cache/pr-b/test_cache.py")
        ),
        (
            "cache-notes.md",
            include_str!("../../examples/merge-assumptions/cache/pr-b/cache-notes.md")
        ),
    ],
    "4 judgments, 23 entries, 0 problems, 4 declared"
);
example!(
    merge_assumptions_downloads_base,
    include_str!("../../examples/merge-assumptions/downloads/base/GROUNDING.yaml"),
    [
        (
            "storage-policy.yaml",
            include_str!("../../examples/merge-assumptions/downloads/base/storage-policy.yaml")
        ),
        (
            "download-email.html",
            include_str!("../../examples/merge-assumptions/downloads/base/download-email.html")
        ),
    ],
    "0 judgments, 4 entries, 0 problems"
);
example!(
    merge_assumptions_downloads_pr_b,
    include_str!("../../examples/merge-assumptions/downloads/pr-b/GROUNDING.yaml"),
    [(
        "download-email.html",
        include_str!("../../examples/merge-assumptions/downloads/pr-b/download-email.html")
    )],
    "1 judgments, 5 entries, 0 problems, 1 declared"
);
example!(
    offer_review_before,
    include_str!("../../examples/offer-review/before/GROUNDING.yaml"),
    [],
    "1 judgments, 4 entries, 0 problems, 1 declared"
);
example!(
    offer_review_after,
    include_str!("../../examples/offer-review/after/GROUNDING.yaml"),
    [],
    "1 judgments, 5 entries, 0 problems, 1 moved, 1 declared"
);
example!(
    workshop,
    include_str!("../../examples/workshop/GROUNDING.yaml"),
    [],
    "1 judgments, 4 entries, 0 problems"
);
