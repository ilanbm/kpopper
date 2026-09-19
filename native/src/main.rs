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
    #[arg(long, global = true)]
    frozen: bool,
    #[arg(long, global = true)]
    json: bool,
    #[arg(long, global = true)]
    no_cache: bool,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Read versioned assessment findings and scoped attention from actual records.
    Assess(kpop_native::public_assessment::Options),
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
        #[arg(long, conflicts_with = "yaml")]
        typed: bool,
        #[arg(long)]
        yaml: bool,
        /// Compute an object ID using its declared scheme, including legacy IDs.
        #[arg(long)]
        object: bool,
    },
    /// Read a strict YAML mapping (or tagged map) and return lossless YAML and types.
    HistoryCodec {
        #[arg(long)]
        typed: bool,
    },
    /// Validate a detached immutable object or complete object closure, without writes.
    HistoryValidate {
        #[arg(long)]
        typed: bool,
        #[arg(long)]
        closure: bool,
    },
    /// Validate detached history envelopes; does not establish accepted history.
    HistoryEnvelope {
        #[arg(value_enum)]
        kind: Envelope,
        #[arg(long)]
        typed: bool,
    },
    /// Capture an active record's exact history and reduce acceptance, without writes.
    HistoryCapture {
        entry: PathBuf,
    },
}
#[derive(Clone, clap::ValueEnum)]
enum Envelope {
    Authority,
    Baseline,
    Commit,
    Cancellation,
    Template,
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
    json_input(&stdin_bytes()?)
}
fn stdin_bytes() -> Result<Vec<u8>> {
    let mut raw = Vec::new();
    io::stdin()
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut raw)?;
    require(raw.len() <= MAX_BYTES, "byte_limit")?;
    Ok(raw)
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
    if let Command::HistoryCapture { entry } = args.command {
        let captured = kpop_native::history_capture::capture(&entry, None, None)?;
        let evidence = captured.evidence();
        return Ok(
            json!({"status":"captured","semantic_assessment":"not_performed","evidence":evidence.to_tagged()?,"digest":evidence.digest()?}),
        );
    }
    if let Command::HistoryEnvelope { kind, typed } = args.command {
        use kpop_native::history_authority as contract;
        let mut value = if typed {
            kpop_native::value::TypedValue::from_tagged(&stdin()?)?
        } else {
            kpop_native::history_yaml::decode_document(&stdin_bytes()?)?
        };
        match kind {
            Envelope::Authority => contract::validate_authority(&value)?,
            Envelope::Baseline => contract::validate_baseline(&value)?,
            Envelope::Commit => contract::validate_commit(&value)?,
            Envelope::Cancellation => contract::validate_cancellation(&value)?,
            Envelope::Template => value = contract::document_template(&value)?,
        }
        return Ok(json!({"status":"valid","typed":value.to_tagged()?,"digest":value.digest()?}));
    }
    if let Command::Identity {
        typed,
        yaml,
        object,
    } = args.command
    {
        let value = if yaml {
            kpop_native::history_yaml::decode_document(&stdin_bytes()?)?
        } else if typed {
            kpop_native::value::TypedValue::from_tagged(&stdin()?)?
        } else {
            kpop_native::value::TypedValue::from_json(&stdin()?)?
        };
        let id = if object {
            identity::typed_object_identity(&value)?
        } else {
            value.digest()?
        };
        return if typed || yaml {
            Ok(json!({"identity":id,"typed":value.to_tagged()?}))
        } else {
            Ok(json!({"identity":id}))
        };
    }
    if let Command::HistoryCodec { typed } = args.command {
        let value = if typed {
            kpop_native::value::TypedValue::from_tagged(&stdin()?)?
        } else {
            kpop_native::history_yaml::decode_document(&stdin_bytes()?)?
        };
        let yaml = kpop_native::history_yaml::encode_document(&value)?;
        return Ok(
            json!({"identity":value.digest()?,"typed":value.to_tagged()?,"yaml":String::from_utf8(yaml).unwrap()}),
        );
    }
    if let Command::HistoryValidate { typed, closure } = args.command {
        let value = if typed {
            kpop_native::value::TypedValue::from_tagged(&stdin()?)?
        } else {
            kpop_native::history_yaml::decode_document(&stdin_bytes()?)?
        };
        if closure {
            let kpop_native::value::TypedValue::Map(objects) = &value else {
                return Err(kpop_native::Error("invalid_schema".into()));
            };
            kpop_native::history_contract::validate_closure(objects)?;
            return Ok(json!({"status":"valid","objects":objects.len(),"digest":value.digest()?}));
        }
        kpop_native::history_contract::validate_object(&value)?;
        return Ok(json!({"status":"valid","id":identity::typed_object_identity(&value)?}));
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
        Command::Identity { .. }
        | Command::HistoryCodec { .. }
        | Command::HistoryValidate { .. }
        | Command::HistoryEnvelope { .. }
        | Command::HistoryCapture { .. }
        | Command::SessionStart => {
            unreachable!()
        }
        Command::Assess(_) => unreachable!(),
    }
}
fn main() {
    let args = Args::parse();
    if let Command::Assess(options) = &args.command {
        let result = (|| {
            let cwd = args
                .workspace
                .clone()
                .map(Ok)
                .unwrap_or_else(std::env::current_dir)?;
            let mode =
                if args.frozen || std::env::var("KPOPPER_READ_MODE").as_deref() == Ok("frozen") {
                    kpop_native::source_capture::ReadMode::Frozen
                } else {
                    kpop_native::source_capture::ReadMode::Live
                };
            kpop_native::public_assessment::run(options, &cwd, mode)
        })();
        match result {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!("kpop-native assess: {error}");
                std::process::exit(2);
            }
        }
        return;
    }
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
