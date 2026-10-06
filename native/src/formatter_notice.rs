//! A pre-commit formatter can reflow the record file on commit. A rewrite of a
//! history-backed record then reads as an unresolved hand edit, so `check` says
//! when the workspace configures one and its ignore file leaves the record exposed.
use std::path::{Path, PathBuf};

/// The note `check` prints for the record at `record`, if any: the record's
/// directory and the Git top level above it are searched for a formatter
/// configuration, and `.prettierignore` in either is searched for the record.
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
    let found = roots.iter().find_map(|root| formatter(root))?;
    if roots.iter().any(|root| ignored(root, record, name)) {
        return None;
    }
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

/// Whether `.prettierignore` in `root` lists the record, by its file name or by
/// its path from `root`. Negated and commented lines list nothing.
fn ignored(root: &Path, record: &Path, name: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(root.join(".prettierignore")) else {
        return false;
    };
    let relative = record
        .strip_prefix(root)
        .ok()
        .and_then(Path::to_str)
        .map(|path| path.replace('\\', "/"));
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with('!'))
        .any(|line| {
            let anchored = line.starts_with('/');
            let pattern = line.trim_start_matches('/');
            let pattern = pattern.strip_prefix("**/").unwrap_or(pattern);
            if let Some(folder) = pattern.strip_suffix('/') {
                return relative.as_deref().is_some_and(|path| {
                    path.split('/')
                        .rev()
                        .skip(1)
                        .any(|segment| glob(folder, segment))
                });
            }
            (!anchored || relative.as_deref() == Some(name)) && glob(pattern, name)
                || relative.as_deref().is_some_and(|path| glob(pattern, path))
        })
}

/// `*` and `?` within one path segment; everything else is literal.
fn glob(pattern: &str, text: &str) -> bool {
    let (p, t) = (pattern.as_bytes(), text.as_bytes());
    let (mut i, mut j, mut star, mut mark) = (0, 0, None, 0);
    while j < t.len() {
        if i < p.len() && (p[i] == b'?' && t[j] != b'/' || p[i] == t[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == b'*' {
            star = Some(i);
            mark = j;
            i += 1;
        } else if let Some(s) = star
            && t[mark] != b'/'
        {
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
            "**/PROVENANCE.yaml",
            "*.yaml",
            "docs/*",
            "docs/",
        ] {
            fs::write(root.path().join(".prettierignore"), format!("{listed}\n")).unwrap();
            assert_eq!(notice(&record), None, "{listed}");
        }
        for unlisted in [
            "/PROVENANCE.yaml",
            "!PROVENANCE.yaml",
            "# PROVENANCE.yaml",
            "*.yml",
        ] {
            fs::write(root.path().join(".prettierignore"), format!("{unlisted}\n")).unwrap();
            assert!(notice(&record).is_some(), "{unlisted}");
        }
    }

    #[test]
    fn glob_stays_within_a_segment() {
        assert!(glob("*.yaml", "GROUNDING.yaml"));
        assert!(glob("GROUNDING.y?ml", "GROUNDING.yaml"));
        assert!(!glob("*.yaml", "docs/GROUNDING.yaml"));
        assert!(glob("docs/*.yaml", "docs/GROUNDING.yaml"));
        assert!(!glob("GROUNDING.yaml", "GROUNDING.yaml.bak"));
    }
}
