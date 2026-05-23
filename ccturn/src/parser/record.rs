// On-disk JSONL uses camelCase keys (parentUuid, sessionId, attributionSkill,
// toolUseResult, ...). `#[serde(rename_all = "camelCase")]` here lets us declare
// idiomatic snake_case Rust fields that deserialise from those camelCase keys.
// The SessionReport output struct (later step) keeps serde's default field naming
// with NO `rename_all` attribute, so the --json output schema matches the snake_case
// shape defined in § JSON Output Schema verbatim. The two structs live on opposite
// sides of the parse/emit boundary by design.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::Context;
use serde::Deserialize;
use serde_json::{Map, Value};

// Many fields document the JSONL schema and round-trip through the parser tests
// but are not consumed by the current extractors. Keeping them named (rather than
// folding everything into `extra`) makes the contract with the on-disk format
// explicit and gives future extractors typed access without re-deserialising.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonlRecord {
    pub r#type: String,
    #[serde(default)]
    pub uuid: Option<String>,
    #[serde(default)]
    pub parent_uuid: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub git_branch: Option<String>,
    #[serde(default)]
    pub user_type: Option<String>,
    #[serde(default)]
    pub entrypoint: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub attribution_skill: Option<String>,
    #[serde(default)]
    pub attribution_agent: Option<String>,
    #[serde(default)]
    pub tool_use_result: Option<Value>,
    #[serde(default)]
    pub tool_use_id: Option<String>,
    #[serde(default)]
    pub is_meta: Option<bool>,
    // `sourceToolUseID` has an all-caps `ID` suffix that `rename_all = "camelCase"`
    // would not produce from `source_tool_use_id`; pin the JSON key explicitly.
    #[serde(default, rename = "sourceToolUseID")]
    pub source_tool_use_id: Option<String>,
    #[serde(default)]
    pub message: Option<Value>,
    #[serde(default)]
    pub subtype: Option<String>,
    #[serde(default)]
    pub attachment: Option<Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

// `Text { text }` and the per-variant `extra` catch-all are part of the
// schema-tolerance contract (so unknown future fields land somewhere instead of
// failing the deserialise) but no extractor reads them today.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        #[serde(default)]
        text: String,
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: Value,
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    ToolResult {
        tool_use_id: String,
        #[serde(default)]
        content: Value,
        #[serde(default)]
        is_error: bool,
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
}

// Returns Err when the file cannot be opened (permission denied, removed
// between resolve and read, etc.). Callers MUST distinguish that from
// per-line `Err` items, which only indicate a malformed JSON record on a
// specific line and are safe to skip-and-warn.
pub fn parse_session(
    path: &Path,
) -> anyhow::Result<impl Iterator<Item = anyhow::Result<JsonlRecord>>> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    Ok(BufReader::new(file).lines().map(|line_result| {
        let line = line_result?;
        let record: JsonlRecord = serde_json::from_str(&line)?;
        Ok(record)
    }))
}

// Phase A test specification for the JSONL parser (design doc § Implementation
// :: Step 2). The Programmer fills in the production code ABOVE this `#[cfg(test)]`
// block in Phase B: a `JsonlRecord` struct with `#[serde(rename_all = "camelCase")]`
// and a serde-flattened `extra: serde_json::Map<String, serde_json::Value>` catch-all,
// plus a `ContentBlock` enum with `#[serde(tag = "type")]` discrimination and a
// per-variant flattened `extra` catch-all. The tests below define the deserialisation
// contract; they reference `super::{JsonlRecord, ContentBlock}` and will compile
// once those types exist.

#[cfg(test)]
mod tests {
    use super::{ContentBlock, JsonlRecord, parse_session};
    use serde_json::Value;
    use std::path::PathBuf;

    fn fixture_path(relative: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/parser")
            .join(relative)
    }

    fn parse_record(json: &str) -> JsonlRecord {
        serde_json::from_str(json)
            .unwrap_or_else(|e| panic!("JsonlRecord deserialisation failed: {e}\njson: {json}"))
    }

    fn parse_block(json: &str) -> ContentBlock {
        serde_json::from_str(json)
            .unwrap_or_else(|e| panic!("ContentBlock deserialisation failed: {e}\njson: {json}"))
    }

    // ---------------------------------------------------------------------
    // Task (a) — each of the eight observed top-level `type` values must
    // deserialise into a `JsonlRecord` whose `r#type` field carries the
    // verbatim string. The exact shape of the rest of the payload is not
    // pinned; tests only assert what § Specification > JSONL Record Taxonomy
    // mandates.
    // ---------------------------------------------------------------------

    #[test]
    fn deserialises_user_record() {
        let r = parse_record(
            r#"{
                "type": "user",
                "uuid": "11111111-1111-1111-1111-111111111111",
                "parentUuid": null,
                "sessionId": "session-1",
                "cwd": "/tmp/test-project",
                "timestamp": "2026-05-19T09:14:02.000000+00:00",
                "version": "2.0.0",
                "userType": "external",
                "entrypoint": "cli",
                "permissionMode": "default",
                "message": {"role": "user", "content": "Hello"}
            }"#,
        );
        assert_eq!(r.r#type, "user");
    }

    #[test]
    fn deserialises_assistant_record() {
        let r = parse_record(
            r#"{
                "type": "assistant",
                "uuid": "22222222-2222-2222-2222-222222222222",
                "parentUuid": "11111111-1111-1111-1111-111111111111",
                "sessionId": "session-1",
                "cwd": "/tmp/test-project",
                "timestamp": "2026-05-19T09:14:05.000000+00:00",
                "version": "2.0.0",
                "message": {
                    "role": "assistant",
                    "content": [{"type": "text", "text": "Hi"}]
                }
            }"#,
        );
        assert_eq!(r.r#type, "assistant");
    }

    #[test]
    fn deserialises_system_record() {
        let r = parse_record(
            r#"{
                "type": "system",
                "subtype": "local_command",
                "uuid": "33333333-3333-3333-3333-333333333333",
                "parentUuid": null,
                "sessionId": "session-1",
                "cwd": "/tmp/test-project",
                "timestamp": "2026-05-19T09:14:06.000000+00:00"
            }"#,
        );
        assert_eq!(r.r#type, "system");
    }

    #[test]
    fn deserialises_attachment_record() {
        let r = parse_record(
            r#"{
                "type": "attachment",
                "uuid": "44444444-4444-4444-4444-444444444444",
                "parentUuid": "33333333-3333-3333-3333-333333333333",
                "sessionId": "session-1",
                "cwd": "/tmp/test-project",
                "timestamp": "2026-05-19T09:14:07.000000+00:00",
                "attachment": {
                    "type": "command_permissions",
                    "allowedTools": ["Bash", "Read"]
                }
            }"#,
        );
        assert_eq!(r.r#type, "attachment");
    }

    #[test]
    fn deserialises_permission_mode_record() {
        let r = parse_record(
            r#"{
                "type": "permission-mode",
                "uuid": "55555555-5555-5555-5555-555555555555",
                "parentUuid": null,
                "sessionId": "session-1",
                "cwd": "/tmp/test-project",
                "timestamp": "2026-05-19T09:14:08.000000+00:00",
                "permissionMode": "default"
            }"#,
        );
        assert_eq!(r.r#type, "permission-mode");
    }

    #[test]
    fn deserialises_file_history_snapshot_record() {
        let r = parse_record(
            r#"{
                "type": "file-history-snapshot",
                "uuid": "66666666-6666-6666-6666-666666666666",
                "parentUuid": null,
                "sessionId": "session-1",
                "cwd": "/tmp/test-project",
                "timestamp": "2026-05-19T09:14:09.000000+00:00",
                "snapshotPath": "/tmp/snapshot.json"
            }"#,
        );
        assert_eq!(r.r#type, "file-history-snapshot");
    }

    #[test]
    fn deserialises_ai_title_record() {
        let r = parse_record(
            r#"{
                "type": "ai-title",
                "uuid": "77777777-7777-7777-7777-777777777777",
                "parentUuid": null,
                "sessionId": "session-1",
                "cwd": "/tmp/test-project",
                "timestamp": "2026-05-19T09:14:10.000000+00:00",
                "title": "Refactor parser"
            }"#,
        );
        assert_eq!(r.r#type, "ai-title");
    }

    #[test]
    fn deserialises_last_prompt_record() {
        let r = parse_record(
            r#"{
                "type": "last-prompt",
                "uuid": "88888888-8888-8888-8888-888888888888",
                "parentUuid": null,
                "sessionId": "session-1",
                "cwd": "/tmp/test-project",
                "timestamp": "2026-05-19T09:14:11.000000+00:00",
                "prompt": "Run the tests"
            }"#,
        );
        assert_eq!(r.r#type, "last-prompt");
    }

    // ---------------------------------------------------------------------
    // Task (a) — assistant record carrying a `tool_use` content block, and
    // assistant record carrying a `tool_result` content block. The full
    // record must deserialise; the JSON shapes mirror the design doc's
    // examples under § Specification > JSONL Record Taxonomy.
    // ---------------------------------------------------------------------

    #[test]
    fn deserialises_assistant_record_with_tool_use_content_block() {
        let r = parse_record(
            r#"{
                "type": "assistant",
                "uuid": "aaaa1111-1111-1111-1111-111111111111",
                "parentUuid": "11111111-1111-1111-1111-111111111111",
                "sessionId": "session-1",
                "cwd": "/tmp/test-project",
                "timestamp": "2026-05-19T09:14:12.000000+00:00",
                "message": {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "tool_use",
                            "id": "toolu_01DcQbctMXWANSHzAUZTyu8g",
                            "name": "Skill",
                            "input": {"skill": "gh-cli", "args": "List open PRs..."},
                            "caller": {"type": "direct"}
                        }
                    ]
                }
            }"#,
        );
        assert_eq!(r.r#type, "assistant");
    }

    #[test]
    fn deserialises_assistant_record_with_tool_result_content_block() {
        let r = parse_record(
            r#"{
                "type": "assistant",
                "uuid": "aaaa2222-2222-2222-2222-222222222222",
                "parentUuid": "aaaa1111-1111-1111-1111-111111111111",
                "sessionId": "session-1",
                "cwd": "/tmp/test-project",
                "timestamp": "2026-05-19T09:14:13.000000+00:00",
                "message": {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "tool_result",
                            "tool_use_id": "toolu_01DcQbctMXWANSHzAUZTyu8g",
                            "content": "Launching skill: gh-cli",
                            "is_error": false,
                            "sourceToolAssistantUUID": "aaaa1111-1111-1111-1111-111111111111"
                        }
                    ]
                },
                "toolUseResult": {
                    "success": true,
                    "commandName": "gh-cli",
                    "allowedTools": ["Bash"]
                }
            }"#,
        );
        assert_eq!(r.r#type, "assistant");
    }

    // ---------------------------------------------------------------------
    // Task (a) — `ContentBlock` variant discrimination via the `type` tag.
    // Pinned because the Skill / Subagent / Tools / Errors extractors all
    // pattern-match on these variants.
    // ---------------------------------------------------------------------

    #[test]
    fn content_block_tool_use_variant() {
        let block = parse_block(
            r#"{
                "type": "tool_use",
                "id": "toolu_01DcQbctMXWANSHzAUZTyu8g",
                "name": "Skill",
                "input": {"skill": "gh-cli", "args": "List open PRs..."},
                "caller": {"type": "direct"}
            }"#,
        );
        assert!(
            matches!(block, ContentBlock::ToolUse { .. }),
            "expected ContentBlock::ToolUse variant"
        );
    }

    #[test]
    fn content_block_tool_result_variant() {
        let block = parse_block(
            r#"{
                "type": "tool_result",
                "tool_use_id": "toolu_01DcQbctMXWANSHzAUZTyu8g",
                "content": "Launching skill: gh-cli",
                "is_error": false
            }"#,
        );
        assert!(
            matches!(block, ContentBlock::ToolResult { .. }),
            "expected ContentBlock::ToolResult variant"
        );
    }

    #[test]
    fn content_block_text_variant() {
        let block = parse_block(r#"{"type": "text", "text": "Hello"}"#);
        assert!(
            matches!(block, ContentBlock::Text { .. }),
            "expected ContentBlock::Text variant"
        );
    }

    // ---------------------------------------------------------------------
    // Task (b) — tolerance fixtures exercising Success Criterion #5.
    // The parser MUST tolerate (1) unknown top-level `type` values,
    // (2) unknown top-level extra fields, and (3) unknown extra fields
    // nested inside a content block. None of these shapes may raise a
    // deserialise error; unknown keys must land in the relevant `extra`
    // catch-all (never lost, never blocking).
    // ---------------------------------------------------------------------

    #[test]
    fn tolerates_unknown_top_level_type_value() {
        let r = parse_record(
            r#"{
                "type": "future-type-x",
                "uuid": "ffff0000-0000-0000-0000-000000000001",
                "parentUuid": null,
                "sessionId": "session-1",
                "cwd": "/tmp/test-project",
                "timestamp": "2026-05-19T09:14:14.000000+00:00"
            }"#,
        );
        assert_eq!(
            r.r#type, "future-type-x",
            "unknown `type` values must round-trip verbatim, not be normalised or dropped"
        );
    }

    #[test]
    fn tolerates_unknown_top_level_extra_fields() {
        let r = parse_record(
            r#"{
                "type": "user",
                "uuid": "ffff0000-0000-0000-0000-000000000002",
                "parentUuid": null,
                "sessionId": "session-1",
                "cwd": "/tmp/test-project",
                "timestamp": "2026-05-19T09:14:15.000000+00:00",
                "futureFlag": true,
                "futureMetric": 42,
                "futureNestedObject": {"k": "v"}
            }"#,
        );
        assert_eq!(r.r#type, "user");
        assert!(
            r.extra.contains_key("futureFlag"),
            "futureFlag should land in extra"
        );
        assert!(
            r.extra.contains_key("futureMetric"),
            "futureMetric should land in extra"
        );
        assert!(
            r.extra.contains_key("futureNestedObject"),
            "futureNestedObject should land in extra"
        );
        assert_eq!(r.extra["futureFlag"], Value::Bool(true));
        assert_eq!(r.extra["futureMetric"], Value::from(42));
    }

    // ---------------------------------------------------------------------
    // Task 3 of Step 2 — `parse_session` streaming iterator contract.
    // Per the design doc: `parse_session(path: &Path) -> impl
    // Iterator<Item = anyhow::Result<JsonlRecord>>` using `BufReader::lines()`,
    // streams instead of loading the whole file, and malformed lines yield
    // `Err` while well-formed lines on either side still yield `Ok` (the
    // caller is responsible for skip + warn).
    // ---------------------------------------------------------------------

    #[test]
    fn parse_session_yields_each_record_from_multi_line_fixture() {
        let path = fixture_path("multi-record-valid.jsonl");
        let results: Vec<_> = parse_session(&path)
            .expect("fixture file must open")
            .collect();
        assert_eq!(
            results.len(),
            3,
            "fixture has three records; iterator must yield one item per line"
        );
        for (i, item) in results.iter().enumerate() {
            assert!(
                item.is_ok(),
                "line {i} should deserialise to Ok(JsonlRecord); got err: {:?}",
                item.as_ref().err()
            );
        }
        let types: Vec<&str> = results
            .iter()
            .map(|r| r.as_ref().expect("Ok").r#type.as_str())
            .collect();
        assert_eq!(types, ["user", "assistant", "system"]);
    }

    #[test]
    fn parse_session_yields_err_for_malformed_line_but_continues_streaming() {
        let path = fixture_path("malformed-line-middle.jsonl");
        let results: Vec<_> = parse_session(&path)
            .expect("fixture file must open")
            .collect();
        assert_eq!(
            results.len(),
            3,
            "iterator must yield one item per line including the malformed one — \
             a malformed line yields Err but does NOT terminate the stream"
        );
        assert!(
            results[0].is_ok(),
            "line 1 (well-formed) must yield Ok; got: {:?}",
            results[0].as_ref().err()
        );
        assert!(
            results[1].is_err(),
            "line 2 is deliberately malformed and must yield Err"
        );
        assert!(
            results[2].is_ok(),
            "line 3 (well-formed) must still yield Ok — streaming continues past the error; \
             got: {:?}",
            results[2].as_ref().err()
        );
        assert_eq!(
            results[0].as_ref().expect("Ok").r#type,
            "user",
            "line 1 type"
        );
        assert_eq!(
            results[2].as_ref().expect("Ok").r#type,
            "assistant",
            "line 3 type"
        );
    }

    #[test]
    fn tolerates_unknown_extra_fields_on_tool_use_content_block() {
        let block = parse_block(
            r#"{
                "type": "tool_use",
                "id": "toolu_01DcQbctMXWANSHzAUZTyu8g",
                "name": "Skill",
                "input": {"skill": "gh-cli"},
                "caller": {"type": "direct"},
                "futureBlockField": "future-value",
                "futureBlockFlag": false
            }"#,
        );
        match block {
            ContentBlock::ToolUse { extra, .. } => {
                assert!(
                    extra.contains_key("futureBlockField"),
                    "futureBlockField should land in the ToolUse variant's extra"
                );
                assert!(
                    extra.contains_key("futureBlockFlag"),
                    "futureBlockFlag should land in the ToolUse variant's extra"
                );
                assert_eq!(
                    extra["futureBlockField"],
                    Value::String("future-value".to_string())
                );
                assert_eq!(extra["futureBlockFlag"], Value::Bool(false));
            }
            _ => panic!("expected ContentBlock::ToolUse variant"),
        }
    }
}
