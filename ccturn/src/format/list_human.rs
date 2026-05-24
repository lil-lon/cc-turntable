use std::fs;
use std::path::{Path, PathBuf};

use chrono::DateTime;

use crate::list::projects::ProjectListing;
use crate::list::sessions::{
    SessionListing, SessionRow, SessionStatus, SubagentSummary, ToolStats,
};

pub fn format_projects(listing: &ProjectListing) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Log root  {}   ({} projects)\n",
        listing.log_root.display(),
        listing.projects.len()
    ));
    if listing.projects.is_empty() {
        return out;
    }
    out.push('\n');
    for project in &listing.projects {
        let count = project.session_count;
        let session_word = if count == 1 { "session" } else { "sessions" };
        let latest = match project.latest_session_at {
            Some(dt) => dt.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            None => "none".to_string(),
        };
        let cwd = project
            .project_cwd
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        out.push_str(&format!(
            "  {}   {} {}   latest {}   {}\n",
            project.encoded_cwd, count, session_word, latest, cwd
        ));
    }
    out
}

pub fn format_sessions_default<F>(listing: &SessionListing, subagents_dir_resolver: F) -> String
where
    F: Fn(&str) -> PathBuf,
{
    let mut out = String::new();
    push_session_header(&mut out, listing);

    for (idx, session) in listing.sessions.iter().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        push_session_block(&mut out, session, &subagents_dir_resolver);
    }
    out
}

pub fn format_sessions_oneline(listing: &SessionListing) -> String {
    let mut out = String::new();
    push_session_header(&mut out, listing);

    for session in &listing.sessions {
        let uuid_prefix: String = session.session_id.chars().take(8).collect();
        let status_s = status_str(session.status);
        let date_str = session
            .started_at
            .as_deref()
            .and_then(reformat_to_z)
            .unwrap_or_default();
        let title = compute_title(session);

        let tag = match session.subagents.len() {
            0 => String::new(),
            1 => "   [1 subagent]".to_string(),
            n => format!("   [{n} subagents]"),
        };

        out.push_str(&format!(
            "{uuid_prefix}  {status_s:<8}  {date_str}   {title}{tag}\n"
        ));
    }
    out
}

fn push_session_header(out: &mut String, listing: &SessionListing) {
    let project_cwd_display = listing
        .project_cwd
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    out.push_str(&format!("Project   {project_cwd_display}\n"));
    out.push_str(&format!("Encoded   {}\n", listing.encoded_cwd));
    if listing.session_count > listing.sessions.len() {
        out.push_str(&format!(
            "Sessions  {} total   (showing {})\n",
            listing.session_count,
            listing.sessions.len()
        ));
    } else {
        out.push_str(&format!("Sessions  {} total\n", listing.session_count));
    }
    out.push('\n');
}

fn push_session_block<F>(out: &mut String, session: &SessionRow, resolver: &F)
where
    F: Fn(&str) -> PathBuf,
{
    out.push_str(&format!("session {}\n", session.session_id));
    out.push_str(&format!("Status: {}\n", status_str(session.status)));
    let date_str = session
        .started_at
        .as_deref()
        .and_then(reformat_to_z)
        .unwrap_or_default();
    out.push_str(&format!("Date:   {date_str}\n"));
    out.push('\n');

    let title = compute_title(session);
    out.push_str(&format!("    {title}\n"));

    if !session.subagents.is_empty() {
        out.push('\n');
        out.push_str(&format!("    Subagents ({}):\n", session.subagents.len()));
        let subagents_dir = resolver(&session.session_id);
        push_subagent_tree(out, &session.subagents, &subagents_dir);
    }
}

fn push_subagent_tree(out: &mut String, subagents: &[SubagentSummary], subagents_dir: &Path) {
    let total = subagents.len();
    for (idx, sub) in subagents.iter().enumerate() {
        let is_last = idx == total - 1;
        let prefix = if is_last { "└─" } else { "├─" };
        let cont_prefix = if is_last { "   " } else { "│  " };

        let duration = match sub.total_duration_ms {
            Some(ms) => format!("{:.1}s", ms as f64 / 1000.0),
            None => "-".to_string(),
        };
        let tokens = match sub.total_tokens {
            Some(t) => format!("{t} tok"),
            None => "- tok".to_string(),
        };
        let dom_tool = sub.tool_stats.as_ref().and_then(dominant_tool);
        let token_col = match dom_tool {
            Some(t) => format!("{tokens}   {t}"),
            None => tokens,
        };

        out.push_str(&format!(
            "      {} {} {}   {}   {}   {}\n",
            prefix, sub.agent_type, sub.agent_id, sub.status, duration, token_col
        ));

        let description = read_meta_description(subagents_dir, &sub.agent_id)
            .unwrap_or_else(|| "(no description)".to_string());
        out.push_str(&format!("      {cont_prefix}     {description}\n"));
    }
}

fn read_meta_description(subagents_dir: &Path, agent_id: &str) -> Option<String> {
    let path = subagents_dir.join(format!("agent-{agent_id}.meta.json"));
    let content = fs::read_to_string(&path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&content).ok()?;
    let description = value.get("description").and_then(|v| v.as_str())?;
    Some(truncate_to(description, 80))
}

fn truncate_to(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        chars.into_iter().collect()
    } else {
        let mut out: String = chars[..max - 1].iter().collect();
        out.push('…');
        out
    }
}

fn dominant_tool(stats: &ToolStats) -> Option<String> {
    let candidates = [
        ("bash", stats.bash_count),
        ("read", stats.read_count),
        ("search", stats.search_count),
        ("edit", stats.edit_file_count),
    ];
    let mut best: Option<(&str, u64)> = None;
    for (name, count) in candidates {
        if count == 0 {
            continue;
        }
        let better = match best {
            None => true,
            Some((_, bc)) => count > bc,
        };
        if better {
            best = Some((name, count));
        }
    }
    best.map(|(n, c)| format!("{n}={c}"))
}

fn status_str(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Success => "success",
        SessionStatus::Error => "error",
        SessionStatus::Aborted => "aborted",
        SessionStatus::Unknown => "unknown",
    }
}

fn compute_title(session: &SessionRow) -> String {
    if let Some(title) = &session.ai_title
        && !title.is_empty()
    {
        return title.clone();
    }
    if let Some(excerpt) = &session.first_user_message_excerpt
        && !excerpt.is_empty()
    {
        return format!(r#"(untitled) "{excerpt}""#);
    }
    if session.status == SessionStatus::Unknown {
        return "(could not read session)".to_string();
    }
    "(no content)".to_string()
}

fn reformat_to_z(raw: &str) -> Option<String> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
}
