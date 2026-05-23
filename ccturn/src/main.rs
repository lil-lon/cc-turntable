use std::path::PathBuf;
use std::process::ExitCode;

use clap::error::ErrorKind;
use clap::{Parser, Subcommand};

use ccturn::format::human::format_human;
use ccturn::format::json::format_json;
use ccturn::locator::{default_log_root, resolve};
use ccturn::report::build_report;

#[derive(Parser)]
#[command(name = "ccturn", about = "Claude Code session inspector")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Play through one session — surface skills, subagents, tools, errors, interventions.
    Spin {
        /// Full session UUID (the .jsonl filename without the suffix).
        session_id: String,
        /// Limit the locator scan to one project subdirectory (an encoded-cwd token).
        #[arg(long)]
        project: Option<String>,
        /// Emit the report as a single JSON object on stdout instead of human text.
        #[arg(long)]
        json: bool,
        /// Override the log root (default: $CLAUDE_CONFIG_DIR/projects, fallback ~/.claude/projects).
        #[arg(long)]
        log_root: Option<PathBuf>,
    },
    /// List every project directory under the log root with session counts and latest timestamps.
    Crates {
        /// Emit the listing as a single JSON object on stdout instead of human text.
        #[arg(long)]
        json: bool,
        /// Override the log root (default: $CLAUDE_CONFIG_DIR/projects, fallback ~/.claude/projects).
        #[arg(long)]
        log_root: Option<PathBuf>,
    },
    /// List the sessions in one project with timestamp / status / one-line summary.
    Tracks {
        /// Encoded-cwd token naming the project directory under the log root.
        project: String,
        /// Cap the row count after sorting (git-log -n analogue).
        #[arg(short = 'n', long = "limit")]
        limit: Option<usize>,
        /// One row per session in a compact format (git-log --oneline analogue).
        #[arg(long, conflicts_with = "json")]
        oneline: bool,
        /// Emit the listing as a single JSON object on stdout instead of human text.
        #[arg(long, conflicts_with = "oneline")]
        json: bool,
        /// Override the log root (default: $CLAUDE_CONFIG_DIR/projects, fallback ~/.claude/projects).
        #[arg(long)]
        log_root: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    // clap usage errors exit 64 per § CLI Surface; an explicit --help / --version
    // is not an error and exits 0.
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            let _ = e.print();
            return match e.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => ExitCode::SUCCESS,
                _ => ExitCode::from(64),
            };
        }
    };

    match cli.command {
        Command::Spin {
            session_id,
            project,
            json,
            log_root,
        } => run_spin(&session_id, project.as_deref(), json, log_root),
        Command::Crates { json, log_root } => run_crates(json, log_root),
        Command::Tracks {
            project,
            limit,
            oneline,
            json,
            log_root,
        } => run_tracks(&project, limit, oneline, json, log_root),
    }
}

fn run_spin(
    session_id: &str,
    project: Option<&str>,
    json: bool,
    log_root: Option<PathBuf>,
) -> ExitCode {
    let log_root = log_root.unwrap_or_else(default_log_root);

    // Locator failures (not found / ambiguous / missing log root) all exit 1.
    let resolved = match resolve(&log_root, session_id, project) {
        Ok(resolved) => resolved,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(1);
        }
    };

    // Parser failures that prevent producing a report exit 2.
    let report = match build_report(&resolved) {
        Ok(report) => report,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };

    if json {
        println!("{}", format_json(&report));
    } else {
        // `format_human` already ends with a newline; `print!` (not `println!`)
        // avoids a spurious trailing blank line.
        print!("{}", format_human(&report));
    }
    ExitCode::SUCCESS
}

fn run_crates(_json: bool, log_root: Option<PathBuf>) -> ExitCode {
    let log_root = log_root.unwrap_or_else(default_log_root);
    if !log_root.exists() {
        eprintln!("error: log root {} does not exist", log_root.display());
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

fn run_tracks(
    _project: &str,
    _limit: Option<usize>,
    _oneline: bool,
    _json: bool,
    log_root: Option<PathBuf>,
) -> ExitCode {
    let log_root = log_root.unwrap_or_else(default_log_root);
    if !log_root.exists() {
        eprintln!("error: log root {} does not exist", log_root.display());
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}
