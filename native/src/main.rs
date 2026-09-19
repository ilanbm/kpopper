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
    about = "Experimental native assessment, readers and history tools"
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
    /// Open the current knowledge context.
    Open(kpop_native::public_readers::Options),
    /// Check the record and its page arrangement.
    Check(kpop_native::public_readers::Options),
    /// Read entries, their sources and findings.
    Pull(kpop_native::public_readers::Options),
    /// Trace the consequences of changed entries.
    Affects(kpop_native::public_readers::Options),
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
    let command = std::env::current_exe()?.canonicalize()?;
    let feasibility = root.join(".kpopper/native-feasibility.json").is_file();
    if feasibility {
        let result = Store::open(&root)?;
        println!(
            "Native feasibility record: {} committed operations; linear readings only; semantic assessment not performed.\n{}",
            result["commits"],
            serde_json::to_string(&result["document"]["readings"])?
        );
    } else {
        let mode = if std::env::var("KPOPPER_READ_MODE").as_deref() == Ok("frozen") {
            kpop_native::source_capture::ReadMode::Frozen
        } else {
            kpop_native::source_capture::ReadMode::Live
        };
        let runtime = kpop_native::public_workspace::runtime()?;
        let opening = kpop_native::public_readers::run(
            "open",
            &kpop_native::public_readers::Options::default(),
            &root,
            mode,
            false,
            runtime.as_ref(),
        )?;
        print!("{}", opening.text);
    }
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
        json!({"command":[command,"--workspace",root],"workspace":root,"environment":environment,"profile":if feasibility{"native-feasibility/v1"}else{"native-public/v1"}})
    );
    if !feasibility {
        println!(
            "For mapping, pass this session environment to the CLI and execute the returned task. The identity routes work back to this session; it grants no source access."
        );
    }
    Ok(())
}
fn public_history_status(root: &std::path::Path) -> Result<Value> {
    let records = kpop_native::public_workspace::records(root)?;
    require(records.len() == 1, "choose one logical record entry")?;
    let entry = &records[0];
    let capture = kpop_native::history_capture::capture(entry, None, None)?;
    let kpop_native::value::TypedValue::Map(state) = &capture.state else {
        return Err(kpop_native::Error("invalid history state".into()));
    };
    let Some(kpop_native::value::TypedValue::Map(subjects)) = state.get("subjects") else {
        return Err(kpop_native::Error("invalid history subjects".into()));
    };
    let subjects = subjects
        .iter()
        .map(|(subject, value)| {
            let kpop_native::value::TypedValue::Map(value) = value else {
                return Err(kpop_native::Error("invalid history subject".into()));
            };
            Ok((
                subject.clone(),
                json!({
                    "acceptance": value["acceptance"].to_json()?,
                    "heads": value["heads"].to_json()?,
                }),
            ))
        })
        .collect::<Result<serde_json::Map<String, Value>>>()?;
    capture.verify_current()?;
    Ok(json!({
        "state":"captured",
        "record":entry,
        "authority":capture.marker.to_json()?,
        "commits":capture.commits.len(),
        "objects":capture.objects.len(),
        "subjects":subjects,
    }))
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
        Command::Open(_) => Store::open(&root),
        Command::History { subject } => {
            if root.join(".kpopper/native-feasibility.json").is_file() {
                Store::history(&root, subject.as_deref())
            } else {
                require(
                    subject.as_deref() == Some("status"),
                    "history operation unsupported",
                )?;
                public_history_status(&root)
            }
        }
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
        Command::Assess(_) | Command::Check(_) | Command::Pull(_) | Command::Affects(_) => {
            unreachable!()
        }
    }
}
fn main() {
    let args = Args::parse();
    let read = match &args.command {
        Command::Open(o) => Some(("open", o)),
        Command::Check(o) => Some(("check", o)),
        Command::Pull(o) => Some(("pull", o)),
        Command::Affects(o) => Some(("affects", o)),
        _ => None,
    };
    if let Some((command, options)) = read {
        let result = (|| {
            let cwd = args
                .workspace
                .clone()
                .map(Ok)
                .unwrap_or_else(std::env::current_dir)?;
            // Preserve the explicitly opted-in feasibility store until its public
            // authoring commands are replaced. Regular records never enter it.
            if command == "open"
                && options.subjects.is_empty()
                && options.profile.is_none()
                && cwd.join(".kpopper/native-feasibility.json").is_file()
            {
                return Ok(kpop_native::public_core_readers::Output {
                    text: Store::open(&cwd)?.to_string() + "\n",
                    code: 0,
                });
            }
            let mode =
                if args.frozen || std::env::var("KPOPPER_READ_MODE").as_deref() == Ok("frozen") {
                    kpop_native::source_capture::ReadMode::Frozen
                } else {
                    kpop_native::source_capture::ReadMode::Live
                };
            let runtime = kpop_native::public_workspace::runtime()?;
            kpop_native::public_readers::run(
                command,
                options,
                &cwd,
                mode,
                args.json,
                runtime.as_ref(),
            )
        })();
        match result {
            Ok(output) => {
                print!("{}", output.text);
                if output.code != 0 {
                    std::process::exit(output.code);
                }
            }
            Err(error) => {
                if error
                    .0
                    .contains("is not an entry or a prefix in this record.")
                {
                    eprintln!("{error}");
                    std::process::exit(1);
                }
                if args.json {
                    eprintln!("{}", json!({"error":error.to_string()}));
                } else {
                    eprintln!("kpop-native {command}: {error}");
                }
                std::process::exit(2);
            }
        }
        return;
    }
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
