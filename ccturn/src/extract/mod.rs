pub mod errors;
pub mod interventions;
pub mod skills;
pub mod subagents;
pub mod tools;

use serde_json::Value;

use crate::parser::record::{ContentBlock, JsonlRecord};

// `JsonlRecord.message` is typed as `Value` so the parser tolerates schema drift
// at the top level. Re-deserialise `message.content[]` into typed `ContentBlock`
// variants on demand. Per-element deserialise + filter_map keeps the pass
// tolerant: an unknown future block type silently drops out instead of killing
// the whole record.
pub(crate) fn assistant_content_blocks(record: &JsonlRecord) -> Vec<ContentBlock> {
    if record.r#type != "assistant" {
        return Vec::new();
    }
    let Some(message) = &record.message else {
        return Vec::new();
    };
    let Some(content) = message.get("content").and_then(|c| c.as_array()) else {
        return Vec::new();
    };
    content
        .iter()
        .filter_map(|el| serde_json::from_value::<ContentBlock>(el.clone()).ok())
        .collect()
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
