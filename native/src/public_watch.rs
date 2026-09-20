//! Native watch CLI. Host delivery is request/receipt based, never a tool call.
use crate::{Result, require, watch_store::Watch};
use serde_json::Value;
use std::path::{Path, PathBuf};
#[derive(Clone, Debug, clap::Args)]
pub struct Args {
    #[command(subcommand)]
    pub command: Command,
}
#[derive(Clone, Debug, clap::Subcommand)]
pub enum Command {
    Setup {
        #[arg(long)]
        base_ref: Option<String>,
        #[arg(long)]
        shared_record: Option<PathBuf>,
        #[arg(long)]
        shared_private: bool,
    },
    Status,
    Pause,
    Scan {
        #[arg(long)]
        all: bool,
        #[arg(long)]
        notify_task: Option<String>,
    },
    Share {
        #[arg(long)]
        file: String,
        #[arg(long)]
        notify_task: Option<String>,
    },
    Shared,
    Resolve {
        event_id: String,
        #[arg(long)]
        evidence: String,
    },
    WaitDelivery {
        job_id: String,
        #[arg(long, default_value_t = 100.0, allow_hyphen_values = true)]
        timeout: f64,
    },
    CompleteDelivery {
        job_id: String,
        #[arg(long)]
        token: String,
        #[arg(long,value_parser=["sent","failed","unknown"])]
        outcome: String,
    },
    #[command(name = "_process", hide = true)]
    Process,
}
pub fn run(args: &Args, workspace: &Path) -> Result<Value> {
    #[cfg(unix)]
    if matches!(&args.command, Command::Process)
        && std::env::var("KPOPPER_WATCH_DETACH").as_deref() == Ok("1")
    {
        nix::unistd::setsid().map_err(|error| crate::Error(error.to_string()))?;
    }
    let watch = Watch::open(workspace)?;
    let notify = match &args.command {
        Command::Scan { notify_task, .. } | Command::Share { notify_task, .. } => {
            notify_task.as_deref()
        }
        _ => None,
    };
    if let Some(recipient) = notify {
        let host = std::env::var("CODEX_SESSION_ID")
            .ok()
            .filter(|v| !v.is_empty())
            .or_else(|| std::env::var("CODEX_THREAD_ID").ok());
        require(
            host.as_deref() == Some(recipient),
            "--notify-task must match the current host task identity",
        )?;
        require(
            watch.enabled()?,
            "enable watch before reserving native delivery",
        )?;
    }
    let mut result = match &args.command {
        Command::Setup {
            base_ref,
            shared_record,
            shared_private,
        } => watch.setup(
            base_ref.as_deref(),
            shared_record.as_deref(),
            *shared_private,
        )?,
        Command::Status => watch.status()?,
        Command::Pause => watch.pause()?,
        Command::Scan { all, .. } => {
            if *all {
                watch.request_all()?
            } else {
                watch.request()?
            }
        }
        Command::Share { file, .. } => {
            crate::watch_shared::capture(&watch, crate::watch_shared::load_report(file)?)?
        }
        Command::Shared => crate::watch_shared::read(&watch)?,
        Command::Resolve { event_id, evidence } => {
            crate::watch_shared::resolve(&watch, event_id, evidence)?
        }
        Command::WaitDelivery { job_id, timeout } => {
            crate::watch_delivery::wait(&watch, job_id, *timeout)?
        }
        Command::CompleteDelivery {
            job_id,
            token,
            outcome,
        } => crate::watch_delivery::complete(&watch, job_id, token, outcome)?,
        Command::Process => watch.process()?,
    };
    if let Some(recipient) = notify {
        result["delivery_job"] = crate::watch_delivery::reserve(&watch, recipient)?;
    }
    Ok(result)
}
