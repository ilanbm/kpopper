use clap::{Parser, Subcommand};
use kpop_native::{
    Result, identity, require,
    store::{MAX_BYTES, Request, Store, json_input},
};
use serde_json::{Value, json};
use std::{
    io::{self, Read},
    path::PathBuf,
};

#[derive(Parser)]
#[command(
    version,
    about = "Experimental opt-in native history CLI; linear readings only"
)]
struct Args {
    #[arg(long, global = true)]
    workspace: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Init {
        #[arg(long)]
        record_id: String,
    },
    Open,
    Add(WriteArgs),
    Set(WriteArgs),
    History {
        subject: Option<String>,
    },
    Recover,
    /// Consume a SessionStart JSON payload; never installs a hook or runtime.
    SessionStart,
    /// Canonical typed identity for JSON-compatible input on stdin.
    Identity {
        #[arg(long)]
        typed: bool,
    },
}
#[derive(clap::Args)]
struct WriteArgs {
    subject: String,
    #[arg(long, allow_hyphen_values = true)]
    value: String,
    #[arg(long)]
    source: Option<String>,
    #[arg(long)]
    operation: String,
    #[arg(long)]
    on: String,
    #[arg(long)]
    expected_revision: Option<String>,
}
fn stdin() -> Result<Value> {
    let mut raw = Vec::new();
    io::stdin()
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut raw)?;
    json_input(&raw)
}
fn session() -> Result<()> {
    let payload = stdin()?;
    require(payload.is_object(), "invalid_hook_payload")?;
    if payload
        .get("agent_id")
        .is_some_and(|v| !v.is_null() && v != false && v != "")
    {
        return Ok(());
    }
    let root = PathBuf::from(
        payload["cwd"]
            .as_str()
            .ok_or_else(|| kpop_native::Error("missing_workspace".into()))?,
    );
    let result = Store::open(&root)?;
    let command = std::env::current_exe()?.canonicalize()?;
    println!(
        "Native feasibility record: {} committed operations; linear readings only; semantic assessment not performed.\n{}",
        result["commits"],
        serde_json::to_string(&result["document"]["readings"])?
    );
    let sid = payload["session_id"].as_str().filter(|s| {
        !s.is_empty()
            && s.len() <= 200
            && s.bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
    });
    let environment = sid
        .map(|s| json!({"KPOPPER_AGENT_SESSION":s}))
        .unwrap_or_else(|| json!({}));
    println!(
        "KPOPPER_AGENT_CONTEXT {}",
        json!({"command":[command,"--workspace",root],"workspace":root,"environment":environment,"profile":"native-feasibility/v1"})
    );
    Ok(())
}
fn run(args: Args) -> Result<Value> {
    if let Command::Identity { typed } = args.command {
        let input = stdin()?;
        if typed {
            let value = kpop_native::value::TypedValue::from_tagged(&input)?;
            return Ok(json!({"identity":value.digest()?,"typed":value.to_tagged()?}));
        }
        return Ok(json!({"identity":identity::identity(&input)?}));
    }
    let root = args
        .workspace
        .ok_or_else(|| kpop_native::Error("workspace_required".into()))?;
    match args.command {
        Command::Init { record_id } => Store::init(&root, &record_id),
        Command::Open => Store::open(&root),
        Command::History { subject } => Store::history(&root, subject.as_deref()),
        Command::Recover => Store::recover(&root),
        command @ (Command::Add(_) | Command::Set(_)) => {
            let kind = if matches!(command, Command::Add(_)) {
                "add"
            } else {
                "set"
            };
            let (Command::Add(write) | Command::Set(write)) = command else {
                unreachable!()
            };
            let value = json_input(write.value.as_bytes())?;
            Store::write(
                &root,
                Request {
                    kind: kind.into(),
                    subject: write.subject,
                    value,
                    source: write.source,
                    operation: write.operation,
                    on: write.on,
                },
                write.expected_revision.as_deref(),
            )
        }
        Command::Identity { .. } | Command::SessionStart => unreachable!(),
    }
}
fn main() {
    let args = Args::parse();
    if matches!(args.command, Command::SessionStart) {
        if let Err(error) = session() {
            eprintln!("kpop-native: record was not opened: {error}");
        }
        return;
    }
    match run(args) {
        Ok(value) => println!("{}", value),
        Err(error) => {
            eprintln!("{}", json!({"status":"refused","error":error.to_string()}));
            std::process::exit(2);
        }
    }
}
