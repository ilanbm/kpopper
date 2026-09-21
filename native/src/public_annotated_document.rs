//! Public CLI boundary for portable annotated document copies.
use crate::{Result, annotated_document as D};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, clap::Args)]
pub struct Options {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Clone, Debug, clap::Subcommand)]
pub enum Command {
    /// Read the authoring contract and examples.
    Guide,
    /// Package authored HTML and source checks in a portable copy.
    Build {
        #[arg(long)]
        html: PathBuf,
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        root: Option<PathBuf>,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        overwrite: bool,
    },
    /// Reread explicit sources and prepare a correction on a new copy.
    Refresh {
        artifact: PathBuf,
        #[arg(long)]
        sources: PathBuf,
        #[arg(long)]
        root: Option<PathBuf>,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        overwrite: bool,
    },
    /// Validate embedded checks and decisions without running authored code.
    Inspect { artifact: PathBuf },
}

pub fn run(options: &Options, cwd: &Path) -> Result<String> {
    let cwd = crate::project_modes::resolved(cwd)?;
    let result = match &options.command {
        Command::Guide => return Ok(format!("{}\n", D::GUIDE)),
        Command::Build {
            html,
            manifest,
            root,
            out,
            overwrite,
        } => {
            let root = root.as_ref().map(|p| cwd.join(p));
            D::build_files(
                &cwd.join(html),
                &cwd.join(manifest),
                root.as_deref(),
                &cwd.join(out),
                *overwrite,
            )?
        }
        Command::Refresh {
            artifact,
            sources,
            root,
            out,
            overwrite,
        } => {
            let root = root.as_ref().map(|p| cwd.join(p));
            D::refresh_file(
                &cwd.join(artifact),
                &cwd.join(sources),
                root.as_deref(),
                &cwd.join(out),
                *overwrite,
            )?
        }
        Command::Inspect { artifact } => D::inspect_file(&cwd.join(artifact))?,
    };
    Ok(serde_json::to_string_pretty(&result)? + "\n")
}
