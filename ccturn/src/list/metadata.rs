use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::Value;

use crate::list::sessions::{SubagentSummary, ToolStats};
use crate::parser::record::{ContentBlock, parse_session};

pub(crate) struct SessionMetadata {
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub cwd: Option<String>,
    pub ai_title: Option<String>,
    pub first_user_message_excerpt: Option<String>,
    pub open_tool_uses: usize,
    pub last_tool_result_is_error: bool,
    pub subagents: Vec<SubagentSummary>,
    pub read_error: bool,
}

pub(crate) fn extract_session_metadata(path: &Path) -> SessionMetadata {
    let mut metadata = SessionMetadata {
        started_at: None,
        ended_at: None,
        cwd: None,
        ai_title: None,
        first_user_message_excerpt: None,
        open_tool_uses: 0,
        last_tool_result_is_error: false,
        subagents: Vec::new(),
        read_error: false,
    };

    let iter = match parse_session(path) {
        Ok(iter) => iter,
        Err(_) => {
            metadata.read_error = true;
            return metadata;
        }
    };

    let mut open_set: HashSet<String> = HashSet::new();
    let mut pending_tasks: HashMap<String, usize> = HashMap::new();
    let mut order_counter: usize = 0;
    let mut ordered_subagents: Vec<(usize, SubagentSummary)> = Vec::new();

    for record_result in iter {
        let Ok(record) = record_result else {
            continue;
        };

        if let Some(ts) = &record.timestamp {
            if metadata.started_at.is_none() {
                metadata.started_at = Some(ts.clone());
            }
            metadata.ended_at = Some(ts.clone());
        }

        if metadata.cwd.is_none()
            && let Some(c) = &record.cwd
        {
            metadata.cwd = Some(c.clone());
        }

        if record.r#type == "ai-title" {
            if let Some(title) = record.extra.get("title").and_then(|v| v.as_str()) {
                metadata.ai_title = Some(title.to_string());
            } else if let Some(msg) = &record.message
                && let Some(content) = msg.get("content").and_then(|v| v.as_str())
            {
                metadata.ai_title = Some(content.to_string());
            }
        }

        if record.r#type == "user" && metadata.first_user_message_excerpt.is_none() {
            let is_meta = record.is_meta.unwrap_or(false);
            let has_source = record.source_tool_use_id.is_some();
            if !is_meta
                && !has_source
                && let Some(msg) = &record.message
                && let Some(text) = extract_user_text(msg)
                && !text.trim_start().starts_with("<command-name>")
            {
                metadata.first_user_message_excerpt = Some(normalize_excerpt(&text));
            }
        }

        if let Some(msg) = &record.message
            && let Some(Value::Array(blocks)) = msg.get("content")
        {
            for block_value in blocks {
                let Ok(block) = serde_json::from_value::<ContentBlock>(block_value.clone()) else {
                    continue;
                };
                match block {
                    ContentBlock::ToolUse { id, name, .. } => {
                        open_set.insert(id.clone());
                        if name == "Task" {
                            order_counter += 1;
                            pending_tasks.insert(id, order_counter);
                        }
                    }
                    ContentBlock::ToolResult {
                        tool_use_id,
                        is_error,
                        ..
                    } => {
                        open_set.remove(&tool_use_id);
                        metadata.last_tool_result_is_error = is_error;
                        if let Some(order) = pending_tasks.remove(&tool_use_id) {
                            let summary = build_subagent_summary(
                                record.tool_use_result.as_ref(),
                                tool_use_id,
                            );
                            ordered_subagents.push((order, summary));
                        }
                    }
                    ContentBlock::Text { .. } => {}
                }
            }
        }
    }

    for (tool_use_id, order) in pending_tasks {
        let summary = SubagentSummary {
            agent_id: String::new(),
            agent_type: String::new(),
            tool_use_id,
            description: None,
            status: "aborted".to_string(),
            total_duration_ms: None,
            total_tokens: None,
            total_tool_use_count: None,
            tool_stats: None,
            log_path: None,
        };
        ordered_subagents.push((order, summary));
    }

    ordered_subagents.sort_by_key(|(order, _)| *order);
    metadata.subagents = ordered_subagents.into_iter().map(|(_, s)| s).collect();
    metadata.open_tool_uses = open_set.len();

    metadata
}

fn extract_user_text(message: &Value) -> Option<String> {
    let content = message.get("content")?;
    match content {
        Value::String(s) => Some(s.clone()),
        Value::Array(blocks) => {
            for block in blocks {
                if block.get("type").and_then(|t| t.as_str()) == Some("text")
                    && let Some(text) = block.get("text").and_then(|t| t.as_str())
                {
                    return Some(text.to_string());
                }
            }
            None
        }
        _ => None,
    }
}

fn normalize_excerpt(raw: &str) -> String {
    let collapsed: String = raw.replace('\n', " ").trim_start().to_string();
    let chars: Vec<char> = collapsed.chars().collect();
    if chars.len() <= 80 {
        chars.into_iter().collect()
    } else {
        let mut s: String = chars[..79].iter().collect();
        s.push('…');
        s
    }
}

fn build_subagent_summary(payload: Option<&Value>, tool_use_id: String) -> SubagentSummary {
    let null = Value::Null;
    let p = payload.unwrap_or(&null);
    SubagentSummary {
        agent_id: p
            .get("agentId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        agent_type: p
            .get("agentType")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        tool_use_id,
        description: None,
        status: p
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string(),
        total_duration_ms: p.get("totalDurationMs").and_then(|v| v.as_u64()),
        total_tokens: p.get("totalTokens").and_then(|v| v.as_u64()),
        total_tool_use_count: p.get("totalToolUseCount").and_then(|v| v.as_u64()),
        tool_stats: parse_tool_stats(p.get("toolStats")),
        log_path: None,
    }
}

fn parse_tool_stats(v: Option<&Value>) -> Option<ToolStats> {
    let v = v?;
    Some(ToolStats {
        read_count: v.get("readCount").and_then(|x| x.as_u64()).unwrap_or(0),
        search_count: v.get("searchCount").and_then(|x| x.as_u64()).unwrap_or(0),
        bash_count: v.get("bashCount").and_then(|x| x.as_u64()).unwrap_or(0),
        edit_file_count: v.get("editFileCount").and_then(|x| x.as_u64()).unwrap_or(0),
        lines_added: v.get("linesAdded").and_then(|x| x.as_u64()).unwrap_or(0),
        lines_removed: v.get("linesRemoved").and_then(|x| x.as_u64()).unwrap_or(0),
        other_tool_count: v
            .get("otherToolCount")
            .and_then(|x| x.as_u64())
            .unwrap_or(0),
    })
}
