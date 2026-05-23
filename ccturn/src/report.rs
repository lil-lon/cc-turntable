use serde::Serialize;

use crate::extract::errors::{ErrorRecord, extract_errors};
use crate::extract::interventions::{Intervention, extract_interventions};
use crate::extract::skills::{SkillInvocation, extract_skills};
use crate::extract::subagents::{SubagentInvocation, extract_subagents};
use crate::extract::tools::{ToolUsage, extract_tools};
use crate::locator::{CwdSource, ResolvedSession};
use crate::parser::record::{JsonlRecord, parse_session};

// `SessionReport` uses serde's DEFAULT field naming (no `rename_all`): every
// field is already snake_case, so the serialised JSON keys match § JSON Output
// Schema's top-level object verbatim. Field declaration order is also the
// serialisation order, kept identical to the schema.
#[derive(Debug, Serialize)]
pub struct SessionReport {
    pub session_id: String,
    pub project_cwd: String,
    pub cwd_source: CwdSource,
    pub log_path: String,
    pub started_at: String,
    pub ended_at: String,
    pub record_count: usize,
    pub skills: Vec<SkillInvocation>,
    pub subagents: Vec<SubagentInvocation>,
    pub tools: Vec<ToolUsage>,
    pub errors: Vec<ErrorRecord>,
    pub interventions: Vec<Intervention>,
}

pub fn build_report(resolved: &ResolvedSession) -> anyhow::Result<SessionReport> {
    // One pass over the file: collect parsed records, skip + warn on malformed
    // lines, count every line toward `record_count`.
    let mut records: Vec<JsonlRecord> = Vec::new();
    let mut record_count = 0usize;
    for (idx, item) in parse_session(&resolved.jsonl_path).enumerate() {
        record_count += 1;
        match item {
            Ok(record) => records.push(record),
            Err(e) => eprintln!("warning: skipped malformed line {}: {}", idx + 1, e),
        }
    }

    let session_id = resolved
        .jsonl_path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();

    let started_at = records
        .first()
        .and_then(|r| r.timestamp.clone())
        .unwrap_or_default();
    let ended_at = records
        .last()
        .and_then(|r| r.timestamp.clone())
        .unwrap_or_default();

    let skills = extract_skills(&records);
    let subagents = extract_subagents(&records, &resolved.subagents_dir);
    let tools = extract_tools(&records);
    let errors = extract_errors(&records);
    let interventions = extract_interventions(&records);

    Ok(SessionReport {
        session_id,
        project_cwd: resolved.project_cwd.to_string_lossy().into_owned(),
        cwd_source: resolved.cwd_source,
        log_path: resolved.jsonl_path.to_string_lossy().into_owned(),
        started_at,
        ended_at,
        record_count,
        skills,
        subagents,
        tools,
        errors,
        interventions,
    })
}

// Phase A test specification for Report assembly (design doc § Implementation
// :: Step 9). The Programmer adds the production code ABOVE this `#[cfg(test)]`
// block in Phase B:
//   - `pub struct SessionReport { session_id, project_cwd, cwd_source,
//      log_path, started_at, ended_at, record_count, skills, subagents,
//      tools, errors, interventions }` — serde DEFAULT field naming (no
//      `rename_all`), so the serialised keys match § JSON Output Schema's
//      top-level object verbatim.
//   - `pub fn build_report(resolved: &ResolvedSession) -> anyhow::Result<SessionReport>`
//      — a single orchestrator that runs ONE parser pass over
//      `resolved.jsonl_path` and threads the records through every extractor
//      (skills, subagents, tools, errors, interventions).
//
// The test below is integration-style: it points a `ResolvedSession` at the
// comprehensive `full-session.jsonl` fixture (one Skill, one Task subagent,
// one error of each category, one mid-stream user intervention) and asserts
// `build_report` populates every top-level field. Assertions run against the
// serialised JSON so they validate the § JSON Output Schema contract directly
// and stay agnostic of the exact Rust field types (String vs PathBuf, enum
// vs String).

#[cfg(test)]
mod tests {
    use super::build_report;
    use crate::locator::{CwdSource, ResolvedSession};
    use std::path::PathBuf;

    fn fixtures_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
    }

    #[test]
    fn build_report_populates_every_top_level_field() {
        let fixtures = fixtures_dir();
        let jsonl_path = fixtures.join("full-session.jsonl");
        let subagents_dir = fixtures.join("full-session-subagents");

        let resolved = ResolvedSession {
            jsonl_path: jsonl_path.clone(),
            subagents_dir,
            project_cwd_encoded: "-tmp-test-project".to_string(),
            project_cwd: PathBuf::from("/tmp/test-project"),
            cwd_source: CwdSource::FirstRecord,
        };

        let report = build_report(&resolved).expect("build_report must succeed on the fixture");
        let json = serde_json::to_value(&report)
            .expect("SessionReport must serialise (it is the --json output payload)");

        // --- Scalar fields sourced from the records --------------------
        assert_eq!(
            json["session_id"], "full-session",
            "session_id matches the fixture's session id"
        );
        assert_eq!(
            json["started_at"], "2026-05-19T09:00:00.000000+00:00",
            "started_at is the FIRST record's timestamp, verbatim"
        );
        assert_eq!(
            json["ended_at"], "2026-05-19T09:00:18.000000+00:00",
            "ended_at is the LAST record's timestamp, verbatim"
        );
        assert_eq!(
            json["record_count"], 19,
            "record_count equals the number of JSONL lines in the fixture"
        );

        // --- Scalar fields sourced from the ResolvedSession ------------
        assert_eq!(
            json["project_cwd"], "/tmp/test-project",
            "project_cwd comes from the resolved session"
        );
        assert_eq!(
            json["cwd_source"], "first-record",
            "cwd_source comes from the resolved session (CwdSource::FirstRecord)"
        );
        assert_eq!(
            json["log_path"],
            jsonl_path.to_string_lossy().as_ref(),
            "log_path is the absolute path to the session JSONL"
        );

        // --- Extractor arrays: each must be present and populated ------
        let skills = json["skills"]
            .as_array()
            .expect("skills must be a JSON array");
        assert!(
            !skills.is_empty(),
            "the fixture has one Skill invocation (gh-cli)"
        );

        let subagents = json["subagents"]
            .as_array()
            .expect("subagents must be a JSON array");
        assert!(
            !subagents.is_empty(),
            "the fixture has one Task subagent (report-agent-1)"
        );

        let tools = json["tools"]
            .as_array()
            .expect("tools must be a JSON array");
        assert!(
            !tools.is_empty(),
            "the fixture has multiple tool_use blocks"
        );

        // All four error categories must be represented.
        let errors = json["errors"]
            .as_array()
            .expect("errors must be a JSON array");
        let categories: Vec<&str> = errors
            .iter()
            .map(|e| e["category"].as_str().expect("error category is a string"))
            .collect();
        assert_eq!(errors.len(), 4, "one error of each category in the fixture");
        for expected in [
            "UserRejection",
            "PermissionDenied",
            "HookBlock",
            "Technical",
        ] {
            assert!(
                categories.contains(&expected),
                "errors must include the {expected} category; got {categories:?}"
            );
        }

        // Interventions: both the error-derived kind and the mid-stream
        // user kind must be present.
        let interventions = json["interventions"]
            .as_array()
            .expect("interventions must be a JSON array");
        let kinds: Vec<&str> = interventions
            .iter()
            .map(|i| i["kind"].as_str().expect("intervention kind is a string"))
            .collect();
        assert!(
            kinds.contains(&"error"),
            "the three non-Technical errors must surface as error interventions; got {kinds:?}"
        );
        assert!(
            kinds.contains(&"user-mid-stream"),
            "the mid-stream user record must surface as a user-mid-stream intervention; got {kinds:?}"
        );
    }
}
