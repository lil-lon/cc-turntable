use std::collections::HashMap;

use serde::Serialize;
use serde_json::Value;

use crate::extract::errors::{ErrorCategory, extract_errors};
use crate::extract::user_record_carries_tool_result;
use crate::parser::record::JsonlRecord;

// kebab-case renders `Error` -> "error" and `UserMidStream` -> "user-mid-stream",
// matching § JSON Output Schema's `interventions[].kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InterventionKind {
    Error,
    UserMidStream,
}

#[derive(Debug, Clone, Serialize)]
pub struct Intervention {
    pub kind: InterventionKind,
    pub timestamp: String,
    pub tool_name: Option<String>,
    pub excerpt: String,
    pub source_uuid: String,
}

const EXCERPT_MAX_CHARS: usize = 200;

// Flatten a `user` record's `message.content` (string or array of content
// blocks) into a single excerpt string of at most 200 chars.
fn user_content_excerpt(message: &Value) -> String {
    let Some(content) = message.get("content") else {
        return String::new();
    };
    let raw = match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| {
                block
                    .get("text")
                    .and_then(|t| t.as_str())
                    .or_else(|| block.as_str())
                    .map(str::to_owned)
            })
            .collect::<Vec<_>>()
            .join(" "),
        other => other.to_string(),
    };
    raw.chars().take(EXCERPT_MAX_CHARS).collect()
}

pub fn extract_interventions(records: &[JsonlRecord]) -> Vec<Intervention> {
    let mut out: Vec<Intervention> = Vec::new();

    // Source 1: re-emit the first three error categories (Technical excluded).
    for error in extract_errors(records) {
        if error.category == ErrorCategory::Technical {
            continue;
        }
        out.push(Intervention {
            kind: InterventionKind::Error,
            timestamp: error.timestamp,
            tool_name: Some(error.tool_name),
            excerpt: error.excerpt,
            source_uuid: error.tool_use_id,
        });
    }

    // Source 2: mid-stream `user` records. uuid -> record type, so we can
    // test whether a record's `parentUuid` points to an `attachment` record.
    let mut record_type_by_uuid: HashMap<&str, &str> = HashMap::new();
    for record in records {
        if let Some(uuid) = &record.uuid {
            record_type_by_uuid.insert(uuid.as_str(), record.r#type.as_str());
        }
    }

    for record in records {
        if record.r#type != "user" {
            continue;
        }
        // Clause 1: `isMeta` absent or false.
        if record.is_meta == Some(true) {
            continue;
        }
        // Clause 2: `sourceToolUseID` absent.
        if record.source_tool_use_id.is_some() {
            continue;
        }
        // Clause 3: `parentUuid` points to an `attachment` record.
        let parent_is_attachment = record
            .parent_uuid
            .as_deref()
            .and_then(|parent| record_type_by_uuid.get(parent).copied())
            == Some("attachment");
        if !parent_is_attachment {
            continue;
        }
        // Clause 4: the record's `message.content` does NOT carry a tool_result
        // block. user records carrying tool_result are the harness echoing a
        // tool result back to the model, not a real human input — they would
        // spuriously qualify if a tool happened to fire while the most recent
        // wrapper was an attachment.
        if user_record_carries_tool_result(record) {
            continue;
        }

        let excerpt = record
            .message
            .as_ref()
            .map(user_content_excerpt)
            .unwrap_or_default();
        out.push(Intervention {
            kind: InterventionKind::UserMidStream,
            timestamp: record.timestamp.clone().unwrap_or_default(),
            tool_name: None,
            excerpt,
            source_uuid: record.uuid.clone().unwrap_or_default(),
        });
    }

    out
}

// Phase A test specification for the Intervention extractor (design doc § Implementation
// :: Step 8). The Programmer adds the production code ABOVE this `#[cfg(test)]`
// block in Phase B:
//   - `pub enum InterventionKind { Error, UserMidStream }` — serialises to
//     "error" / "user-mid-stream" per § JSON Output Schema's
//     `interventions[].kind`.
//   - `pub struct Intervention { kind, timestamp, tool_name: Option<String>,
//      excerpt, source_uuid }` matching the `interventions[]` schema entry.
//   - `pub fn extract_interventions(records: &[JsonlRecord]) -> Vec<Intervention>`
//     emitting from two sources:
//       1. The first three error categories (UserRejection, PermissionDenied,
//          HookBlock) re-surfaced as `kind = Error`; `source_uuid` is the
//          blocked call's `tool_use_id`. Technical errors are NOT re-surfaced.
//       2. Mid-stream `user` records as `kind = UserMidStream` — qualifying
//          iff all three clauses hold: `isMeta` absent/false, `sourceToolUseID`
//          absent, and `parentUuid` points to an `attachment` record.
//          `source_uuid` is the user record's own `uuid`.

#[cfg(test)]
mod tests {
    use super::{Intervention, InterventionKind, extract_interventions};
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

    fn find<'a>(interventions: &'a [Intervention], source_uuid: &str) -> Option<&'a Intervention> {
        interventions.iter().find(|i| i.source_uuid == source_uuid)
    }

    // ---------------------------------------------------------------------
    // Source 1 — error-derived interventions. The first three error
    // categories re-surface as `kind = Error` interventions; `Technical`
    // errors do NOT. `errors-by-category.jsonl` has one error of each
    // category, so exactly three interventions must come out.
    // ---------------------------------------------------------------------

    #[test]
    fn first_three_error_categories_re_emitted_as_interventions_technical_excluded() {
        let records = load_records("errors-by-category.jsonl");
        let interventions = extract_interventions(&records);

        assert_eq!(
            interventions.len(),
            3,
            "UserRejection + PermissionDenied + HookBlock surface as interventions; \
             Technical does not"
        );
        for iv in &interventions {
            assert_eq!(
                iv.kind,
                InterventionKind::Error,
                "every error-derived intervention has kind = Error"
            );
        }

        // source_uuid is the blocked call's tool_use_id (matches the
        // errors[] entry's tool_use_id).
        let rejection = find(&interventions, "tu_reject")
            .expect("UserRejection error must surface as an intervention");
        assert_eq!(rejection.tool_name.as_deref(), Some("Bash"));
        assert_eq!(
            rejection.timestamp, "2026-05-19T09:14:02.000000+00:00",
            "error-intervention timestamp is the error record's timestamp"
        );

        let permission = find(&interventions, "tu_perm")
            .expect("PermissionDenied error must surface as an intervention");
        assert_eq!(permission.tool_name.as_deref(), Some("Edit"));

        let hook = find(&interventions, "tu_hook")
            .expect("HookBlock error must surface as an intervention");
        assert_eq!(hook.tool_name.as_deref(), Some("Bash"));

        assert!(
            find(&interventions, "tu_tech").is_none(),
            "Technical errors must NOT be re-emitted as interventions"
        );
    }

    // ---------------------------------------------------------------------
    // Source 2 — a mid-stream `user` record that satisfies all three
    // clauses (isMeta absent, sourceToolUseID absent, parentUuid -> an
    // attachment record) surfaces as `kind = UserMidStream`. `source_uuid`
    // is the user record's own uuid; `tool_name` is null.
    // ---------------------------------------------------------------------

    #[test]
    fn mid_stream_user_record_surfaces_as_user_mid_stream_intervention() {
        let records = load_records("interventions-user-mid-stream.jsonl");
        let interventions = extract_interventions(&records);

        // Two qualifying mid-stream records in the fixture (string-content
        // and array-content); the /clear and skill-injection records do not.
        assert_eq!(
            interventions.len(),
            2,
            "only the two qualifying mid-stream user records surface"
        );

        let iv = find(&interventions, "u-midstream-str")
            .expect("the string-content mid-stream correction must surface");
        assert_eq!(iv.kind, InterventionKind::UserMidStream);
        assert_eq!(
            iv.source_uuid, "u-midstream-str",
            "source_uuid for a user-mid-stream intervention is the user record's own uuid"
        );
        assert_eq!(
            iv.tool_name, None,
            "user-mid-stream interventions have no tool_name"
        );
        assert_eq!(iv.timestamp, "2026-05-19T11:00:03.000000+00:00");
        assert_eq!(
            iv.excerpt, "actually, reconsider this approach and read the file first",
            "excerpt is the first 200 chars of a string-shaped message.content"
        );
    }

    // ---------------------------------------------------------------------
    // `message.content` may be an array of content blocks rather than a
    // bare string — the extractor must tolerate that shape. The assertion
    // on `excerpt` is intentionally lenient (`contains`) because the design
    // doc does not pin how array content is flattened into the excerpt.
    // ---------------------------------------------------------------------

    #[test]
    fn mid_stream_user_record_with_array_content_is_tolerated() {
        let records = load_records("interventions-user-mid-stream.jsonl");
        let interventions = extract_interventions(&records);

        let iv = find(&interventions, "u-midstream-arr")
            .expect("the array-content mid-stream correction must still surface");
        assert_eq!(iv.kind, InterventionKind::UserMidStream);
        assert_eq!(iv.source_uuid, "u-midstream-arr");
        assert!(
            iv.excerpt.contains("wait, check the logs first"),
            "excerpt must carry the user's text even when message.content is an array; got {:?}",
            iv.excerpt
        );
    }

    // ---------------------------------------------------------------------
    // Negative — a Skill body injection (`type: user`, `isMeta: true`,
    // `sourceToolUseID` set) must NOT surface. In the fixture this record's
    // `parentUuid` DOES point to an attachment (clause 3 passes), so the
    // exclusion is proven to come from the isMeta / sourceToolUseID clauses.
    // ---------------------------------------------------------------------

    #[test]
    fn skill_body_injection_does_not_surface_as_intervention() {
        let records = load_records("interventions-user-mid-stream.jsonl");
        let interventions = extract_interventions(&records);
        assert!(
            find(&interventions, "u-skillinject").is_none(),
            "a user record with isMeta=true and sourceToolUseID set is framework \
             noise and must NOT surface, even though its parentUuid -> attachment"
        );
    }

    // ---------------------------------------------------------------------
    // Negative — a `/clear` slash command at session start has
    // `parentUuid: null` (no attachment parent), so clause 3 fails and it
    // must NOT surface. Slash commands get no special category; they are
    // ordinary `user` records subject to the same three-clause filter.
    // ---------------------------------------------------------------------

    #[test]
    fn slash_command_at_session_start_does_not_surface_as_intervention() {
        let records = load_records("interventions-user-mid-stream.jsonl");
        let interventions = extract_interventions(&records);
        assert!(
            find(&interventions, "u-clear").is_none(),
            "a /clear slash command with no attachment parent fails the mid-stream \
             clause and must NOT surface"
        );
    }
}
