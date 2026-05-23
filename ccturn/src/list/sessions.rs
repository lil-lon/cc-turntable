use std::cmp::Reverse;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::anyhow;
use chrono::{DateTime, FixedOffset};
use serde::Serialize;

use crate::list::metadata::{SessionMetadata, extract_session_metadata};
use crate::locator::{CwdSource, read_first_cwd_in_session, reconstruct_cwd_from_encoded};

#[derive(Serialize)]
pub struct SessionListing {
    pub log_root: PathBuf,
    pub encoded_cwd: String,
    pub project_cwd: Option<PathBuf>,
    pub cwd_source: CwdSource,
    pub session_count: usize,
    pub limit: usize,
    pub sessions: Vec<SessionRow>,
}

#[derive(Serialize)]
pub struct SessionRow {
    pub session_id: String,
    pub log_path: PathBuf,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub status: SessionStatus,
    pub ai_title: Option<String>,
    pub first_user_message_excerpt: Option<String>,
    pub open_tool_uses: usize,
    pub last_tool_result_is_error: bool,
    pub subagents: Vec<SubagentSummary>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Success,
    Error,
    Aborted,
    Unknown,
}

#[derive(Serialize)]
pub struct SubagentSummary {
    pub agent_id: String,
    pub agent_type: String,
    pub tool_use_id: String,
    pub description: Option<String>,
    pub status: String,
    pub total_duration_ms: Option<u64>,
    pub total_tokens: Option<u64>,
    pub total_tool_use_count: Option<u64>,
    pub tool_stats: Option<ToolStats>,
    pub log_path: Option<PathBuf>,
}

#[derive(Serialize)]
pub struct ToolStats {
    pub read_count: u64,
    pub search_count: u64,
    pub bash_count: u64,
    pub edit_file_count: u64,
    pub lines_added: u64,
    pub lines_removed: u64,
    pub other_tool_count: u64,
}

pub fn list_sessions(
    log_root: &Path,
    encoded_cwd: &str,
    limit: Option<usize>,
) -> anyhow::Result<SessionListing> {
    let project_dir = log_root.join(encoded_cwd);
    if !project_dir.exists() {
        return Err(anyhow!(
            "project {} not found under {}",
            encoded_cwd,
            log_root.display()
        ));
    }

    let jsonl_paths = read_jsonl_children(&project_dir);

    let mut rows: Vec<SessionRow> = jsonl_paths
        .iter()
        .map(|path| build_session_row(path))
        .collect();

    rows.sort_by(|a, b| {
        let a_dt = parse_started_at(a.started_at.as_deref());
        let b_dt = parse_started_at(b.started_at.as_deref());
        Reverse(a_dt)
            .cmp(&Reverse(b_dt))
            .then_with(|| a.session_id.cmp(&b.session_id))
    });

    let session_count = rows.len();
    let applied_limit = limit.unwrap_or(0);

    if let Some(l) = limit {
        rows.truncate(l);
    }

    let (project_cwd, cwd_source) = resolve_project_cwd(&jsonl_paths, encoded_cwd);

    Ok(SessionListing {
        log_root: log_root.to_path_buf(),
        encoded_cwd: encoded_cwd.to_string(),
        project_cwd,
        cwd_source,
        session_count,
        limit: applied_limit,
        sessions: rows,
    })
}

fn parse_started_at(raw: Option<&str>) -> Option<DateTime<FixedOffset>> {
    raw.and_then(|s| DateTime::parse_from_rfc3339(s).ok())
}

fn build_session_row(path: &Path) -> SessionRow {
    let session_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let metadata = extract_session_metadata(path);
    let status = compute_status(&metadata);
    SessionRow {
        session_id,
        log_path: path.to_path_buf(),
        started_at: metadata.started_at,
        ended_at: metadata.ended_at,
        status,
        ai_title: metadata.ai_title,
        first_user_message_excerpt: metadata.first_user_message_excerpt,
        open_tool_uses: metadata.open_tool_uses,
        last_tool_result_is_error: metadata.last_tool_result_is_error,
        subagents: metadata.subagents,
    }
}

fn compute_status(metadata: &SessionMetadata) -> SessionStatus {
    if metadata.read_error {
        SessionStatus::Unknown
    } else if metadata.open_tool_uses > 0 {
        SessionStatus::Aborted
    } else if metadata.last_tool_result_is_error {
        SessionStatus::Error
    } else {
        SessionStatus::Success
    }
}

fn read_jsonl_children(project_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(project_dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let ft = entry.file_type().ok()?;
            if !ft.is_file() {
                return None;
            }
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                return None;
            }
            Some(path)
        })
        .collect()
}

fn resolve_project_cwd(jsonl_paths: &[PathBuf], encoded_cwd: &str) -> (Option<PathBuf>, CwdSource) {
    let mut newest: Option<(SystemTime, &PathBuf)> = None;
    for path in jsonl_paths {
        let Ok(metadata) = fs::metadata(path) else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        match &newest {
            Some((current, _)) if *current >= modified => {}
            _ => newest = Some((modified, path)),
        }
    }

    match newest.and_then(|(_, path)| read_first_cwd_in_session(path)) {
        Some(cwd) => (Some(PathBuf::from(cwd)), CwdSource::FirstRecord),
        None => (
            Some(PathBuf::from(reconstruct_cwd_from_encoded(encoded_cwd))),
            CwdSource::ReconstructedFromEncodedCwd,
        ),
    }
}
