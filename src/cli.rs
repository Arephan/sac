//! Command-line surface. Behaviour lives in `main.rs` and the library modules.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "sac",
    version,
    about = "setup-as-code: one YAML manifest for a personal AI assistant setup. import, plan, verify, apply, lint.",
    long_about = None,
    propagate_version = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Cmd,
    /// Instance folder (or a setup.yaml path). Default: $SAC_INSTANCE, ./setup.yaml, or the only folder in ./instance/
    #[arg(short = 'i', long, global = true)]
    pub instance: Option<String>,
    /// Read the machine as if this directory were $HOME (sandbox a "new person")
    #[arg(long, global = true)]
    pub home: Option<PathBuf>,
    /// Machine-readable output
    #[arg(long, global = true)]
    pub json: bool,
    /// Show every change / every piece of evidence
    #[arg(short, long, global = true)]
    pub verbose: bool,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Scaffold a new person's setup (setup.yaml, policies.yaml, overrides.yaml, lint-denylist.txt)
    Init {
        dir: PathBuf,
        /// Instance name (default: the folder name)
        #[arg(long)]
        name: Option<String>,
        /// Overwrite files that already exist
        #[arg(long)]
        force: bool,
    },
    /// Read the machine (read-only) plus overrides.yaml, rewrite <instance>/setup.yaml
    Import {
        /// Show counts only, write nothing
        #[arg(long)]
        dry_run: bool,
    },
    /// Schema, duplicate-id and cross-reference checks
    Validate,
    /// Diff setup.yaml against a fresh read of the machine; writes nothing
    Plan,
    /// Plan plus safety policies on desired and live state; exit 1 on a hard failure
    Verify {
        /// Exit 2 when the machine drifts from setup.yaml
        #[arg(long)]
        fail_on_drift: bool,
    },
    /// Render config into <out>/<instance>/; with --live, merge into the home directory
    Apply {
        /// Output root for rendered files (default: ./out)
        #[arg(long)]
        out: Option<PathBuf>,
        /// Write into the live home (settings, MCP servers, launchd plists) with backups
        #[arg(long)]
        live: bool,
        /// Required with --live
        #[arg(long)]
        yes: bool,
        /// With --live: overwrite existing plists and MCP server definitions
        #[arg(long)]
        force: bool,
    },
    /// Secret and personal-data lint over the shareable tree
    Lint {
        /// Files or directories to scan (default: the working directory minus target/, out/, instance/)
        paths: Vec<PathBuf>,
        /// Folder holding instance/<name>/lint-denylist.txt files (default: ./instance)
        #[arg(long)]
        instance_root: Option<PathBuf>,
    },
    /// Write a shape-only, lint-clean copy of an instance
    Redact {
        #[arg(long)]
        out: PathBuf,
    },
    /// List the layer definitions
    Layers,
    /// List the available importers (built-in plus the instance's executables)
    Importers,
}
