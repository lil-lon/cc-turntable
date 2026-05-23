pub mod errors;
pub mod interventions;
pub mod skills;
pub mod subagents;
pub mod tools;

use serde_json::Value;

use crate::parser::record::{ContentBlock, JsonlRecord};

// Per the canonical Messages API tool-use loop, `tool_use` blocks live on
// assistant records and `tool_result` blocks live on the FOLLOWING user record
// the application sends back. Claude Code's JSONL preserves that split: the
// per-line `type` field mirrors `message.role`, so the canonical assignment is
// `type == "assistant"` carries tool_use and `type == "user"` carries
// tool_result. The helpers do NOT enforce that split, however: the block-type
// filter (matches! on `ContentBlock::ToolUse` / `::ToolResult`) is sufficient
// on its own, and being permissive about the enclosing record type hedges
// against non-canonical or future Claude Code session shapes (e.g. legacy logs
// whose parser tolerance accepts assistant-side `tool_result`).
//
// Both helpers re-deserialise `message.content[]` into typed `ContentBlock`
// variants on demand. Per-element deserialise + filter_map keeps the pass
// tolerant: an unknown future block type silently drops out instead of killing
// the whole record.

pub(crate) fn tool_use_blocks(record: &JsonlRecord) -> Vec<ContentBlock> {
    content_blocks_filtered(record, |block| {
        matches!(block, ContentBlock::ToolUse { .. })
    })
}

pub(crate) fn tool_result_blocks(record: &JsonlRecord) -> Vec<ContentBlock> {
    content_blocks_filtered(record, |block| {
        matches!(block, ContentBlock::ToolResult { .. })
    })
}

fn content_blocks_filtered(
    record: &JsonlRecord,
    keep: impl Fn(&ContentBlock) -> bool,
) -> Vec<ContentBlock> {
    let Some(message) = &record.message else {
        return Vec::new();
    };
    let Some(content) = message.get("content").and_then(|c| c.as_array()) else {
        return Vec::new();
    };
    content
        .iter()
        .filter_map(|el| serde_json::from_value::<ContentBlock>(el.clone()).ok())
        .filter(|block| keep(block))
        .collect()
}

// True when a record's `message.content` carries a `tool_result` block (i.e.,
// this record is the harness echoing a tool result back to the model, NOT a
// real human input). The mid-stream intervention rule uses this to exclude
// harness echoes that happen to satisfy the other three clauses.
pub(crate) fn user_record_carries_tool_result(record: &JsonlRecord) -> bool {
    !tool_result_blocks(record).is_empty()
}

// A tool_result's `content` is a JSON `Value` (usually a string, occasionally a
// structured payload). Render it to a plain string for prefix matching and
// excerpting.
pub(crate) fn value_to_content_string(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}
