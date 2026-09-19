//! Explicit optional Hub lifecycle; assessment is captured once before presentation.
use crate::{
    Error, Result, core_html, core_page,
    history_contract::*,
    public_core_readers::Output,
    public_workspace as W,
    reasoning_context::CapturedAssessment,
    reasoning_runtime::OperationalBounds,
    require,
    source_capture::{self, ReadMode},
    source_inventory::{Inventory, absolute},
};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Debug, Default, clap::Args)]
pub struct Options {
    pub files: Vec<PathBuf>,
    #[arg(long)]
    pub out: Option<PathBuf>,
    #[arg(long)]
    pub brief: Option<PathBuf>,
    #[arg(long, value_parser=["core/v1"])]
    pub profile: Option<String>,
    /// Verify the page projection without writing HTML.
    #[arg(long, conflicts_with = "checks")]
    pub verify: bool,
    /// Run the bundled browser acceptance checks on an existing HTML page.
    #[arg(long)]
    pub checks: bool,
    #[arg(long, conflicts_with_all=["verify", "checks"])]
    pub open: bool,
    #[arg(long)]
    pub tree: bool,
}

const MAX_HTML: usize = 32 * 1024 * 1024;

fn destination(options: &Options, cwd: &Path, first: &Path) -> Result<PathBuf> {
    absolute(&match &options.out {
        Some(path) => cwd.join(path),
        None if first.file_name().is_some_and(|s| s == "GROUNDING.yaml") => {
            first.parent().unwrap().join(".kpopper/build/page.html")
        }
        None => cwd.join("record.html"),
    })
}

fn verify_core(page: &serde_json::Value) -> Result<Output> {
    let values = &page["page_inputs"]["values"];
    let nodes = values["nodes"]
        .as_array()
        .ok_or_else(|| error("Hub projection has no node array"))?;
    let judgments = nodes.iter().filter(|n| n["kind"] == "judgment").count();
    let coverage = &values["coverage"];
    let mut failures = vec![];
    for (key, prefix) in [
        ("unresolved_selectors", "core page selectors unresolved: "),
        ("renderer_misfits", "core page renderer does not fit: "),
        ("stale_shapes", "core page shape moved: "),
    ] {
        let entries = coverage[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>();
        if key == "unresolved_selectors" && !entries.is_empty() {
            failures.push(format!("{prefix}{}", entries.join(", ")));
        } else {
            failures.extend(entries.iter().map(|s| format!("{prefix}{s}")));
        }
    }
    let mut text =
        "NOTE core/v1 generic page seam: arrangement changes only page-secondary/v1\n".to_string();
    for failure in &failures {
        text.push_str(&format!("FAIL {failure}\n"));
    }
    text.push_str(&format!(
        "{} elements, {} entries, {} judgments, {} tabs, {} problems\n",
        nodes.len(),
        nodes.len() - judgments,
        judgments,
        values["arrangements"]
            .as_array()
            .ok_or_else(|| error("Hub projection has no arrangement array"))?
            .len()
            + 1,
        failures.len()
    ));
    Ok(Output {
        text,
        code: i32::from(!failures.is_empty()),
    })
}

fn output_path(path: &Path, protected: &[PathBuf]) -> Result<PathBuf> {
    require(
        path.extension().is_some_and(|s| s == "html" || s == "htm"),
        "Hub output must use an .html or .htm extension",
    )?;
    // Replacing a leaf link would make its rendered relative links disagree with
    // the path actually opened. Parent links are resolved once and revalidated.
    if let Ok(meta) = fs::symlink_metadata(path) {
        require(
            meta.is_file() && !meta.file_type().is_symlink(),
            "Hub output must be a regular file",
        )?;
    }
    let resolved = crate::project_modes::resolved(path)?;
    for source in protected {
        require(
            crate::project_modes::resolved(source)? != resolved,
            "Hub output would overwrite a captured input",
        )?;
    }
    require(
        !resolved.components().any(|c| c.as_os_str() == ".git"),
        "Hub output cannot be written inside Git metadata",
    )?;
    Ok(resolved)
}

fn browser_checks(page: &Path) -> Result<Output> {
    require(
        page.is_file(),
        "The Hub page is unavailable; build it before --checks",
    )?;
    let mut script = tempfile::Builder::new().suffix(".cjs").tempfile()?;
    script.write_all(include_bytes!("../../scripts/verify_page.js"))?;
    let result = Command::new("node")
        .arg(script.path())
        .arg(page)
        .output()
        .map_err(|e| {
            Error(format!(
                "Hub browser checks require Node and playwright-core: {e}"
            ))
        })?;
    let mut text = String::from_utf8_lossy(&result.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&result.stderr));
    Ok(Output {
        text,
        code: result.status.code().unwrap_or(1),
    })
}

fn declared_source_paths(document: &crate::value::TypedValue, base: &Path) -> Result<Vec<PathBuf>> {
    use crate::value::TypedValue;
    fn collect(
        value: &TypedValue,
        base: &Path,
        paths: &mut Vec<PathBuf>,
        declarations: &mut Vec<String>,
    ) {
        match value {
            TypedValue::Map(fields) => {
                for field in ["file", "url"] {
                    if let Some(TypedValue::Text(value)) = fields.get(field) {
                        if field == "file" && !value.is_empty() {
                            paths.push(base.join(value));
                            paths.push(base.join(value.trim()));
                        }
                        if field == "url" || value.to_ascii_lowercase().starts_with("file:") {
                            declarations.push(value.trim().to_owned());
                        }
                    }
                }
                for value in fields.values() {
                    collect(value, base, paths, declarations);
                }
            }
            TypedValue::List(values) => {
                for value in values {
                    collect(value, base, paths, declarations);
                }
            }
            _ => {}
        }
    }
    let mut paths = Vec::new();
    let mut declarations = Vec::new();
    for (section, members) in map(document)? {
        if matches!(section.as_str(), "meta" | "schema") {
            continue;
        }
        collect(members, base, &mut paths, &mut declarations);
    }
    for href in declarations {
        if href.is_empty() {
            continue;
        }
        let href = if href
            .get(..5)
            .is_some_and(|s| s.eq_ignore_ascii_case("file:"))
        {
            "file:".to_string() + &href[5..]
        } else {
            href
        };
        let href = if let Some(file) = href.strip_prefix("file://localhost/") {
            format!("/{file}")
        } else if let Some(file) = href.strip_prefix("file://") {
            // A remote file authority is not a local output path.
            if !file.starts_with('/') {
                continue;
            }
            file.to_string()
        } else if let Some(file) = href.strip_prefix("file:") {
            file.to_string()
        } else if href.split('/').next().is_some_and(|p| p.contains(':')) {
            continue;
        } else {
            href
        };
        let raw = href.split(['?', '#']).next().unwrap_or("").as_bytes();
        let mut decoded = Vec::with_capacity(raw.len());
        let mut i = 0;
        while i < raw.len() {
            if raw[i] == b'%' && i + 2 < raw.len() {
                let hex = std::str::from_utf8(&raw[i + 1..i + 3])
                    .ok()
                    .and_then(|s| u8::from_str_radix(s, 16).ok());
                if let Some(byte) = hex {
                    decoded.push(byte);
                    i += 3;
                    continue;
                }
            }
            decoded.push(raw[i]);
            i += 1;
        }
        let Ok(local) = String::from_utf8(decoded) else {
            continue;
        };
        paths.push(base.join(local));
    }
    Ok(paths)
}

fn open_page(path: &Path, tree: bool) -> Result<()> {
    let path = path
        .to_str()
        .ok_or_else(|| error("invalid Hub output URL"))?;
    #[cfg(target_os = "windows")]
    let path = path.replace('\\', "/");
    let mut encoded = String::new();
    for b in path.as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~:".contains(b) {
            encoded.push(*b as char);
        } else {
            encoded.push_str(&format!("%{b:02X}"));
        }
    }
    let uri = if encoded.starts_with("//") {
        format!("file:{encoded}")
    } else if encoded.starts_with('/') {
        format!("file://{encoded}")
    } else {
        format!("file:///{encoded}")
    };
    let target = format!("{uri}{}", if tree { "#tree" } else { "" });
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = Command::new("explorer.exe");
        c.arg(&target);
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = Command::new("xdg-open");
    #[cfg(not(target_os = "windows"))]
    command.arg(&target);
    require(
        command.status()?.success(),
        "The Hub page was written but the browser could not open it",
    )
}

pub fn run(options: &Options, cwd: &Path, mode: ReadMode) -> Result<Output> {
    let cwd = cwd.canonicalize()?;
    if options.checks {
        let pages = options
            .files
            .iter()
            .filter(|p| p.extension().is_some_and(|s| s == "html" || s == "htm"))
            .collect::<Vec<_>>();
        require(pages.len() <= 1, "--checks accepts one HTML page")?;
        let page = if let Some(page) = pages.first() {
            absolute(&cwd.join(page))?
        } else {
            destination(options, &cwd, &W::records(&cwd)?[0])?
        };
        return browser_checks(&page);
    }
    let paths = if options.files.is_empty() {
        W::records(&cwd)?
    } else {
        let mut paths = vec![];
        for path in &options.files {
            let pattern = cwd.join(path);
            let found = crate::source_inventory::glob(&pattern)?;
            if found.is_empty() {
                paths.push(absolute(&pattern)?);
            } else {
                paths.extend(found);
            }
        }
        paths
    };
    let first = paths.first().ok_or_else(|| error("record_required"))?;
    let runtime = W::runtime_for_paths(&paths, &cwd, options.profile.as_deref())?;
    let captured =
        source_capture::capture_source_with_runtime(&paths, &cwd, mode, None, runtime.as_ref())?;
    let capabilities =
        crate::reasoning_fields::capabilities(&captured.document(), options.profile.as_deref())?;
    let core = string_is(&map(&capabilities)?["profile"], "core/v1");
    let brief = match &options.brief {
        Some(path) => absolute(&cwd.join(path))?,
        None => first.parent().unwrap().join(
            crate::history_transaction::Layout::for_entry(
                first
                    .file_name()
                    .and_then(|s| s.to_str())
                    .ok_or_else(|| error("invalid_path"))?,
            )?
            .view,
        ),
    };
    let mut inventory = Inventory::default();
    let content = if inventory.exists(&brief)? {
        Some(inventory.read(&brief)?)
    } else {
        None
    };
    if options.verify {
        let output = if core {
            let context = CapturedAssessment::from_snapshot(
                captured.snapshot().clone(),
                None,
                "focused-review/v1",
                runtime.as_ref(),
                OperationalBounds::default(),
                None,
            )?;
            let projection = core_page::project(&context, content.as_deref(), true)?;
            let page = &projection["page_assessment"];
            core_html::render(page, MAX_HTML).map_err(Error)?;
            verify_core(page)?
        } else {
            let report = crate::ordinary_assessment_report::from_capture(
                &captured,
                runtime.as_ref(),
                crate::ordinary_assessment::POLICY,
            )?;
            let destination = destination(options, &cwd, first)?;
            let page = crate::ordinary_hub::build(
                &captured,
                &report,
                content.as_deref(),
                first,
                &destination,
                MAX_HTML,
                runtime.as_ref(),
            )?;
            let mut text = String::new();
            for note in &page.notes {
                text.push_str(&format!("NOTE {note}\n"));
            }
            for failure in &page.failures {
                text.push_str(&format!("FAIL {failure}\n"));
            }
            text.push_str(&format!(
                "{} elements, {} entries, {} judgments, {} tabs, {} problems\n",
                page.elements,
                page.entries,
                page.judgments,
                page.tabs,
                page.failures.len()
            ));
            Output {
                text,
                code: i32::from(!page.failures.is_empty()),
            }
        };
        captured.verify()?;
        inventory.verify()?;
        return Ok(output);
    }
    let destination = destination(options, &cwd, first)?;
    let mut protected = captured.files().keys().cloned().collect::<Vec<_>>();
    protected.extend(declared_source_paths(
        &captured.document(),
        first.parent().unwrap(),
    )?);
    for hypothesis in map(captured.hypotheses())?.values() {
        let hypothesis = map(hypothesis)?;
        if let Some(document) = hypothesis.get("doc").or_else(|| hypothesis.get("document")) {
            protected.extend(declared_source_paths(document, first.parent().unwrap())?);
        }
    }
    protected.push(brief);
    let resolved = output_path(&destination, &protected)?;
    let html = if core {
        let context = CapturedAssessment::from_snapshot(
            captured.snapshot().clone(),
            None,
            "focused-review/v1",
            runtime.as_ref(),
            OperationalBounds::default(),
            None,
        )?;
        let projection =
            core_page::project_for_page(&context, content.as_deref(), first, &resolved)?;
        core_html::render(&projection["page_assessment"], MAX_HTML).map_err(Error)?
    } else {
        let report = crate::ordinary_assessment_report::from_capture(
            &captured,
            runtime.as_ref(),
            crate::ordinary_assessment::POLICY,
        )?;
        crate::ordinary_hub::build(
            &captured,
            &report,
            content.as_deref(),
            first,
            &resolved,
            MAX_HTML,
            runtime.as_ref(),
        )?
        .html
    };
    captured.verify()?;
    inventory.verify()?;
    let parent = resolved
        .parent()
        .ok_or_else(|| error("invalid Hub output parent"))?;
    fs::create_dir_all(parent)?;
    require(
        output_path(&destination, &protected)? == resolved,
        "Hub output path changed; retry",
    )?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(html.as_bytes())?;
    temporary.as_file().sync_all()?;
    captured.verify()?;
    inventory.verify()?;
    temporary
        .persist(&resolved)
        .map_err(|e| Error(e.error.to_string()))?;
    if options.out.is_none() && first.file_name().is_some_and(|s| s == "GROUNDING.yaml") {
        use std::fs::OpenOptions;
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(parent.join(".gitignore"))
        {
            Ok(mut file) => file.write_all(b"*\n")?,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
    }
    if options.open {
        open_page(&resolved, options.tree)?;
    }
    Ok(Output {
        text: String::new(),
        code: 0,
    })
}
