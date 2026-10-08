use clap::{Parser, Subcommand};
use kpop_model::{package, runtime, Result};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "kpop-model",
    version,
    about = "Build and use isolated native knowledge model plugins"
)]
struct Cli {
    /// The installed plugin directory. Never inferred from the caller's project.
    #[arg(long, global = true)]
    bundle: Option<PathBuf>,
    /// Override the private installation cache root.
    #[arg(long, global = true)]
    cache: Option<PathBuf>,
    /// Results are always JSON; accepted for explicit scripted use.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Build a self-contained plugin from committed model and product files.
    Build {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        descriptor: PathBuf,
        #[arg(long)]
        engine_archive: PathBuf,
        #[arg(long)]
        adapter: Option<PathBuf>,
        #[arg(long)]
        output: PathBuf,
    },
    /// Verify every file in an existing plugin bundle.
    Verify,
    /// Prepare and activate this bundle in an isolated private cache.
    Setup,
    /// Verify the active installation and report its identity.
    Doctor,
    /// List qualified generations for this plugin installation.
    Versions,
    /// Explicitly select a previously qualified generation.
    Activate {
        #[arg(long)]
        digest: String,
    },
    /// Read model entries and their sources through the native engine.
    Pull {
        #[arg(required = true, num_args = 1..)]
        ids: Vec<String>,
    },
    /// Read entries with their declared dependencies.
    Context {
        #[arg(required = true, num_args = 1..)]
        ids: Vec<String>,
    },
    /// Search this model's captured evidence.
    Search { query: String },
    /// Run the model's optional application adapter on a private case.
    Run {
        #[arg(long)]
        case: PathBuf,
        #[arg(long)]
        as_of: String,
        #[arg(long)]
        policy_overlay: Option<PathBuf>,
    },
}

fn run(cli: Cli) -> Result<serde_json::Value> {
    if let Command::Build {
        source,
        descriptor,
        engine_archive,
        adapter,
        output,
    } = &cli.command
    {
        let digest = package::build(
            source,
            descriptor,
            engine_archive,
            adapter.as_deref(),
            output,
        )?;
        return Ok(serde_json::json!({"status":"built", "bundle":output, "package_digest":digest}));
    }
    let bundle = cli
        .bundle
        .ok_or("--bundle must name the installed model plugin")?;
    if matches!(&cli.command, Command::Verify) {
        let (descriptor, lock, digest) = package::verify_bundle(&bundle)?;
        return Ok(serde_json::json!({"status":"verified", "id":descriptor.id,
            "version":descriptor.version,"package_digest":digest,"source_commit":lock.source_commit}));
    }
    let operation = match cli.command {
        Command::Setup => runtime::Operation::Setup,
        Command::Doctor => runtime::Operation::Doctor,
        Command::Versions => runtime::Operation::Versions,
        Command::Activate { digest } => runtime::Operation::Activate { digest },
        Command::Pull { ids } => runtime::Operation::Pull { ids },
        Command::Context { ids } => runtime::Operation::Context { ids },
        Command::Search { query } => runtime::Operation::Search { query },
        Command::Run {
            case,
            as_of,
            policy_overlay,
        } => runtime::Operation::Run {
            case,
            as_of,
            policy_overlay,
        },
        Command::Build { .. } | Command::Verify => unreachable!(),
    };
    runtime::execute(&bundle, cli.cache.as_deref(), operation)
}

fn main() {
    runtime::install_signal_handlers();
    match run(Cli::parse()) {
        Ok(value) => println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("JSON result")
        ),
        Err(error) => {
            eprintln!(
                "{}",
                serde_json::json!({"status":"operational_error","error":error.to_string()})
            );
            std::process::exit(runtime::cancellation_exit_code().unwrap_or(1));
        }
    }
}
