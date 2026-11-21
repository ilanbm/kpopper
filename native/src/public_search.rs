//! Bounded local evidence discovery over captured records, reports and sources.
#[path = "search_captures.rs"]
mod search_captures;
#[path = "search_corpus.rs"]
mod search_corpus;
#[path = "search_ordinary.rs"]
mod search_ordinary;
#[path = "search_yaml_identity.rs"]
mod search_yaml_identity;
use crate::source_capture::ReadMode;
use serde_json::{Value as J, json};
use std::path::{Path, PathBuf};
#[derive(Clone, Debug, clap::Args)]
pub struct Options {
    #[arg(skip)]
    pub no_cache: bool,
    #[arg(allow_negative_numbers = true)]
    pub query: Option<String>,
    #[arg(long)]
    pub record: Option<PathBuf>,
    #[arg(long)]
    pub state_dir: Option<PathBuf>,
    #[arg(long = "source-root")]
    pub source_roots: Vec<PathBuf>,
    #[arg(long, default_value_t = 5, allow_negative_numbers = true)]
    pub limit: i64,
    #[arg(long, default_value_t = 6000, allow_negative_numbers = true)]
    pub chars: i64,
    #[arg(long = "read")]
    pub reference: Option<String>,
    #[arg(long)]
    pub revision: Option<String>,
    #[arg(long, default_value_t = 0, allow_negative_numbers = true)]
    pub offset: i64,
    #[arg(long, default_value_t = 4000, allow_negative_numbers = true)]
    pub length: i64,
    #[arg(long,value_parser=["core/v1"])]
    pub profile: Option<String>,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            no_cache: false,
            query: None,
            record: None,
            state_dir: None,
            source_roots: vec![],
            limit: 5,
            chars: 6000,
            reference: None,
            revision: None,
            offset: 0,
            length: 4000,
            profile: None,
        }
    }
}
#[derive(Debug)]
pub struct Failure {
    pub message: String,
    pub capture_failure: Option<J>,
}
impl From<crate::Error> for Failure {
    fn from(e: crate::Error) -> Self {
        Self {
            message: e.0,
            capture_failure: None,
        }
    }
}
impl From<std::io::Error> for Failure {
    fn from(e: std::io::Error) -> Self {
        crate::Error(e.to_string()).into()
    }
}
impl From<serde_json::Error> for Failure {
    fn from(e: serde_json::Error) -> Self {
        crate::Error(e.to_string()).into()
    }
}
type Result<T> = std::result::Result<T, Failure>;
impl Failure {
    fn captured(e: crate::Error, stage: &str) -> Self {
        let envelope =
            crate::reasoning_context::capture_failure(&e.0, None, stage).and_then(|v| v.to_json());
        match envelope {
            Ok(envelope) => Self {
                message: format!("{}: {}", envelope["code"].as_str().unwrap(), e.0),
                capture_failure: Some(envelope),
            },
            Err(other) => other.into(),
        }
    }
}
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}
pub fn corpus(options: &Options, cwd: &Path, mode: ReadMode) -> Result<J> {
    let record = search_corpus::record(options, cwd)?;
    search_corpus::corpus(&record, options, cwd, mode, None)
}

thread_local! {
    static WRITE_INDEX: std::cell::RefCell<(usize,std::collections::BTreeSet<PathBuf>)> = Default::default();
    static DISABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[doc(hidden)]
pub fn disable_write_cache(disabled: bool) {
    DISABLED.set(disabled);
}

/// Enclose publication, never its locks. Nested paths join the outer operation.
pub(crate) struct WriteIndex<'a>(&'a Path);
pub(crate) fn writing(cwd: &Path) -> WriteIndex<'_> {
    WRITE_INDEX.with_borrow_mut(|state| state.0 += 1);
    WriteIndex(cwd)
}

pub(crate) fn published(root: &Path, files: &[crate::history_transaction::FileImage]) {
    if DISABLED.get() {
        return;
    }
    WRITE_INDEX.with_borrow_mut(|state| {
        if state.0 != 0 {
            for file in files {
                state.1.insert(root.join(&file.path));
            }
        }
    });
}
impl Drop for WriteIndex<'_> {
    fn drop(&mut self) {
        let mut paths = WRITE_INDEX.with_borrow_mut(|state| {
            state.0 -= 1;
            if state.0 == 0 {
                std::mem::take(&mut state.1)
            } else {
                Default::default()
            }
        });
        if paths.is_empty() {
            return;
        }
        if let Ok(default) = crate::public_workspace::records(self.0) {
            paths.extend(default);
        }
        for path in paths {
            refresh(&path, self.0);
        }
    }
}
fn validate(options: &Options) -> Result<()> {
    if options.reference.as_deref().is_some_and(|r| !r.is_empty()) {
        crate::require(
            options.query.as_deref().is_none_or(str::is_empty)
                && options.revision.as_deref().is_some_and(|r| !r.is_empty()),
            "--read requires --revision and no query",
        )?;
    } else {
        crate::require(
            options.revision.as_deref().is_none_or(str::is_empty) && options.offset == 0,
            "--revision and --offset require --read",
        )?;
        let query = options.query.as_deref().unwrap_or("");
        crate::require(
            !query.trim().is_empty() && query.chars().count() <= 2000,
            "query must contain 1..2000 characters",
        )?;
        crate::require(
            (1..=50).contains(&options.limit) && (500..=100000).contains(&options.chars),
            "limit must be 1..50; chars must be 500..100000",
        )?;
    }
    Ok(())
}
fn run_validated(options: &Options, cwd: &Path, mode: ReadMode) -> Result<J> {
    let record = search_corpus::record(options, cwd)?;
    let mut index = (!options.no_cache
        && mode == ReadMode::Live
        && options.source_roots.is_empty()
        && options.state_dir.is_none()
        && options.profile.is_none())
    .then(|| {
        crate::project_modes::write_paths(std::slice::from_ref(&record), cwd)
            .ok()
            .and_then(|paths| crate::search_index::Index::open(paths.first()?, true))
    })
    .flatten();
    let mut attempts = 0;
    let data = loop {
        match search_corpus::corpus(&record, options, cwd, mode, index.as_mut()) {
            Err(e)
                if attempts < 2
                    && matches!(
                        e.message.as_str(),
                        "snapshot_changed"
                            | "record changed during search; retry"
                            | "capture state changed during search; retry"
                            | "source changed during search; retry"
                    ) =>
            {
                attempts += 1;
                std::thread::sleep(std::time::Duration::from_millis(20 * attempts));
            }
            result => break result?,
        }
    };
    let hits = index.as_mut().and_then(|i| {
        i.synchronize(&data, options.query.as_deref())
            .ok()
            .flatten()
    });
    if let Some(reference) = options.reference.as_deref().filter(|r| !r.is_empty()) {
        Ok(crate::public_search_rank::read(
            &data,
            reference,
            options.revision.as_deref().unwrap(),
            options.offset,
            options.length,
        )?)
    } else {
        Ok(crate::public_search_rank::search_ranked(
            data,
            options.query.as_deref().unwrap_or(""),
            options.limit,
            options.chars,
            hits,
        )?)
    }
}

/// Refresh a previously searched default view after canonical publication.
/// Explicit grants and frozen reads never enter this shared default index.
pub(crate) fn refresh(record: &Path, cwd: &Path) {
    let options = Options {
        record: Some(record.to_owned()),
        ..Default::default()
    };
    for _ in 0..3 {
        let Some(mut index) = crate::search_index::Index::open(record, false) else {
            return;
        };
        match search_corpus::corpus(record, &options, cwd, ReadMode::Live, Some(&mut index)) {
            Ok(data) if matches!(index.synchronize(&data, None), Ok(None)) => continue,
            Err(e)
                if matches!(
                    e.message.as_str(),
                    "snapshot_changed"
                        | "record changed during search; retry"
                        | "capture state changed during search; retry"
                ) =>
            {
                continue;
            }
            _ => return,
        }
    }
}
pub fn run(options: &Options, cwd: &Path, mode: ReadMode) -> Result<J> {
    validate(options)?;
    run_validated(options, cwd, mode)
}
pub fn dispatch(options: &Options, cwd: &Path, mode: ReadMode, as_json: bool) -> Output {
    let result = validate(options).and_then(|_| {
        if mode == ReadMode::Live
            && std::env::var("KPOPPER_READ_MODE")
                .is_ok_and(|m| !["live", "frozen"].contains(&m.as_str()))
        {
            return Err(crate::Error("read mode must be live or frozen".into()).into());
        }
        run_validated(options, cwd, mode)
    });
    match result {
        Ok(result) => match crate::public_search_rank::encode(&result) {
            Ok(text) => Output {
                stdout: text + "\n",
                stderr: String::new(),
                code: 0,
            },
            Err(e) => Output {
                stdout: String::new(),
                stderr: e.0 + "\n",
                code: 2,
            },
        },
        Err(e) => {
            let payload = if let Some(failure) = &e.capture_failure {
                json!({"error":e.message,"capture_failure":failure})
            } else {
                json!({"error":e.message})
            };
            let text = if as_json || e.capture_failure.is_some() {
                crate::public_search_rank::encode(&payload).unwrap_or_else(|_| payload.to_string())
            } else {
                e.message
            };
            Output {
                stdout: String::new(),
                stderr: text + "\n",
                code: 2,
            }
        }
    }
}
