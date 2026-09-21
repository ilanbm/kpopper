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
    name = "kpop",
    bin_name = "kpop",
    version,
    about = "Keep what you know, its grounds, and what needs another look"
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
    /// Search captured local evidence or read a revision-bound result.
    Search(kpop_native::public_search::Options),
    /// Inspect the project mode or change local preferences.
    Config(kpop_native::public_config::Options),
    /// Convert formulas or preview an explicit expression migration.
    Expressions(kpop_native::public_expressions::Args),
    /// Read versioned assessment findings and scoped attention from actual records.
    Assess(kpop_native::public_assessment::Options),
    Init {
        #[arg(long)]
        record_id: String,
    },
    /// Open the current knowledge context.
    Open(kpop_native::public_readers::Options),
    /// Locate this workspace's record without reading its contents.
    Where,
    /// Check record integrity and findings.
    Check(kpop_native::public_readers::Options),
    /// Read entries, their sources and findings.
    Pull(kpop_native::public_readers::Options),
    /// Trace the consequences of changed entries.
    Affects(kpop_native::public_readers::Options),
    /// Optional applications built from the captured record.
    Experimental {
        #[command(subcommand)]
        application: Application,
    },
    /// Compatibility alias for experimental hub.
    Page(kpop_native::public_hub::Options),
    /// Compatibility alias for experimental annotated-doc.
    Document(kpop_native::public_annotated_document::Options),
    Add(WriteArgs),
    Set(WriteArgs),
    Review(WriteArgs),
    /// Apply one source report atomically and return its durable receipt.
    Update(kpop_native::public_update::Options),
    /// Capture, process and inspect durable asynchronous source reports.
    Ingest(kpop_native::public_ingestion::CommandOptions),
    /// Deliver bounded ingestion attention through a host hook payload.
    IngestionHook(kpop_native::ingestion_hooks::Options),
    /// Preview, fold or refute named hypotheses in active history.
    Consolidate(ConsolidateArgs),
    /// Record that two subjects refer to the same thing.
    Same(kpop_native::public_identity::SameOptions),
    /// Record why two similar subjects are distinct.
    Distinct(kpop_native::public_identity::DistinctOptions),
    /// Export a bounded record assessment as Markdown or Mermaid.
    Export(kpop_native::public_export::CommandOptions),
    /// Inspect or materialize captured knowledge contributions.
    Knowledge(kpop_native::public_knowledge::Options),
    /// Inspect local pending contribution state.
    Pending(kpop_native::public_pending::Options),
    /// Manage durable local followups.
    Followups(kpop_native::public_followups::Args),
    /// Check branch compatibility and coordinate local observation delivery.
    Watch(kpop_native::public_watch::Args),
    /// Request an initial map from the current host agent.
    Map(kpop_native::public_map::Options),
    #[command(name = "_agent", hide = true)]
    Agent(kpop_native::public_map::AgentOptions),
    Remeasure(kpop_native::public_remeasure::Options),

    History(kpop_native::public_history::Options),
    Recover {
        #[arg(long)]
        record: Option<PathBuf>,
        #[arg(long)]
        rollback: bool,
    },
    /// Consume a SessionStart JSON payload; never installs a hook or runtime.
    SessionStart(kpop_native::public_session::StartOptions),
    /// Consume a Stop payload and deliver each new finding once per session.
    SessionStop(kpop_native::public_session::HookOptions),
    /// Save a private session-start baseline.
    Mark(kpop_native::public_session::Options),
    /// Assess against a saved baseline; --session enables once-only delivery.
    Gate(kpop_native::public_session::Options),
    /// Open and read revision-bound checked sessions; serve their MCP transport.
    Session(kpop_native::public_checked_session::Options),
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
#[derive(Subcommand)]
enum Application {
    /// Build or verify the optional record Hub.
    #[command(alias = "page")]
    Hub(kpop_native::public_hub::Options),
    /// Standalone HTML documents with embedded evidence and source checks.
    #[command(alias = "document")]
    AnnotatedDoc(kpop_native::public_annotated_document::Options),
}
#[derive(Clone, clap::ValueEnum)]
enum Envelope {
    Authority,
    Baseline,
    Commit,
    Cancellation,
    Template,
}
type WriteArgs = kpop_native::public_authoring::Options;
#[derive(clap::Args)]
struct ConsolidateArgs {
    /// Hypothesis names and, optionally, one record YAML path.
    subjects: Vec<String>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long, num_args = 2, value_names = ["NAME", "WHY"])]
    refute: Vec<String>,
    #[arg(long = "as")]
    source: Option<String>,
    #[arg(long)]
    as_of: Option<String>,
    #[arg(long)]
    take: Vec<String>,
    #[arg(long = "drop")]
    drops: Vec<String>,
    #[arg(long = "from")]
    from_refs: Vec<String>,
    #[arg(long)]
    by: Option<String>,
    #[arg(long = "choose")]
    choices: Vec<String>,
    #[arg(long)]
    source_revision: Option<String>,
}
impl ConsolidateArgs {
    fn options(&self) -> Result<kpop_native::public_consolidation::Options> {
        let mut options = kpop_native::public_consolidation::Options {
            dry_run: self.dry_run,
            refute: self.refute.first().cloned(),
            why: self.refute.get(1).cloned(),
            source: self.source.clone(),
            as_of: self.as_of.clone(),
            take: self.take.clone(),
            drops: self.drops.clone(),
            from_refs: self.from_refs.clone(),
            by: self.by.clone(),
            choices: self.choices.clone(),
            source_revision: self.source_revision.clone(),
            ..Default::default()
        };
        for subject in &self.subjects {
            if subject.ends_with(".yaml") || subject.ends_with(".yml") {
                require(options.record.is_none(), "choose one logical record entry")?;
                options.record = Some(subject.into());
            } else {
                options.names.push(subject.clone());
            }
        }
        Ok(options)
    }
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
fn session(options: &kpop_native::public_session::StartOptions) -> Result<String> {
    let raw = stdin_bytes()?;
    let mut payload = if raw.iter().all(u8::is_ascii_whitespace) {
        json!({})
    } else {
        serde_json::from_slice(&raw)?
    };
    require(payload.is_object(), "invalid_hook_payload")?;
    if payload
        .get("agent_id")
        .is_some_and(|v| !v.is_null() && v != false && v != "")
    {
        return Ok(String::new());
    }
    if options.cursor {
        payload["session_id"] = payload["conversation_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(|s| json!(format!("cursor-{s}")))
            .unwrap_or(json!(""));
    }
    let cwd = payload["cwd"]
        .as_str()
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(std::env::current_dir)?;
    let mode = if std::env::var("KPOPPER_READ_MODE").as_deref() == Ok("frozen") {
        kpop_native::source_capture::ReadMode::Frozen
    } else {
        kpop_native::source_capture::ReadMode::Live
    };
    let cwd = cwd.canonicalize()?;
    let feasibility = cwd.join(".kpopper/native-feasibility.json").is_file();
    let location = if feasibility {
        None
    } else {
        Some(kpop_native::public_workspace::locate(&cwd, mode)?)
    };
    let root = location.as_ref().map(|l| l.workspace.clone()).unwrap_or(cwd);
    payload["cwd"] = json!(root);
    let command = std::env::current_exe()?.canonicalize()?;
    let mut output = Vec::<String>::new();
    let first_use = location.as_ref().map(|location| {
        kpop_native::onboarding::context_with_host(location, options.host.as_deref())
            .unwrap_or_else(|e| format!("kpopper first-use preferences unavailable: {e}"))
    });
    if location.as_ref().is_some_and(|l| l.status == "unavailable") {
        return Ok(first_use.unwrap_or_default());
    }
    if feasibility {
        let result = Store::open(&root)?;
        output.push(format!("Native feasibility record: {} committed operations; linear readings only; semantic assessment not performed.\n{}", result["commits"], serde_json::to_string(&result["document"]["readings"])?));
    } else if let Some(location) = location.as_ref().filter(|l| l.status != "missing") {
        match kpop_native::session_admin::hook_opening(&root, mode) {
            Ok(Some(text)) => {
                output.push(text.trim_end().into());
                if let Some(host) = options.host.as_deref() {
                    let (ground, record) = if host == "claude" {
                        ("/kpopper:ground", "/kpopper:record")
                    } else {
                        ("$ground", "$record")
                    };
                    output.push(format!("next: {ground} <entry|prefix> (values with sources, what a change reaches) · {record} (what this session found) · check"));
                }
            }
            Ok(None) => {
                let options = kpop_native::public_readers::Options {
                    host: options.host.clone(),
                    ..Default::default()
                };
                match kpop_native::public_readers::run_auto("open", &options, &root, mode, false) {
                    Ok(opening) => {
                        if !opening.text.is_empty() {
                            output.push(opening.text.trim_end().into());
                        }
                    }
                    Err(error) => {
                        output.push(format!("The knowledge record could not be opened. Read it before relying on it: {}", location.record.display()));
                        eprintln!("{error}");
                    }
                }
            }
            Err(error) => {
                output.push(format!(
                    "Checked session view unavailable. Read the record before relying on it: {}",
                    location.record.display()
                ));
                eprintln!("{error}");
            }
        }
    }
    if let Some(first_use) = first_use.filter(|s| !s.is_empty()) {
        output.push(first_use);
    }
    if !feasibility {
        match kpop_native::session_admin::followup_summary(&root) {
            Ok(Some(summary)) => output.push(summary),
            Ok(None) => (),
            Err(error) => output.push(format!("Followups unavailable: {error}")),
        }
    }
    let sid = payload["session_id"]
        .as_str()
        .filter(|s| kpop_native::public_session::valid_session(s));
    let environment = sid
        .map(|s| json!({"KPOPPER_AGENT_SESSION":s}))
        .unwrap_or_else(|| json!({}));
    output.push(format!("KPOPPER_AGENT_CONTEXT {}", json!({"command":[command,"--workspace",root],"workspace":root,"environment":environment,"profile":if feasibility{"native-feasibility/v1"}else{"native-public/v1"}})));
    if !feasibility {
        output.push("Pass this session environment to record-writing and mapping commands. For mapping, execute the returned task. The identity routes work back to this session; it grants no source access.".into());
        kpop_native::public_session::start_mark(&root, &payload, mode)?;
    }
    Ok(output.join("\n"))
}

fn run(args: Args) -> Result<Value> {
    if let Command::Map(options) = &args.command {
        let root = args.workspace.clone().unwrap_or(std::env::current_dir()?);
        return kpop_native::public_map::run(options, &root);
    }
    if let Command::Agent(options) = &args.command {
        let root = args.workspace.clone().unwrap_or(std::env::current_dir()?);
        return kpop_native::public_map::run_agent(options, &root);
    }
    if let Command::Watch(options) = &args.command {
        let root = args
            .workspace
            .clone()
            .map(Ok)
            .unwrap_or_else(std::env::current_dir)?;
        return kpop_native::public_watch::run(options, &root);
    }
    if let Command::Remeasure(options) = &args.command {
        let root = args.workspace.clone().unwrap_or(std::env::current_dir()?);
        let output = kpop_native::public_remeasure::run(options, &root, args.frozen)?;
        return Ok(json!({"text": output.text, "stderr": output.stderr, "code": output.code}));
    }
    if let Command::Followups(options) = &args.command {
        let root = args
            .workspace
            .clone()
            .map(Ok)
            .unwrap_or_else(std::env::current_dir)?;
        return kpop_native::public_followups::run(options, &root);
    }
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
        Command::History(options) => {
            if root.join(".kpopper/native-feasibility.json").is_file() {
                Store::history(&root, options.operation.as_deref())
            } else {
                unreachable!()
            }
        }
        Command::Recover { .. } => Store::recover(&root),
        command @ (Command::Add(_) | Command::Set(_)) => {
            let kind = if matches!(command, Command::Add(_)) {
                "add"
            } else {
                "set"
            };
            let (Command::Add(write) | Command::Set(write)) = command else {
                unreachable!()
            };
            let value = json_input(
                write
                    .value
                    .ok_or_else(|| kpop_native::Error("--value is required".into()))?
                    .as_bytes(),
            )?;
            Store::write(
                &root,
                Request {
                    kind: kind.into(),
                    subject: write.subject,
                    value,
                    source: write.source,
                    operation: write
                        .operation
                        .ok_or_else(|| kpop_native::Error("--operation is required".into()))?,
                    on: write
                        .on
                        .ok_or_else(|| kpop_native::Error("--on is required".into()))?,
                },
                write.expected_revision.as_deref(),
            )
        }
        Command::Identity { .. }
        | Command::HistoryCodec { .. }
        | Command::HistoryValidate { .. }
        | Command::HistoryEnvelope { .. }
        | Command::HistoryCapture { .. }
        | Command::SessionStart(_)
        | Command::SessionStop(_)
        | Command::Mark(_)
        | Command::Gate(_) => {
            unreachable!()
        }
        Command::Review(_) => Err(kpop_native::Error("review requires a public record".into())),
        Command::Update(_)
        | Command::Ingest(_)
        | Command::IngestionHook(_)
        | Command::Assess(_)
        | Command::Where
        | Command::Session(_)
        | Command::Consolidate(_)
        | Command::Same(_)
        | Command::Distinct(_)
        | Command::Export(_)
        | Command::Knowledge(_)
        | Command::Pending(_)
        | Command::Expressions(_)
        | Command::Search(_)
        | Command::Config(_)
        | Command::Followups(_)
        | Command::Remeasure(_)
        | Command::Watch(_)
        | Command::Map(_)
        | Command::Agent(_)
        | Command::Check(_)
        | Command::Pull(_)
        | Command::Affects(_)
        | Command::Page(_)
        | Command::Document(_)
        | Command::Experimental { .. } => {
            unreachable!()
        }
    }
}
fn main() {
    let argv =
        kpop_native::public_expressions::preprocess_argv(&std::env::args_os().collect::<Vec<_>>());
    let application = kpop_native::application_cli::inspect(&argv);
    if let Some(output) = &application.early {
        print!("{}", output.stdout);
        eprint!("{}", output.stderr);
        std::process::exit(output.code);
    }
    if let Some(notice) = &application.notice {
        eprint!("{notice}");
    }
    let args = match Args::try_parse_from(&argv) {
        Ok(args) => args,
        Err(error) => {
            if let Some(output) = kpop_native::public_expressions::parse_failure(&argv, &error) {
                print!("{}", output.stdout);
                eprint!("{}", output.stderr);
                std::process::exit(output.code);
            }
            error.exit();
        }
    };
    if matches!(args.command, Command::Search(_) | Command::Config(_)) {
        let cwd = match args
            .workspace
            .clone()
            .map(Ok)
            .unwrap_or_else(std::env::current_dir)
        {
            Ok(cwd) => cwd,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(2);
            }
        };
        let (stdout, stderr, code) = match &args.command {
            Command::Search(options) => {
                let mode = if args.frozen
                    || std::env::var("KPOPPER_READ_MODE").as_deref() == Ok("frozen")
                {
                    kpop_native::source_capture::ReadMode::Frozen
                } else {
                    kpop_native::source_capture::ReadMode::Live
                };
                let output = kpop_native::public_search::dispatch(options, &cwd, mode, args.json);
                (output.stdout, output.stderr, output.code)
            }
            Command::Config(options) => {
                let output = kpop_native::public_config::dispatch(options, &cwd, args.json);
                (output.stdout, output.stderr, output.code)
            }
            _ => unreachable!(),
        };
        print!("{stdout}");
        eprint!("{stderr}");
        std::process::exit(code);
    }
    if let Command::Expressions(options) = &args.command {
        let cwd = match args
            .workspace
            .clone()
            .map(Ok)
            .unwrap_or_else(std::env::current_dir)
        {
            Ok(cwd) => cwd,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(2);
            }
        };
        let mode = if args.frozen || std::env::var("KPOPPER_READ_MODE").as_deref() == Ok("frozen") {
            kpop_native::source_capture::ReadMode::Frozen
        } else {
            kpop_native::source_capture::ReadMode::Live
        };
        let output = kpop_native::public_expressions::dispatch(options, &cwd, mode);
        print!("{}", output.stdout);
        eprint!("{}", output.stderr);
        std::process::exit(output.code);
    }
    if matches!(args.command, Command::Same(_) | Command::Distinct(_)) {
        let output = match args
            .workspace
            .clone()
            .map(Ok)
            .unwrap_or_else(std::env::current_dir)
        {
            Ok(cwd) => match &args.command {
                Command::Same(options) => {
                    kpop_native::public_identity::dispatch_same(options, &cwd)
                }
                Command::Distinct(options) => {
                    kpop_native::public_identity::dispatch_distinct(options, &cwd)
                }
                _ => unreachable!(),
            },
            Err(error) => kpop_native::public_identity::CommandOutput {
                stdout: format!("refused: {error}\n"),
                stderr: String::new(),
                code: 1,
            },
        };
        if args.json {
            let command = if matches!(args.command, Command::Same(_)) {
                "same"
            } else {
                "distinct"
            };
            println!(
                "{}",
                json!({"command":command,"exit_code":output.code,"output":output.stdout,"error":output.stderr})
            );
        } else {
            print!("{}", output.stdout);
            eprint!("{}", output.stderr);
        }
        std::process::exit(output.code);
    }
    if matches!(args.command, Command::Knowledge(_) | Command::Pending(_)) {
        let cwd = match args
            .workspace
            .clone()
            .map(Ok)
            .unwrap_or_else(std::env::current_dir)
        {
            Ok(cwd) => cwd,
            Err(error) => {
                eprintln!("{}", json!({"error":error.to_string()}));
                std::process::exit(2);
            }
        };
        let (stdout, stderr, code) = match &args.command {
            Command::Knowledge(options) => {
                let mode = if args.frozen
                    || std::env::var("KPOPPER_READ_MODE").as_deref() == Ok("frozen")
                {
                    kpop_native::source_capture::ReadMode::Frozen
                } else {
                    kpop_native::source_capture::ReadMode::Live
                };
                let result = kpop_native::public_knowledge::dispatch(options, &cwd, mode);
                (result.stdout, result.stderr, result.code)
            }
            Command::Pending(options) => {
                let result = kpop_native::public_pending::dispatch(options, &cwd, args.json);
                (result.stdout, result.stderr, result.code)
            }
            _ => unreachable!(),
        };
        print!("{stdout}");
        eprint!("{stderr}");
        std::process::exit(code);
    }
    if let Command::Export(options) = &args.command {
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
            kpop_native::public_export::run(options, &cwd, mode)
        })();
        let (output, error, code) = match result {
            Ok(output) => (output, String::new(), 0),
            Err(error) => (
                String::new(),
                kpop_native::public_export::command_error(&error.to_string()),
                2,
            ),
        };
        if args.json {
            println!(
                "{}",
                json!({"command":"export","exit_code":code,"output":output,"error":error})
            );
        } else {
            print!("{output}");
            eprint!("{error}");
        }
        std::process::exit(code);
    }
    if matches!(args.command, Command::Where) {
        let result = (|| -> Result<Option<PathBuf>> {
            let cwd = args
                .workspace
                .clone()
                .map(Ok)
                .unwrap_or_else(std::env::current_dir)?;
            Ok(kpop_native::public_workspace::records(&cwd)?
                .into_iter()
                .next()
                .filter(|p| p.exists()))
        })();
        let (output, error, code) = match result {
            Ok(Some(path)) => (format!("{}\n", path.display()), String::new(), 0),
            Ok(None) => (String::new(), String::new(), 1),
            Err(error) => (String::new(), format!("{error}\n"), 1),
        };
        if args.json {
            println!(
                "{}",
                json!({"command":"where","exit_code":code,"output":output,"error":error})
            );
        } else {
            print!("{output}");
            eprint!("{error}");
        }
        std::process::exit(code);
    }
    if let Command::Consolidate(arguments) = &args.command {
        let result = (|| {
            let cwd = args
                .workspace
                .clone()
                .map(Ok)
                .unwrap_or_else(std::env::current_dir)?;
            Ok::<_, kpop_native::Error>(kpop_native::public_consolidation::dispatch(
                &arguments.options()?,
                &cwd,
            ))
        })();
        let (output, error, code) = match result {
            Ok(output) => (output.stdout, output.stderr, output.code),
            Err(error) => (String::new(), format!("{error}\n"), 1),
        };
        if args.json {
            println!(
                "{}",
                json!({"command":"consolidate","exit_code":code,"output":output,"error":error})
            );
        } else {
            print!("{output}");
            eprint!("{error}");
        }
        std::process::exit(code);
    }
    if let Command::Session(options) = &args.command {
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
            if kpop_native::session_admin::handles(&options.operation) {
                kpop_native::session_admin::run(options, &cwd)
            } else {
                kpop_native::public_checked_session::run(options, &cwd, mode)
                    .map(|text| kpop_native::session_admin::Output { text, code: 0 })
            }
        })();
        match result {
            Ok(output) => {
                let text = output.text;
                print!("{text}");
                if !text.is_empty() && !text.ends_with('\n') {
                    println!();
                }
                if output.code != 0 {
                    std::process::exit(output.code);
                }
            }
            Err(error) => {
                eprintln!("{}", json!({"error":error.to_string()}));
                std::process::exit(2);
            }
        }
        return;
    }
    let session_command = match &args.command {
        Command::Mark(o) => Some(("mark", o)),
        Command::Gate(o) => Some(("gate", o)),
        _ => None,
    };
    if let Some((kind, options)) = session_command {
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
            kpop_native::public_session::run(kind, options, &cwd, mode)
        })();
        match result {
            Ok(output) => {
                print!("{}", output.text);
                std::process::exit(output.code);
            }
            Err(error) => {
                eprintln!("kpop-native {kind}: {error}");
                std::process::exit(1);
            }
        }
    }
    if let Command::SessionStop(options) = &args.command {
        let result = (|| {
            let payload = stdin()?;
            let mode =
                if args.frozen || std::env::var("KPOPPER_READ_MODE").as_deref() == Ok("frozen") {
                    kpop_native::source_capture::ReadMode::Frozen
                } else {
                    kpop_native::source_capture::ReadMode::Live
                };
            kpop_native::public_session::stop(&payload, options.host.as_deref(), mode)
        })();
        match result {
            Ok(output) => {
                eprint!("{}", output.text);
                std::process::exit(output.code);
            }
            Err(error) => {
                eprintln!("kpop-native session-stop: assessment unavailable: {error}");
                return;
            }
        }
    }
    if let Command::Page(options)
    | Command::Experimental {
        application: Application::Hub(options),
    } = &args.command
    {
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
            kpop_native::public_hub::run(options, &cwd, mode)
        })();
        let (output, error, code) = match result {
            Ok(output) => (output.text, String::new(), output.code),
            Err(error) => (
                String::new(),
                format!("kpop-native experimental hub: {error}\n"),
                2,
            ),
        };
        if args.json {
            println!(
                "{}",
                json!({"command":application.requested.as_deref().unwrap_or("hub"),"exit_code":code,"output":output,"error":error})
            );
        } else {
            print!("{output}");
            eprint!("{error}");
        }
        std::process::exit(code);
    }
    if let Command::Document(options)
    | Command::Experimental {
        application: Application::AnnotatedDoc(options),
    } = &args.command
    {
        let result = (|| {
            let cwd = args
                .workspace
                .clone()
                .map(Ok)
                .unwrap_or_else(std::env::current_dir)?;
            kpop_native::public_annotated_document::run(options, &cwd)
        })();
        let (output, error, code) = match result {
            Ok(text) => (text, String::new(), 0),
            Err(error) => (String::new(), format!("document: {error}\n"), 2),
        };
        if args.json {
            println!(
                "{}",
                json!({"command":application.requested.as_deref().unwrap_or("annotated-doc"),"exit_code":code,"output":output,"error":error})
            );
        } else {
            print!("{output}");
            eprint!("{error}");
        }
        std::process::exit(code);
    }
    if let Command::Update(options) = &args.command {
        let result = (|| {
            let cwd = args
                .workspace
                .clone()
                .map(Ok)
                .unwrap_or_else(std::env::current_dir)?;
            let input = (options.file == "-").then(stdin_bytes).transpose()?;
            kpop_native::public_update::run(options, &cwd, input.as_deref())
        })();
        match result {
            Ok(output) => {
                print!("{}", output.text);
                std::process::exit(output.code);
            }
            Err(error) => {
                println!("{}", json!({"error":error.to_string()}));
                std::process::exit(2);
            }
        }
    }
    if let Command::IngestionHook(options) = &args.command {
        let result = (|| -> Result<kpop_native::ingestion_hooks::Output> {
            let payload: Value = serde_json::from_slice(&stdin_bytes()?)?;
            let cwd = args.workspace.clone()
                .or_else(|| payload.get("cwd").and_then(Value::as_str)
                    .filter(|path| !path.is_empty()).map(PathBuf::from))
                .map(Ok).unwrap_or_else(std::env::current_dir)?;
            kpop_native::ingestion_hooks::run(options, payload, &cwd)
        })();
        match result {
            Ok(output) => {
                print!("{}", output.stdout);
                eprint!("{}", output.stderr);
                std::process::exit(output.code);
            }
            Err(error) => {
                eprintln!("ingestion-hook: {error}");
                std::process::exit(1);
            }
        }
    }
    if let Command::Ingest(options) = &args.command {
        let cwd = args.workspace.clone().unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let input = matches!(&options.command, kpop_native::public_ingestion::Command::Capture(capture) if capture.file == "-")
            .then(stdin_bytes)
            .transpose();
        let output = match input {
            Ok(input) => kpop_native::public_ingestion::dispatch(options, &cwd, input.as_deref()),
            Err(error) => kpop_native::public_ingestion::Output { stdout: format!("{}\n", json!({"error":error.to_string()})), stderr: String::new(), code: 2 },
        };
        print!("{}", output.stdout);
        eprint!("{}", output.stderr);
        std::process::exit(output.code);
    }
    let write = match &args.command {
        Command::Add(o) => Some(("add", o)),
        Command::Set(o) => Some(("set", o)),
        Command::Review(o) => Some(("review", o)),
        _ => None,
    };
    if let Some((kind, options)) = write {
        let cwd = match args
            .workspace
            .clone()
            .map(Ok)
            .unwrap_or_else(std::env::current_dir)
        {
            Ok(cwd) => cwd,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        };
        if !cwd.join(".kpopper/native-feasibility.json").is_file() {
            match kpop_native::public_authoring::run(kind, options, &cwd) {
                Ok(output) => print!("{output}"),
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(1);
                }
            }
            return;
        }
    }
    if let Command::Recover { record, rollback } = &args.command {
        let result = (|| {
            let cwd = args
                .workspace
                .clone()
                .map(Ok)
                .unwrap_or_else(std::env::current_dir)?;
            if cwd.join(".kpopper/native-feasibility.json").is_file() {
                require(
                    record.is_none() && !rollback,
                    "public recovery options require a public record",
                )?;
                return Store::recover(&cwd);
            }
            let paths = match record {
                Some(path) => vec![cwd.join(path)],
                None => kpop_native::public_workspace::records(&cwd)?,
            };
            if kpop_native::legacy_authoring::recovery_pending(&paths, &cwd)? {
                kpop_native::legacy_authoring::recover(&paths, &cwd, *rollback)?.to_json()
            } else {
                kpop_native::direct_history::recover(&paths, &cwd, *rollback)?.to_json()
            }
        })();
        match result {
            Ok(value) => {
                if args.json || value.get("mutation_digest").is_none() {
                    println!("{value}");
                } else {
                    println!(
                        "{}: {}",
                        value["state"].as_str().unwrap(),
                        value["operation"].as_str().unwrap()
                    );
                }
            }
            Err(error) => {
                if args.json {
                    println!("{}", kpop_native::public_history::refusal(&error));
                } else {
                    println!("refused: {error}");
                }
                std::process::exit(1);
            }
        }
        return;
    }
    if let Command::History(options) = &args.command {
        let result = (|| {
            let cwd = args
                .workspace
                .clone()
                .map(Ok)
                .unwrap_or_else(std::env::current_dir)?;
            if options.operation.as_deref() == Some("capabilities") {
                return kpop_native::public_history::run(options, &cwd);
            }
            if cwd.join(".kpopper/native-feasibility.json").is_file() {
                require(
                    !options.selects_public_operation(),
                    "public history options require a public record",
                )?;
                return Store::history(&cwd, options.operation.as_deref());
            }
            kpop_native::public_history::run(options, &cwd)
        })();
        match result {
            Ok(value) => print!("{}", kpop_native::public_history::output(&value)),
            Err(error) => {
                print!(
                    "{}",
                    kpop_native::public_history::output(&kpop_native::public_history::refusal(
                        &error
                    ))
                );
                std::process::exit(1);
            }
        }
        return;
    }
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
            kpop_native::public_readers::run_auto(command, options, &cwd, mode, args.json)
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
                if command == "pull" && options.from_ref.is_some() {
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
    if let Command::SessionStart(options) = &args.command {
        let text = match session(options) {
            Ok(text) => text,
            Err(error) => {
                eprintln!("kpop-native: record was not opened: {error}");
                String::new()
            }
        };
        if options.cursor {
            println!("{}", json!({"additional_context":text}));
        } else if !text.is_empty() {
            println!("{text}");
        }
        return;
    }
    let is_watch = matches!(&args.command, Command::Watch(_));
    let watch_args = match &args.command {
        Command::Watch(watch) => Some(watch.clone()),
        _ => None,
    };
    let is_map = matches!(args.command, Command::Map(_));
    let is_agent = matches!(args.command, Command::Agent(_));
    let as_json = args.json;
    let is_remeasure = matches!(&args.command, Command::Remeasure(_));
    match run(args) {
        Ok(value) if is_remeasure => {
            let text = value["text"].as_str().unwrap_or("");
            let stderr = value["stderr"].as_str().unwrap_or("");
            let code = value["code"].as_i64().unwrap_or(0) as i32;
            print!("{}", text);
            eprint!("{}", stderr);
            if code != 0 {
                std::process::exit(code);
            }
        }
        Ok(value) if is_watch => {
            if let Some(watch) = watch_args {
                println!("{}", kpop_native::public_watch::cli_json(&watch, &value));
            } else {
                println!("{}", serde_json::to_string_pretty(&value).unwrap());
            }
        }
        Ok(value) if is_map => {
            if as_json {
                println!("{}", serde_json::to_string_pretty(&value).unwrap());
            } else {
                println!(
                    "Mapping task {} is {} for the current agent session.\nWork is complete only after the agent returns a report.",
                    value["request"].as_str().unwrap_or(""),
                    value["status"].as_str().unwrap_or("")
                );
            }
        }
        Ok(Value::String(text)) if is_agent => {
            if !text.is_empty() {
                println!("{text}");
            }
        }
        Ok(value) if is_agent => println!("{}", serde_json::to_string_pretty(&value).unwrap()),

        Ok(value) => println!("{}", value),
        Err(error) => {
            if is_map && as_json {
                println!(
                    "{{\n  \"status\": \"unavailable\",\n  \"error\": {}\n}}",
                    serde_json::to_string(&error.to_string()).unwrap()
                );
            } else if is_agent {
                eprintln!("kpopper _agent: {error}");
            } else if is_map {
                eprintln!("{error}");
            } else if is_watch {
                eprintln!("{}", json!({"error":error.to_string()}));
            } else {
                eprintln!("{}", json!({"status":"refused","error":error.to_string()}));
            }
            std::process::exit(2);
        }
    }
}
