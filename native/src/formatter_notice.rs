//! A pre-commit formatter can reflow the record file on commit. A rewrite of a
//! history-backed record then reads as an unresolved hand edit, so `check` says
//! when the workspace configures one and its ignore file leaves the record exposed.
use std::path::{Path, PathBuf};

/// The note `check` prints for the record at `record`, if any. The record's
/// directory and the Git top level above it are each a place a formatter may run
/// from; each one that configures a formatter must exclude the record through
/// its own `.prettierignore`, since a formatter run there reads no other.
pub(crate) fn notice(record: &Path) -> Option<String> {
    let name = record.file_name()?.to_str()?;
    let directory = record
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut roots = vec![directory.to_path_buf()];
    if let Some(top) = git_top(directory)
        && !roots.contains(&top)
    {
        roots.push(top);
    }
    let found = roots
        .iter()
        .filter_map(|root| Some((root, formatter(root)?)))
        .find(|(root, _)| !ignored(root, record))?
        .1;
    Some(format!(
        "formatter: {found} is present and .prettierignore does not list {name} - a pre-commit formatter may rewrite the record; add {name} and .kpopper/ to .prettierignore"
    ))
}

fn git_top(directory: &Path) -> Option<PathBuf> {
    directory
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(Path::to_path_buf)
}

/// The first formatter configuration in `root`, named as the person sees it.
fn formatter(root: &Path) -> Option<String> {
    let mut names = std::fs::read_dir(root)
        .ok()?
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .collect::<Vec<_>>();
    names.sort();
    let named = |test: &dyn Fn(&str) -> bool| names.iter().find(|n| test(n)).cloned();
    named(&|n| n.starts_with(".prettierrc"))
        .or_else(|| named(&|n| n.starts_with("prettier.config.")))
        .or_else(|| named(&|n| n.starts_with(".lintstagedrc")))
        .or_else(|| named(&|n| n.starts_with("lint-staged.config.")))
        .or_else(|| package_key(root))
        .or_else(|| root.join(".husky").is_dir().then(|| ".husky/".to_owned()))
}

fn package_key(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join("package.json")).ok()?;
    let package = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    ["prettier", "lint-staged"]
        .into_iter()
        .find(|key| package.get(key).is_some())
        .map(|key| format!("the {key} key in package.json"))
}

/// Whether `.prettierignore` in `root` excludes the record, read with gitignore
/// rules: the last matching line decides, `!` re-includes, and nothing under an
/// excluded directory can be re-included.
fn ignored(root: &Path, record: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(root.join(".prettierignore")) else {
        return false;
    };
    let Some(relative) = record
        .strip_prefix(root)
        .ok()
        .and_then(Path::to_str)
        .map(|path| path.replace('\\', "/"))
    else {
        return false;
    };
    let rules = text.lines().filter_map(rule).collect::<Vec<_>>();
    let excluded = |path: &[&str], directory: bool| {
        rules
            .iter()
            .rev()
            .find(|rule| rule.matches(path, directory))
            .is_some_and(|rule| !rule.negated)
    };
    let segments = relative.split('/').collect::<Vec<_>>();
    (1..segments.len()).any(|end| excluded(&segments[..end], true)) || excluded(&segments, false)
}

struct Rule<'a> {
    negated: bool,
    directory_only: bool,
    anchored: bool,
    segments: Vec<&'a str>,
}

fn rule(line: &str) -> Option<Rule<'_>> {
    let line = line.trim_end();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (negated, line) = match line.strip_prefix('!') {
        Some(rest) => (true, rest),
        None => (false, line.strip_prefix('\\').unwrap_or(line)),
    };
    let (directory_only, line) = match line.strip_suffix('/') {
        Some(rest) => (true, rest),
        None => (false, line),
    };
    let anchored = line.contains('/');
    let segments = line.trim_start_matches('/').split('/').collect::<Vec<_>>();
    (!segments.iter().all(|s| s.is_empty())).then_some(Rule {
        negated,
        directory_only,
        anchored,
        segments,
    })
}

impl Rule<'_> {
    fn matches(&self, path: &[&str], directory: bool) -> bool {
        if self.directory_only && !directory {
            return false;
        }
        if self.anchored {
            segments(&self.segments, path)
        } else {
            path.last().is_some_and(|last| glob(self.segments[0], last))
        }
    }
}

/// Pattern segments against path segments; `**` stands for any number of segments.
fn segments(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|skip| segments(rest, &path[skip..])),
        Some((first, rest)) => path
            .split_first()
            .is_some_and(|(segment, tail)| glob(first, segment) && segments(rest, tail)),
    }
}

/// `*` and `?` within one path segment; everything else is literal.
fn glob(pattern: &str, text: &str) -> bool {
    let (p, t) = (pattern.as_bytes(), text.as_bytes());
    let (mut i, mut j, mut star, mut mark) = (0, 0, None, 0);
    while j < t.len() {
        if i < p.len() && (p[i] == b'?' || p[i] == t[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == b'*' {
            star = Some(i);
            mark = j;
            i += 1;
        } else if let Some(s) = star {
            i = s + 1;
            mark += 1;
            j = mark;
        } else {
            return false;
        }
    }
    p[i..].iter().all(|&c| c == b'*')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn workspace(files: &[(&str, &str)]) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        for (path, text) in files {
            let path = root.path().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        root
    }

    #[test]
    fn each_configuration_is_named() {
        for (file, text, named) in [
            (".prettierrc.json", "{}", ".prettierrc.json"),
            ("prettier.config.mjs", "", "prettier.config.mjs"),
            (".lintstagedrc", "{}", ".lintstagedrc"),
            (
                "package.json",
                r#"{"lint-staged": {"*": "prettier --write"}}"#,
                "the lint-staged key in package.json",
            ),
            (".husky/pre-commit", "", ".husky/"),
        ] {
            let root = workspace(&[(file, text)]);
            let note = notice(&root.path().join("GROUNDING.yaml")).unwrap();
            assert!(
                note.starts_with(&format!("formatter: {named} is present")),
                "{note}"
            );
        }
        let root = workspace(&[("package.json", r#"{"name": "x"}"#)]);
        assert_eq!(notice(&root.path().join("GROUNDING.yaml")), None);
    }

    #[test]
    fn the_git_top_level_is_searched_from_a_nested_record() {
        let root = workspace(&[(".git/HEAD", ""), (".prettierrc", "{}")]);
        let record = root.path().join("docs/PROVENANCE.yaml");
        assert!(
            notice(&record)
                .unwrap()
                .contains("does not list PROVENANCE.yaml")
        );
        for listed in [
            "docs/PROVENANCE.yaml",
            "/docs/PROVENANCE.yaml",
            "**/PROVENANCE.yaml",
            "PROVENANCE.yaml",
            "*.yaml",
            "docs/*",
            "docs/",
            "/docs/",
            "docs",
            "docs/**",
            "**/docs/**/*.yaml",
            "\\!x\nPROVENANCE.yaml",
        ] {
            fs::write(root.path().join(".prettierignore"), format!("{listed}\n")).unwrap();
            assert_eq!(notice(&record), None, "{listed}");
        }
        for unlisted in [
            "/PROVENANCE.yaml",
            "!PROVENANCE.yaml",
            "# PROVENANCE.yaml",
            "*.yml",
            "docs/PROVENANCE.yaml/",
        ] {
            fs::write(root.path().join(".prettierignore"), format!("{unlisted}\n")).unwrap();
            assert!(notice(&record).is_some(), "{unlisted}");
        }
    }

    #[test]
    fn a_later_negation_re_includes_the_record_unless_its_directory_is_excluded() {
        let root = workspace(&[(".git/HEAD", ""), (".prettierrc", "{}")]);
        let record = root.path().join("GROUNDING.yaml");
        let nested = root.path().join("docs/GROUNDING.yaml");
        let ignore = |text: &str| fs::write(root.path().join(".prettierignore"), text).unwrap();
        ignore("*.yaml\n!GROUNDING.yaml\n");
        assert!(notice(&record).is_some());
        assert!(notice(&nested).is_some());
        ignore("!GROUNDING.yaml\n*.yaml\n");
        assert_eq!(notice(&record), None);
        ignore("docs/\n!docs/GROUNDING.yaml\n");
        assert_eq!(notice(&nested), None);
    }

    #[test]
    fn an_anchored_directory_excludes_only_its_own_path() {
        let root = workspace(&[(".git/HEAD", ""), (".prettierrc", "{}")]);
        fs::write(root.path().join(".prettierignore"), "/docs/\n").unwrap();
        assert_eq!(notice(&root.path().join("docs/GROUNDING.yaml")), None);
        assert!(notice(&root.path().join("other/docs/GROUNDING.yaml")).is_some());
        fs::write(root.path().join(".prettierignore"), "docs/\n").unwrap();
        assert_eq!(notice(&root.path().join("other/docs/GROUNDING.yaml")), None);
    }

    #[test]
    fn each_formatter_root_needs_its_own_ignore_file() {
        let root = workspace(&[
            (".git/HEAD", ""),
            (
                "package.json",
                r#"{"lint-staged": {"*": "prettier --write"}}"#,
            ),
            ("notes/.prettierignore", "GROUNDING.yaml\n"),
        ]);
        let record = root.path().join("notes/GROUNDING.yaml");
        assert!(
            notice(&record)
                .unwrap()
                .starts_with("formatter: the lint-staged key in package.json is present")
        );
        fs::write(
            root.path().join(".prettierignore"),
            "notes/GROUNDING.yaml\n",
        )
        .unwrap();
        assert_eq!(notice(&record), None);
        fs::write(root.path().join("notes/.prettierrc"), "{}").unwrap();
        fs::remove_file(root.path().join("notes/.prettierignore")).unwrap();
        assert!(
            notice(&record)
                .unwrap()
                .starts_with("formatter: .prettierrc is present")
        );
    }

    #[test]
    fn glob_stays_within_a_segment() {
        assert!(glob("*.yaml", "GROUNDING.yaml"));
        assert!(glob("GROUNDING.y?ml", "GROUNDING.yaml"));
        assert!(!glob("GROUNDING.yaml", "GROUNDING.yaml.bak"));
        assert!(segments(&["docs", "*.yaml"], &["docs", "GROUNDING.yaml"]));
        assert!(!segments(&["*.yaml"], &["docs", "GROUNDING.yaml"]));
        assert!(segments(&["**", "*.yaml"], &["a", "b", "GROUNDING.yaml"]));
    }
}
