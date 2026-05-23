use std::collections::HashMap;

use serde::Serialize;
use serde_json::Value;

use crate::extract::{assistant_content_blocks, value_to_content_string};
use crate::parser::record::{ContentBlock, JsonlRecord};

// Serde's default enum serialisation emits the variant name verbatim, which is
// exactly the four strings § JSON Output Schema's `errors[].category` expects —
// so no `rename_all` is needed here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ErrorCategory {
    UserRejection,
    PermissionDenied,
    HookBlock,
    Technical,
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorRecord {
    pub category: ErrorCategory,
    pub tool_name: String,
    pub tool_use_id: String,
    pub excerpt: String,
    pub timestamp: String,
}

const USER_REJECTED: &str = "User rejected tool use";
const EXCERPT_MAX_CHARS: usize = 200;

// Type-guard (§ Errors): UserRejection matches ONLY when the enclosing record's
// top-level `toolUseResult` is a JSON *string* equal to "User rejected tool
// use". An object-valued `toolUseResult` (Skill / Task payloads) must fall
// through to the prefix checks — so we match `Value::String` explicitly rather
// than stringifying the value first.
fn classify(tool_use_result: Option<&Value>, content: &str) -> ErrorCategory {
    if let Some(Value::String(s)) = tool_use_result
        && s == USER_REJECTED
    {
        return ErrorCategory::UserRejection;
    }
    if content.starts_with("Permission to use ") {
        ErrorCategory::PermissionDenied
    } else if content.starts_with("PreToolUse:") {
        ErrorCategory::HookBlock
    } else {
        ErrorCategory::Technical
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

pub fn extract_errors(records: &[JsonlRecord]) -> Vec<ErrorRecord> {
    // tool_use_id -> tool_name, for resolving the originating tool of each error.
    let mut tool_name_map: HashMap<String, String> = HashMap::new();
    for record in records {
        for block in assistant_content_blocks(record) {
            if let ContentBlock::ToolUse { id, name, .. } = block {
                tool_name_map.insert(id, name);
            }
        }
    }

    let mut out: Vec<ErrorRecord> = Vec::new();
    for record in records {
        for block in assistant_content_blocks(record) {
            let ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
                ..
            } = block
            else {
                continue;
            };
            if !is_error {
                continue;
            }
            let content_string = value_to_content_string(&content);
            let category = classify(record.tool_use_result.as_ref(), &content_string);
            let tool_name = tool_name_map
                .get(&tool_use_id)
                .cloned()
                .unwrap_or_else(|| "<unknown>".to_string());
            out.push(ErrorRecord {
                category,
                tool_name,
                tool_use_id,
                excerpt: truncate_chars(&content_string, EXCERPT_MAX_CHARS),
                timestamp: record.timestamp.clone().unwrap_or_default(),
            });
        }
    }
    out
}

// Phase A test specification for the Error classifier (design doc § Implementation
// :: Step 7). The Programmer adds the production code ABOVE this `#[cfg(test)]`
// block in Phase B:
//   - `pub enum ErrorCategory { UserRejection, PermissionDenied, HookBlock,
//      Technical }` — serialises to the four exact strings in § JSON Output
//     Schema's `errors[].category`.
//   - `pub struct ErrorRecord { category, tool_name, tool_use_id, excerpt,
//      timestamp }` matching the `errors[]` schema entry.
//   - `pub fn extract_errors(records: &[JsonlRecord]) -> Vec<ErrorRecord>` —
//     filter `tool_result` blocks with `is_error: true`, classify by the
//     ordered rules (UserRejection -> PermissionDenied -> HookBlock ->
//     Technical, first match wins), resolve `tool_name` via `tool_use_id`
//     (unknown ids -> the literal "<unknown>"), and truncate `content` to
//     the first 200 chars for `excerpt`.
//
// Type-guard (§ Errors): UserRejection matches ONLY when the enclosing
// record's top-level `toolUseResult` is a JSON string equal to
// "User rejected tool use". An object-valued `toolUseResult` (Skill /
// Task payloads) must never match that rule.

#[cfg(test)]
mod tests {
    use super::{ErrorCategory, ErrorRecord, extract_errors};
    use crate::parser::record::{JsonlRecord, parse_session};
    use std::path::PathBuf;

    fn load_records(fixture: &str) -> Vec<JsonlRecord> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(fixture);
        parse_session(&path)
            .unwrap_or_else(|e| panic!("fixture {fixture} must open: {e}"))
            .collect::<anyhow::Result<Vec<_>>>()
            .unwrap_or_else(|e| panic!("fixture {fixture} must parse: {e}"))
    }

    fn find<'a>(errors: &'a [ErrorRecord], tool_use_id: &str) -> &'a ErrorRecord {
        errors
            .iter()
            .find(|e| e.tool_use_id == tool_use_id)
            .unwrap_or_else(|| panic!("no ErrorRecord with tool_use_id {tool_use_id}"))
    }

    // ---------------------------------------------------------------------
    // One error per category. `errors-by-category.jsonl` contains exactly
    // four error tool_results, one matching each classification rule
    // cleanly. The extractor must emit them in document order.
    // ---------------------------------------------------------------------

    #[test]
    fn classifies_one_error_per_category() {
        let records = load_records("errors-by-category.jsonl");
        let errors = extract_errors(&records);
        assert_eq!(
            errors.len(),
            4,
            "fixture has exactly four error tool_results"
        );

        // UserRejection — toolUseResult is the JSON string "User rejected
        // tool use" on the enclosing record.
        let rejection = &errors[0];
        assert_eq!(rejection.category, ErrorCategory::UserRejection);
        assert_eq!(rejection.tool_name, "Bash");
        assert_eq!(rejection.tool_use_id, "tu_reject");
        assert_eq!(
            rejection.excerpt, "The user doesn't want to proceed with this tool use.",
            "excerpt is the tool_result content; short content is NOT truncated"
        );
        assert_eq!(
            rejection.timestamp, "2026-05-19T09:14:02.000000+00:00",
            "timestamp is the enclosing record's timestamp, verbatim"
        );

        // PermissionDenied — content starts with "Permission to use ".
        let permission = &errors[1];
        assert_eq!(permission.category, ErrorCategory::PermissionDenied);
        assert_eq!(permission.tool_name, "Edit");
        assert_eq!(permission.tool_use_id, "tu_perm");

        // HookBlock — content starts with "PreToolUse:".
        let hook = &errors[2];
        assert_eq!(hook.category, ErrorCategory::HookBlock);
        assert_eq!(hook.tool_name, "Bash");
        assert_eq!(hook.tool_use_id, "tu_hook");

        // Technical — none of the above prefixes.
        let technical = &errors[3];
        assert_eq!(technical.category, ErrorCategory::Technical);
        assert_eq!(technical.tool_name, "Read");
        assert_eq!(technical.tool_use_id, "tu_tech");
        assert_eq!(
            technical.excerpt, "exit 1: file not found",
            "short Technical content round-trips verbatim into excerpt"
        );
    }

    // ---------------------------------------------------------------------
    // Classification order — first match wins. The `tu_order` error has a
    // `content` starting with "Permission to use " (PermissionDenied rule)
    // AND an enclosing `toolUseResult` string "User rejected tool use"
    // (UserRejection rule). UserRejection is checked first, so it wins.
    // ---------------------------------------------------------------------

    #[test]
    fn classification_order_user_rejection_wins_over_permission_denied() {
        let records = load_records("errors-edge-cases.jsonl");
        let errors = extract_errors(&records);
        let err = find(&errors, "tu_order");
        assert_eq!(
            err.category,
            ErrorCategory::UserRejection,
            "both UserRejection and PermissionDenied rules match this record; \
             UserRejection is first in the ordered rule list, so it wins"
        );
    }

    // ---------------------------------------------------------------------
    // Type-guard — an object-valued `toolUseResult` (Skill's
    // {success, commandName, allowedTools} payload here) must NOT match the
    // UserRejection string rule. The `tu_guard` error falls through to the
    // prefix checks; its content matches none, so it lands in Technical.
    // ---------------------------------------------------------------------

    #[test]
    fn object_valued_tool_use_result_does_not_match_user_rejection() {
        let records = load_records("errors-edge-cases.jsonl");
        let errors = extract_errors(&records);
        let err = find(&errors, "tu_guard");
        assert_ne!(
            err.category,
            ErrorCategory::UserRejection,
            "object-valued toolUseResult must never classify as UserRejection"
        );
        assert_eq!(
            err.category,
            ErrorCategory::Technical,
            "content has no Permission/PreToolUse prefix => Technical"
        );
        assert_eq!(
            err.tool_name, "Skill",
            "tool_name still resolves from the launching Skill tool_use"
        );
    }

    // ---------------------------------------------------------------------
    // tool_name resolution — an error tool_result whose `tool_use_id` has
    // no matching `tool_use` block resolves `tool_name` to the literal
    // "<unknown>".
    // ---------------------------------------------------------------------

    #[test]
    fn unknown_tool_use_id_resolves_tool_name_to_unknown_literal() {
        let records = load_records("errors-edge-cases.jsonl");
        let errors = extract_errors(&records);
        let err = find(&errors, "tu_orphan");
        assert_eq!(
            err.tool_name, "<unknown>",
            "an orphan tool_use_id with no matching tool_use => tool_name = \"<unknown>\""
        );
    }

    // ---------------------------------------------------------------------
    // Excerpt truncation — `content` longer than 200 chars is truncated to
    // the first 200. The `tu_long` fixture content is the 20-char block
    // "0123456789abcdefghij" repeated 12 times (240 chars); the excerpt
    // must be that block repeated exactly 10 times (200 chars).
    // ---------------------------------------------------------------------

    #[test]
    fn excerpt_is_truncated_to_first_200_chars() {
        let records = load_records("errors-edge-cases.jsonl");
        let errors = extract_errors(&records);
        let err = find(&errors, "tu_long");
        let block = "0123456789abcdefghij";
        assert_eq!(
            err.excerpt.chars().count(),
            200,
            "excerpt must be truncated to exactly 200 chars"
        );
        assert_eq!(
            err.excerpt,
            block.repeat(10),
            "excerpt must be the first 200 chars of the 240-char content"
        );
    }
}
