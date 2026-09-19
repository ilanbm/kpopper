//! CLI surface for the native local followup coordinator.
use crate::{
    Error, Result,
    followup_store::{MAX_BYTES, Store, parse_input},
};
use clap::Subcommand;
use serde_json::Value;
use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, clap::Args)]
pub struct Args {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Clone, Debug, Subcommand)]
pub enum Command {
    /// Choose an existing task destination or the private fallback.
    Setup {
        #[arg(long)]
        store: Option<PathBuf>,
        #[arg(long, default_value = "UTC")]
        timezone: String,
        #[arg(long)]
        record: Option<PathBuf>,
        #[arg(long)]
        private: bool,
    },
    Status,
    List,
    Show {
        id: String,
    },
    Add {
        #[arg(long)]
        file: String,
    },
    Observe {
        #[arg(long)]
        file: String,
    },
    Refresh {
        id: String,
        #[arg(long)]
        file: String,
        #[arg(long)]
        evidence: String,
    },
    /// Read-only readiness check; never executes a task.
    Scan {
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    Claim {
        id: String,
        #[arg(long)]
        occurrence: String,
        #[arg(long)]
        owner: String,
        #[arg(long)]
        daily_token: Option<String>,
    },
    Renew {
        id: String,
        #[arg(long)]
        token: String,
    },
    Release {
        id: String,
        #[arg(long)]
        token: String,
        #[arg(long)]
        evidence: String,
    },
    Finish {
        id: String,
        #[arg(long)]
        token: String,
        #[arg(long, value_parser = ["checked", "done", "cancelled", "needs_user"])]
        outcome: String,
        #[arg(long)]
        evidence: String,
        #[arg(long)]
        next_at: Option<String>,
    },
    Recover {
        id: String,
        #[arg(long)]
        evidence: String,
    },
    Resume {
        id: String,
        #[arg(long)]
        evidence: String,
    },
    Resolve {
        id: String,
        #[arg(long, value_parser = ["done", "cancelled"])]
        outcome: String,
        #[arg(long)]
        evidence: String,
    },
    /// Explicitly rebind a moved pinned record, preserving history.
    Relocate {
        #[arg(long)]
        record: PathBuf,
        #[arg(long)]
        evidence: String,
    },
    /// Restore an inspected backup and park uncertain unfinished work.
    Restore {
        #[arg(long)]
        backup: PathBuf,
        #[arg(long)]
        evidence: String,
    },
}

fn supplied(path: &str) -> Result<Value> {
    let raw = if path == "-" {
        let mut raw = Vec::new();
        io::stdin()
            .take((MAX_BYTES + 1) as u64)
            .read_to_end(&mut raw)?;
        raw
    } else {
        let metadata = fs::metadata(path)?;
        if metadata.len() > MAX_BYTES as u64 {
            return Err(Error(format!("File is too large: {path}")));
        }
        fs::read(path)?
    };
    parse_input(&raw)
}

pub fn run(args: &Args, workspace: &Path) -> Result<Value> {
    let store = Store::open(workspace)?;
    match &args.command {
        Command::Setup {
            store: destination,
            timezone,
            record,
            private,
        } => store.setup(
            destination.as_deref(),
            timezone,
            record.as_deref(),
            *private,
        ),
        Command::Status => store.status(),
        Command::List => store.list(),
        Command::Show { id } => store.show(id),
        Command::Add { file } => store.add(supplied(file)?),
        Command::Observe { file } => store.observe(supplied(file)?),
        Command::Refresh { id, file, evidence } => store.refresh(id, supplied(file)?, evidence),
        Command::Scan { limit } => store.scan(*limit),
        Command::Claim {
            id,
            occurrence,
            owner,
            daily_token,
        } => store.claim(id, occurrence, owner, daily_token.as_deref()),
        Command::Renew { id, token } => store.renew(id, token),
        Command::Release {
            id,
            token,
            evidence,
        } => store.finish(id, token, "released", evidence, None),
        Command::Finish {
            id,
            token,
            outcome,
            evidence,
            next_at,
        } => store.finish(id, token, outcome, evidence, next_at.as_deref()),
        Command::Recover { id, evidence } => store.recover(id, evidence),
        Command::Resume { id, evidence } => store.resume(id, evidence),
        Command::Resolve {
            id,
            outcome,
            evidence,
        } => store.resolve(id, outcome, evidence),
        Command::Relocate { record, evidence } => store.relocate(record, evidence),
        Command::Restore { backup, evidence } => store.restore(backup, evidence),
    }
}
