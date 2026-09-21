//! Public CLI adapter for durable asynchronous ingestion.
use crate::{Result, ingestion_orchestration as O};
use clap::{Args, Subcommand};
use serde_json::{Value as J, json};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Debug, Args)]
pub struct CommandOptions {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Clone, Debug, Subcommand)]
pub enum Command {
    Capture(CaptureOptions),
    Process(ProcessOptions),
    Status(StatusOptions),
    Pending(PendingOptions),
    Acknowledge(AcknowledgeOptions),
    #[command(name = "wait-delivery")]
    WaitDelivery(WaitDeliveryOptions),
    #[command(name = "complete-delivery")]
    CompleteDelivery(CompleteDeliveryOptions),
    #[command(name = "_worker", hide = true)]
    Worker(WorkerOptions),
}

#[derive(Clone, Debug, Args)]
pub struct CaptureOptions {
    #[arg(long)]
    pub file: String,
    #[arg(long)]
    pub record: Option<PathBuf>,
    #[arg(long)]
    pub state_dir: Option<PathBuf>,
    #[arg(long)]
    pub no_start: bool,
    #[arg(long)]
    pub notify_task: Option<String>,
}
#[derive(Clone, Debug, Args)]
pub struct ProcessOptions {
    #[arg(long)]
    pub record: Option<PathBuf>,
    #[arg(long)]
    pub state_dir: Option<PathBuf>,
    #[arg(long)]
    pub event_id: Option<String>,
    #[arg(long, default_value_t = 32)]
    pub max_events: usize,
}
#[derive(Clone, Debug, Args)]
pub struct StatusOptions {
    #[arg(long)]
    pub record: Option<PathBuf>,
    #[arg(long)]
    pub state_dir: Option<PathBuf>,
    #[arg(long)]
    pub event_id: Option<String>,
}
#[derive(Clone, Debug, Args)]
pub struct PendingOptions {
    #[arg(long)]
    pub record: Option<PathBuf>,
    #[arg(long)]
    pub state_dir: Option<PathBuf>,
    #[arg(long)]
    pub include_handled: bool,
}
#[derive(Clone, Debug, Args)]
pub struct AcknowledgeOptions {
    pub signal_ids: Vec<String>,
    #[arg(long)]
    pub record: Option<PathBuf>,
    #[arg(long)]
    pub state_dir: Option<PathBuf>,
}
#[derive(Clone, Debug, Args)]
pub struct WaitDeliveryOptions {
    pub job_id: String,
    #[arg(long)]
    pub record: Option<PathBuf>,
    #[arg(long)]
    pub state_dir: Option<PathBuf>,
    #[arg(long, default_value_t = 60.0)]
    pub timeout: f64,
}
#[derive(Clone, Debug, Args)]
pub struct CompleteDeliveryOptions {
    pub job_id: String,
    #[arg(long)]
    pub claim_token: String,
    #[arg(long)]
    pub outcome: String,
    #[arg(long)]
    pub record: Option<PathBuf>,
    #[arg(long)]
    pub state_dir: Option<PathBuf>,
}
#[derive(Clone, Debug, Args)]
pub struct WorkerOptions {
    #[arg(long)]
    pub record: PathBuf,
    #[arg(long)]
    pub state_dir: PathBuf,
    #[arg(long)]
    pub lease_token: String,
}

#[derive(Debug)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

fn rendered(value: &J) -> Result<String> {
    Ok(format!("{}\n", serde_json::to_string(value)?))
}
fn execute(options: &CommandOptions, cwd: &Path, stdin: Option<&[u8]>) -> Result<Output> {
    let value = match &options.command {
        Command::Capture(options) => {
            if let Some(recipient) = &options.notify_task {
                let host = std::env::var("CODEX_SESSION_ID")
                    .ok()
                    .or_else(|| std::env::var("CODEX_THREAD_ID").ok());
                crate::require(
                    host.as_deref() == Some(recipient.as_str()),
                    "--notify-task must match this host's CODEX_SESSION_ID (or CODEX_THREAD_ID)",
                )?;
            }
            let raw = if options.file == "-" {
                stdin
                    .ok_or_else(|| crate::Error("missing standard input".into()))?
                    .to_vec()
            } else {
                std::fs::read(cwd.join(&options.file))?
            };
            if let Some(recipient) = &options.notify_task {
                crate::ingestion_delivery::capture(
                    &raw,
                    recipient,
                    options.record.as_deref(),
                    options.state_dir.as_deref(),
                    cwd,
                    !options.no_start,
                )?
            } else {
                O::capture(
                    &raw,
                    options.record.as_deref(),
                    options.state_dir.as_deref(),
                    cwd,
                    !options.no_start,
                )?
            }
        }
        Command::Process(options) => J::Array(O::process(
            options.record.as_deref(),
            options.state_dir.as_deref(),
            cwd,
            options.event_id.as_deref(),
            options.max_events,
        )?),
        Command::Status(options) => O::status(
            options.event_id.as_deref(),
            options.record.as_deref(),
            options.state_dir.as_deref(),
            cwd,
        )?
        .unwrap_or(J::Null),
        Command::Pending(options) => J::Array(O::pending(
            options.record.as_deref(),
            options.state_dir.as_deref(),
            cwd,
            options.include_handled,
        )?),
        Command::Acknowledge(options) => O::acknowledge(
            &options.signal_ids,
            options.record.as_deref(),
            options.state_dir.as_deref(),
            cwd,
        )?,
        Command::WaitDelivery(options) => {
            crate::require(
                options.timeout.is_finite() && options.timeout >= 0.0 && options.timeout <= 3600.0,
                "timeout must be between 0 and 3600 seconds",
            )?;
            crate::ingestion_delivery::wait(
                &options.job_id,
                options.record.as_deref(),
                options.state_dir.as_deref(),
                cwd,
                Duration::from_secs_f64(options.timeout),
            )?
        }
        Command::CompleteDelivery(options) => crate::ingestion_delivery::complete(
            &options.job_id,
            &options.claim_token,
            &options.outcome,
            options.record.as_deref(),
            options.state_dir.as_deref(),
            cwd,
        )?,
        Command::Worker(options) => {
            O::worker(
                &options.record,
                &options.state_dir,
                cwd,
                &options.lease_token,
            )?;
            return Ok(Output {
                stdout: String::new(),
                stderr: String::new(),
                code: 0,
            });
        }
    };
    Ok(Output {
        stdout: rendered(&value)?,
        stderr: String::new(),
        code: 0,
    })
}

pub fn dispatch(options: &CommandOptions, cwd: &Path, stdin: Option<&[u8]>) -> Output {
    execute(options, cwd, stdin).unwrap_or_else(|error| Output {
        stdout: format!("{}\n", json!({"error":error.to_string()})),
        stderr: String::new(),
        code: 2,
    })
}

pub fn wait(options: &StatusOptions, cwd: &Path, timeout: Duration) -> Result<Option<J>> {
    let id = options
        .event_id
        .as_deref()
        .ok_or_else(|| crate::Error("--event-id is required".into()))?;
    O::wait_for_terminal(
        options.record.as_deref(),
        options.state_dir.as_deref(),
        cwd,
        id,
        timeout,
    )
}
