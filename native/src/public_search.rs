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
    search_corpus::corpus(options, cwd, mode)
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
    let data = corpus(options, cwd, mode)?;
    if let Some(reference) = options.reference.as_deref().filter(|r| !r.is_empty()) {
        Ok(crate::public_search_rank::read(
            &data,
            reference,
            options.revision.as_deref().unwrap(),
            options.offset,
            options.length,
        )?)
    } else {
        Ok(crate::public_search_rank::search(
            data,
            options.query.as_deref().unwrap_or(""),
            options.limit,
            options.chars,
        )?)
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
