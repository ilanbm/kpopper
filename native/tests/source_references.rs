//! File locators are checked against this checkout; historical locators name a Git blob.
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

struct Fixture {
    root: tempfile::TempDir,
    private: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        let f = Self {
            root: tempfile::tempdir().unwrap(),
            private: tempfile::tempdir().unwrap(),
        };
        f.git(&["init", "-q"]);
        fs::create_dir(f.root.path().join("sources")).unwrap();
        fs::write(f.root.path().join("sources/original.txt"), "original").unwrap();
        f.git(&["add", "."]);
        f.git(&[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "commit",
            "-qm",
            "Original source",
        ]);
        f.git(&["tag", "source-final"]);
        f
    }
    fn git(&self, args: &[&str]) {
        let mut c = Command::new("git");
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("GIT_") {
                c.env_remove(name);
            }
        }
        let o = c.args(args).current_dir(self.root.path()).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }
    fn check(&self, record: &str) -> Output {
        self.check_in(record, self.root.path())
    }
    fn check_in(&self, record: &str, cwd: &Path) -> Output {
        fs::write(self.root.path().join("GROUNDING.yaml"), record).unwrap();
        self.command(cwd).output().unwrap()
    }
    fn command(&self, cwd: &Path) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_kpop"));
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("GIT_") {
                c.env_remove(name);
            }
        }
        c.args(["--frozen", "check"])
            .current_dir(cwd)
            .env("XDG_STATE_HOME", self.private.path())
            .env("XDG_CONFIG_HOME", self.private.path())
            .env("KPOPPER_NATIVE_CACHE", self.private.path().join("cache"))
            .env_remove("KPOPPER_ROOT")
            .env_remove("KPOPPER_NATIVE_RESOURCES");
        c
    }
}
fn text(output: Output) -> String {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
#[test]
fn missing_file_is_a_note_with_a_reread_or_revision_hint() {
    let f = Fixture::new();
    let out = text(f.check("sources:\n  s.gone: {file: sources/gone.txt}\nknown:\n  p.direct: {v: 1, from: sources/gone.rs}\n"));
    assert!(out.contains("NOTE s.gone: file sources/gone.txt"), "{out}");
    assert!(out.contains("NOTE p.direct: file sources/gone.rs"), "{out}");
    assert!(
        out.contains("re-read") && out.contains("revision:path"),
        "{out}"
    );
    assert!(out.contains("0 problems"), "{out}");
}

#[test]
fn a_bare_filename_is_checked_without_mistaking_entry_ids_for_files() {
    let f = Fixture::new();
    fs::write(f.root.path().join("package.json"), "{}").unwrap();
    let out = text(f.check("sources:\n  s.report: {name: report}\n  s.actual.md: {name: source id}\nknown:\n  p.gone: {v: 1, from: pyproject.toml}\n  p.present: {v: 1, from: package.json}\n  p.ref: {v: 1, from: s.report}\n  p.file_like_id: {v: 1, from: s.actual.md}\n"));
    assert!(out.contains("NOTE p.gone: file pyproject.toml"), "{out}");
    for id in ["p.present", "p.ref", "p.file_like_id"] {
        assert!(!out.contains(&format!("NOTE {id}: file")), "{out}");
    }
}

#[test]
fn relative_sources_follow_the_primary_record_even_when_named_from_another_directory() {
    let f = Fixture::new();
    let nested = f.root.path().join("nested");
    fs::create_dir(&nested).unwrap();
    fs::write(nested.join("source.txt"), "present").unwrap();
    fs::write(
        nested.join("GROUNDING.yaml"),
        "sources:\n  s.present: {file: source.txt}\n  s.missing: {file: gone.txt}\n",
    )
    .unwrap();
    for mut command in [f.command(&nested), {
        let mut c = f.command(f.root.path());
        c.arg("nested/GROUNDING.yaml");
        c
    }] {
        let out = text(command.output().unwrap());
        assert!(!out.contains("NOTE s.present:"), "{out}");
        assert!(out.contains("NOTE s.missing: file gone.txt"), "{out}");
    }
}

#[test]
fn core_check_reports_the_same_advisory_without_a_false_computation_failure() {
    let f = Fixture::new();
    let target = kpop_native::reasoning_runtime::target_name().unwrap();
    let resources = f.private.path().join("resources");
    fs::create_dir_all(resources.join("reasoning")).unwrap();
    let archive = format!("{target}.kpopper-runtime");
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(&archive),
        resources.join("reasoning").join(&archive),
    )
    .unwrap();
    fs::write(f.root.path().join("GROUNDING.yaml"), "meta: {reasoning: {version: 1, profile: core/v1, requires: [arithmetic/v1]}}\nsources:\n  s.missing: {file: sources/no.txt}\n").unwrap();
    let output = f
        .command(f.root.path())
        .env("KPOPPER_NATIVE_RESOURCES", resources)
        .output()
        .unwrap();
    let out = text(output);
    assert!(out.contains("NOTE s.missing: file sources/no.txt"), "{out}");
    assert!(out.contains("1 nodes, 0 problems"), "{out}");
}
#[test]
fn live_sources_symbolic_references_prose_urls_and_patterns_are_not_missing_files() {
    let f = Fixture::new();
    let out = text(f.check("sources:\n  s.file: {file: sources/original.txt}\n  s.web: {file: 'https://example.test/a'}\nknown:\n  p.reference: {v: 1, from: s.file}\n  p.unknown: {v: 1, from: s.other}\n  p.prose: {v: 1, from: 'a comparison of sources/ on another machine'}\n  p.pattern: {v: 1, from: 'sources/*.txt'}\n"));
    assert!(!out.contains("file "), "{out}");
}

#[test]
fn line_number_citations_and_relative_colon_names_are_not_pins() {
    let f = Fixture::new();
    let out = text(f.check(
        "sources:\n  s.line: {file: 'sources/original.txt:42'}\n  s.range: {file: 'sources/original.txt:42-45'}\n  s.column: {file: 'sources/original.txt:42:7'}\n  s.pinned_column: {file: 'source-final:sources/original.txt:42:7'}\n  s.trailing: {file: 'docs/guide:'}\nknown:\n  p.line: {v: 1, from: 'sources/original.txt:42'}\n",
    ));
    assert!(!out.contains("pinned file"), "{out}");
    for id in [
        "s.line",
        "s.range",
        "s.column",
        "s.pinned_column",
        "p.line",
    ] {
        assert!(!out.contains(&format!("NOTE {id}:")), "{out}");
    }

    let missing = text(f.check("sources:\n  s.missing_line: {file: 'sources/missing.py:42'}\n"));
    assert!(
        missing.contains("NOTE s.missing_line: file sources/missing.py:42 is absent"),
        "{missing}"
    );
    assert!(!missing.contains("pinned file"), "{missing}");

    let missing_colon_path =
        text(f.check("sources:\n  s.missing_colon: {file: 'sources/missing:part.py'}\n"));
    assert!(
        missing_colon_path.contains("NOTE s.missing_colon: no local file matches")
            && missing_colon_path.contains("Git does not know revision sources/missing"),
        "{missing_colon_path}"
    );
    assert!(
        !missing_colon_path.contains("pinned file"),
        "{missing_colon_path}"
    );
    let trailing_colon = text(f.check("sources:\n  s.trailing: {file: 'docs/guide:'}\n"));
    assert!(
        trailing_colon.contains("NOTE s.trailing: file docs/guide: is absent"),
        "{trailing_colon}"
    );
    assert!(!trailing_colon.contains("pinned file"), "{trailing_colon}");
}

#[cfg(unix)]
#[test]
fn colon_paths_are_supported_for_local_and_pinned_sources() {
    let f = Fixture::new();
    fs::write(f.root.path().join("sources/part:name.py"), "source").unwrap();
    fs::create_dir(f.root.path().join("logs")).unwrap();
    fs::write(f.root.path().join("logs/run:1"), "source").unwrap();
    let local = text(f.check(
        "sources:\n  s.colon: {file: 'sources/part:name.py'}\n  s.digit_colon: {file: 'logs/run:1'}\n",
    ));
    assert!(!local.contains("NOTE s.colon:"), "{local}");
    assert!(!local.contains("NOTE s.digit_colon:"), "{local}");

    f.git(&["checkout", "-b", "colon-pins"]);
    f.git(&["add", "logs/run:1"]);
    f.git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.test",
        "commit",
        "-qm",
        "Record colon path",
    ]);
    f.git(&["tag", "colon-pins"]);
    fs::remove_file(f.root.path().join("logs/run:1")).unwrap();
    let pinned = text(f.check(
        "sources:\n  s.colon_pin: {file: 'colon-pins:logs/run:1'}\n",
    ));
    assert!(!pinned.contains("NOTE s.colon_pin:"), "{pinned}");
}
#[test]
fn numeric_pinned_paths_are_not_stripped_as_line_numbers() {
    let f = Fixture::new();
    f.git(&["checkout", "-b", "numeric-paths"]);
    fs::write(f.root.path().join("2024"), "source").unwrap();
    f.git(&["add", "2024"]);
    f.git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.test",
        "commit",
        "-qm",
        "Add numeric path",
    ]);
    f.git(&["tag", "numeric-paths"]);
    fs::remove_file(f.root.path().join("2024")).unwrap();
    let out = text(f.check(
        "sources:\n  s.numeric: {file: 'numeric-paths:2024'}\n  s.numeric_line: {file: 'numeric-paths:2024:5'}\n  s.missing: {file: 'numeric-paths:2042'}\n",
    ));
    assert!(!out.contains("NOTE s.numeric:"), "{out}");
    assert!(!out.contains("NOTE s.numeric_line:"), "{out}");
    assert!(out.contains("NOTE s.missing: pinned file"), "{out}");
}
#[test]
fn absolute_paths_are_checked_only_inside_this_repository() {
    let f = Fixture::new();
    let inside = f.root.path().join("inside missing.txt");
    let outside = f.private.path().join("outside missing.txt");
    let out = text(f.check(&format!(
        "sources:\n  s.inside: {{file: '{}' }}\n  s.outside: {{file: '{}' }}\n",
        inside.display(),
        outside.display()
    )));
    assert!(out.contains("NOTE s.inside:"), "{out}");
    assert!(!out.contains("s.outside:"), "{out}");
}
#[test]
fn pinned_sources_remain_openable_after_the_working_file_is_deleted() {
    let f = Fixture::new();
    fs::remove_file(f.root.path().join("sources/original.txt")).unwrap();
    let out = text(f.check("sources:\n  s.old: {file: 'source-final:sources/original.txt', at: 'first line', read: '2026-09-01'}\nknown:\n  p.old: {v: 1, from: 'source-final:sources/original.txt'}\n"));
    assert!(
        !out.contains("NOTE s.old:") && !out.contains("NOTE p.old:"),
        "{out}"
    );
    f.git(&["show", "source-final:sources/original.txt"]);
}

#[test]
fn dot_relative_pins_resolve_from_the_primary_record_directory() {
    let f = Fixture::new();
    f.git(&["checkout", "-b", "nested-pins"]);
    fs::create_dir(f.root.path().join("nested")).unwrap();
    fs::write(f.root.path().join("nested/source.txt"), "nested").unwrap();
    fs::write(f.root.path().join("sibling.txt"), "parent").unwrap();
    f.git(&["add", "nested/source.txt", "sibling.txt"]);
    f.git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.test",
        "commit",
        "-qm",
        "Add nested pinned sources",
    ]);
    f.git(&["tag", "nested-pins"]);
    fs::remove_file(f.root.path().join("nested/source.txt")).unwrap();
    fs::remove_file(f.root.path().join("sibling.txt")).unwrap();
    fs::write(
        f.root.path().join("nested/GROUNDING.yaml"),
        "sources:\n  s.dot: {file: 'nested-pins:./source.txt'}\n  s.parent: {file: 'nested-pins:../sibling.txt'}\n",
    )
    .unwrap();
    let out = text(f.command(&f.root.path().join("nested")).output().unwrap());
    assert!(!out.contains("NOTE s.dot:"), "{out}");
    assert!(!out.contains("NOTE s.parent:"), "{out}");
}

#[cfg(unix)]
#[test]
fn dot_relative_pins_resolve_when_the_record_path_uses_a_symlink() {
    let f = Fixture::new();
    f.git(&["checkout", "-b", "symlinked-record-pins"]);
    fs::create_dir(f.root.path().join("documentation")).unwrap();
    fs::write(f.root.path().join("documentation/source.txt"), "source").unwrap();
    f.git(&["add", "documentation/source.txt"]);
    f.git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.test",
        "commit",
        "-qm",
        "Add source behind symlinked record path",
    ]);
    f.git(&["tag", "symlinked-record-pins"]);
    fs::remove_file(f.root.path().join("documentation/source.txt")).unwrap();
    std::os::unix::fs::symlink(
        f.root.path().join("documentation"),
        f.root.path().join("docs"),
    )
    .unwrap();
    fs::write(
        f.root.path().join("documentation/GROUNDING.yaml"),
        "sources:\n  s.dot: {file: 'symlinked-record-pins:./source.txt'}\n",
    )
    .unwrap();
    let record = f.root.path().join("docs/GROUNDING.yaml");
    let mut command = f.command(f.root.path());
    command.arg(record);
    let out = text(command.output().unwrap());
    assert!(!out.contains("NOTE s.dot:"), "{out}");
}

#[test]
fn tree_lookup_finds_a_pinned_file_without_its_blob_object() {
    let f = Fixture::new();
    let output = Command::new("git")
        .args(["rev-parse", "source-final:sources/original.txt"])
        .current_dir(f.root.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let object = String::from_utf8(output.stdout).unwrap();
    let object = object.trim();
    let object_path = f
        .root
        .path()
        .join(".git/objects")
        .join(&object[..2])
        .join(&object[2..]);
    assert!(object_path.is_file(), "{}", object_path.display());
    fs::remove_file(object_path).unwrap();
    let out = text(f.check(
        "sources:\n  s.blobless: {file: 'source-final:sources/original.txt'}\n",
    ));
    assert!(!out.contains("NOTE s.blobless:"), "{out}");
}
#[test]
fn slash_revisions_are_pinned_when_the_git_ref_resolves() {
    let f = Fixture::new();
    f.git(&["branch", "feature/source-reference"]);
    let out = text(f.check(
        "sources:\n  s.branch: {file: 'feature/source-reference:sources/original.txt'}\n  s.missing: {file: 'feature/source-reference:sources/missing.txt'}\n",
    ));
    assert!(!out.contains("NOTE s.branch:"), "{out}");
    assert!(out.contains("NOTE s.missing: pinned file"), "{out}");
}
#[test]
fn a_pin_does_not_hide_an_unknown_revision_or_a_missing_blob() {
    let f = Fixture::new();
    let out = text(f.check("sources:\n  s.ref: {file: 'no-such-ref:sources/original.txt'}\n  s.slash_ref: {file: 'feature/unknown:sources/original.txt'}\n  s.root_ref: {file: 'v9.9:README.md'}\n  s.blob: {file: 'source-final:sources/missing.txt'}\n  s.tree: {file: 'source-final:sources'}\n  s.line: {file: 'source-final:sources/missing.txt:42'}\nknown:\n  p.doi: {v: 1, from: 'doi:10.1145/3368089.3409747'}\n"));
    for id in ["s.ref", "s.blob", "s.tree", "s.line"] {
        assert!(out.contains(&format!("NOTE {id}: pinned file")), "{out}");
    }
    assert!(
        out.contains("NOTE s.slash_ref: no local file matches")
            && out.contains("Git does not know revision feature/unknown"),
        "{out}"
    );
    assert!(
        out.contains("NOTE s.root_ref: no local file matches")
            && out.contains("Git does not know revision v9.9"),
        "{out}"
    );
    assert!(
        out.contains("NOTE p.doi: no local file matches")
            && out.contains("Git does not know revision doi"),
        "{out}"
    );
    assert!(out.contains("git show"), "{out}");
    assert!(
        out.contains("git show source-final:sources/missing.txt, or re-read"),
        "{out}"
    );
    assert!(
        !out.contains("git show source-final:sources/missing.txt:42"),
        "{out}"
    );
}

#[test]
fn long_missing_pinned_paths_remain_missing_not_probe_failures() {
    let f = Fixture::new();
    let mut components = (0..10)
        .map(|index| format!("{index:02}{}", "x".repeat(98)))
        .collect::<Vec<_>>();
    components.push("missing.txt".into());
    let path = components.join("/");
    let locator = format!("source-final:{path}");
    assert!(locator.len() > 1024);
    let record = format!("sources:\n  s.long: {{file: '{locator}'}}\n");
    let out = text(f.check(&record));
    assert!(out.contains("NOTE s.long: pinned file"), "{out}");
    assert!(!out.contains("probe was unavailable"), "{out}");
}

#[cfg(unix)]
#[test]
fn pinned_git_probe_io_failures_are_advisory_notes() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    f.git(&["branch", "feature/source-reference"]);
    let real_git = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|directory| directory.join("git"))
        .find(|candidate| candidate.is_file())
        .expect("git must be available on the original PATH");
    let bin = f.private.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let git = bin.join("git");
    fs::write(
        &git,
        "#!/bin/sh\nfor arg in \"$@\"; do\n  if [ \"$arg\" = ls-tree ]; then printf '%16384s' ''; exit 0; fi\ndone\nexec \"$KPOPPER_TEST_GIT\" \"$@\"\n",
    )
    .unwrap();
    fs::set_permissions(&git, fs::Permissions::from_mode(0o755)).unwrap();
    let record = "sources:\n  s.pinned: {file: 'source-final:sources/original.txt'}\n  s.slash_pin: {file: 'feature/source-reference:sources/original.txt'}\n";
    let mut command = f.command(f.root.path());
    command.env(
        "PATH",
        format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        ),
    );
    command.env("KPOPPER_TEST_GIT", real_git);
    fs::write(f.root.path().join("GROUNDING.yaml"), record).unwrap();
    let output = command.output().unwrap();
    let out = text(output);
    assert!(
        out.contains("NOTE s.pinned: could not check locator")
            && out.contains("s.pinned")
            && out.contains("object probe was unavailable"),
        "{out}"
    );
    assert!(
        out.contains("NOTE s.slash_pin: could not check locator")
            && out.contains("s.slash_pin")
            && out.contains("object probe was unavailable"),
        "{out}"
    );
    assert!(!out.contains("NOTE s.pinned: pinned file"), "{out}");
    assert!(!out.contains("NOTE s.slash_pin: pinned file"), "{out}");
    assert!(out.contains("0 problems"), "{out}");
}
#[cfg(unix)]
#[test]
fn failed_revision_probes_are_advisory_without_claiming_a_pin_or_absent_file() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let real_git = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|directory| directory.join("git"))
        .find(|candidate| candidate.is_file())
        .expect("git must be available on the original PATH");
    let bin = f.private.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let git = bin.join("git");
    fs::write(
        &git,
        "#!/bin/sh\nfor arg in \"$@\"; do\n  if [ \"$arg\" = --verify ]; then printf '%16384s' ''; exit 0; fi\ndone\nexec \"$KPOPPER_TEST_GIT\" \"$@\"\n",
    )
    .unwrap();
    fs::set_permissions(&git, fs::Permissions::from_mode(0o755)).unwrap();
    let mut command = f.command(f.root.path());
    command.env(
        "PATH",
        format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        ),
    );
    command.env("KPOPPER_TEST_GIT", real_git);
    fs::write(
        f.root.path().join("GROUNDING.yaml"),
        "sources:\n  s.slash_pin: {file: 'feature/source-reference:sources/original.txt'}\n  s.line: {file: 'sources/original.txt:42'}\n",
    )
    .unwrap();
    let out = text(command.output().unwrap());
    assert!(out.contains("NOTE s.slash_pin: could not check locator"), "{out}");
    assert!(!out.contains("NOTE s.line:"), "{out}");
    assert!(!out.contains("pinned file"), "{out}");
    assert!(!out.contains("file feature/source-reference:sources/original.txt is absent"), "{out}");
    assert!(out.contains("0 problems"), "{out}");
}
#[test]
fn file_paths_resolve_from_the_repository_when_called_in_a_subdirectory() {
    let f = Fixture::new();
    let out = text(f.check_in(
        "sources:\n  s.live: {file: sources/original.txt}\n  s.gone: {file: sources/gone.txt}\n",
        &f.root.path().join("sources"),
    ));
    assert!(!out.contains("NOTE s.live:"), "{out}");
    assert!(out.contains("NOTE s.gone:"), "{out}");
}
#[cfg(unix)]
#[test]
fn a_symlink_leading_outside_the_repository_is_not_a_missing_local_source() {
    let f = Fixture::new();
    std::os::unix::fs::symlink(f.private.path(), f.root.path().join("external")).unwrap();
    let out = text(f.check("sources:\n  s.external: {file: external/missing.txt}\n  s.traversal: {file: ../missing-outside.txt}\n"));
    assert!(
        !out.contains("NOTE s.external:") && !out.contains("NOTE s.traversal:"),
        "{out}"
    );
}
