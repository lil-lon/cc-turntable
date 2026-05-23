use std::collections::HashMap;
use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::extract::assistant_content_blocks;
use crate::parser::record::{ContentBlock, JsonlRecord};

#[derive(Debug, Clone, Default, Serialize)]
pub struct ToolStats {
    pub read_count: u64,
    pub search_count: u64,
    pub bash_count: u64,
    pub edit_file_count: u64,
    pub lines_added: u64,
    pub lines_removed: u64,
    pub other_tool_count: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SubagentInvocation {
    pub agent_id: String,
    pub agent_type: String,
    pub description: String,
    pub tool_use_id: String,
    pub status: String,
    pub total_duration_ms: u64,
    pub total_tokens: u64,
    pub total_tool_use_count: u64,
    pub tool_stats: ToolStats,
    pub log_path: String,
}

// The on-disk `toolUseResult.toolStats` payload uses camelCase keys
// (`readCount`, `bashCount`, …); we manually translate to snake_case so the
// output JSON matches § JSON Output Schema verbatim, while keeping serde's
// default serialize on `ToolStats` (no `rename_all`).
fn parse_tool_stats(v: &Value) -> ToolStats {
    let pick = |key: &str| v.get(key).and_then(|x| x.as_u64()).unwrap_or(0);
    ToolStats {
        read_count: pick("readCount"),
        search_count: pick("searchCount"),
        bash_count: pick("bashCount"),
        edit_file_count: pick("editFileCount"),
        lines_added: pick("linesAdded"),
        lines_removed: pick("linesRemoved"),
        other_tool_count: pick("otherToolCount"),
    }
}

pub fn extract_subagents(records: &[JsonlRecord], subagents_dir: &Path) -> Vec<SubagentInvocation> {
    // tool_use_id -> parent record's top-level `tool_use_result` Value. The
    // toolUseResult payload sits on the assistant record that wraps the
    // tool_result content block.
    let mut tool_use_result_map: HashMap<String, Value> = HashMap::new();
    for record in records {
        for block in assistant_content_blocks(record) {
            if let ContentBlock::ToolResult { tool_use_id, .. } = block
                && let Some(tur) = &record.tool_use_result
            {
                tool_use_result_map.insert(tool_use_id, tur.clone());
            }
        }
    }

    let mut out: Vec<SubagentInvocation> = Vec::new();
    for record in records {
        for block in assistant_content_blocks(record) {
            let ContentBlock::ToolUse { id, name, .. } = block else {
                continue;
            };
            if name != "Task" {
                continue;
            }
            let Some(tur) = tool_use_result_map.get(&id) else {
                continue;
            };

            let agent_id = tur
                .get("agentId")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let agent_type = tur
                .get("agentType")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let status = tur
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let total_duration_ms = tur
                .get("totalDurationMs")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let total_tokens = tur.get("totalTokens").and_then(|v| v.as_u64()).unwrap_or(0);
            let total_tool_use_count = tur
                .get("totalToolUseCount")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let tool_stats = tur
                .get("toolStats")
                .map(parse_tool_stats)
                .unwrap_or_default();

            let agent_log = subagents_dir.join(format!("agent-{agent_id}.jsonl"));
            let meta_path = subagents_dir.join(format!("agent-{agent_id}.meta.json"));
            let log_exists = agent_log.exists();
            let meta_exists = meta_path.exists();

            let log_path = if log_exists {
                agent_log.to_string_lossy().into_owned()
            } else {
                // Per Director arbitration on Step 5: warn only when the meta
                // sidecar IS present (case c — true orphan reference); if
                // both meta and jsonl are missing (case d), stay silent.
                if meta_exists {
                    eprintln!(
                        "warning: subagent {} referenced by parent toolUseResult but no log file at {}",
                        agent_id,
                        agent_log.display()
                    );
                }
                String::new()
            };

            let description = if meta_exists {
                std::fs::read_to_string(&meta_path)
                    .ok()
                    .and_then(|content| serde_json::from_str::<Value>(&content).ok())
                    .and_then(|v| {
                        v.get("description")
                            .and_then(|d| d.as_str())
                            .map(str::to_owned)
                    })
                    .unwrap_or_default()
            } else {
                String::new()
            };

            out.push(SubagentInvocation {
                agent_id,
                agent_type,
                description,
                tool_use_id: id,
                status,
                total_duration_ms,
                total_tokens,
                total_tool_use_count,
                tool_stats,
                log_path,
            });
        }
    }
    out
}

// Phase A test specification for the Subagent extractor (design doc § Implementation
// :: Step 5). The Programmer adds the production code ABOVE this `#[cfg(test)]`
// block in Phase B:
//   - `pub struct SubagentInvocation { agent_id, agent_type, description,
//      tool_use_id, status, total_duration_ms, total_tokens,
//      total_tool_use_count, tool_stats, log_path }` matching the
//     § JSON Output Schema entry for `subagents[]`.
//   - `pub struct ToolStats { read_count, search_count, bash_count,
//      edit_file_count, lines_added, lines_removed, other_tool_count }`.
//     The on-disk `toolUseResult.toolStats` payload uses camelCase keys
//     (`readCount`, `bashCount`, …); Programmer chooses how to bridge that
//     to the snake_case output schema (e.g. a separate wire struct).
//   - `pub fn extract_subagents(records: &[JsonlRecord], subagents_dir: &Path)
//      -> Vec<SubagentInvocation>` — scan for `tool_use.name == "Task"`,
//     locate the matching `tool_result`, pull the parent record's top-level
//     `toolUseResult` payload, and join with the on-disk meta.json sidecar
//     for `description` and the agent JSONL for `log_path` per § Error Handling.

#[cfg(test)]
mod tests {
    use super::{SubagentInvocation, extract_subagents};
    use crate::parser::record::{JsonlRecord, parse_session};
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    const AGENT_ID: &str = "agent-id-1";
    const TOOL_USE_ID: &str = "toolu_TASK";

    fn load_session_records() -> Vec<JsonlRecord> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/subagent-invocation.jsonl");
        parse_session(&path)
            .collect::<anyhow::Result<Vec<_>>>()
            .expect("fixture must parse cleanly")
    }

    fn write_meta_json(subagents_dir: &Path, agent_id: &str, description: &str) {
        let path = subagents_dir.join(format!("agent-{agent_id}.meta.json"));
        let body = serde_json::json!({
            "agentType": "Explore",
            "description": description,
            "toolUseId": TOOL_USE_ID,
        });
        fs::write(&path, body.to_string()).expect("write meta.json");
    }

    fn write_agent_jsonl(subagents_dir: &Path, agent_id: &str) {
        let path = subagents_dir.join(format!("agent-{agent_id}.jsonl"));
        fs::write(&path, "").expect("write agent jsonl");
    }

    fn expected_log_path(subagents_dir: &Path, agent_id: &str) -> String {
        subagents_dir
            .join(format!("agent-{agent_id}.jsonl"))
            .to_string_lossy()
            .into_owned()
    }

    fn assert_tool_use_result_fields(subagent: &SubagentInvocation) {
        // Every case in the 4-case matrix must surface the same
        // toolUseResult-derived fields — they come from the parent session
        // JSONL, not from the on-disk sidecars.
        assert_eq!(subagent.agent_id, AGENT_ID);
        assert_eq!(subagent.agent_type, "Explore");
        assert_eq!(subagent.tool_use_id, TOOL_USE_ID);
        assert_eq!(subagent.status, "completed");
        assert_eq!(subagent.total_duration_ms, 11588);
        assert_eq!(subagent.total_tokens, 20792);
        assert_eq!(subagent.total_tool_use_count, 4);
        assert_eq!(subagent.tool_stats.bash_count, 4);
        assert_eq!(subagent.tool_stats.read_count, 0);
        assert_eq!(subagent.tool_stats.search_count, 0);
        assert_eq!(subagent.tool_stats.edit_file_count, 0);
        assert_eq!(subagent.tool_stats.lines_added, 0);
        assert_eq!(subagent.tool_stats.lines_removed, 0);
        assert_eq!(subagent.tool_stats.other_tool_count, 0);
    }

    // ---------------------------------------------------------------------
    // Case (a) — full pair present. `meta.json` populates `description`;
    // `log_path` points at `<subagents_dir>/agent-<agentId>.jsonl`.
    // ---------------------------------------------------------------------

    #[test]
    fn case_a_full_pair_present_populates_description_and_log_path() {
        let records = load_session_records();
        let tmp = TempDir::new().unwrap();
        write_meta_json(tmp.path(), AGENT_ID, "Verify Skill and Task tool fields");
        write_agent_jsonl(tmp.path(), AGENT_ID);

        let subagents = extract_subagents(&records, tmp.path());
        assert_eq!(
            subagents.len(),
            1,
            "exactly one Task tool_use in the fixture"
        );
        let subagent = &subagents[0];
        assert_tool_use_result_fields(subagent);
        assert_eq!(
            subagent.description, "Verify Skill and Task tool fields",
            "description must be read verbatim from meta.json"
        );
        assert_eq!(
            subagent.log_path,
            expected_log_path(tmp.path(), AGENT_ID),
            "log_path = <subagents_dir>/agent-<agentId>.jsonl when the file exists"
        );
    }

    // ---------------------------------------------------------------------
    // Case (b) — meta.json missing, agent JSONL present.
    // Per § Error Handling: `description = ""` but `log_path` is preserved
    // (meta-sidecar absence does NOT invalidate the drill-down link).
    // ---------------------------------------------------------------------

    #[test]
    fn case_b_meta_missing_but_log_present_keeps_log_path_and_blanks_description() {
        let records = load_session_records();
        let tmp = TempDir::new().unwrap();
        write_agent_jsonl(tmp.path(), AGENT_ID);

        let subagents = extract_subagents(&records, tmp.path());
        assert_eq!(subagents.len(), 1);
        let subagent = &subagents[0];
        assert_tool_use_result_fields(subagent);
        assert_eq!(
            subagent.description, "",
            "missing meta.json => description = \"\""
        );
        assert_eq!(
            subagent.log_path,
            expected_log_path(tmp.path(), AGENT_ID),
            "missing meta.json must NOT invalidate the drill-down log_path; \
             the agent JSONL is still present so the link is still valid"
        );
    }

    // ---------------------------------------------------------------------
    // Case (c) — meta.json present, agent JSONL missing.
    // Per § Error Handling: `log_path = ""` (the report layer will surface
    // a `warning: subagent ... no log file at ...` to stderr, but the
    // extractor's contract is to emit the empty string).
    // ---------------------------------------------------------------------

    #[test]
    fn case_c_meta_present_but_log_missing_emits_empty_log_path() {
        let records = load_session_records();
        let tmp = TempDir::new().unwrap();
        write_meta_json(tmp.path(), AGENT_ID, "Verify Skill and Task tool fields");

        let subagents = extract_subagents(&records, tmp.path());
        assert_eq!(subagents.len(), 1);
        let subagent = &subagents[0];
        assert_tool_use_result_fields(subagent);
        assert_eq!(
            subagent.description, "Verify Skill and Task tool fields",
            "description from meta.json must still populate when the agent JSONL is missing"
        );
        assert_eq!(
            subagent.log_path, "",
            "missing agent JSONL => log_path = \"\" (the only condition that justifies an empty log_path)"
        );
    }

    // ---------------------------------------------------------------------
    // Case (d) — both meta.json and agent JSONL missing.
    // `description` and `log_path` both empty; the toolUseResult fields
    // still populate because they come from the parent session JSONL.
    // ---------------------------------------------------------------------

    #[test]
    fn case_d_both_meta_and_log_missing_emits_empty_description_and_log_path() {
        let records = load_session_records();
        let tmp = TempDir::new().unwrap();
        // Deliberately do not write either file.

        let subagents = extract_subagents(&records, tmp.path());
        assert_eq!(subagents.len(), 1);
        let subagent = &subagents[0];
        assert_tool_use_result_fields(subagent);
        assert_eq!(subagent.description, "");
        assert_eq!(subagent.log_path, "");
    }
}
