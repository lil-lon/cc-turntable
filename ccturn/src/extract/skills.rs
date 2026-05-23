use std::collections::HashMap;

use serde::Serialize;

use crate::extract::{assistant_content_blocks, value_to_content_string};
use crate::parser::record::{ContentBlock, JsonlRecord};

#[derive(Debug, Clone, Serialize)]
pub struct SkillInvocation {
    pub skill_name: String,
    pub invocation_uuid: String,
    pub args: Option<String>,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub inner_tool_uses: usize,
    pub inner_errors: usize,
    pub launch_is_error: bool,
    pub launch_content: String,
}

pub fn extract_skills(records: &[JsonlRecord]) -> Vec<SkillInvocation> {
    // First pass: tool_use_id -> (is_error, content_string).
    let mut tool_result_map: HashMap<String, (bool, String)> = HashMap::new();
    for record in records {
        for block in assistant_content_blocks(record) {
            if let ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
                ..
            } = block
            {
                tool_result_map.insert(tool_use_id, (is_error, value_to_content_string(&content)));
            }
        }
    }

    // Second pass: emit one SkillInvocation per launching `Skill` tool_use.
    let mut out: Vec<SkillInvocation> = Vec::new();
    for (i, record) in records.iter().enumerate() {
        for block in assistant_content_blocks(record) {
            let ContentBlock::ToolUse {
                id, name, input, ..
            } = block
            else {
                continue;
            };
            if name != "Skill" {
                continue;
            }
            let skill_name = input
                .get("skill")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let args = input
                .get("args")
                .and_then(|v| v.as_str())
                .map(str::to_string);

            // Window: contiguous run of records (scanning forward from i+1)
            // whose `attribution_skill` equals `skill_name`. The launching
            // record itself carries no `attribution_skill`, so it is
            // naturally excluded; the run closes on the first non-matching
            // record after at least one match.
            let mut window_start: Option<usize> = None;
            let mut window_end: Option<usize> = None;
            for (j, r) in records.iter().enumerate().skip(i + 1) {
                let matches = r.attribution_skill.as_deref() == Some(skill_name.as_str());
                if matches {
                    if window_start.is_none() {
                        window_start = Some(j);
                    }
                    window_end = Some(j);
                } else if window_start.is_some() {
                    break;
                }
            }

            let (started_at, ended_at, inner_tool_uses, inner_errors) =
                match (window_start, window_end) {
                    (Some(s), Some(e)) => {
                        let mut tu_count = 0usize;
                        let mut err_count = 0usize;
                        for r in &records[s..=e] {
                            for block in assistant_content_blocks(r) {
                                match block {
                                    ContentBlock::ToolUse { .. } => tu_count += 1,
                                    ContentBlock::ToolResult { is_error: true, .. } => {
                                        err_count += 1;
                                    }
                                    _ => {}
                                }
                            }
                        }
                        let started_at = records[s].timestamp.clone().unwrap_or_default();
                        let ended_at = records[e].timestamp.clone();
                        (started_at, ended_at, tu_count, err_count)
                    }
                    _ => (String::new(), None, 0, 0),
                };

            let (launch_is_error, launch_content) = tool_result_map
                .get(&id)
                .cloned()
                .unwrap_or((false, String::new()));

            out.push(SkillInvocation {
                skill_name,
                invocation_uuid: record.uuid.clone().unwrap_or_default(),
                args,
                started_at,
                ended_at,
                inner_tool_uses,
                inner_errors,
                launch_is_error,
                launch_content,
            });
        }
    }
    out
}

// Phase A test specification for the Skill extractor (design doc § Implementation
// :: Step 4). The Programmer adds the production code ABOVE this `#[cfg(test)]`
// block in Phase B:
//   - `pub struct SkillInvocation { skill_name, invocation_uuid, args,
//      started_at, ended_at, inner_tool_uses, inner_errors, launch_is_error,
//      launch_content }` matching the schema under § JSON Output Schema.
//   - `pub fn extract_skills(records: &[JsonlRecord]) -> Vec<SkillInvocation>`
//     — streaming pass that tracks the active `attribution_skill` across
//     assistant records, emits one `SkillInvocation` per launching
//     `Skill` `tool_use`, and closes each window when `attribution_skill`
//     differs or disappears.
//
// The tests reference `super::{SkillInvocation, extract_skills}` and will
// compile once those items exist. The wiring (`mod extract;` in main.rs and
// `pub mod skills;` in src/extract/mod.rs) is Programmer-owned Phase B work.

#[cfg(test)]
mod tests {
    use super::{SkillInvocation, extract_skills};
    use crate::parser::record::{JsonlRecord, parse_session};
    use std::path::PathBuf;

    fn extract_from_fixture(name: &str) -> Vec<SkillInvocation> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        let records: Vec<JsonlRecord> = parse_session(&path)
            .unwrap_or_else(|e| panic!("fixture {name} must open: {e}"))
            .collect::<anyhow::Result<Vec<_>>>()
            .unwrap_or_else(|e| panic!("fixture {name} must parse cleanly: {e}"));
        extract_skills(&records)
    }

    // ---------------------------------------------------------------------
    // Prescribed fixture (Step 4 task 3): one Skill invocation with three
    // inner tool_uses and one inner error. Asserts every field of the
    // single emitted `SkillInvocation`, including the launch tool_result
    // fields (`launch_is_error`, `launch_content`) that come from the
    // tool_result matching the launching `tool_use_id`.
    // ---------------------------------------------------------------------

    #[test]
    fn prescribed_fixture_emits_one_skill_invocation_with_expected_counts_and_launch_fields() {
        let skills = extract_from_fixture("skill-invocation.jsonl");
        assert_eq!(
            skills.len(),
            1,
            "fixture has exactly one launching `Skill` tool_use; \
             extractor must emit exactly one SkillInvocation"
        );
        let skill = &skills[0];

        assert_eq!(skill.skill_name, "gh-cli");
        assert_eq!(skill.invocation_uuid, "a-launch");
        assert_eq!(
            skill.args.as_deref(),
            Some("List open PRs"),
            "args is `input.args` from the launching tool_use; per the JSON \
             schema this is `string | null`"
        );

        // 3 inner tool_uses (records a1, a2, a3); 1 inner error (a2-result).
        // The launching `Skill` tool_use at record a-launch must NOT be
        // counted — the window is exclusive of the launching tool_use and
        // its tool_result.
        assert_eq!(skill.inner_tool_uses, 3);
        assert_eq!(skill.inner_errors, 1);

        // Launch tool_result fields: matched via tool_use_id `toolu_LAUNCH`
        // to record a-launch-result.
        assert!(!skill.launch_is_error);
        assert_eq!(skill.launch_content, "Launching skill: gh-cli");

        // Window bounds: first attributionSkill="gh-cli" record is a1
        // (09:14:03); last contiguous is a3-result (09:14:08). Timestamps
        // round-trip verbatim per the JSON-output policy.
        assert_eq!(skill.started_at, "2026-05-19T09:14:03.000000+00:00");
        assert_eq!(
            skill.ended_at.as_deref(),
            Some("2026-05-19T09:14:08.000000+00:00")
        );
    }

    // ---------------------------------------------------------------------
    // Window-close behaviour: when `attributionSkill` disappears
    // mid-stream, the window closes at the LAST attributed record. The
    // emitted `ended_at` MUST NOT extend past that record into subsequent
    // un-attributed activity.
    // ---------------------------------------------------------------------

    #[test]
    fn window_closes_when_attribution_skill_disappears_mid_stream() {
        let skills = extract_from_fixture("skill-window-closes-mid-stream.jsonl");
        assert_eq!(skills.len(), 1, "exactly one Skill in this fixture");
        let skill = &skills[0];

        assert_eq!(skill.skill_name, "skill-x");

        // The skill-x window is records x1 (09:14:03) and x1-result (09:14:04).
        // Record `after-skill` at 09:14:05 has NO attributionSkill — the
        // window closes there. The post-window tool_use at 09:14:06 and its
        // tool_result at 09:14:07 must NOT be counted in inner_tool_uses
        // or inner_errors, and must NOT extend `ended_at`.
        assert_eq!(skill.started_at, "2026-05-19T09:14:03.000000+00:00");
        assert_eq!(
            skill.ended_at.as_deref(),
            Some("2026-05-19T09:14:04.000000+00:00"),
            "ended_at must be the timestamp of the last contiguous record \
             with attributionSkill = skill-x — NOT a later un-attributed \
             record"
        );
        assert_eq!(
            skill.inner_tool_uses, 1,
            "one tool_use inside the window (x1)"
        );
        assert_eq!(skill.inner_errors, 0, "no errors inside the window");
    }

    // ---------------------------------------------------------------------
    // Sanity assertion: the launching `Skill` tool_use is excluded from
    // `inner_tool_uses`. This fixture has exactly ONE tool_use in the whole
    // file (the launching one); the inner window contains only a text
    // block. `inner_tool_uses` MUST be 0 — if the extractor mistakenly
    // counts the launching tool_use it would report 1.
    // ---------------------------------------------------------------------

    #[test]
    fn launching_tool_use_is_excluded_from_inner_tool_uses() {
        let skills = extract_from_fixture("skill-launching-exclusion.jsonl");
        assert_eq!(skills.len(), 1);
        let skill = &skills[0];

        assert_eq!(skill.skill_name, "tiny-skill");
        assert_eq!(
            skill.inner_tool_uses, 0,
            "fixture has only the launching tool_use; inner_tool_uses must \
             be 0 because the launching tool_use is explicitly excluded \
             per design doc § Skills task 3"
        );
        assert_eq!(skill.inner_errors, 0);
    }
}
